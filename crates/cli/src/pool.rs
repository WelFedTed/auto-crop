// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! A small worker pool over `0..n`: `jobs` threads take the next index until none is left or the
//! batch is cancelled; results come back to the calling thread, in completion order, through one
//! callback (so printing and bookkeeping need no lock). A job already running when a cancel
//! arrives finishes: the engine's commit is never abandoned half way.

use auto_crop_core::CancelToken;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

pub fn run<T: Send>(
    n: usize,
    jobs: usize,
    cancel: &CancelToken,
    work: impl Fn(usize) -> T + Sync,
    mut on_result: impl FnMut(usize, T),
) {
    if n == 0 {
        return;
    }
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, T)>();
    std::thread::scope(|s| {
        for _ in 0..jobs.clamp(1, n) {
            let tx = tx.clone();
            let (next, work) = (&next, &work);
            s.spawn(move || {
                while !cancel.is_cancelled() {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= n {
                        break;
                    }
                    if tx.send((i, work(i))).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        for (i, v) in rx {
            on_result(i, v);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_index_runs_once_on_any_number_of_jobs() {
        for jobs in [1, 2, 8] {
            let mut seen = [0u32; 50];
            run(
                50,
                jobs,
                &CancelToken::never(),
                |i| i * 2,
                |i, v| {
                    assert_eq!(v, i * 2);
                    seen[i] += 1;
                },
            );
            assert!(seen.iter().all(|c| *c == 1), "{jobs} jobs");
        }
    }

    #[test]
    fn a_cancel_stops_the_taking_of_new_work() {
        let token = CancelToken::new_batch();
        let mut done = 0;
        run(
            1000,
            1,
            &token,
            |i| {
                if i == 3 {
                    token.cancel();
                }
            },
            |_, ()| done += 1,
        );
        assert_eq!(
            done, 4,
            "the job that was running finished, none started after"
        );
    }
}
