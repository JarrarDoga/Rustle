//! The play queue: one list in play order, a current entry, shuffle and repeat.
//!
//! Pure data, no I/O. The UI and the backend both run these operations (the UI
//! optimistically, the backend for real), so everything here is deterministic:
//! shuffling takes its seed from the caller.

use std::sync::Arc;

use model::Track;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

impl Repeat {
    /// Off, then all, then one, then off again (the order of the button).
    pub fn cycle(self) -> Self {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

/// One slot in the queue. `uid` tells two copies of the same track apart.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub uid: u64,
    pub track: Arc<Track>,
}

#[derive(Debug, Clone, Default)]
pub struct Queue {
    /// Play order. Entries before the current one are history.
    entries: Vec<Entry>,
    current: Option<u64>,
    shuffle: bool,
    repeat: Repeat,
    /// While shuffle is on: every uid in its unshuffled order, so turning
    /// shuffle off restores it.
    unshuffled: Vec<u64>,
    next_uid: u64,
}

impl Queue {
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn current_index(&self) -> Option<usize> {
        let uid = self.current?;
        self.entries.iter().position(|e| e.uid == uid)
    }

    pub fn current(&self) -> Option<&Entry> {
        self.current_index().map(|i| &self.entries[i])
    }

    /// Index of the first entry after the current one.
    pub fn upcoming_start(&self) -> usize {
        self.current_index().map_or(0, |i| i + 1)
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    fn mint(&mut self, track: Arc<Track>) -> Entry {
        self.next_uid += 1;
        Entry {
            uid: self.next_uid,
            track,
        }
    }

    /// Replaces the queue with `tracks` and makes `tracks[start]` current.
    /// With shuffle on, the chosen track plays first and the rest are shuffled.
    pub fn play_list(&mut self, tracks: &[Arc<Track>], start: usize, seed: u64) -> Option<&Entry> {
        if tracks.is_empty() {
            return None;
        }
        let start = start.min(tracks.len() - 1);
        self.entries = tracks.iter().map(|t| self.mint_owned(t)).collect();
        self.current = Some(self.entries[start].uid);
        if self.shuffle {
            self.unshuffled = self.entries.iter().map(|e| e.uid).collect();
            let chosen = self.entries.remove(start);
            self.entries.insert(0, chosen);
            shuffle_slice(&mut self.entries[1..], seed);
        }
        self.current()
    }

    fn mint_owned(&mut self, track: &Arc<Track>) -> Entry {
        self.mint(Arc::clone(track))
    }

    /// Inserts `track` right after the current entry and makes it current.
    pub fn play_now(&mut self, track: Arc<Track>) -> &Entry {
        let at = self.insert_after_current(track);
        self.current = Some(self.entries[at].uid);
        &self.entries[at]
    }

    /// Inserts `track` right after the current entry.
    pub fn play_next(&mut self, track: Arc<Track>) {
        self.insert_after_current(track);
    }

    /// Appends `track` to the end of the queue.
    pub fn add(&mut self, track: Arc<Track>) {
        let entry = self.mint(track);
        if self.shuffle {
            self.unshuffled.push(entry.uid);
        }
        self.entries.push(entry);
    }

    fn insert_after_current(&mut self, track: Arc<Track>) -> usize {
        let entry = self.mint(track);
        if self.shuffle {
            let after = self
                .current
                .and_then(|c| self.unshuffled.iter().position(|&u| u == c))
                .map_or(0, |i| i + 1);
            self.unshuffled.insert(after, entry.uid);
        }
        let at = self.upcoming_start();
        self.entries.insert(at, entry);
        at
    }

    /// Moves the entry at `from` so it lands at index `to` (both in play order).
    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() {
            return;
        }
        let entry = self.entries.remove(from);
        let to = to.min(self.entries.len());
        self.entries.insert(to, entry);
    }

    /// Removes the entry at `index`. If it was current, the entry that
    /// followed it becomes current (or none, at the end) and this returns true.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.entries.len() {
            return false;
        }
        let removed = self.entries.remove(index);
        self.unshuffled.retain(|&u| u != removed.uid);
        if self.current != Some(removed.uid) {
            return false;
        }
        self.current = self.entries.get(index).map(|e| e.uid);
        true
    }

    /// Makes the entry at `index` current.
    pub fn jump(&mut self, index: usize) -> Option<&Entry> {
        let uid = self.entries.get(index)?.uid;
        self.current = Some(uid);
        self.current()
    }

    /// Turning shuffle on keeps history and the current entry in place and
    /// shuffles what follows. Turning it off restores the original order.
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.unshuffled = self.entries.iter().map(|e| e.uid).collect();
            let start = self.upcoming_start();
            shuffle_slice(&mut self.entries[start..], seed);
        } else {
            let order = std::mem::take(&mut self.unshuffled);
            let rank = |uid: u64| order.iter().position(|&u| u == uid).unwrap_or(usize::MAX);
            self.entries.sort_by_key(|e| rank(e.uid));
        }
    }

    /// Moves to the next entry. `auto` is true when the current track ended
    /// by itself; only then does repeat-one replay it (a manual next with
    /// repeat-one wraps like repeat-all). Returns `None` (and
    /// keeps the current entry) when playback should stop.
    pub fn advance(&mut self, auto: bool) -> Option<&Entry> {
        let Some(i) = self.current_index() else {
            return self.jump(0);
        };
        if auto && self.repeat == Repeat::One {
            return self.current();
        }
        if i + 1 < self.entries.len() {
            return self.jump(i + 1);
        }
        if self.repeat == Repeat::Off {
            return None;
        }
        self.jump(0)
    }

    // b3-queue: the methods below are stubs until the queue worker
    // implements them.

    /// Replaces the queue with `tracks` (a scope of the main playlist),
    /// shuffled with `seed` or in order, starting at the first entry.
    /// `label` is the "Playing from" text, e.g. "Saint Juniper · 41".
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn play_scope(
        &mut self,
        _tracks: &[Arc<Track>],
        _label: String,
        _shuffle: bool,
        _seed: u64,
    ) -> Option<&Entry> {
        None
    }

    /// The "Playing from" label of the scope the queue was built from.
    #[allow(dead_code)] // b3-queue; read by the UI batch
    pub fn playing_from(&self) -> Option<&str> {
        None
    }

    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn set_autoplay(&mut self, _on: bool) {}

    #[allow(dead_code)] // b3-queue; read by the UI batch
    pub fn autoplay(&self) -> bool {
        false
    }

    /// True when autoplay is on and the queue has run out (or is about to),
    /// so the backend should fetch radio for the last track. Pure query: it
    /// stays true until `extend_radio` is called, because the reducer runs
    /// on both the UI and the backend and must stay deterministic.
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn needs_radio(&self) -> bool {
        false
    }

    /// Appends radio tracks after the last entry, marked as autoplay radio
    /// (not in the main playlist). Clears `needs_radio` for this run-out.
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn extend_radio(&mut self, _tracks: Vec<Arc<Track>>) {}

    /// Moves to the previous entry. At the start it wraps with repeat-all and
    /// otherwise stays on the first entry.
    pub fn back(&mut self) -> Option<&Entry> {
        let i = self.current_index()?;
        if i > 0 {
            return self.jump(i - 1);
        }
        if self.repeat == Repeat::All {
            let last = self.entries.len() - 1;
            return self.jump(last);
        }
        self.current()
    }
}

/// Fisher-Yates with xorshift64*. Deterministic for a given seed.
fn shuffle_slice<T>(items: &mut [T], seed: u64) {
    let mut s = seed | 1;
    let mut next = || {
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        s.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    for i in (1..items.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn track(id: &str) -> Arc<Track> {
        Arc::new(Track {
            id: id.to_owned(),
            title: id.to_uppercase(),
            artists: Vec::new(),
            album: None,
            duration: Some(std::time::Duration::from_secs(100)),
            thumbnails: Vec::new(),
        })
    }

    fn tracks(ids: &[&str]) -> Vec<Arc<Track>> {
        ids.iter().map(|id| track(id)).collect()
    }

    fn ids(q: &Queue) -> Vec<String> {
        q.entries().iter().map(|e| e.track.id.clone()).collect()
    }

    fn current_id(q: &Queue) -> Option<String> {
        q.current().map(|e| e.track.id.clone())
    }

    #[test]
    fn play_list_starts_at_chosen_track() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 1, 7);
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        assert_eq!(q.upcoming_start(), 2);
    }

    #[test]
    fn play_next_goes_right_after_current() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 0, 7);
        q.play_next(track("x"));
        q.play_next(track("y"));
        assert_eq!(ids(&q), ["a", "y", "x", "b", "c"]);
        q.add(track("z"));
        assert_eq!(ids(&q), ["a", "y", "x", "b", "c", "z"]);
        assert_eq!(current_id(&q).as_deref(), Some("a"));
    }

    #[test]
    fn play_now_becomes_current_and_keeps_the_rest() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 7);
        q.play_now(track("x"));
        assert_eq!(ids(&q), ["a", "x", "b"]);
        assert_eq!(current_id(&q).as_deref(), Some("x"));
        q.advance(false);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn reorder_keeps_current_entry() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d"]), 1, 7);
        q.move_entry(3, 2);
        assert_eq!(ids(&q), ["a", "b", "d", "c"]);
        q.move_entry(1, 3);
        assert_eq!(ids(&q), ["a", "d", "c", "b"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        q.move_entry(99, 0);
        assert_eq!(ids(&q), ["a", "d", "c", "b"]);
    }

    #[test]
    fn duplicates_are_distinct_entries() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "a"]), 1, 7);
        assert_eq!(q.current_index(), Some(1));
        assert!(!q.remove(0));
        assert_eq!(q.current_index(), Some(0));
    }

    #[test]
    fn remove_current_moves_to_following_entry() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 1, 7);
        assert!(q.remove(1));
        assert_eq!(current_id(&q).as_deref(), Some("c"));
        assert!(!q.remove(0));
        assert_eq!(current_id(&q).as_deref(), Some("c"));
        assert!(q.remove(0));
        assert!(q.current().is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn advance_with_repeat_off_stops_at_end() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 7);
        assert_eq!(q.advance(true).map(|e| e.track.id.as_str()), Some("b"));
        assert!(q.advance(true).is_none());
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn advance_with_repeat_all_wraps() {
        let mut q = Queue::default();
        q.set_repeat(Repeat::All);
        q.play_list(&tracks(&["a", "b"]), 1, 7);
        assert_eq!(q.advance(true).map(|e| e.track.id.as_str()), Some("a"));
        assert_eq!(q.back().map(|e| e.track.id.as_str()), Some("b"));
    }

    #[test]
    fn repeat_one_replays_only_on_track_end() {
        let mut q = Queue::default();
        q.set_repeat(Repeat::One);
        q.play_list(&tracks(&["a", "b"]), 0, 7);
        assert_eq!(q.advance(true).map(|e| e.track.id.as_str()), Some("a"));
        assert_eq!(q.advance(false).map(|e| e.track.id.as_str()), Some("b"));
        // A manual next at the end wraps, as with repeat-all.
        assert_eq!(q.advance(false).map(|e| e.track.id.as_str()), Some("a"));
    }

    #[test]
    fn back_at_start_stays_without_repeat() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 7);
        assert_eq!(q.back().map(|e| e.track.id.as_str()), Some("a"));
    }

    #[test]
    fn shuffle_keeps_history_and_current_then_restores() {
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut q = Queue::default();
        q.play_list(&tracks(&names), 2, 7);
        q.set_shuffle(true, 12345);
        let shuffled = ids(&q);
        assert_eq!(&shuffled[..3], ["a", "b", "c"]);
        let mut rest = shuffled[3..].to_vec();
        assert_ne!(rest, ["d", "e", "f", "g", "h"], "seed should reorder");
        rest.sort();
        assert_eq!(rest, ["d", "e", "f", "g", "h"]);
        assert_eq!(current_id(&q).as_deref(), Some("c"));

        // Additions while shuffled keep their natural slot when unshuffled.
        q.play_next(track("x"));
        q.add(track("z"));
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), ["a", "b", "c", "x", "d", "e", "f", "g", "h", "z"]);
        assert_eq!(current_id(&q).as_deref(), Some("c"));
    }

    #[test]
    fn shuffle_is_deterministic_per_seed() {
        let names = ["a", "b", "c", "d", "e", "f"];
        let mut one = Queue::default();
        one.set_shuffle(true, 1);
        one.play_list(&tracks(&names), 3, 99);
        let mut two = Queue::default();
        two.set_shuffle(true, 1);
        two.play_list(&tracks(&names), 3, 99);
        assert_eq!(ids(&one), ids(&two));
        assert_eq!(ids(&one)[0], "d");
        assert_eq!(one.current_index(), Some(0));
    }

    #[test]
    fn advance_with_nothing_current_starts_at_top() {
        let mut q = Queue::default();
        q.add(track("a"));
        assert!(q.current().is_none());
        assert_eq!(q.advance(false).map(|e| e.track.id.as_str()), Some("a"));
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::Off);
    }
}
