// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Decode guard (ROADMAP M1.14, PLAN 3.10.1).
//!
//! * [`guard_item`] turns a panic inside a codec into [`CodecError::InternalPanic`], so one hostile
//!   file fails one item and the batch goes on. It needs `panic = "unwind"` in every profile, which
//!   `cargo xtask check-profiles` enforces.
//! * [`guard_item_timeout`] and [`decode_guarded`] add the soft timeout of `max_decode_ms`:
//!   in-process decoders cannot be pre-empted, so on a timeout the caller gets
//!   [`CodecError::DecodeTimeout`] immediately, the result is discarded when it eventually
//!   arrives, and everything the closure owns (a `MemoryBudget` hold, the input bytes) is released
//!   only when the worker thread really ends.
//! * [`pool_panic_handler`] is the `panic_handler` that rayon pools must install, so a panic on a
//!   pool thread that nobody joins cannot abort the process.

use crate::{CodecError, DecodeLimits, Decoded, decode_with};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Stack for guarded decode threads; deep enough for the recursive decoders with headroom.
const DECODE_STACK: usize = 16 << 20;

static POOL_PANICS: AtomicU64 = AtomicU64::new(0);

/// Best-effort text of a panic payload.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_owned()
    }
}

/// Runs `f`; a panic inside it becomes [`CodecError::InternalPanic`].
pub fn guard_item<T>(f: impl FnOnce() -> Result<T, CodecError>) -> Result<T, CodecError> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(CodecError::InternalPanic(panic_message(&*p))),
    }
}

/// [`guard_item`] on a worker thread with a soft timeout. With `max_ms == None` the closure runs
/// on the calling thread (no timer). On a timeout the thread is left to finish by itself.
pub fn guard_item_timeout<T, F>(max_ms: Option<u64>, f: F) -> Result<T, CodecError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, CodecError> + Send + 'static,
{
    let Some(ms) = max_ms else {
        return guard_item(f);
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("auto-crop-decode".into())
        .stack_size(DECODE_STACK)
        .spawn(move || {
            // The receiver may be gone after a timeout; the result is simply dropped.
            let _ = tx.send(guard_item(f));
        })
        .map_err(|e| CodecError::InternalPanic(format!("could not start a decode thread: {e}")))?;
    match rx.recv_timeout(Duration::from_millis(ms)) {
        Ok(r) => r,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(CodecError::DecodeTimeout),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(CodecError::InternalPanic(
            "the decode thread ended without a result".into(),
        )),
    }
}

/// Decodes `bytes` under `limits`, honouring `limits.max_decode_ms` (soft timeout, see the module
/// docs). Takes the bytes by `Arc` because an abandoned thread keeps reading them.
pub fn decode_guarded(bytes: Arc<[u8]>, limits: &DecodeLimits) -> Result<Decoded, CodecError> {
    let limits = limits.clone();
    guard_item_timeout(limits.max_decode_ms, move || decode_with(&bytes, &limits))
}

/// The handler for `rayon::ThreadPoolBuilder::panic_handler`. Without one, a panic in a spawned
/// pool task aborts the process (rayon's default). It records the panic; callers that need the
/// text use [`guard_item`] around the task.
pub fn pool_panic_handler(payload: Box<dyn Any + Send>) {
    POOL_PANICS.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "auto-crop: a pool task panicked: {}",
        panic_message(&*payload)
    );
}

/// Number of panics seen by [`pool_panic_handler`] (for tests and diagnostics).
pub fn pool_panic_count() -> u64 {
    POOL_PANICS.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::time::Instant;

    #[test]
    fn a_panic_becomes_internal_panic_and_the_next_item_decodes() {
        let bad = guard_item::<()>(|| panic!("decoder blew up"));
        match bad {
            Err(CodecError::InternalPanic(m)) => assert!(m.contains("decoder blew up")),
            other => panic!("{other:?}"),
        }
        // The batch continues: the next item is fine.
        let png = crate::fixtures::png_rgb(8, 8);
        assert!(decode_with(&png, &DecodeLimits::default()).is_ok());
        assert_eq!(guard_item(|| Ok(7)).unwrap(), 7);
    }

    #[test]
    fn a_formatted_panic_message_is_kept() {
        let r = guard_item::<()>(|| panic!("bad index {}", 9));
        assert_eq!(r, Err(CodecError::InternalPanic("bad index 9".into())));
    }

    #[test]
    fn timeout_returns_at_once_but_the_closure_keeps_its_resources_until_it_ends() {
        struct Hold(Arc<AtomicBool>);
        impl Drop for Hold {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let released = Arc::new(AtomicBool::new(false));
        let hold = Hold(released.clone());
        let t0 = Instant::now();
        let r = guard_item_timeout(Some(50), move || {
            let _hold = hold; // stands in for the MemoryBudget hold
            std::thread::sleep(Duration::from_millis(600));
            Ok(1)
        });
        assert_eq!(r, Err(CodecError::DecodeTimeout));
        assert!(t0.elapsed() < Duration::from_millis(500), "returned late");
        assert!(
            !released.load(Ordering::SeqCst),
            "hold must outlive the timeout"
        );
        let t1 = Instant::now();
        while !released.load(Ordering::SeqCst) && t1.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            released.load(Ordering::SeqCst),
            "hold released at thread end"
        );
    }

    #[test]
    fn a_fast_closure_with_a_timer_returns_its_value_and_panics_still_convert() {
        assert_eq!(guard_item_timeout(Some(5_000), || Ok(3)), Ok(3));
        let r = guard_item_timeout::<(), _>(Some(5_000), || panic!("late panic"));
        assert!(matches!(r, Err(CodecError::InternalPanic(_))));
    }

    #[test]
    fn decode_guarded_honours_the_limits_timeout_field() {
        let png = Arc::<[u8]>::from(crate::fixtures::png_rgb(16, 16));
        let limits = DecodeLimits {
            max_decode_ms: Some(10_000),
            ..DecodeLimits::default()
        };
        assert!(decode_guarded(png, &limits).is_ok());
    }

    #[test]
    fn a_pool_with_the_panic_handler_survives_a_panicking_task() {
        let before = pool_panic_count();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .panic_handler(pool_panic_handler)
            .build()
            .unwrap();
        pool.spawn(|| panic!("task panic on a pool thread"));
        // The pool still works afterwards.
        let ok = pool.install(|| 40 + 2);
        assert_eq!(ok, 42);
        let t = Instant::now();
        while pool_panic_count() == before && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(pool_panic_count() > before);
    }
}
