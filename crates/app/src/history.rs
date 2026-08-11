//! Recent captures, so one can be copied again without retaking it.
//!
//! In daemon mode the editor is rebuilt for every capture, so the history
//! outlives it and is shared by reference. Entries hold the **encoded PNG**
//! rather than a decoded buffer: a handful of 4K frames as raw RGBA would be
//! hundreds of megabytes, while the same frames as PNG are a few, and the
//! clipboard needs them decoded only at the moment of use.
//!
//! History is deliberately in memory only. Writing recent screenshots to disk
//! behind the user's back would leave copies of whatever they captured — often
//! the very thing they redacted — lying around after the process exits.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// One remembered capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The annotated result, PNG-encoded.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// A short label for the menu.
    pub label: String,
}

impl Entry {
    pub fn size_label(&self) -> String {
        format!("{}×{}", self.width, self.height)
    }
}

/// A bounded, most-recent-first list of captures.
#[derive(Debug, Default)]
pub struct History {
    entries: VecDeque<Entry>,
    capacity: usize,
}

impl History {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            // Zero means zero: the settings slider offers it and describes it
            // as disabling the history, so clamping to 1 would keep a
            // screenshot the user asked it not to.
            capacity,
        }
    }

    pub fn shared(capacity: usize) -> Rc<RefCell<Self>> {
        Rc::new(RefCell::new(Self::new(capacity)))
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Change how many captures are kept, dropping any excess immediately.
    ///
    /// Dropping straight away is the point: lowering this is how a user says
    /// "stop holding my screenshots", so leaving the surplus in memory until
    /// some later eviction would ignore the request.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        while self.entries.len() > self.capacity {
            self.entries.pop_back();
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Kept alongside `len` because clippy rightly insists the pair exist
    /// together; the UI happens to ask only for the count.
    #[cfg_attr(not(test), expect(dead_code, reason = "the counterpart of len()"))]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Most recent first.
    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remember a capture, evicting the oldest if full.
    ///
    /// Re-copying the same image should not fill the history with duplicates,
    /// so an entry identical to the newest one is ignored.
    pub fn push(&mut self, entry: Entry) -> bool {
        // Zero means the user turned the history off.
        if self.capacity == 0 {
            return false;
        }
        if self
            .entries
            .front()
            .is_some_and(|newest| newest.png == entry.png)
        {
            return false;
        }
        self.entries.push_front(entry);
        while self.entries.len() > self.capacity {
            self.entries.pop_back();
        }
        true
    }

    /// Total bytes held, for diagnostics and for the settings window.
    pub fn memory_used(&self) -> usize {
        self.entries.iter().map(|e| e.png.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tag: u8, label: &str) -> Entry {
        Entry {
            png: vec![tag; 16],
            width: 100,
            height: 50,
            label: label.to_owned(),
        }
    }

    #[test]
    fn captures_come_back_most_recent_first() {
        let mut history = History::new(5);
        history.push(entry(1, "first"));
        history.push(entry(2, "second"));

        let labels: Vec<&str> = history.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["second", "first"]);
        assert_eq!(history.get(0).unwrap().label, "second");
    }

    #[test]
    fn the_oldest_capture_is_evicted_when_full() {
        let mut history = History::new(3);
        for i in 1..=5u8 {
            history.push(entry(i, &format!("shot {i}")));
        }
        assert_eq!(history.len(), 3);
        let labels: Vec<&str> = history.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["shot 5", "shot 4", "shot 3"]);
    }

    #[test]
    fn re_copying_the_same_image_does_not_duplicate_it() {
        let mut history = History::new(5);
        assert!(history.push(entry(1, "shot")));
        assert!(!history.push(entry(1, "shot again")), "identical bytes");
        assert_eq!(history.len(), 1);

        // A different image after it is still recorded.
        assert!(history.push(entry(2, "different")));
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn a_capacity_of_zero_keeps_nothing() {
        // The settings slider offers 0 and says "Zero disables it", so zero
        // has to mean zero. It previously clamped to 1 and kept a screenshot
        // the user had asked it not to.
        let mut history = History::new(0);
        assert_eq!(history.capacity(), 0);
        assert!(!history.push(entry(1, "shot")));
        assert!(history.is_empty());
    }

    #[test]
    fn lowering_the_capacity_drops_what_is_already_held() {
        let mut history = History::new(5);
        for i in 1..=5u8 {
            history.push(entry(i, &format!("shot {i}")));
        }
        assert_eq!(history.len(), 5);

        history.set_capacity(2);
        assert_eq!(history.len(), 2, "surplus captures must be released");
        let labels: Vec<&str> = history.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["shot 5", "shot 4"], "the newest are kept");

        // And to nothing at all.
        history.set_capacity(0);
        assert!(history.is_empty());
        assert_eq!(history.memory_used(), 0);
    }

    #[test]
    fn clearing_forgets_everything() {
        let mut history = History::new(3);
        history.push(entry(1, "a"));
        history.push(entry(2, "b"));
        history.clear();
        assert!(history.is_empty());
        assert_eq!(history.memory_used(), 0);
    }

    #[test]
    fn memory_use_tracks_what_is_held() {
        let mut history = History::new(2);
        history.push(entry(1, "a"));
        assert_eq!(history.memory_used(), 16);
        history.push(entry(2, "b"));
        assert_eq!(history.memory_used(), 32);
        // Evicting frees the oldest, so the total stops growing.
        history.push(entry(3, "c"));
        assert_eq!(history.memory_used(), 32);
    }

    #[test]
    fn entries_describe_their_size_for_the_menu() {
        assert_eq!(entry(1, "a").size_label(), "100×50");
    }

    #[test]
    fn a_shared_history_is_visible_through_every_handle() {
        // Daemon mode rebuilds the editor per capture, so the history has to
        // survive independently of it.
        let shared = History::shared(3);
        let other = Rc::clone(&shared);
        shared.borrow_mut().push(entry(1, "from the first editor"));
        assert_eq!(other.borrow().len(), 1);
        assert_eq!(
            other.borrow().get(0).unwrap().label,
            "from the first editor"
        );
    }
}
