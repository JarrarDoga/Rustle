//! Resolved stream URLs per track id, kept until shortly before they expire,
//! so prefetch plus play resolves a track once.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use model::StreamSource;

use crate::StreamOrigin;
use crate::ytdlp::url_expiry;

/// Drop a URL this long before its `expire=`. The player keeps issuing Range
/// requests for the whole track, so a URL must outlive one playback.
pub(crate) const EXPIRY_MARGIN: Duration = Duration::from_secs(30 * 60);

/// The app prefetches one track ahead, so a handful of entries is plenty.
const MAX_ENTRIES: usize = 32;

struct Entry {
    source: StreamSource,
    origin: StreamOrigin,
    fresh_until: SystemTime,
}

#[derive(Default)]
pub(crate) struct StreamCache {
    entries: HashMap<String, Entry>,
}

impl StreamCache {
    pub(crate) fn get(
        &self,
        track_id: &str,
        now: SystemTime,
    ) -> Option<(StreamSource, StreamOrigin)> {
        self.entries
            .get(track_id)
            .filter(|e| now < e.fresh_until)
            .map(|e| (e.source.clone(), e.origin))
    }

    /// Stores `source` unless its URL has no `expire=` or is already too
    /// close to it.
    pub(crate) fn insert(&mut self, source: &StreamSource, origin: StreamOrigin, now: SystemTime) {
        let Some(fresh_until) = url_expiry(&source.url).and_then(|t| t.checked_sub(EXPIRY_MARGIN))
        else {
            return;
        };
        if fresh_until <= now {
            return;
        }
        self.entries.retain(|_, e| now < e.fresh_until);
        if self.entries.len() >= MAX_ENTRIES
            && !self.entries.contains_key(&source.track_id)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.fresh_until)
                .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            source.track_id.clone(),
            Entry {
                source: source.clone(),
                origin,
                fresh_until,
            },
        );
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use model::AudioCodec;

    use super::*;

    fn source(id: &str, expire: Option<u64>) -> StreamSource {
        let q = expire.map_or(String::new(), |e| format!("expire={e}&"));
        StreamSource {
            track_id: id.to_owned(),
            url: format!("https://example.invalid/videoplayback?{q}itag=251"),
            codec: AudioCodec::Opus,
            bitrate_bps: 130_000,
            content_length: 1,
            duration: None,
            loudness_db: None,
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn serves_until_margin_before_expiry() {
        let mut c = StreamCache::default();
        let expire = 100_000;
        c.insert(&source("a", Some(expire)), StreamOrigin::YtDlp, at(0));
        let edge = expire - EXPIRY_MARGIN.as_secs();
        assert_eq!(c.get("a", at(edge - 1)).unwrap().1, StreamOrigin::YtDlp);
        assert!(c.get("a", at(edge)).is_none());
        assert!(c.get("b", at(0)).is_none());
    }

    #[test]
    fn skips_urls_without_or_near_expiry() {
        let mut c = StreamCache::default();
        c.insert(&source("a", None), StreamOrigin::Ios, at(0));
        let near = 1_000 + EXPIRY_MARGIN.as_secs();
        c.insert(&source("b", Some(near)), StreamOrigin::Ios, at(1_000));
        assert!(c.get("a", at(0)).is_none());
        assert!(c.get("b", at(1_000)).is_none());
    }

    #[test]
    fn evicts_soonest_expiring_when_full() {
        let mut c = StreamCache::default();
        let base = 100_000;
        for i in 0..MAX_ENTRIES as u64 {
            c.insert(
                &source(&i.to_string(), Some(base + i)),
                StreamOrigin::Web,
                at(0),
            );
        }
        c.insert(&source("new", Some(base * 2)), StreamOrigin::Web, at(0));
        assert_eq!(c.entries.len(), MAX_ENTRIES);
        assert!(c.get("0", at(0)).is_none());
        assert!(c.get("1", at(0)).is_some());
        assert!(c.get("new", at(0)).is_some());
        c.clear();
        assert!(c.get("new", at(0)).is_none());
    }
}
