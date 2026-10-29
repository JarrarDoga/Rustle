//! The play queue: one list in play order, a current entry, the tracks the
//! user queued to play next, shuffle and repeat.
//!
//! Pure data, no I/O. The UI and the backend both run these operations (the UI
//! optimistically, the backend for real), so everything here is deterministic:
//! shuffling takes its seed from the caller.

use std::collections::HashMap;
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
    /// Queued by the user ("Add to queue", "Play next") and not played yet:
    /// part of the up-next segment right after the current entry. Cleared
    /// when the entry becomes current.
    pub queued: bool,
}

/// The play queue. Play order is history, the current entry, the up-next
/// segment (entries the user queued, in the order they play), then the
/// context: what the playlist, shuffle or radio order plays next.
///
/// The up-next segment always sits right after the current entry, whatever
/// becomes current (advance, back, jump, a new context), so queued tracks
/// play next and the context resumes after them. Queued entries are not
/// part of the scope, so they are not in `unshuffled`.
#[derive(Debug, Clone, Default)]
pub struct Queue {
    /// Play order. Entries before the current one are history.
    entries: Vec<Entry>,
    current: Option<u64>,
    shuffle: bool,
    repeat: Repeat,
    /// While shuffle is on: the uids of the scope (and radio) in their
    /// unshuffled order, so turning shuffle off restores it. Entries outside
    /// it (queued, played-after-queueing, `play_now`) keep their place
    /// after the entry they follow.
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

    /// How many entries the user queued: the up-next segment, which runs
    /// from `upcoming_start()`.
    pub fn queued_len(&self) -> usize {
        let start = self.upcoming_start();
        self.entries[start..]
            .iter()
            .take_while(|e| e.queued)
            .count()
    }

    /// Index of the first context entry after the up-next segment.
    pub fn context_start(&self) -> usize {
        self.upcoming_start() + self.queued_len()
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
            queued: false,
        }
    }

    /// Makes `uid` current. It has played now, so it leaves the up-next
    /// segment, and the segment moves to right after it.
    fn set_current(&mut self, uid: u64) {
        self.current = Some(uid);
        if let Some(e) = self.entries.iter_mut().find(|e| e.uid == uid) {
            e.queued = false;
        }
        self.anchor_queued();
    }

    /// Takes the queued entries out, in play order.
    fn take_queued(&mut self) -> Vec<Entry> {
        let (queued, rest) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|e| e.queued);
        self.entries = rest;
        queued
    }

    /// Moves the queued entries, in order, to right after the current entry.
    fn anchor_queued(&mut self) {
        let start = self.upcoming_start();
        let total = self.entries.iter().filter(|e| e.queued).count();
        if self.entries[start..]
            .iter()
            .take_while(|e| e.queued)
            .count()
            == total
        {
            return;
        }
        let queued = self.take_queued();
        let at = self.upcoming_start();
        self.entries.splice(at..at, queued);
    }

    /// Replaces the context with `tracks` and makes `tracks[start]` current.
    /// With shuffle on, the chosen track plays first and the rest are shuffled.
    /// The up-next segment is kept and plays right after the chosen track.
    /// Clears the "Playing from" label.
    pub fn play_list(&mut self, tracks: &[Arc<Track>], start: usize, seed: u64) -> Option<&Entry> {
        if tracks.is_empty() {
            return None;
        }
        let start = start.min(tracks.len() - 1);
        self.playing_from = None;
        self.radio_after = None;
        let queued = self.take_queued();
        self.entries = tracks.iter().map(|t| self.mint_owned(t)).collect();
        self.current = Some(self.entries[start].uid);
        if self.shuffle {
            self.unshuffled = self.entries.iter().map(|e| e.uid).collect();
            let chosen = self.entries.remove(start);
            self.entries.insert(0, chosen);
            shuffle_slice(&mut self.entries[1..], seed);
        }
        let at = self.upcoming_start();
        self.entries.splice(at..at, queued);
        self.current()
    }

    fn mint_owned(&mut self, track: &Arc<Track>) -> Entry {
        self.mint(Arc::clone(track))
    }

    /// Inserts `track` right after the current entry and makes it current.
    /// It goes before the up-next segment, which then follows it.
    pub fn play_now(&mut self, track: Arc<Track>) -> &Entry {
        let entry = self.mint(track);
        let uid = entry.uid;
        let at = self.upcoming_start();
        self.entries.insert(at, entry);
        self.set_current(uid);
        &self.entries[at]
    }

    /// Queues `track` at the front of the up-next segment: it plays right
    /// after the current entry, before what was queued earlier.
    pub fn play_next(&mut self, track: Arc<Track>) {
        let mut entry = self.mint(track);
        entry.queued = true;
        let at = self.upcoming_start();
        self.entries.insert(at, entry);
    }

    /// Queues `track` at the end of the up-next segment: after the current
    /// entry and what was queued before it, ahead of the context.
    pub fn add(&mut self, track: Arc<Track>) {
        let mut entry = self.mint(track);
        entry.queued = true;
        let at = self.context_start();
        self.entries.insert(at, entry);
    }

    /// Moves the entry at `from` so it lands at index `to` (both in play
    /// order). Landing inside the up-next segment queues the entry; landing
    /// outside it (history or context) takes it out. Landing right after the
    /// segment's last entry is either, so the entry keeps what it was. The
    /// current entry never becomes queued, and the segment follows it.
    pub fn move_entry(&mut self, from: usize, to: usize) {
        if from >= self.entries.len() {
            return;
        }
        let entry = self.entries.remove(from);
        let to = to.min(self.entries.len());
        let (uid, was) = (entry.uid, entry.queued);
        self.entries.insert(to, entry);
        if self.current != Some(uid) {
            let start = self.upcoming_start();
            let others = self
                .entries
                .iter()
                .filter(|e| e.queued && e.uid != uid)
                .count();
            let queued = to >= start && (to < start + others || (to == start + others && was));
            self.entries[to].queued = queued;
            if queued && !was {
                self.unshuffled.retain(|&u| u != uid);
            }
        }
        self.anchor_queued();
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
        match self.entries.get(index).map(|e| e.uid) {
            Some(uid) => self.set_current(uid),
            None => self.current = None,
        }
        true
    }

    /// Index of the first entry in the up-next segment playing `track_id`.
    /// History, the current entry and the context never match.
    pub fn upcoming_position(&self, track_id: &str) -> Option<usize> {
        let start = self.upcoming_start();
        self.entries[start..start + self.queued_len()]
            .iter()
            .position(|e| e.track.id == track_id)
            .map(|i| start + i)
    }

    /// Removes the first entry in the up-next segment playing `track_id`.
    /// Returns false (and changes nothing) when none does.
    pub fn remove_upcoming_track(&mut self, track_id: &str) -> bool {
        let Some(index) = self.upcoming_position(track_id) else {
            return false;
        };
        // Upcoming, so never the current entry: `remove` keeps `current`.
        self.remove(index);
        true
    }

    /// Makes the entry at `index` current. The up-next segment moves to
    /// right after it, so queued tracks still play next. Jumping to a queued
    /// entry plays it now; the entries queued before it stay queued.
    pub fn jump(&mut self, index: usize) -> Option<&Entry> {
        let uid = self.entries.get(index)?.uid;
        self.set_current(uid);
        self.current()
    }

    /// Turning shuffle on keeps history, the current entry and the up-next
    /// segment in place and shuffles the context. Turning it off restores
    /// the original order; the up-next segment stays right after the
    /// current entry.
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        if on == self.shuffle {
            return;
        }
        self.shuffle = on;
        if on {
            self.unshuffled = self
                .entries
                .iter()
                .filter(|e| !e.queued)
                .map(|e| e.uid)
                .collect();
            let start = self.context_start();
            shuffle_slice(&mut self.entries[start..], seed);
        } else {
            let rank: HashMap<u64, usize> = std::mem::take(&mut self.unshuffled)
                .into_iter()
                .enumerate()
                .map(|(i, uid)| (uid, i + 1))
                .collect();
            let queued = self.take_queued();
            // An entry outside the unshuffled order stays after the entry
            // it follows now.
            let (mut anchor, mut offset) = (0, 0);
            let mut keyed: Vec<((usize, usize), Entry)> = std::mem::take(&mut self.entries)
                .into_iter()
                .map(|e| {
                    match rank.get(&e.uid) {
                        Some(&r) => (anchor, offset) = (r, 0),
                        None => offset += 1,
                    }
                    ((anchor, offset), e)
                })
                .collect();
            keyed.sort_by_key(|(k, _)| *k);
            self.entries = keyed.into_iter().map(|(_, e)| e).collect();
            let at = self.upcoming_start();
            self.entries.splice(at..at, queued);
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

    /// Replaces the context with `tracks` (a scope of the main playlist) and
    /// makes the first entry current. With `shuffle`, every track appears
    /// once in an order fixed by `seed`; without it, in playlist order. The
    /// queue's shuffle state follows `shuffle`, so turning shuffle off later
    /// restores playlist order. The up-next segment is kept and plays right
    /// after the first entry. `label` is the "Playing from" text, e.g.
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
        let queued = self.take_queued();
        self.entries = tracks.iter().map(|t| self.mint_owned(t)).collect();
        self.shuffle = shuffle;
        if shuffle {
            self.unshuffled = self.entries.iter().map(|e| e.uid).collect();
            shuffle_slice(&mut self.entries, seed);
        } else {
            self.unshuffled.clear();
        }
        self.current = Some(self.entries[0].uid);
        self.entries.splice(1..1, queued);
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
    /// "Playing from" label and each entry's radio and queued marks. Each
    /// list of flags whose length does not match the queue is ignored.
    /// Queued entries go back to the up-next segment, in saved order, right
    /// after the current entry (which is never queued).
    ///
    /// The flags follow the saved order. `play_list` keeps that order unless
    /// shuffle is on; then it is the unshuffled order.
    pub fn restore_extras(&mut self, label: Option<String>, radio: &[bool], queued: &[bool]) {
        self.playing_from = label;
        let saved: Vec<u64> = if self.shuffle {
            self.unshuffled.clone()
        } else {
            self.entries.iter().map(|e| e.uid).collect()
        };
        let mut mark = |flags: &[bool], set: fn(&mut Entry, bool)| {
            if flags.len() != self.entries.len() {
                return;
            }
            for (uid, &on) in saved.iter().zip(flags) {
                if let Some(e) = self.entries.iter_mut().find(|e| e.uid == *uid) {
                    set(e, on);
                }
            }
        };
        mark(radio, |e, on| e.radio = on);
        mark(queued, |e, on| e.queued = on);
        if let Some(uid) = self.current
            && let Some(e) = self.entries.iter_mut().find(|e| e.uid == uid)
        {
            e.queued = false;
        }
        let mut up = self.take_queued();
        if up.is_empty() {
            return;
        }
        up.sort_by_key(|e| saved.iter().position(|&u| u == e.uid));
        self.unshuffled.retain(|u| !up.iter().any(|e| e.uid == *u));
        let at = self.upcoming_start();
        self.entries.splice(at..at, up);
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
    /// otherwise stays on the first entry. The up-next segment follows the
    /// new current entry.
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

    fn queued_ids(q: &Queue) -> Vec<String> {
        let start = q.upcoming_start();
        let up = &q.entries()[start..start + q.queued_len()];
        assert!(up.iter().all(|e| e.queued));
        // Queued entries only ever sit in the up-next segment.
        assert_eq!(q.entries().iter().filter(|e| e.queued).count(), up.len());
        up.iter().map(|e| e.track.id.clone()).collect()
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
            &[],
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
            q.restore_extras(Some("Saint Juniper \u{b7} 3".into()), flags, flags);
            assert_eq!(q.playing_from(), Some("Saint Juniper \u{b7} 3"));
            assert!(radio_ids(&q).is_empty(), "{flags:?}");
            assert_eq!(q.queued_len(), 0, "{flags:?}");
        }
        let mut q = Queue::default();
        q.play_list(&tracks(&["a"]), 0, 7);
        q.restore_extras(None, &[true], &[]);
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
        q.restore_extras(None, &flags, &[]);
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
        q.add(track("x"));
        q.add(track("y"));
        let saved: Vec<Arc<Track>> = q.entries().iter().map(|e| e.track.clone()).collect();
        let flags: Vec<bool> = q.entries().iter().map(|e| e.radio).collect();
        let queued: Vec<bool> = q.entries().iter().map(|e| e.queued).collect();
        let label = q.playing_from().map(str::to_owned);

        let mut restored = Queue::default();
        restored.play_list(&saved, 0, 0);
        restored.restore_extras(label, &flags, &queued);
        assert_eq!(restored.playing_from(), Some("Everything \u{b7} 2"));
        assert_eq!(radio_ids(&restored), ["r1", "r2"]);
        assert_eq!(queued_ids(&restored), ["x", "y"]);
        assert_eq!(ids(&restored), ["a", "x", "y", "b", "r1", "r2"]);
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
        assert_eq!(queued_ids(&q), ["y", "x"]);
        q.add(track("z"));
        assert_eq!(ids(&q), ["a", "y", "x", "z", "b", "c"]);
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

        // Queued while shuffled: still next when unshuffled.
        q.play_next(track("x"));
        q.add(track("z"));
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), ["a", "b", "c", "x", "z", "d", "e", "f", "g", "h"]);
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

        let mut q = Queue::default();
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
        assert_eq!(ids(&q), ["a", "b", "x", "c", "x", "c"]);
        let uids: Vec<u64> = q.entries().iter().map(|e| e.uid).collect();
        assert!(q.remove_upcoming_track("x"));
        assert_eq!(ids(&q), ["a", "b", "c", "x", "c"]);
        // The later copy stayed, not a fresh entry.
        assert_eq!(q.entries()[3].uid, uids[4]);
        // The queued "c", not the playlist's.
        assert!(q.remove_upcoming_track("c"));
        assert_eq!(ids(&q), ["a", "b", "x", "c"]);
        assert_eq!(q.entries()[3].uid, uids[5]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn remove_upcoming_track_never_touches_history_current_or_context() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 1, 0);
        for id in ["a", "b", "c", "zzz"] {
            assert!(!q.remove_upcoming_track(id), "{id}");
            assert_eq!(q.upcoming_position(id), None, "{id}");
        }
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        // The current track queued again: the queued copy goes, not the
        // one playing.
        q.add(track("b"));
        assert_eq!(q.upcoming_position("b"), Some(2));
        assert!(q.remove_upcoming_track("b"));
        assert_eq!(ids(&q), ["a", "b", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
    }

    #[test]
    fn add_and_remove_upcoming_track_count_copies() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 0);
        for _ in 0..3 {
            q.add(track("x"));
        }
        assert_eq!(ids(&q), ["a", "x", "x", "x", "b"]);
        let uids: Vec<u64> = q.entries().iter().map(|e| e.uid).collect();
        assert!(q.remove_upcoming_track("x"));
        assert_eq!(ids(&q), ["a", "x", "x", "b"]);
        assert_eq!(queued_ids(&q), ["x", "x"]);
        // The first added went; the second added plays next.
        assert_eq!(q.entries()[1].uid, uids[2]);
        assert_eq!(q.entries()[2].uid, uids[3]);
    }

    #[test]
    fn remove_upcoming_track_with_nothing_current_or_shuffled() {
        // Nothing current: what was added is all upcoming.
        let mut q = Queue::default();
        q.add(track("a"));
        q.add(track("b"));
        assert_eq!(q.upcoming_start(), 0);
        assert!(q.remove_upcoming_track("a"));
        assert_eq!(ids(&q), ["b"]);

        // While shuffled: the queued copy goes, the playlist's stays.
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d"]), 0, 0);
        q.set_shuffle(true, 7);
        assert!(!q.remove_upcoming_track("c"));
        q.add(track("c"));
        assert!(q.remove_upcoming_track("c"));
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), ["a", "b", "c", "d"]);
    }

    #[test]
    fn add_plays_right_after_current_and_after_earlier_adds() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d", "e", "f"]), 1, 0);
        q.add(track("x"));
        assert_eq!(ids(&q), ["a", "b", "x", "c", "d", "e", "f"]);
        q.add(track("y"));
        q.add(track("z"));
        assert_eq!(ids(&q), ["a", "b", "x", "y", "z", "c", "d", "e", "f"]);
        assert_eq!(queued_ids(&q), ["x", "y", "z"]);
        assert_eq!(q.context_start(), 5);

        // Queued tracks play in order, then the playlist continues.
        let mut played = Vec::new();
        while let Some(e) = q.advance(true) {
            played.push(e.track.id.clone());
        }
        assert_eq!(played, ["x", "y", "z", "c", "d", "e", "f"]);
    }

    #[test]
    fn add_to_a_long_context_never_goes_to_the_end() {
        let names: Vec<String> = (0..1517).map(|i| format!("t{i}")).collect();
        let list: Vec<Arc<Track>> = names.iter().map(|n| track(n)).collect();
        let mut q = Queue::default();
        q.play_scope(&list, scope_label("Liked Music", 1517), true, 5);
        q.advance(true);
        q.add(track("wrist-cry"));
        assert_eq!(q.upcoming_position("wrist-cry"), Some(2));
        assert_eq!(q.advance(true).unwrap().track.id, "wrist-cry");
    }

    #[test]
    fn play_next_queues_at_the_front() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 0);
        q.add(track("x"));
        q.play_next(track("y"));
        assert_eq!(ids(&q), ["a", "y", "x", "b"]);
        assert_eq!(queued_ids(&q), ["y", "x"]);
        assert_eq!(q.upcoming_position("y"), Some(1));
    }

    #[test]
    fn play_now_goes_before_the_queued_tracks() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 0);
        q.add(track("x"));
        q.play_now(track("n"));
        assert_eq!(ids(&q), ["a", "n", "x", "b"]);
        assert_eq!(current_id(&q).as_deref(), Some("n"));
        assert_eq!(queued_ids(&q), ["x"]);
    }

    #[test]
    fn a_new_context_keeps_the_queued_tracks_next() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 0, 0);
        q.add(track("x"));
        q.add(track("y"));

        // Play a row of another list.
        q.play_list(&tracks(&["p", "q", "r", "s"]), 2, 0);
        assert_eq!(ids(&q), ["p", "q", "r", "x", "y", "s"]);
        assert_eq!(current_id(&q).as_deref(), Some("r"));
        assert_eq!(queued_ids(&q), ["x", "y"]);

        // Shuffle all.
        let names = ["a", "b", "c", "d", "e", "f"];
        q.play_scope(&tracks(&names), "Everything \u{b7} 6".into(), true, 3);
        assert_eq!(q.current_index(), Some(0));
        assert_eq!(queued_ids(&q), ["x", "y"]);
        assert_eq!(q.len(), 8);
        // Unshuffling restores the scope's order behind the queued tracks.
        let first = current_id(&q).unwrap();
        q.set_shuffle(false, 0);
        let context: Vec<String> = ids(&q)
            .into_iter()
            .filter(|n| n != "x" && n != "y")
            .collect();
        assert_eq!(context, names);
        assert_eq!(current_id(&q), Some(first));
        assert_eq!(queued_ids(&q), ["x", "y"]);

        // Play in order, with shuffle on before: same.
        q.set_shuffle(true, 1);
        q.play_scope(&tracks(&["m", "n"]), "Two \u{b7} 2".into(), false, 0);
        assert_eq!(ids(&q), ["m", "x", "y", "n"]);
        assert!(!q.shuffle());
    }

    #[test]
    fn shuffle_toggle_keeps_the_queued_tracks_next() {
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let mut q = Queue::default();
        q.play_list(&tracks(&names), 2, 0);
        q.add(track("x"));
        q.add(track("y"));
        q.set_shuffle(true, 99);
        assert_eq!(&ids(&q)[..5], ["a", "b", "c", "x", "y"]);
        assert_ne!(
            &ids(&q)[5..],
            ["d", "e", "f", "g", "h"],
            "seed should reorder"
        );
        q.add(track("z"));
        assert_eq!(queued_ids(&q), ["x", "y", "z"]);
        q.set_shuffle(false, 0);
        assert_eq!(
            ids(&q),
            ["a", "b", "c", "x", "y", "z", "d", "e", "f", "g", "h"]
        );

        // A queued track played while shuffled stays in history after the
        // track it followed when shuffle goes off.
        q.set_shuffle(true, 4);
        q.advance(true);
        assert_eq!(current_id(&q).as_deref(), Some("x"));
        q.set_shuffle(false, 0);
        assert_eq!(
            ids(&q),
            ["a", "b", "c", "x", "y", "z", "d", "e", "f", "g", "h"]
        );
        assert_eq!(current_id(&q).as_deref(), Some("x"));
        assert_eq!(queued_ids(&q), ["y", "z"]);
    }

    #[test]
    fn played_queued_tracks_are_history() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 0, 0);
        q.add(track("x"));
        assert_eq!(q.advance(true).unwrap().track.id, "x");
        assert!(!q.current().unwrap().queued);
        assert_eq!(q.queued_len(), 0);
        assert_eq!(q.upcoming_position("x"), None);
        assert_eq!(q.advance(true).unwrap().track.id, "b");
        assert_eq!(q.back().unwrap().track.id, "x");
        assert_eq!(q.back().unwrap().track.id, "a");
        assert_eq!(ids(&q), ["a", "x", "b", "c"]);
        // Going back through it does not queue it again.
        assert_eq!(q.queued_len(), 0);
        assert!(!q.remove_upcoming_track("x"));
    }

    #[test]
    fn back_and_jump_keep_the_queued_tracks_next() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c", "d", "e"]), 1, 0);
        q.add(track("x"));
        q.add(track("y"));
        // Jump ahead in the context.
        q.jump(6);
        assert_eq!(current_id(&q).as_deref(), Some("e"));
        assert_eq!(ids(&q), ["a", "b", "c", "d", "e", "x", "y"]);
        // Jump into history.
        q.jump(0);
        assert_eq!(ids(&q), ["a", "x", "y", "b", "c", "d", "e"]);
        // Back from the first queued track's predecessor.
        q.jump(3);
        assert_eq!(ids(&q), ["a", "b", "x", "y", "c", "d", "e"]);
        q.back();
        assert_eq!(current_id(&q).as_deref(), Some("a"));
        assert_eq!(ids(&q), ["a", "x", "y", "b", "c", "d", "e"]);
        // Jump to the second queued track: it plays, the first stays next.
        q.jump(2);
        assert_eq!(current_id(&q).as_deref(), Some("y"));
        assert_eq!(ids(&q), ["a", "y", "x", "b", "c", "d", "e"]);
        assert_eq!(queued_ids(&q), ["x"]);
    }

    #[test]
    fn repeat_all_wraps_with_queued_tracks() {
        let mut q = Queue::default();
        q.set_repeat(Repeat::All);
        q.play_list(&tracks(&["a", "b"]), 1, 0);
        q.add(track("x"));
        assert_eq!(q.advance(false).unwrap().track.id, "x");
        assert_eq!(q.advance(false).unwrap().track.id, "a");
        q.add(track("y"));
        assert_eq!(q.back().unwrap().track.id, "x");
        assert_eq!(ids(&q), ["a", "b", "x", "y"]);
        assert_eq!(queued_ids(&q), ["y"]);
    }

    #[test]
    fn removing_the_current_entry_plays_the_first_queued() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b"]), 0, 0);
        q.add(track("x"));
        q.add(track("y"));
        assert!(q.remove(0));
        assert_eq!(current_id(&q).as_deref(), Some("x"));
        assert_eq!(queued_ids(&q), ["y"]);
        // Removing a queued entry by index.
        assert!(!q.remove(1));
        assert_eq!(ids(&q), ["x", "b"]);
        assert_eq!(q.queued_len(), 0);
    }

    #[test]
    fn move_entry_in_and_out_of_the_queued_tracks() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "c"]), 0, 0);
        q.add(track("x"));
        q.add(track("y"));
        assert_eq!(ids(&q), ["a", "x", "y", "b", "c"]);
        // Reorder inside the segment.
        q.move_entry(2, 1);
        assert_eq!(queued_ids(&q), ["y", "x"]);
        // A context entry dropped into the segment is queued.
        q.move_entry(4, 1);
        assert_eq!(ids(&q), ["a", "c", "y", "x", "b"]);
        assert_eq!(queued_ids(&q), ["c", "y", "x"]);
        // A queued entry dropped into the context leaves the segment.
        q.move_entry(2, 4);
        assert_eq!(ids(&q), ["a", "c", "x", "b", "y"]);
        assert_eq!(queued_ids(&q), ["c", "x"]);
        // Right after the segment, an entry keeps what it was.
        q.move_entry(4, 3);
        assert_eq!(ids(&q), ["a", "c", "x", "y", "b"]);
        assert_eq!(queued_ids(&q), ["c", "x"]);
        q.move_entry(1, 2);
        assert_eq!(queued_ids(&q), ["x", "c"]);
        // Into history: not queued.
        q.move_entry(1, 0);
        assert_eq!(ids(&q), ["x", "a", "c", "y", "b"]);
        assert_eq!(current_id(&q).as_deref(), Some("a"));
        assert_eq!(queued_ids(&q), ["c"]);
        // Moving the current entry takes the segment along.
        q.move_entry(1, 4);
        assert_eq!(ids(&q), ["x", "y", "b", "a", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("a"));
        assert_eq!(queued_ids(&q), ["c"]);
    }

    #[test]
    fn radio_stays_after_the_queued_tracks_and_the_context() {
        let mut q = Queue::default();
        q.set_autoplay(true);
        q.play_scope(&tracks(&["a", "b"]), "Mix \u{b7} 2".into(), false, 0);
        q.advance(true);
        assert!(q.needs_radio());
        q.add(track("x"));
        assert!(!q.needs_radio(), "the queued track plays first");
        q.extend_radio(tracks(&["r1", "r2"]));
        assert_eq!(ids(&q), ["a", "b", "x", "r1", "r2"]);
        q.add(track("y"));
        assert_eq!(ids(&q), ["a", "b", "x", "y", "r1", "r2"]);
        assert_eq!(radio_ids(&q), ["r1", "r2"]);

        // Queued tracks run out last, so radio follows them.
        let mut q = Queue::default();
        q.set_autoplay(true);
        q.play_list(&tracks(&["a"]), 0, 0);
        q.add(track("x"));
        assert!(!q.needs_radio());
        q.advance(true);
        assert!(q.needs_radio());
        q.extend_radio(tracks(&["r1"]));
        assert_eq!(ids(&q), ["a", "x", "r1"]);
        q.add(track("y"));
        assert_eq!(ids(&q), ["a", "x", "y", "r1"]);
    }

    #[test]
    fn restore_extras_puts_queued_tracks_back_next() {
        let mut q = Queue::default();
        q.play_list(&tracks(&["a", "b", "x", "y", "c"]), 1, 0);
        q.restore_extras(None, &[], &[false, false, true, true, false]);
        assert_eq!(ids(&q), ["a", "b", "x", "y", "c"]);
        assert_eq!(queued_ids(&q), ["x", "y"]);

        // Stray marks (history, the current entry) still end up next, and
        // the current entry is never queued.
        let mut q = Queue::default();
        q.play_list(&tracks(&["x", "b", "c", "y"]), 1, 0);
        q.restore_extras(None, &[], &[true, true, false, true]);
        assert_eq!(ids(&q), ["b", "x", "y", "c"]);
        assert_eq!(current_id(&q).as_deref(), Some("b"));
        assert_eq!(queued_ids(&q), ["x", "y"]);

        // While shuffled, in saved order, and out of the unshuffled order.
        let mut q = Queue::default();
        q.set_shuffle(true, 1);
        let names = ["a", "x", "y", "z", "b", "c", "d", "e"];
        q.play_list(&tracks(&names), 0, 21);
        q.restore_extras(
            None,
            &[],
            &[false, true, true, true, false, false, false, false],
        );
        assert_eq!(queued_ids(&q), ["x", "y", "z"]);
        q.set_shuffle(false, 0);
        assert_eq!(ids(&q), names);
    }

    #[test]
    fn repeat_cycles() {
        assert_eq!(Repeat::Off.cycle(), Repeat::All);
        assert_eq!(Repeat::All.cycle(), Repeat::One);
        assert_eq!(Repeat::One.cycle(), Repeat::Off);
    }
}
