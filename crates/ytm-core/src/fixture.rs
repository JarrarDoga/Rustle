use model::{AudioCodec, Playlist, PlaylistSummary, SearchResults, StreamSource, Track};

use crate::{BackendError, MusicBackend, Result};

/// Search for "daft punk", recorded from YouTube Music on 2026-10-26.
const SEARCH_JSON: &str = include_str!("../fixtures/search.json");
/// A public YouTube Music playlist, recorded on 2026-10-26.
const PLAYLIST_JSON: &str = include_str!("../fixtures/playlist.json");

/// The recorded playlist's first this-many tracks form "Focus"; the rest form
/// "Evening".
const FOCUS_LEN: usize = 20;

/// Offline backend for `--demo` and tests. No network, no account.
///
/// Serves recorded responses from `fixtures/`, compiled into the binary.
pub struct FixtureBackend {
    search: SearchResults,
    playlists: Vec<Playlist>,
}

impl Default for FixtureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureBackend {
    pub fn new() -> Self {
        let search: SearchResults =
            serde_json::from_str(SEARCH_JSON).expect("fixtures/search.json");
        let recorded: Playlist =
            serde_json::from_str(PLAYLIST_JSON).expect("fixtures/playlist.json");
        let split = FOCUS_LEN.min(recorded.tracks.len());
        let demo = |id: &str, name: &str, tracks: &[Track]| Playlist {
            summary: PlaylistSummary {
                id: id.to_owned(),
                name: name.to_owned(),
                owner: Some("You".to_owned()),
                track_count: Some(tracks.len() as u64),
                thumbnails: recorded.summary.thumbnails.clone(),
            },
            tracks: tracks.to_vec(),
        };
        let playlists = vec![
            demo("PLdemo_focus", "Focus", &recorded.tracks[..split]),
            demo("PLdemo_evening", "Evening", &recorded.tracks[split..]),
        ];
        Self { search, playlists }
    }

    fn all_tracks(&self) -> impl Iterator<Item = &Track> {
        self.search
            .tracks
            .iter()
            .chain(self.playlists.iter().flat_map(|p| &p.tracks))
    }
}

impl MusicBackend for FixtureBackend {
    async fn set_cookies(&self, _cookies: &str) -> Result<()> {
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<SearchResults> {
        let q = query.trim().to_lowercase();
        let hit = |s: &str| s.to_lowercase().contains(&q);
        let s = &self.search;
        Ok(SearchResults {
            tracks: s
                .tracks
                .iter()
                .filter(|t| hit(&t.title) || t.artists.iter().any(|a| hit(&a.name)))
                .cloned()
                .collect(),
            albums: s
                .albums
                .iter()
                .filter(|a| hit(&a.name) || a.artists.iter().any(|ar| hit(&ar.name)))
                .cloned()
                .collect(),
            artists: s.artists.iter().filter(|a| hit(&a.name)).cloned().collect(),
            playlists: s
                .playlists
                .iter()
                .chain(self.playlists.iter().map(|p| &p.summary))
                .filter(|p| hit(&p.name))
                .cloned()
                .collect(),
        })
    }

    async fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        Ok(self.playlists.iter().map(|p| p.summary.clone()).collect())
    }

    async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.playlists
            .iter()
            .find(|p| p.summary.id == id)
            .cloned()
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))
    }

    async fn liked_tracks(&self) -> Result<Vec<Track>> {
        Ok(self
            .playlists
            .iter()
            .flat_map(|p| &p.tracks)
            .step_by(2)
            .cloned()
            .collect())
    }

    async fn stream(&self, track_id: &str) -> Result<StreamSource> {
        let track = self
            .all_tracks()
            .find(|t| t.id == track_id)
            .ok_or_else(|| BackendError::NotFound(track_id.to_owned()))?;
        // Demo mode has no audio. The player must treat this URL as silence.
        Ok(StreamSource {
            track_id: track.id.clone(),
            url: "demo://silence".to_owned(),
            codec: AudioCodec::Opus,
            bitrate_bps: 0,
            content_length: 0,
            duration: track.duration,
            loudness_db: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn demo_playlists_resolve() {
        let b = FixtureBackend::new();
        let lib = block_on(b.library_playlists()).unwrap();
        let ids: Vec<&str> = lib.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["PLdemo_focus", "PLdemo_evening"]);
        for summary in &lib {
            let pl = block_on(b.playlist(&summary.id)).unwrap();
            assert!(!pl.tracks.is_empty());
            assert_eq!(pl.summary.track_count, Some(pl.tracks.len() as u64));
        }
        assert!(matches!(
            block_on(b.playlist("PLnope")),
            Err(BackendError::NotFound(id)) if id == "PLnope"
        ));
    }

    #[test]
    fn search_filters_recorded_results() {
        let b = FixtureBackend::new();
        let all = block_on(b.search("")).unwrap();
        assert!(!all.tracks.is_empty() && !all.albums.is_empty() && !all.artists.is_empty());

        let res = block_on(b.search("Harder, Better")).unwrap();
        assert!(!res.tracks.is_empty());
        assert!(
            res.tracks
                .iter()
                .all(|t| t.title.contains("Harder, Better"))
        );

        let res = block_on(b.search("focus")).unwrap();
        assert!(res.playlists.iter().any(|p| p.id == "PLdemo_focus"));

        assert_eq!(
            block_on(b.search("zzz no such thing")).unwrap(),
            SearchResults::default()
        );
    }

    #[test]
    fn stream_is_silence_for_known_tracks() {
        let b = FixtureBackend::new();
        let liked = block_on(b.liked_tracks()).unwrap();
        assert!(!liked.is_empty());
        let s = block_on(b.stream(&liked[0].id)).unwrap();
        assert_eq!(s.url, "demo://silence");
        assert_eq!(s.track_id, liked[0].id);
        assert_eq!(s.duration, liked[0].duration);

        let searched = &block_on(b.search("")).unwrap().tracks[0];
        assert_eq!(
            block_on(b.stream(&searched.id)).unwrap().url,
            "demo://silence"
        );

        assert!(matches!(
            block_on(b.stream("nonexistent")),
            Err(BackendError::NotFound(_))
        ));
    }
}
