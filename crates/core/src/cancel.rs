// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Hierarchical cancellation (PLAN 2.8): batch, job and stage tokens with a generation check and a
//! deadline. Cancelling a parent cancels every descendant; cancelling a child never reaches its
//! parent or siblings. Long kernels call [`CancelToken::check_band`] once per row and the token
//! looks at the flags every [`BAND_ROWS`] rows.

use crate::error::ErrKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Rows between cancellation checks (PLAN 2.5: "every 64 rows").
pub const BAND_ROWS: u32 = 64;

/// Where a token sits in the hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Batch,
    Job,
    Stage,
}

/// Why work should stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interrupt {
    /// A token in the chain was cancelled.
    Cancelled,
    /// A newer generation was requested for the same image and level (PLAN 2.5).
    Superseded,
    /// A deadline in the chain passed.
    Deadline,
}

impl From<Interrupt> for ErrKind {
    fn from(i: Interrupt) -> Self {
        match i {
            Interrupt::Cancelled | Interrupt::Superseded => ErrKind::Cancelled,
            Interrupt::Deadline => ErrKind::DeadlineExceeded,
        }
    }
}

/// The per-image "latest requested generation", shared by the engine and its workers. A token
/// created with [`CancelToken::child_for_generation`] is superseded once the counter moves on.
#[derive(Debug, Clone, Default)]
pub struct GenerationCounter(Arc<AtomicU64>);

impl GenerationCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Starts a new generation and returns its number.
    pub fn bump(&self) -> u64 {
        self.0.fetch_add(1, Ordering::AcqRel) + 1
    }
}

#[derive(Debug)]
struct Node {
    level: Level,
    cancelled: AtomicBool,
    /// A shielded token ignores `cancel()` and has no parent (the `Committing` section, 2.8).
    shielded: bool,
    parent: Option<Arc<Node>>,
    deadline: Option<Instant>,
    generation: Option<(GenerationCounter, u64)>,
}

impl Node {
    fn own_status(&self) -> Option<Interrupt> {
        if self.cancelled.load(Ordering::Acquire) {
            return Some(Interrupt::Cancelled);
        }
        if let Some((latest, mine)) = &self.generation
            && latest.get() != *mine
        {
            return Some(Interrupt::Superseded);
        }
        if let Some(d) = self.deadline
            && Instant::now() >= d
        {
            return Some(Interrupt::Deadline);
        }
        None
    }
}

/// A cheap, cloneable handle. Clones share state.
#[derive(Debug, Clone)]
pub struct CancelToken {
    node: Arc<Node>,
}

impl CancelToken {
    fn make(
        level: Level,
        parent: Option<Arc<Node>>,
        deadline: Option<Instant>,
        generation: Option<(GenerationCounter, u64)>,
    ) -> Self {
        Self {
            node: Arc::new(Node {
                level,
                cancelled: AtomicBool::new(false),
                shielded: false,
                parent,
                deadline,
                generation,
            }),
        }
    }

    /// A root token for a batch.
    pub fn new_batch() -> Self {
        Self::make(Level::Batch, None, None, None)
    }

    /// A token that never fires: for the shielded `Committing` section and for callers with
    /// nothing to cancel. `cancel()` on it does nothing.
    pub fn never() -> Self {
        Self {
            node: Arc::new(Node {
                level: Level::Stage,
                cancelled: AtomicBool::new(false),
                shielded: true,
                parent: None,
                deadline: None,
                generation: None,
            }),
        }
    }

    pub fn level(&self) -> Level {
        self.node.level
    }

    fn child_level(&self) -> Level {
        match self.node.level {
            Level::Batch => Level::Job,
            Level::Job | Level::Stage => Level::Stage,
        }
    }

    /// A child one level down (batch to job, job to stage, stage to stage). Cancelled when this
    /// token is, but never the other way round.
    pub fn child(&self) -> Self {
        Self::make(self.child_level(), Some(self.node.clone()), None, None)
    }

    /// A child that also fires when `timeout` has elapsed.
    pub fn child_with_timeout(&self, timeout: Duration) -> Self {
        self.child_with_deadline(Instant::now() + timeout)
    }

    pub fn child_with_deadline(&self, deadline: Instant) -> Self {
        Self::make(
            self.child_level(),
            Some(self.node.clone()),
            Some(deadline),
            None,
        )
    }

    /// A child that fires once `latest` no longer equals `generation`.
    pub fn child_for_generation(&self, latest: &GenerationCounter, generation: u64) -> Self {
        Self::make(
            self.child_level(),
            Some(self.node.clone()),
            None,
            Some((latest.clone(), generation)),
        )
    }

    /// Cancels this token and, through the parent links, every descendant.
    pub fn cancel(&self) {
        if !self.node.shielded {
            self.node.cancelled.store(true, Ordering::Release);
        }
    }

    /// Why work under this token should stop, or `None` to carry on. Walks the (at most a few
    /// links long) parent chain; an explicit cancel wins over supersession over a deadline.
    pub fn status(&self) -> Option<Interrupt> {
        let mut worst: Option<Interrupt> = None;
        let mut node = Some(&self.node);
        while let Some(n) = node {
            match n.own_status() {
                Some(Interrupt::Cancelled) => return Some(Interrupt::Cancelled),
                Some(Interrupt::Superseded) => worst = Some(Interrupt::Superseded),
                Some(Interrupt::Deadline) => worst = worst.or(Some(Interrupt::Deadline)),
                None => {}
            }
            node = n.parent.as_ref();
        }
        worst
    }

    pub fn is_cancelled(&self) -> bool {
        self.status().is_some()
    }

    pub fn check(&self) -> Result<(), Interrupt> {
        match self.status() {
            Some(i) => Err(i),
            None => Ok(()),
        }
    }

    /// Call once per output row: looks at the token every [`BAND_ROWS`] rows (row 0 included) and
    /// is a single modulo otherwise.
    #[inline]
    pub fn check_band(&self, row: u32) -> Result<(), Interrupt> {
        if row.is_multiple_of(BAND_ROWS) {
            self.check()
        } else {
            Ok(())
        }
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::never()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_cancels_children_only() {
        let batch = CancelToken::new_batch();
        let job_a = batch.child();
        let job_b = batch.child();
        let stage = job_a.child();
        assert_eq!(
            (batch.level(), job_a.level(), stage.level()),
            (Level::Batch, Level::Job, Level::Stage)
        );

        // A child never reaches its parent or siblings.
        stage.cancel();
        assert!(stage.is_cancelled());
        assert!(!job_a.is_cancelled() && !job_b.is_cancelled() && !batch.is_cancelled());

        job_a.cancel();
        assert!(job_a.is_cancelled() && !job_b.is_cancelled() && !batch.is_cancelled());

        // A parent reaches every descendant.
        let stage_b = job_b.child();
        assert!(!stage_b.is_cancelled());
        batch.cancel();
        assert!(job_b.is_cancelled() && stage_b.is_cancelled());
        assert_eq!(stage_b.check(), Err(Interrupt::Cancelled));
    }

    #[test]
    fn generation_supersedes() {
        let latest = GenerationCounter::new();
        let g1 = latest.bump();
        let t = CancelToken::new_batch().child_for_generation(&latest, g1);
        assert_eq!(t.check(), Ok(()));
        let g2 = latest.bump();
        assert_ne!(g1, g2);
        assert_eq!(t.check(), Err(Interrupt::Superseded));
        assert_eq!(ErrKind::from(Interrupt::Superseded), ErrKind::Cancelled);
        let fresh = CancelToken::new_batch().child_for_generation(&latest, g2);
        assert_eq!(fresh.check(), Ok(()));
    }

    #[test]
    fn deadline_fires_and_maps_to_a_code() {
        let t = CancelToken::new_batch().child_with_timeout(Duration::from_millis(30));
        assert_eq!(t.check(), Ok(()));
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(t.check(), Err(Interrupt::Deadline));
        assert_eq!(
            ErrKind::from(Interrupt::Deadline),
            ErrKind::DeadlineExceeded
        );
        // An explicit cancel outranks a passed deadline.
        t.cancel();
        assert_eq!(t.check(), Err(Interrupt::Cancelled));
    }

    #[test]
    fn check_band_only_looks_every_64_rows() {
        let t = CancelToken::new_batch();
        t.cancel();
        assert!(t.check_band(0).is_err());
        assert!(t.check_band(1).is_ok());
        assert!(t.check_band(63).is_ok());
        assert!(t.check_band(64).is_err());
        assert!(t.check_band(128).is_err());
        // A kernel noticing within one band after a cancel.
        let live = CancelToken::new_batch();
        let mut stopped_at = None;
        for row in 0..1000u32 {
            if row == 100 {
                live.cancel();
            }
            if live.check_band(row).is_err() {
                stopped_at = Some(row);
                break;
            }
        }
        assert_eq!(stopped_at, Some(128));
    }

    #[test]
    fn a_shielded_token_ignores_cancel() {
        let t = CancelToken::never();
        t.cancel();
        assert!(!t.is_cancelled());
        let parent = CancelToken::new_batch();
        parent.cancel();
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn clones_share_state_across_threads() {
        let t = CancelToken::new_batch();
        let c = t.child();
        let h = std::thread::spawn(move || {
            while !c.is_cancelled() {
                std::thread::yield_now();
            }
        });
        t.cancel();
        h.join().unwrap();
    }
}
