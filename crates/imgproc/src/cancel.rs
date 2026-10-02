// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Cooperative cancellation for long kernels (PLAN 2.8: checked every 64 rows).
//!
//! The kernels in this crate take any [`Cancel`] implementor, so they stay usable without the
//! engine: `AtomicBool` and [`NeverCancel`] cover tests and simple callers, and core's
//! hierarchical [`CancelToken`] (batch, job, stage, generation, deadline) implements the trait,
//! so a render under a job token stops when the job is cancelled, superseded or out of time.

use auto_crop_core::{CancelToken, ErrKind};
use std::sync::atomic::{AtomicBool, Ordering};

/// A cancellation flag that can be polled from any worker thread.
pub trait Cancel: Sync {
    /// True once the work should stop as soon as practical.
    fn is_cancelled(&self) -> bool;
}

/// A token that is never cancelled.
#[derive(Debug, Clone, Copy, Default)]
pub struct NeverCancel;

impl Cancel for NeverCancel {
    fn is_cancelled(&self) -> bool {
        false
    }
}

impl Cancel for AtomicBool {
    fn is_cancelled(&self) -> bool {
        self.load(Ordering::Relaxed)
    }
}

impl Cancel for CancelToken {
    fn is_cancelled(&self) -> bool {
        self.status().is_some()
    }
}

/// Returned by a kernel that stopped because its token was cancelled. No partial output escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A kernel that was cancelled reports `ErrKind::Cancelled`. (A deadline or supersession is told
/// apart by the caller from the token's `status()`.)
impl From<Cancelled> for ErrKind {
    fn from(_: Cancelled) -> Self {
        ErrKind::Cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_tokens_drive_the_trait() {
        let batch = CancelToken::new_batch();
        let job = batch.child();
        assert!(!job.is_cancelled());
        batch.cancel();
        assert!(job.is_cancelled());
        assert!(!CancelToken::never().is_cancelled());
        assert_eq!(ErrKind::from(Cancelled), ErrKind::Cancelled);
    }
}
