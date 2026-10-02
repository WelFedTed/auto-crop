// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Undo and redo (PLAN 2.6): a cursor over immutable snapshots, one entry per committed gesture,
//! plus a session log of multi-image commands that restores every affected cursor in one step.

use crate::edit::EditState;

/// Identifies one continuous gesture (a drag, a slider burst, a nudge run). Commits that carry the
/// id of the current top entry replace it instead of pushing a new one. The caller allocates ids
/// and ends a gesture on `phase: "end"` or after about 500 ms idle (PLAN 2.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GestureId(pub u64);

#[derive(Debug, Clone)]
struct Entry<T> {
    label: String,
    state: T,
    gesture: Option<GestureId>,
}

/// Linear undo and redo over complete states with a label per step. `entries[0]` is the initial
/// state; each later entry carries the label of the commit that produced it.
#[derive(Debug, Clone)]
pub struct History<T: Clone + PartialEq> {
    entries: Vec<Entry<T>>,
    cursor: usize,
    cap: usize,
    /// Entries dropped off the front by the cap, so [`History::position`] stays stable.
    trimmed: usize,
    /// "Is this commit a no-op?" Plain equality, or the render hash for edit states.
    same: fn(&T, &T) -> bool,
}

impl<T: Clone + PartialEq> History<T> {
    /// Entries kept per item (PLAN 2.6: 200).
    pub const DEFAULT_CAP: usize = 200;

    pub fn new(initial: T) -> Self {
        Self::with_equivalence(initial, |a, b| a == b)
    }

    /// A history that drops a commit when `same(current, new)` holds.
    pub fn with_equivalence(initial: T, same: fn(&T, &T) -> bool) -> Self {
        Self {
            entries: vec![Entry {
                label: String::new(),
                state: initial,
                gesture: None,
            }],
            cursor: 0,
            cap: Self::DEFAULT_CAP,
            trimmed: 0,
            same,
        }
    }

    /// Overrides the entry cap (at least 2 so one step is always undoable).
    pub fn with_cap(mut self, cap: usize) -> Self {
        self.cap = cap.max(2);
        self
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    pub fn current(&self) -> &T {
        &self.entries[self.cursor].state
    }

    /// Commits `state` as a new step. An unchanged state is dropped; the redo tail is cut.
    /// Returns whether the state changed.
    pub fn commit(&mut self, label: impl Into<String>, state: T) -> bool {
        self.commit_gesture(label, state, None)
    }

    /// [`History::commit`] with gesture coalescing: if `gesture` matches the gesture of the
    /// current entry, that entry is replaced (and the redo tail cut) instead of pushing, so one
    /// drag is one undo step. If the replacement returns to the previous entry's state the step
    /// disappears altogether.
    pub fn commit_gesture(
        &mut self,
        label: impl Into<String>,
        state: T,
        gesture: Option<GestureId>,
    ) -> bool {
        if (self.same)(self.current(), &state) {
            return false;
        }
        let coalesce =
            gesture.is_some() && self.cursor > 0 && self.entries[self.cursor].gesture == gesture;
        self.entries.truncate(self.cursor + 1);
        if coalesce {
            if (self.same)(&self.entries[self.cursor - 1].state, &state) {
                self.entries.pop();
                self.cursor -= 1;
            } else {
                let e = &mut self.entries[self.cursor];
                e.state = state;
                e.label = label.into();
            }
            return true;
        }
        self.entries.push(Entry {
            label: label.into(),
            state,
            gesture,
        });
        self.cursor += 1;
        if self.entries.len() > self.cap {
            self.entries.remove(0);
            self.cursor -= 1;
            self.trimmed += 1;
        }
        true
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    /// Label of the step `undo` would take back.
    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.entries[self.cursor].label.as_str())
    }

    /// Label of the step `redo` would reapply.
    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.entries[self.cursor + 1].label.as_str())
    }

    pub fn undo(&mut self) -> Option<&T> {
        if self.can_undo() {
            self.cursor -= 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<&T> {
        if self.can_redo() {
            self.cursor += 1;
            Some(self.current())
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    /// Stable position of the cursor: it does not change when old entries fall off the cap.
    pub fn position(&self) -> usize {
        self.trimmed + self.cursor
    }

    /// Moves the cursor to a [`History::position`] taken earlier. Returns `false` (and does
    /// nothing) if that entry has since been trimmed or cut.
    pub fn seek(&mut self, position: usize) -> bool {
        match position.checked_sub(self.trimmed) {
            Some(c) if c < self.entries.len() => {
                self.cursor = c;
                true
            }
            _ => false,
        }
    }
}

impl History<EditState> {
    /// A history of edit states whose "unchanged" test is an equal `render_hash` (PLAN 2.6): a
    /// commit that only touches provenance is dropped.
    pub fn for_edit(initial: EditState) -> Self {
        Self::with_equivalence(initial, |a, b| a.render_hash() == b.render_hash())
    }
}

/// One session-level command (apply to selection, remove, change enhancement for all): the
/// memento is, per affected image, its history position before and after the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCmd<K> {
    pub label: String,
    marks: Vec<(K, usize, usize)>,
}

impl<K> SessionCmd<K> {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            marks: Vec::new(),
        }
    }

    /// Records that `key`'s history moved from position `before` to `after`.
    pub fn mark(&mut self, key: K, before: usize, after: usize) {
        self.marks.push((key, before, after));
    }

    /// True if no affected history actually moved.
    pub fn is_noop(&self) -> bool {
        self.marks.iter().all(|(_, b, a)| b == a)
    }

    pub fn len(&self) -> usize {
        self.marks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.marks.is_empty()
    }
}

/// Undo and redo across images, capped like a per-image history.
#[derive(Debug, Clone)]
pub struct SessionHistory<K> {
    cmds: Vec<SessionCmd<K>>,
    cursor: usize,
    cap: usize,
}

impl<K> Default for SessionHistory<K> {
    fn default() -> Self {
        Self {
            cmds: Vec::new(),
            cursor: 0,
            cap: History::<()>::DEFAULT_CAP,
        }
    }
}

impl<K> SessionHistory<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an executed command; the redo tail is cut and a no-op command is ignored.
    pub fn push(&mut self, cmd: SessionCmd<K>) -> bool {
        if cmd.is_noop() {
            return false;
        }
        self.cmds.truncate(self.cursor);
        self.cmds.push(cmd);
        self.cursor += 1;
        if self.cmds.len() > self.cap {
            self.cmds.remove(0);
            self.cursor -= 1;
        }
        true
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.cmds.len()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.can_undo()
            .then(|| self.cmds[self.cursor - 1].label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.can_redo()
            .then(|| self.cmds[self.cursor].label.as_str())
    }

    /// Steps back one command, calling `restore(key, position)` once per affected image with the
    /// position it had before the command (use [`History::seek`]). Returns the command label.
    pub fn undo(&mut self, mut restore: impl FnMut(&K, usize)) -> Option<&str> {
        if !self.can_undo() {
            return None;
        }
        self.cursor -= 1;
        let cmd = &self.cmds[self.cursor];
        for (k, before, _) in &cmd.marks {
            restore(k, *before);
        }
        Some(cmd.label.as_str())
    }

    /// Redoes one command, restoring every affected image to the position it had after it.
    pub fn redo(&mut self, mut restore: impl FnMut(&K, usize)) -> Option<&str> {
        if !self.can_redo() {
            return None;
        }
        let cmd = &self.cmds[self.cursor];
        self.cursor += 1;
        for (k, _, after) in &cmd.marks {
            restore(k, *after);
        }
        Some(cmd.label.as_str())
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::Confidence;
    use crate::edit::Origin;
    use crate::geometry::QuadWarp;
    use proptest::prelude::*;

    #[test]
    fn history_undo_redo_and_labels() {
        let mut h = History::new(0);
        assert!(!h.can_undo() && !h.can_redo());
        assert!(h.commit("Move corner", 1));
        assert!(h.commit("Rotate", 2));
        assert_eq!(h.undo_label(), Some("Rotate"));
        assert_eq!(h.undo(), Some(&1));
        assert_eq!(h.redo_label(), Some("Rotate"));
        assert_eq!(h.undo(), Some(&0));
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), Some(&1));
        // Committing cuts the redo tail.
        assert!(h.commit("Move corner", 9));
        assert!(!h.can_redo());
        assert_eq!(*h.current(), 9);
    }

    #[test]
    fn history_drops_unchanged_states_and_caps() {
        let mut h = History::new(0);
        assert!(!h.commit("noop", 0));
        for i in 1..=(History::<i32>::DEFAULT_CAP as i32 + 50) {
            h.commit("step", i);
        }
        assert!(h.len() <= History::<i32>::DEFAULT_CAP);
        assert_eq!(*h.current(), History::<i32>::DEFAULT_CAP as i32 + 50);
        let mut undone = 0;
        while h.undo().is_some() {
            undone += 1;
        }
        assert_eq!(undone, History::<i32>::DEFAULT_CAP - 1);
    }

    #[test]
    fn a_matching_gesture_replaces_the_top_entry() {
        let mut h = History::new(0);
        let drag = Some(GestureId(1));
        assert!(h.commit_gesture("Move corner", 1, drag));
        assert!(h.commit_gesture("Move corner", 2, drag));
        assert!(h.commit_gesture("Move corner", 3, drag));
        assert_eq!(h.len(), 2, "one drag is one step");
        assert_eq!(h.undo(), Some(&0));
        assert_eq!(h.redo(), Some(&3));
        // A different gesture pushes.
        assert!(h.commit_gesture("Move corner", 4, Some(GestureId(2))));
        assert_eq!(h.len(), 3);
        // The same id after an interleaved commit does not reach back.
        assert!(h.commit_gesture("Late", 5, drag));
        assert_eq!(h.len(), 4);
        // No gesture never coalesces.
        assert!(h.commit("a", 6));
        assert!(h.commit("b", 7));
        assert_eq!(h.len(), 6);
    }

    #[test]
    fn coalescing_back_to_the_previous_state_removes_the_step() {
        let mut h = History::new(0);
        let g = Some(GestureId(7));
        h.commit_gesture("Drag", 5, g);
        assert_eq!(h.len(), 2);
        // Dragging back to where it started leaves no undo step behind.
        assert!(h.commit_gesture("Drag", 0, g));
        assert_eq!(h.len(), 1);
        assert!(!h.can_undo());
        assert_eq!(*h.current(), 0);
        // And the gesture id no longer matches anything: the next commit pushes.
        assert!(h.commit_gesture("Drag", 4, g));
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn a_gesture_commit_cuts_the_redo_tail() {
        let mut h = History::new(0);
        let g = Some(GestureId(1));
        h.commit_gesture("a", 1, g);
        h.commit("b", 2);
        h.undo();
        assert!(h.can_redo());
        // Matching gesture on the current entry replaces it and cuts redo.
        assert!(h.commit_gesture("a", 9, g));
        assert!(!h.can_redo());
        assert_eq!(h.len(), 2);
        assert_eq!(*h.current(), 9);
    }

    #[test]
    fn positions_survive_the_cap_and_seek_validates() {
        let mut h = History::new(0).with_cap(4);
        h.commit("1", 1);
        let p1 = h.position();
        h.commit("2", 2);
        h.commit("3", 3);
        assert_eq!(h.len(), 4);
        assert!(h.seek(p1));
        assert_eq!(*h.current(), 1);
        h.seek(h.position() + 2);
        for i in 4..10 {
            h.commit("n", i);
        }
        assert!(!h.seek(p1), "trimmed away");
        assert!(!h.seek(10_000));
        assert_eq!(*h.current(), 9);
    }

    #[test]
    fn edit_history_ignores_provenance_only_commits() {
        let a = EditState::single(QuadWarp::inset_frame(0.1));
        let mut h = History::for_edit(a.clone());
        let mut rescored = a.clone();
        rescored.items[0].origin = Origin::Auto { pipeline_ver: 2 };
        rescored.items[0].confidence = Some(Confidence {
            score: 0.5,
            forced: None,
            reasons: vec![],
        });
        assert!(!h.commit("Re-score", rescored), "same render hash: dropped");
        assert_eq!(h.len(), 1);
        let mut moved = a;
        moved.quad_mut().unwrap().corners[0].x = 0.2;
        assert!(h.commit("Move corner", moved));
    }

    #[test]
    fn session_commands_restore_every_affected_cursor_in_one_step() {
        let mut a = History::new(0);
        let mut b = History::new(10);
        let mut other = History::new(100);
        let mut log: SessionHistory<&str> = SessionHistory::new();

        let mut cmd = SessionCmd::new("Apply to selection");
        let (pa, pb) = (a.position(), b.position());
        a.commit("Apply", 1);
        b.commit("Apply", 11);
        cmd.mark("a", pa, a.position());
        cmd.mark("b", pb, b.position());
        assert!(log.push(cmd));
        other.commit("Unrelated", 101);

        let restore = |a: &mut History<i32>, b: &mut History<i32>, k: &&str, pos: usize| match *k {
            "a" => assert!(a.seek(pos)),
            _ => assert!(b.seek(pos)),
        };
        assert_eq!(log.undo_label(), Some("Apply to selection"));
        log.undo(|k, p| restore(&mut a, &mut b, k, p));
        assert_eq!((*a.current(), *b.current()), (0, 10));
        assert_eq!(*other.current(), 101, "unaffected images keep their state");
        assert!(log.can_redo());
        log.redo(|k, p| restore(&mut a, &mut b, k, p));
        assert_eq!((*a.current(), *b.current()), (1, 11));
        assert!(log.undo(|_, _| {}).is_some() && log.undo(|_, _| {}).is_none());
        // A no-op command is not recorded.
        let mut noop = SessionCmd::new("nothing");
        noop.mark("a", 1, 1);
        assert!(!log.push(noop));
    }

    #[derive(Debug, Clone)]
    enum Op {
        Commit(u8, Option<u8>),
        Undo,
        Redo,
    }

    fn ops() -> impl Strategy<Value = Vec<Op>> {
        prop::collection::vec(
            prop_oneof![
                (0u8..6, prop::option::of(0u8..3)).prop_map(|(v, g)| Op::Commit(v, g)),
                Just(Op::Undo),
                Just(Op::Redo),
            ],
            0..80,
        )
    }

    fn check_invariants<T: Clone + PartialEq + std::fmt::Debug>(h: &History<T>) {
        assert!(
            !h.is_empty() && h.len() <= h.cap(),
            "bounds: len {}",
            h.len()
        );
        assert!(h.cursor < h.len(), "cursor in range");
        assert_eq!(h.can_undo(), h.cursor > 0);
        assert_eq!(h.can_redo(), h.cursor + 1 < h.len());
        for w in h.entries.windows(2) {
            assert!(w[0].state != w[1].state, "adjacent entries differ");
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// undo(redo(x)) == x and redo(undo(x)) == x, plus the structural bounds, over random
        /// operation sequences with a tiny cap so trimming is exercised too.
        #[test]
        fn undo_of_redo_is_the_identity(seq in ops(), cap in 2usize..12) {
            let mut h = History::new(0u8).with_cap(cap);
            for op in seq {
                match op {
                    Op::Commit(v, g) => {
                        let before = *h.current();
                        let changed = h.commit_gesture("c", v, g.map(|g| GestureId(u64::from(g))));
                        prop_assert_eq!(changed, before != v);
                        prop_assert_eq!(*h.current(), if changed { v } else { before });
                    }
                    Op::Undo => {
                        let x = *h.current();
                        let pos = h.position();
                        if h.undo().is_some() {
                            prop_assert_eq!(h.position(), pos - 1);
                            prop_assert_eq!(h.redo().copied(), Some(x));
                            prop_assert_eq!(h.position(), pos);
                        }
                    }
                    Op::Redo => {
                        let x = *h.current();
                        let pos = h.position();
                        if h.redo().is_some() {
                            prop_assert_eq!(h.position(), pos + 1);
                            prop_assert_eq!(h.undo().copied(), Some(x));
                            prop_assert_eq!(h.position(), pos);
                        }
                    }
                }
                check_invariants(&h);
            }
        }

        /// The cap holds for any number of commits, and undoing all the way never underflows.
        #[test]
        fn the_cap_always_holds(n in 0usize..600) {
            let mut h = History::new(0usize);
            for i in 1..=n { h.commit("s", i); }
            prop_assert!(h.len() <= History::<usize>::DEFAULT_CAP);
            let mut steps = 0;
            while h.undo().is_some() { steps += 1; }
            prop_assert_eq!(steps, h.len() - 1);
        }

        /// The same law for real edit states under the render-hash equivalence.
        #[test]
        fn undo_of_redo_for_edit_states(seq in prop::collection::vec((0u8..5, prop::bool::ANY), 0..40)) {
            let mut h = History::for_edit(EditState::default());
            for (k, undo_first) in seq {
                if undo_first { h.undo(); }
                let s = EditState::single(QuadWarp::inset_frame(f64::from(k) / 10.0));
                h.commit("Edit", s);
                let x = h.current().render_hash();
                if h.undo().is_some() {
                    prop_assert_eq!(h.redo().map(EditState::render_hash), Some(x));
                }
                prop_assert!(h.len() <= h.cap());
            }
        }
    }
}
