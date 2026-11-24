//! L2 immutable-snapshot undo/redo history.

#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unimplemented,
    clippy::todo,
    clippy::unreachable
)]

use std::collections::VecDeque;
use std::sync::Arc;

use drawcraft_doc::Document;

#[derive(Clone, Debug)]
pub struct HistoryEntry {
    pub label: String,
    pub snapshot: Arc<Document>,
}

/// Bounded undo/redo stack holding `Arc<Document>` snapshots.
#[derive(Clone, Debug)]
pub struct History {
    capacity: usize,
    undo_stack: VecDeque<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
}

impl Default for History {
    fn default() -> Self {
        Self::new(100)
    }
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            undo_stack: VecDeque::with_capacity(capacity.min(128)),
            redo_stack: Vec::new(),
        }
    }

    pub fn record(&mut self, label: impl Into<String>, before: Arc<Document>) {
        if self.undo_stack.len() >= self.capacity {
            self.undo_stack.pop_front();
        }
        self.undo_stack.push_back(HistoryEntry {
            label: label.into(),
            snapshot: before,
        });
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, current: Arc<Document>) -> Option<(String, Arc<Document>)> {
        let entry = self.undo_stack.pop_back()?;
        let label = entry.label.clone();
        self.redo_stack.push(HistoryEntry {
            label: entry.label,
            snapshot: current,
        });
        Some((label, entry.snapshot))
    }

    pub fn redo(&mut self, current: Arc<Document>) -> Option<(String, Arc<Document>)> {
        let entry = self.redo_stack.pop()?;
        let label = entry.label.clone();
        self.undo_stack.push_back(HistoryEntry {
            label: entry.label,
            snapshot: current,
        });
        Some((label, entry.snapshot))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo_stack.back().map(|e| e.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo_stack.last().map(|e| e.label.as_str())
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use drawcraft_doc::PaperTemplate;

    #[test]
    fn undo_redo_round_trip_and_capacity_limit() {
        let mut h = History::new(2);
        let d0 = Arc::new(Document::default());
        let mut d1_val = (*d0).clone();
        d1_val.paper_template = PaperTemplate::Grid;
        let d1 = Arc::new(d1_val);

        let mut d2_val = (*d1).clone();
        d2_val.paper_template = PaperTemplate::Dotted;
        let d2 = Arc::new(d2_val);

        let mut d3_val = (*d2).clone();
        d3_val.paper_template = PaperTemplate::Cornell;
        let d3 = Arc::new(d3_val);

        h.record("Grid", Arc::clone(&d0));
        h.record("Dotted", Arc::clone(&d1));
        h.record("Cornell", Arc::clone(&d2));

        assert!(h.can_undo());
        assert!(!h.can_redo());

        let (l1, prev1) = h.undo(Arc::clone(&d3)).unwrap();
        assert_eq!(l1, "Cornell");
        assert_eq!(prev1.paper_template, PaperTemplate::Dotted);

        let (l2, prev2) = h.undo(prev1).unwrap();
        assert_eq!(l2, "Dotted");
        assert_eq!(prev2.paper_template, PaperTemplate::Grid);

        // Capacity was 2, so d0 was evicted.
        assert!(h.undo(prev2.clone()).is_none());

        let (r1, next1) = h.redo(prev2).unwrap();
        assert_eq!(r1, "Dotted");
        assert_eq!(next1.paper_template, PaperTemplate::Dotted);
    }
}
