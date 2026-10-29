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
    /// Appended by autoplay radio, not part of the scope the user played.
    #[allow(dead_code)] // b3-queue; read by the UI batch
    pub radio: bool,
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
    /// The "Playing from" label of the scope the queue was built from.
    playing_from: Option<String>,
    autoplay: bool,
    /// The uid that was the last entry when radio was last appended. While
    /// it is still the last entry, that run-out has had its radio.
    radio_after: Option<u64>,
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
            radio: false,
        }
    }

    /// Replaces the queue with `tracks` and makes `tracks[start]` current.
    /// With shuffle on, the chosen track plays first and the rest are shuffled.
    /// Clears the "Playing from" label.
    pub fn play_list(&mut self, tracks: &[Arc<Track>], start: usize, seed: u64) -> Option<&Entry> {
        if tracks.is_empty() {
            return None;
        }
        let start = start.min(tracks.len() - 1);
        self.playing_from = None;
        self.radio_after = None;
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
        if self.entries.is_empty() {
            self.playing_from = None;
        }
        if self.current != Some(removed.uid) {
            return false;
        }
        self.current = self.entries.get(index).map(|e| e.uid);
        true
    }

    /// Index of the first upcoming entry (after the current one) playing
    /// `track_id`. History and the current entry never match.
    pub fn upcoming_position(&self, track_id: &str) -> Option<usize> {
        let start = self.upcoming_start();
        self.entries
            .get(start..)?
            .iter()
            .position(|e| e.track.id == track_id)
            .map(|i| start + i)
    }

    /// Removes the first upcoming entry playing `track_id`. Returns false
    /// (and changes nothing) when no upcoming entry plays it.
    pub fn remove_upcoming_track(&mut self, track_id: &str) -> bool {
        let Some(index) = self.upcoming_position(track_id) else {
            return false;
        };
        // Upcoming, so never the current entry: `remove` keeps `current`.
        self.remove(index);
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

    /// Replaces the queue with `tracks` (a scope of the main playlist) and
    /// makes the first entry current. With `shuffle`, every track appears
    /// once in an order fixed by `seed`; without it, in playlist order. The
    /// queue's shuffle state follows `shuffle`, so turning shuffle off later
    /// restores playlist order. `label` is the "Playing from" text, e.g.
    /// "Saint Juniper · 41" (see [`scope_label`]). Empty `tracks` leave the
    /// queue as it is.
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn play_scope(
        &mut self,
        tracks: &[Arc<Track>],
        label: String,
        shuffle: bool,
        seed: u64,
    ) -> Option<&Entry> {
        if tracks.is_empty() {
            return None;
        }
        self.entries = tracks.iter().map(|t| self.mint_owned(t)).collect();
        self.shuffle = shuffle;
        if shuffle {
            self.unshuffled = self.entries.iter().map(|e| e.uid).collect();
            shuffle_slice(&mut self.entries, seed);
        } else {
            self.unshuffled.clear();
        }
        self.current = Some(self.entries[0].uid);
        self.playing_from = Some(label);
        self.radio_after = None;
        self.current()
    }

    /// The "Playing from" label of the scope the queue was built from.
    /// `play_list` and emptying the queue clear it; adding, inserting,
    /// moving, removing and autoplay radio keep it.
    #[allow(dead_code)] // b3-queue; read by the UI batch
    pub fn playing_from(&self) -> Option<&str> {
        self.playing_from.as_deref()
    }

    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn set_autoplay(&mut self, on: bool) {
        self.autoplay = on;
    }

    /// Puts back what `Store::save_queue_extras` saved after a restore: the
    /// "Playing from" label and each entry's radio mark. Flags whose length
    /// does not match the queue are ignored.
    ///
    /// The flags follow the saved order. `play_list` keeps that order unless
    /// shuffle is on; then it is the unshuffled order.
    pub fn restore_extras(&mut self, label: Option<String>, radio: &[bool]) {
        self.playing_from = label;
        if radio.len() != self.entries.len() {
            return;
        }
        let saved: Vec<u64> = if self.shuffle {
            self.unshuffled.clone()
        } else {
            self.entries.iter().map(|e| e.uid).collect()
        };
        for (uid, &on) in saved.iter().zip(radio) {
            if let Some(e) = self.entries.iter_mut().find(|e| e.uid == *uid) {
                e.radio = on;
            }
        }
    }

    /// The autoplay setting. Repeat all or one still keeps radio off.
    #[allow(dead_code)] // b3-queue; read by the UI batch
    pub fn autoplay(&self) -> bool {
        self.autoplay
    }

    /// True when autoplay is on, repeat is off, the current entry is the last
    /// one, and radio has not been appended since that entry became last.
    /// The backend then fetches radio for the last track. Pure query: it
    /// stays true until `extend_radio` is called, because the reducer runs
    /// on both the UI and the backend and must stay deterministic.
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn needs_radio(&self) -> bool {
        if !self.autoplay || self.repeat != Repeat::Off {
            return false;
        }
        let Some(i) = self.current_index() else {
            return false;
        };
        i + 1 == self.entries.len() && self.radio_after != Some(self.entries[i].uid)
    }

    /// Appends radio tracks after the last entry, marked as autoplay radio
    /// (not in the main playlist). Clears `needs_radio` for this run-out,
    /// even when `tracks` is empty; it fires again when the new last entry
    /// becomes current.
    #[allow(dead_code)] // b3-queue; called by b3-wiring
    pub fn extend_radio(&mut self, tracks: Vec<Arc<Track>>) {
        self.radio_after = self.entries.last().map(|e| e.uid);
        for track in tracks {
            let mut entry = self.mint(track);
            entry.radio = true;
            if self.shuffle {
                self.unshuffled.push(entry.uid);
            }
            self.entries.push(entry);
        }
    }

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

/// The "Playing from" label for a scope: `name`, a middle dot, and the
/// track count with thousands separators, e.g. "Everything · 1,535".
#[allow(dead_code)] // b3-queue; called by b3-wiring
pub fn scope_label(name: &str, count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{name} \u{b7} {grouped}")
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

    fn radio_ids(q: &Queue) -> Vec<String> {
        q.entries()
            .iter()
            .filter(|e| e.radio)
            .map(|e| e.track.id.clone())
            .collect()
    }

    #[test]
    fn restore_extras_sets_label_and_radio_marks() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d"]), 1, 7);
        q.restore_extras(
            Some("Everything \u{b7} 2".into()),
            &[false, false, true, true],
        );
        assert_eq!(q.playing_from(), Some("Everything \u{b7} 2"));
        assert_eq!(radio_ids(&q), ["c", "d"]);
        assert_eq!(ids(&q), ["a", "b", "c", "d"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn restore_extras_ignores_flags_of_another_length() {
        for flags in [&[true][..], &[true, true, true, true, true][..], &[][..]] {
            let mut q = Queue::default();
            q.play_list(&tracks(&["a", "b", "c"]), 0, 7);
            q.restore_extras(Some("Saint Juniper \u{b7} 3".into()), flags);
            assert_eq!(q.playing_from(), Some("Saint Juniper \u{b7} 3"));
            assert!(radio_ids(&q).is_empty(), "{flags:?}");
        }
        let mut q = Queue::default();
        q.play_list(&tracks(&["a"]), 0, 7);
        q.restore_extras(None, &[true]);
        assert_eq!(q.playing_from(), None);
        assert_eq!(radio_ids(&q), ["a"]);
    }

    #[test]
    fn restore_extras_follows_saved_order_while_shuffled() {
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut q = Queue::default();
        q.set_shuffle(true, 3);
        q.play_list(&tracks(&names), 0, 11);
        assert_ne!(ids(&q), names, "seed must shuffle");
        let flags: Vec<bool> = names.iter().map(|n| *n == "c" || *n == "h").collect();
        q.restore_extras(None, &flags);
        let mut marked = radio_ids(&q);
        marked.sort();
        assert_eq!(marked, ["c", "h"]);
    }

    #[test]
    fn restore_after_save_keeps_radio_marks() {
        let mut q = Queue::default();
        q.set_autoplay(true);
        q.play_scope(&tracks(&["a", "b"]), "Everything \u{b7} 2".into(), false, 1);
        q.extend_radio(tracks(&["r1", "r2"]));
        let saved: Vec<Arc<Track>> = q.entries().iter().map(|e| e.track.clone()).collect();
        let flags: Vec<bool> = q.entries().iter().map(|e| e.radio).collect();
        let label = q.playing_from().map(str::to_owned);

        let mut restored = Queue::default();
        restored.play_list(&saved, 0, 0);
        restored.restore_extras(label, &flags);
        assert_eq!(restored.playing_from(), Some("Everything \u{b7} 2"));
        assert_eq!(radio_ids(&restored), ["r1", "r2"]);
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
    fn scope_shuffle_covers_every_track_once_per_seed() {
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut one = Queue::default();
        one.play_scope(&tracks(&names), "Mix · 8".into(), true, 42);
        let mut two = Queue::default();
        two.play_scope(&tracks(&names), "Mix · 8".into(), true, 42);
        assert_eq!(ids(&one), ids(&two));
        assert_ne!(ids(&one), names, "seed should reorder");
        let mut sorted = ids(&one);
        sorted.sort();
        assert_eq!(sorted, names);
        assert_eq!(one.current_index(), Some(0));
        assert!(one.shuffle());

        // Unshuffling restores playlist order and keeps the current entry.
        let first = current_id(&one);
        one.set_shuffle(false, 0);
        assert_eq!(ids(&one), names);
        assert_eq!(current_id(&one), first);
    }

    #[test]
    fn scope_in_order_keeps_playlist_order() {
        let mut q = Queue::default();
        q.set_shuffle(true, 1);
        q.play_scope(&tracks(&["a", "b", "c"]), "Mix · 3".into(), false, 42);
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("a"));
        assert!(!q.shuffle());
        assert!(q.play_scope(&[], "Empty · 0".into(), true, 1).is_none());
        assert_eq!(ids(&q), ["a", "b", "c"]);
    }

    #[test]
    fn playing_from_label_until_replaced() {
        let mut q = Queue::default();
        assert_eq!(q.playing_from(), None);
        let label = scope_label("Saint Juniper", 41);
        q.play_scope(&tracks(&["a", "b"]), label, true, 3);
        assert_eq!(q.playing_from(), Some("Saint Juniper \u{b7} 41"));
        q.play_next(track("x"));
        q.add(track("y"));
        q.remove(0);
        assert_eq!(q.playing_from(), Some("Saint Juniper \u{b7} 41"));
        q.play_list(&tracks(&["c"]), 0, 0);
        assert_eq!(q.playing_from(), None);

        q.play_scope(&tracks(&["a"]), "One · 1".into(), false, 0);
        q.remove(0);
        assert_eq!(q.playing_from(), None);
        q.play_now(track("z"));
        assert_eq!(q.playing_from(), None);
    }

    #[test]
    fn scope_label_groups_thousands() {
        assert_eq!(scope_label("Everything", 1535), "Everything \u{b7} 1,535");
        assert_eq!(scope_label("#380 to #520", 141), "#380 to #520 \u{b7} 141");
        assert_eq!(scope_label("Big", 1_234_567), "Big \u{b7} 1,234,567");
        assert_eq!(scope_label("None", 0), "None \u{b7} 0");
        assert_eq!(scope_label("K", 1000), "K \u{b7} 1,000");
    }

    #[test]
    fn needs_radio_fires_once_per_run_out() {
        let mut q = Queue::default();
        q.set_autoplay(true);
        assert!(q.autoplay());
        assert!(!q.needs_radio(), "empty queue has no track to seed radio");
        q.play_scope(&tracks(&["a", "b"]), "Mix · 2".into(), false, 0);
        assert!(!q.needs_radio());
        q.advance(true);
        assert!(q.needs_radio(), "on the last track");
        assert!(q.needs_radio(), "pure query");
        assert!(q.advance(true).is_none());
        assert!(q.needs_radio(), "still true once the queue has run out");

        q.extend_radio(vec![track("r1"), track("r2")]);
        assert!(!q.needs_radio());
        assert_eq!(ids(&q), ["a", "b", "r1", "r2"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        q.advance(true);
        assert!(!q.needs_radio());
        q.advance(true);
        assert_eq!(current_id(&q).as_deref(), Some("r2"));
        assert!(q.needs_radio(), "the radio tracks ran out");
    }

    #[test]
    fn empty_radio_result_clears_needs_radio() {
        let mut q = Queue::default();
        q.set_autoplay(true);
        q.play_list(&tracks(&["a"]), 0, 0);
        assert!(q.needs_radio());
        q.extend_radio(Vec::new());
        assert!(!q.needs_radio());
        assert!(q.advance(true).is_none());
        assert!(!q.needs_radio());
        assert_eq!(ids(&q), ["a"]);

        // A new last entry is a new run-out.
        q.add(track("b"));
        q.advance(true);
        assert!(q.needs_radio());
    }

    #[test]
    fn repeat_and_autoplay_off_disable_radio() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a"]), 0, 0);
        assert!(!q.needs_radio(), "autoplay is off by default");
        q.set_autoplay(true);
        q.set_repeat(Repeat::All);
        assert!(!q.needs_radio());
        q.set_repeat(Repeat::One);
        assert!(!q.needs_radio());
        q.set_repeat(Repeat::Off);
        assert!(q.needs_radio());
        q.set_autoplay(false);
        assert!(!q.needs_radio());
        assert!(!q.autoplay());
    }

    #[test]
    fn radio_entries_are_marked_and_stay_marked_after_unshuffle() {
        let names = ["a", "b", "c", "d", "e"];
        let mut q = Queue::default();
        q.set_autoplay(true);
        q.play_scope(&tracks(&names), "Mix · 5".into(), true, 9);
        q.jump(names.len() - 1);
        q.extend_radio(vec![track("r1"), track("r2")]);
        q.play_next(track("x"));
        let radio: Vec<_> = q.entries().iter().filter(|e| e.radio).collect();
        assert_eq!(radio.len(), 2);
        assert_eq!(q.entries().iter().filter(|e| !e.radio).count(), 6);

        q.set_shuffle(false, 0);
        let tail: Vec<_> = q.entries()[q.len() - 2..]
            .iter()
            .map(|e| (e.track.id.as_str(), e.radio))
            .collect();
        assert_eq!(tail, [("r1", true), ("r2", true)]);
        assert!(q.entries()[..q.len() - 2].iter().all(|e| !e.radio));
    }

    #[test]
    fn remove_upcoming_track_takes_the_first_queued_copy() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 1, 0);
        q.add(track("x"));
        q.add(track("c"));
        q.add(track("x"));
        let uids: Vec<u64> = q.entries().iter().map(|e| e.uid).collect();
        assert!(q.remove_upcoming_track("x"));
        assert_eq!(ids(&q), ["a", "b", "c", "c", "x"]);
        // The later copy stayed, not a fresh entry.
        assert_eq!(q.entries()[4].uid, uids[5]);
        assert!(q.remove_upcoming_track("c"));
        assert_eq!(ids(&q), ["a", "b", "c", "x"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn remove_upcoming_track_never_touches_history_or_current() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 1, 0);
        assert!(!q.remove_upcoming_track("a"));
        assert!(!q.remove_upcoming_track("b"));
        assert!(!q.remove_upcoming_track("zzz"));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        // The current track queued again: the queued copy goes, not the
        // one playing.
        q.add(track("b"));
        assert_eq!(q.upcoming_position("b"), Some(3));
        assert!(q.remove_upcoming_track("b"));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn add_and_remove_upcoming_track_count_copies() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a"]), 0, 0);
        for _ in 0..3 {
            q.add(track("x"));
        }
        assert_eq!(ids(&q), ["a", "x", "x", "x"]);
        assert!(q.remove_upcoming_track("x"));
        assert_eq!(ids(&q), ["a", "x", "x"]);
    }

    #[test]
    fn remove_upcoming_track_with_nothing_current_or_shuffled() {
        // Nothing current: every entry is upcoming.
        let mut q = Queue::default();
        q.add(track("a"));
        q.add(track("b"));
        assert_eq!(q.upcoming_start(), 0);
        assert!(q.remove_upcoming_track("a"));
        assert_eq!(ids(&q), ["b"]);

        // While shuffled, the removed entry is also gone from the
        // unshuffled order.
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d"]), 0, 0);
        q.set_shuffle(true, 7);
        assert!(q.remove_upcoming_track("c"));
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), ["a", "b", "d"]);
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::Off);
    }
}
