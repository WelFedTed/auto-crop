// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Cooperative cancellation for long kernels (PLAN 2.8: checked every 64 rows).
//!
//! `auto-crop-core` does not yet export a `CancelToken` (PLAN 2.1 plans one), so the kernels in
//! this crate take any [`Cancel`] implementor. The core token will implement this trait when it
//! lands; until then `AtomicBool` and [`NeverCancel`] cover tests and callers without a token.

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

/// Returned by a kernel that stopped because its token was cancelled. No partial output escapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation cancelled")
    }
}

impl std::error::Error for Cancelled {}
