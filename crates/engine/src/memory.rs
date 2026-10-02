// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Byte-weighted memory budget (ROADMAP M1.10, PLAN 2.8): a weighted semaphore that keeps the
//! decoded pixels of concurrent jobs under a cap, so a batch of 100 MP files cannot take the
//! machine down.
//!
//! * `cap = min(25% of RAM, 4 GiB, 50% of free RAM)` (PROVISIONAL).
//! * A job weighs `pixels * 9 + 64 MiB` (about three decoded RGB8 copies plus overhead).
//! * Strictly first-in, first-out: a large job at the head is not overtaken by small ones, so it
//!   cannot starve.
//! * A job heavier than the whole cap waits until nothing else runs and is then admitted alone, so
//!   one big file cannot deadlock the queue.
//! * `acquire` honours a [`CancelToken`]: a cancelled waiter leaves the queue at once.
//!
//! M2.03 wires this into the scheduler; until then nothing in the engine calls it.

use crate::error::ErrKind;
use auto_crop_core::CancelToken;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

pub const MIB: u64 = 1 << 20;
pub const GIB: u64 = 1 << 30;

/// Fixed overhead added to every job (PROVISIONAL).
pub const JOB_OVERHEAD_BYTES: u64 = 64 * MIB;
/// Hard ceiling of the cap.
pub const MAX_CAP_BYTES: u64 = 4 * GIB;

/// How long a blocked `acquire` sleeps between cancellation checks.
const POLL: Duration = Duration::from_millis(10);

/// Weight of a job that decodes `pixels` pixels: `pixels * 9 + 64 MiB`.
pub fn job_weight(pixels: u64) -> u64 {
    pixels.saturating_mul(9).saturating_add(JOB_OVERHEAD_BYTES)
}

/// The cap for a machine with `total` bytes of RAM of which `free` are free:
/// `min(25% of total, 4 GiB, 50% of free)`.
pub fn cap_for(total: u64, free: u64) -> u64 {
    (total / 4).min(MAX_CAP_BYTES).min(free / 2)
}

/// The cap for this machine, from a fresh reading of total and free RAM.
pub fn system_cap() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    cap_for(sys.total_memory(), sys.available_memory())
}

#[derive(Debug, Default)]
struct State {
    in_use: u64,
    peak: u64,
    /// Waiting tickets, oldest first.
    queue: VecDeque<u64>,
    next_ticket: u64,
    /// Admissions so far (for tests and `doctor`).
    admitted: u64,
}

#[derive(Debug)]
struct Inner {
    cap: u64,
    state: Mutex<State>,
    cv: Condvar,
}

/// A cloneable handle to one budget.
#[derive(Debug, Clone)]
pub struct MemoryBudget {
    inner: Arc<Inner>,
}

/// Bytes held; returned to the budget when dropped.
#[derive(Debug)]
#[must_use = "dropping a Permit releases its bytes immediately"]
pub struct Permit {
    budget: MemoryBudget,
    bytes: u64,
}

impl Permit {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.bytes == 0 {
            return;
        }
        let mut st = self.budget.lock();
        st.in_use -= self.bytes;
        drop(st);
        self.budget.inner.cv.notify_all();
    }
}

/// A snapshot for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub cap: u64,
    pub in_use: u64,
    pub peak: u64,
    pub waiting: usize,
    pub admitted: u64,
}

impl MemoryBudget {
    /// A budget with an explicit cap in bytes.
    pub fn new(cap: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                cap,
                state: Mutex::new(State::default()),
                cv: Condvar::new(),
            }),
        }
    }

    /// A budget sized for this machine (see [`cap_for`]).
    pub fn from_system() -> Self {
        Self::new(system_cap())
    }

    pub fn cap(&self) -> u64 {
        self.inner.cap
    }

    pub fn stats(&self) -> Stats {
        let st = self.lock();
        Stats {
            cap: self.inner.cap,
            in_use: st.in_use,
            peak: st.peak,
            waiting: st.queue.len(),
            admitted: st.admitted,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic in another holder must not wedge the whole engine: the counters stay valid.
        self.inner.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a request of `bytes` can be admitted right now given `in_use`.
    fn fits(&self, in_use: u64, bytes: u64) -> bool {
        if bytes > self.inner.cap {
            in_use == 0 // oversize: alone
        } else {
            in_use <= self.inner.cap - bytes
        }
    }

    /// Takes `bytes` from the budget, waiting in FIFO order until they fit. A request larger than
    /// the whole cap waits for an empty budget and then holds it alone. Returns
    /// `Cancelled` (or `DeadlineExceeded`) as soon as `cancel` fires, leaving the queue.
    pub fn acquire(&self, bytes: u64, cancel: &CancelToken) -> Result<Permit, ErrKind> {
        if bytes == 0 {
            return Ok(Permit {
                budget: self.clone(),
                bytes: 0,
            });
        }
        let mut st = self.lock();
        let ticket = st.next_ticket;
        st.next_ticket += 1;
        st.queue.push_back(ticket);
        loop {
            if let Err(i) = cancel.check() {
                st.queue.retain(|t| *t != ticket);
                drop(st);
                // The next in line may fit now that this one has left.
                self.inner.cv.notify_all();
                return Err(i.into());
            }
            if st.queue.front() == Some(&ticket) && self.fits(st.in_use, bytes) {
                st.queue.pop_front();
                st.in_use += bytes;
                st.peak = st.peak.max(st.in_use);
                st.admitted += 1;
                drop(st);
                // The new head may fit as well.
                self.inner.cv.notify_all();
                return Ok(Permit {
                    budget: self.clone(),
                    bytes,
                });
            }
            st = self
                .inner
                .cv
                .wait_timeout(st, POLL)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
    }

    /// Like [`MemoryBudget::acquire`] for a job that decodes `pixels` pixels.
    pub fn acquire_pixels(&self, pixels: u64, cancel: &CancelToken) -> Result<Permit, ErrKind> {
        self.acquire(job_weight(pixels), cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::Instant;

    fn never() -> CancelToken {
        CancelToken::never()
    }

    #[test]
    fn the_cap_is_the_minimum_of_the_three_rules() {
        // 25% of 16 GiB is 4 GiB; plenty free.
        assert_eq!(cap_for(16 * GIB, 12 * GIB), 4 * GIB);
        // Big machine: the 4 GiB ceiling wins.
        assert_eq!(cap_for(64 * GIB, 60 * GIB), 4 * GIB);
        // 25% of 8 GiB.
        assert_eq!(cap_for(8 * GIB, 7 * GIB), 2 * GIB);
        // Little free RAM: half of it.
        assert_eq!(cap_for(16 * GIB, 2 * GIB), GIB);
        assert_eq!(cap_for(0, 0), 0);
    }

    #[test]
    fn job_weight_is_nine_bytes_per_pixel_plus_64_mib() {
        assert_eq!(job_weight(0), 64 * MIB);
        // About 175 MB at 12 MP and about 1 GB at 100 MP (PLAN 2.8).
        let mb12 = job_weight(12_000_000) as f64 / 1e6;
        assert!((170.0..180.0).contains(&mb12), "{mb12}");
        let gb100 = job_weight(100_000_000) as f64 / 1e9;
        assert!((0.95..1.05).contains(&gb100), "{gb100}");
        assert_eq!(job_weight(u64::MAX), u64::MAX);
    }

    #[test]
    fn the_system_cap_is_sane() {
        let c = system_cap();
        assert!(c <= MAX_CAP_BYTES);
        assert!(MemoryBudget::from_system().cap() <= MAX_CAP_BYTES);
    }

    #[test]
    fn permits_return_their_bytes_and_zero_is_free() {
        let b = MemoryBudget::new(100);
        let p = b.acquire(60, &never()).unwrap();
        assert_eq!((b.stats().in_use, p.bytes()), (60, 60));
        let q = b.acquire(40, &never()).unwrap();
        assert_eq!(b.stats().in_use, 100);
        drop(p);
        assert_eq!(b.stats().in_use, 40);
        drop(q);
        let s = b.stats();
        assert_eq!((s.in_use, s.peak, s.waiting, s.admitted), (0, 100, 0, 2));
        let z = b.acquire(0, &never()).unwrap();
        assert_eq!(b.stats().admitted, 2, "zero-byte permits bypass the queue");
        drop(z);
    }

    #[test]
    fn admission_is_strictly_fifo() {
        let b = MemoryBudget::new(100);
        let hold = b.acquire(100, &never()).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        // Queue 60, 60, 10 in that order. After the hold is released the second 60 must be
        // admitted before the 10, although the 10 would fit alongside the first 60.
        for (name, w) in [("a60", 60u64), ("b60", 60), ("c10", 10)] {
            let (bb, order) = (b.clone(), order.clone());
            handles.push(thread::spawn(move || {
                let p = bb.acquire(w, &CancelToken::never()).unwrap();
                order.lock().unwrap().push(name);
                thread::sleep(Duration::from_millis(60));
                drop(p);
            }));
            // Make sure each is queued before the next one starts.
            let want = handles.len();
            let t0 = Instant::now();
            while b.stats().waiting < want && t0.elapsed() < Duration::from_secs(5) {
                thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(b.stats().waiting, want);
        }
        drop(hold);
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*order.lock().unwrap(), ["a60", "b60", "c10"]);
    }

    #[test]
    fn an_oversize_job_runs_alone_and_completes_in_five_seconds() {
        let b = MemoryBudget::new(100);
        let t0 = Instant::now();
        // A small job is running when the oversize one arrives.
        let small = b.acquire(30, &never()).unwrap();
        let (b2, got) = (b.clone(), Arc::new(AtomicUsize::new(0)));
        let got2 = got.clone();
        let big = thread::spawn(move || {
            let p = b2.acquire(1_000, &CancelToken::never()).unwrap();
            // Alone: the budget shows exactly this job and nothing else.
            assert_eq!(b2.stats().in_use, 1_000);
            got2.store(1, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(100));
            drop(p);
        });
        thread::sleep(Duration::from_millis(80));
        assert_eq!(got.load(Ordering::SeqCst), 0, "must wait for the small job");
        // Anything queued behind the oversize job waits for it too (FIFO).
        let (b3, late_in_use) = (b.clone(), Arc::new(AtomicUsize::new(usize::MAX)));
        let late2 = late_in_use.clone();
        let late = thread::spawn(move || {
            let p = b3.acquire(10, &CancelToken::never()).unwrap();
            late2.store(b3.stats().in_use as usize, Ordering::SeqCst);
            drop(p);
        });
        thread::sleep(Duration::from_millis(40));
        drop(small);
        big.join().unwrap();
        late.join().unwrap();
        assert_eq!(
            late_in_use.load(Ordering::SeqCst),
            10,
            "ran after, not beside"
        );
        assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
        assert_eq!(b.stats().in_use, 0);
        // On an idle budget an oversize job is admitted at once.
        let t1 = Instant::now();
        drop(b.acquire(u64::MAX, &never()).unwrap());
        assert!(t1.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_cancelled_waiter_leaves_the_queue_and_unblocks_the_next() {
        let b = MemoryBudget::new(100);
        let hold = b.acquire(90, &never()).unwrap();
        let token = CancelToken::new_batch();
        let (b1, t1) = (b.clone(), token.child());
        let blocked = thread::spawn(move || b1.acquire(50, &t1).map(|_| ()));
        while b.stats().waiting < 1 {
            thread::sleep(Duration::from_millis(2));
        }
        // A second waiter that fits once the first one is gone.
        let b2 = b.clone();
        let second =
            thread::spawn(move || b2.acquire(10, &CancelToken::never()).map(|p| p.bytes()));
        while b.stats().waiting < 2 {
            thread::sleep(Duration::from_millis(2));
        }
        let t0 = Instant::now();
        token.cancel();
        assert_eq!(blocked.join().unwrap(), Err(ErrKind::Cancelled));
        assert!(t0.elapsed() < Duration::from_secs(1));
        // The 10-byte waiter needed 10 <= 100-90, but was queued behind the 50: it runs now.
        assert_eq!(second.join().unwrap(), Ok(10));
        drop(hold);
        assert_eq!(b.stats().waiting, 0);
        // An already-cancelled token is refused without queueing.
        let dead = CancelToken::new_batch();
        dead.cancel();
        assert_eq!(b.acquire(1, &dead).unwrap_err(), ErrKind::Cancelled);
        assert_eq!(b.stats().waiting, 0);
        // A deadline works the same way.
        let full = b.acquire(100, &never()).unwrap();
        let timed = CancelToken::new_batch().child_with_timeout(Duration::from_millis(50));
        assert_eq!(b.acquire(1, &timed).unwrap_err(), ErrKind::DeadlineExceeded);
        drop(full);
    }

    /// A tiny deterministic generator so the stress run is reproducible.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }
    }

    #[test]
    fn eight_threads_never_exceed_the_cap() {
        const CAP: u64 = 1_000;
        let b = MemoryBudget::new(CAP);
        // (sum of held bytes, holders) as seen by the holders themselves.
        let seen = Arc::new(Mutex::new((0u64, 0u64)));
        let t0 = Instant::now();
        let handles: Vec<_> = (0..8u64)
            .map(|t| {
                let (b, seen) = (b.clone(), seen.clone());
                thread::spawn(move || {
                    let mut rng = Lcg(t + 1);
                    for _ in 0..300 {
                        // 1..=400 bytes, so up to two or three jobs fit together.
                        let w = 1 + rng.next() % 400;
                        let p = b.acquire(w, &CancelToken::never()).unwrap();
                        {
                            let mut s = seen.lock().unwrap();
                            s.0 += w;
                            s.1 += 1;
                            assert!(s.0 <= CAP, "{} bytes held across {} jobs", s.0, s.1);
                        }
                        if rng.next().is_multiple_of(4) {
                            thread::yield_now();
                        }
                        {
                            let mut s = seen.lock().unwrap();
                            s.0 -= w;
                            s.1 -= 1;
                        }
                        drop(p);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let s = b.stats();
        assert!(s.peak <= CAP, "peak {} over cap {CAP}", s.peak);
        assert_eq!((s.in_use, s.waiting, s.admitted), (0, 0, 8 * 300));
        assert!(t0.elapsed() < Duration::from_secs(60));
    }

    #[test]
    fn eight_threads_with_oversize_jobs_run_those_alone() {
        const CAP: u64 = 500;
        let b = MemoryBudget::new(CAP);
        let seen = Arc::new(Mutex::new((0u64, 0u64)));
        let handles: Vec<_> = (0..8u64)
            .map(|t| {
                let (b, seen) = (b.clone(), seen.clone());
                thread::spawn(move || {
                    let mut rng = Lcg(100 + t);
                    for _ in 0..120 {
                        let w = if rng.next().is_multiple_of(10) {
                            CAP + 1 + rng.next() % 500 // oversize
                        } else {
                            1 + rng.next() % 300
                        };
                        let p = b.acquire(w, &CancelToken::never()).unwrap();
                        {
                            let mut s = seen.lock().unwrap();
                            s.0 += w;
                            s.1 += 1;
                            assert!(
                                s.0 <= CAP || s.1 == 1,
                                "oversize shared: {} bytes, {} jobs",
                                s.0,
                                s.1
                            );
                        }
                        thread::yield_now();
                        {
                            let mut s = seen.lock().unwrap();
                            s.0 -= w;
                            s.1 -= 1;
                        }
                        drop(p);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(b.stats().in_use, 0);
    }
}
