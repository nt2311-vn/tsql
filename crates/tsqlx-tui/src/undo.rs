#![allow(dead_code)]
//! Undo / redo history for the SQL editor.
//!
//! Pure data structure: snapshots of `(text, cursor)` pairs, two
//! stacks (past = `history`, alternative future = `future`). Any
//! fresh edit clears `future` — same semantics as vim's redo tree
//! pruning on a divergent branch. Capacity-bounded so a long session
//! doesn't grow without limit.

/// One frozen instant of the editor: the buffer text and the byte
/// offset of the cursor. Clones because we keep them by value in the
/// stacks — the editor buffers are small enough that this is fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub text: String,
    pub cursor: usize,
}

impl Snapshot {
    pub fn new(text: impl Into<String>, cursor: usize) -> Self {
        Self {
            text: text.into(),
            cursor,
        }
    }
}

/// Capacity-bounded undo / redo stack.
#[derive(Debug, Clone)]
pub struct UndoStack {
    history: Vec<Snapshot>,
    future: Vec<Snapshot>,
    max: usize,
}

impl UndoStack {
    /// Create a stack capped at `max` entries each side.
    pub fn new(max: usize) -> Self {
        Self {
            history: Vec::with_capacity(max),
            future: Vec::with_capacity(max),
            max,
        }
    }

    /// Push a snapshot onto `history`. Clears `future` (we're
    /// branching). If history would exceed `max`, drop the oldest.
    /// Does NOT push when the new snapshot equals the top of
    /// `history` — avoids noise from no-op operations.
    pub fn push(&mut self, snap: Snapshot) {
        if self.history.last() == Some(&snap) {
            return;
        }
        self.future.clear();
        self.history.push(snap);
        if self.history.len() > self.max {
            self.history.drain(0..self.history.len() - self.max);
        }
    }

    /// Pop the most-recent history entry, push `current` onto
    /// `future`, return the popped entry. `None` when history is
    /// empty. Caller is expected to replace the live buffer with the
    /// returned snapshot.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let popped = self.history.pop()?;
        self.future.push(current);
        if self.future.len() > self.max {
            self.future.drain(0..self.future.len() - self.max);
        }
        Some(popped)
    }

    /// Symmetric to [`Self::undo`]. Pops `future`, pushes `current`
    /// onto `history`, returns the popped entry.
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let popped = self.future.pop()?;
        self.history.push(current);
        if self.history.len() > self.max {
            self.history.drain(0..self.history.len() - self.max);
        }
        Some(popped)
    }

    /// Number of snapshots available for `undo` (not counting the
    /// live buffer the caller will pair with each pop).
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// Number of snapshots available for `redo`.
    pub fn future_len(&self) -> usize {
        self.future.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str, cursor: usize) -> Snapshot {
        Snapshot::new(text, cursor)
    }

    #[test]
    fn new_is_empty() {
        let u = UndoStack::new(10);
        assert_eq!(u.history_len(), 0);
        assert_eq!(u.future_len(), 0);
    }

    #[test]
    fn push_increments_history() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 0));
        assert_eq!(u.history_len(), 1);
        assert_eq!(u.future_len(), 0);
    }

    #[test]
    fn undo_returns_lifo() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("b", 2));
        u.push(s("c", 3));
        let r1 = u.undo(s("live", 99)).unwrap();
        let r2 = u.undo(s("live2", 100)).unwrap();
        let r3 = u.undo(s("live3", 101)).unwrap();
        assert_eq!(r1, s("c", 3));
        assert_eq!(r2, s("b", 2));
        assert_eq!(r3, s("a", 1));
    }

    #[test]
    fn undo_on_empty_returns_none_and_leaves_future_untouched() {
        let mut u = UndoStack::new(10);
        assert!(u.undo(s("live", 0)).is_none());
        assert_eq!(u.future_len(), 0);
        assert_eq!(u.history_len(), 0);
    }

    #[test]
    fn redo_round_trips_after_undo() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("b", 2));
        let popped = u.undo(s("c", 3)).unwrap();
        assert_eq!(popped, s("b", 2));
        assert_eq!(u.future_len(), 1);
        let r = u.redo(s("b", 2)).unwrap();
        assert_eq!(r, s("c", 3));
        assert_eq!(u.history_len(), 2);
        assert_eq!(u.future_len(), 0);
    }

    #[test]
    fn push_after_undo_clears_future() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("b", 2));
        u.undo(s("c", 3));
        assert_eq!(u.future_len(), 1);
        u.push(s("d", 4));
        assert_eq!(u.future_len(), 0);
    }

    #[test]
    fn cap_drops_oldest() {
        let mut u = UndoStack::new(3);
        u.push(s("a", 1));
        u.push(s("b", 2));
        u.push(s("c", 3));
        u.push(s("d", 4));
        assert_eq!(u.history_len(), 3);
        let r = u.undo(s("live", 0)).unwrap();
        assert_eq!(r, s("d", 4));
        let r = u.undo(s("live", 0)).unwrap();
        assert_eq!(r, s("c", 3));
        let r = u.undo(s("live", 0)).unwrap();
        assert_eq!(r, s("b", 2));
        assert!(u.undo(s("live", 0)).is_none());
    }

    #[test]
    fn dedup_same_snapshot_twice() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("a", 1));
        assert_eq!(u.history_len(), 1);
    }

    #[test]
    fn dedup_requires_cursor_match() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("a", 2));
        assert_eq!(u.history_len(), 2);
    }

    #[test]
    fn max_zero_accepts_push_without_panic() {
        let mut u = UndoStack::new(0);
        u.push(s("a", 1));
        u.push(s("b", 2));
        assert_eq!(u.history_len(), 0);
        assert_eq!(u.future_len(), 0);
        assert!(u.undo(s("live", 0)).is_none());
    }

    #[test]
    fn fresh_push_invalidates_redo_target() {
        let mut u = UndoStack::new(10);
        u.push(s("a", 1));
        u.push(s("b", 2));
        u.undo(s("c", 3));
        u.push(s("d", 4));
        assert!(u.redo(s("live", 0)).is_none());
    }

    #[test]
    fn redo_on_empty_returns_none() {
        let mut u = UndoStack::new(10);
        assert!(u.redo(s("live", 0)).is_none());
        u.push(s("a", 1));
        assert!(u.redo(s("live", 0)).is_none());
    }
}
