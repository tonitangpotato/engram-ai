//! Bounded coreference window for conversational ingest (ISS-162).
//!
//! A [`TurnWindow`] keeps the most recent `capacity` conversation turns,
//! oldest-first, so a caller replaying or streaming a dialogue can hand each
//! turn's preceding context to [`crate::memory::Memory::ingest_turn`] without
//! reimplementing ring-buffer bookkeeping.
//!
//! Why this exists: the extractor sees one turn at a time, so a bare reply
//! ("Luna and Oliver!") loses its referent and the self-contained gold fact
//! is never stored. Supplying the preceding turns as coreference-only context
//! lets the extractor resolve the referent at store time. The ISS-201 /
//! ISS-162 sweeps showed `capacity = 4` lifts conv-26 LoCoMo overall J from
//! 0.2697 → 0.3882 (window=1 alone already recovers most of the gain, and the
//! lift survives the canonical `FACTUAL_REWEIGHT=on` envelope).
//!
//! # Example
//!
//! ```
//! use engramai::turn_window::TurnWindow;
//!
//! let mut win = TurnWindow::new(4);
//! // First turn has no preceding context.
//! assert!(win.context().is_empty());
//! win.push("Have you thought about adopting?");
//!
//! // Second turn sees the question as context.
//! assert_eq!(win.context(), &["Have you thought about adopting?".to_string()]);
//! win.push("Researching adoption agencies");
//! ```

/// The default coreference window size (ISS-162).
///
/// Pinned to 4 from the isolation sweep: window=4 gave the lowest residual
/// SEMANTIC-GAP count (11) and the highest single-hop score across both the
/// reservation and reweight envelopes. window=1 already moves the needle but
/// leaves more coref-dependent gold turns stranded.
pub const DEFAULT_WINDOW: usize = 4;

/// A bounded, oldest-first window of recent conversation turns.
///
/// Capacity 0 disables windowing entirely: [`context`](Self::context) always
/// returns an empty slice, so ingest is byte-identical to the pre-ISS-162
/// path. This makes `TurnWindow::new(0)` a safe no-op toggle.
#[derive(Debug, Clone)]
pub struct TurnWindow {
    capacity: usize,
    turns: std::collections::VecDeque<String>,
}

impl TurnWindow {
    /// Create a window holding at most `capacity` preceding turns.
    ///
    /// `capacity = 0` disables windowing (context is always empty).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            turns: std::collections::VecDeque::with_capacity(capacity),
        }
    }

    /// Create a window with the [`DEFAULT_WINDOW`] capacity.
    pub fn with_default() -> Self {
        Self::new(DEFAULT_WINDOW)
    }

    /// The current preceding-turn context, oldest-first.
    ///
    /// Hand this to [`crate::memory::Memory::ingest_turn`] as the `context`
    /// argument for the turn you are about to ingest, then [`push`](Self::push)
    /// that turn so it becomes context for the next one.
    pub fn context(&self) -> Vec<String> {
        self.turns.iter().cloned().collect()
    }

    /// Append a turn, evicting the oldest if at capacity.
    ///
    /// No-op when `capacity == 0`.
    pub fn push(&mut self, turn: impl Into<String>) {
        if self.capacity == 0 {
            return;
        }
        if self.turns.len() == self.capacity {
            self.turns.pop_front();
        }
        self.turns.push_back(turn.into());
    }

    /// Drop all buffered turns (e.g. at a session / conversation boundary)
    /// so coreference does not leak across unrelated dialogues.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    /// Number of turns currently buffered.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether the window currently holds no turns.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// The configured maximum window size.
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_yields_no_context() {
        let win = TurnWindow::new(4);
        assert!(win.context().is_empty());
        assert!(win.is_empty());
        assert_eq!(win.len(), 0);
    }

    #[test]
    fn evicts_oldest_at_capacity() {
        let mut win = TurnWindow::new(3);
        win.push("a");
        win.push("b");
        win.push("c");
        assert_eq!(win.context(), vec!["a", "b", "c"]);
        // Fourth push evicts "a".
        win.push("d");
        assert_eq!(win.context(), vec!["b", "c", "d"]);
        assert_eq!(win.len(), 3);
        assert_eq!(win.capacity(), 3);
    }

    #[test]
    fn capacity_zero_is_noop() {
        let mut win = TurnWindow::new(0);
        win.push("a");
        win.push("b");
        assert!(win.context().is_empty());
        assert!(win.is_empty());
    }

    #[test]
    fn clear_resets_window() {
        let mut win = TurnWindow::with_default();
        assert_eq!(win.capacity(), DEFAULT_WINDOW);
        win.push("a");
        win.push("b");
        assert_eq!(win.len(), 2);
        win.clear();
        assert!(win.is_empty());
        assert!(win.context().is_empty());
    }
}
