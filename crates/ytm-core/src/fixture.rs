use std::sync::{Mutex, MutexGuard};

use model::{
    Album, AlbumSummary, AlbumType, Artist, ArtistSummary, AudioCodec, LyricLine, Lyrics, Playlist,
    PlaylistSummary, SearchResults, StreamSource, Track, TrackId,
};

use crate::{BackendError, MusicBackend, Result};

/// Search for "daft punk", recorded from YouTube Music on 2026-10-26.
const SEARCH_JSON: &str = include_str!("../fixtures/search.json");
/// A public YouTube Music playlist, recorded on 2026-10-26.
const PLAYLIST_JSON: &str = include_str!("../fixtures/playlist.json");

/// The recorded playlist's first this-many tracks form "Focus"; the rest form
/// "Evening".
const FOCUS_LEN: usize = 20;

/// The demo's main playlist id; `rustle --demo` opens it.
pub const DEMO_MAIN: &str = "PLdemo_everything";

/// Most tracks `radio` returns.
const RADIO_LEN: usize = 25;

/// Thumbnail sizes [`FixtureBackend::large`] lists per track, smallest
/// first, as YouTube Music lists them for songs.
pub const LARGE_ART_SIZES: [u32; 4] = [60, 120, 226, 544];
/// Distinct artists in the generated library.
const LARGE_ARTISTS: usize = 140;
const FIRST: &[&str] = &[
    "Saint", "Static", "Velvet", "Hollow", "Neon", "Paper", "Silver", "Quiet", "Golden", "Wild",
    "Lunar", "Broken", "Electric", "Glass", "Northern", "Midnight",
];
const SECOND: &[&str] = &[
    "Juniper", "Bloom", "Harbor", "Echo", "Cascade", "Meadow", "Signal", "Ember", "Tide", "Orbit",
    "Canyon", "Parade", "Lantern", "Rivers", "Avenue",
];
const THIRD: &[&str] = &[
    "Nights", "Dreams", "Heart", "Lights", "Motion", "Season", "Waves", "Road", "Fire", "Sky",
    "Letters", "Garden", "Machine", "Summer", "Static", "Rain", "Coast",
];

/// xorshift64: deterministic names for the generated library.
struct XorShift(u64);

impl XorShift {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

/// Offline backend for `--demo` and tests. No network, no account.
///
/// Serves recorded responses from `fixtures/`, compiled into the binary.
/// Likes and playlist edits change in-memory copies only, deterministically.
pub struct FixtureBackend {
    search: SearchResults,
    data: Mutex<Data>,
}

struct Data {
    playlists: Vec<Playlist>,
    /// Liked track ids, most recently liked first.
    liked: Vec<TrackId>,
    /// Source of entry ids minted by `add_to_playlist`.
    next_entry: u64,
}

impl Default for FixtureBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureBackend {
    pub fn new() -> Self {
        let recorded: Playlist =
            serde_json::from_str(PLAYLIST_JSON).expect("fixtures/playlist.json");
        Self::with_tracks(recorded.tracks, &recorded.summary.thumbnails)
    }

    /// A generated library of `n` tracks for perf runs (`RUSTLE_DEMO_LARGE`).
    /// Track `i`'s art is `{art_base}/{i}-{size}.jpg` for each size in
    /// [`LARGE_ART_SIZES`], so a local server can serve it with no network.
    /// The playlists are built as in [`FixtureBackend::new`]; search still
    /// serves the recorded results.
    pub fn large(n: usize, art_base: &str) -> Self {
        let base = art_base.trim_end_matches('/');
        let mut rng = XorShift(0x9e37_79b9_7f4a_7c15);
        let tracks: Vec<Track> = (0..n)
            .map(|i| {
                let artist = rng.below(LARGE_ARTISTS);
                let album = rng.below(6);
                let name = |list: &[&'static str], k: usize| list[k % list.len()];
                let artist_name = format!(
                    "{} {}",
                    name(FIRST, artist),
                    name(SECOND, artist / FIRST.len() + artist)
                );
                Track {
                    id: format!("demo{i:05}"),
                    title: format!(
                        "{} {} {}",
                        name(FIRST, rng.below(1000)),
                        name(SECOND, rng.below(1000)),
                        name(THIRD, rng.below(1000))
                    ),
                    artists: vec![model::ArtistRef {
                        id: Some(format!("UCdemo{artist:03}")),
                        name: artist_name,
                    }],
                    album: Some(model::AlbumRef {
                        id: format!("MPREdemo{artist:03}{album}"),
                        name: format!("{} {}", name(THIRD, artist + album), name(SECOND, album)),
                    }),
                    duration: Some(std::time::Duration::from_secs(120 + rng.below(300) as u64)),
                    thumbnails: LARGE_ART_SIZES
                        .iter()
                        .map(|&px| model::Thumbnail {
                            url: format!("{base}/{i}-{px}.jpg"),
                            width: px,
                            height: px,
                        })
                        .collect(),
                }
            })
            .collect();
        let thumbs = tracks
            .first()
            .map(|t| t.thumbnails.clone())
            .unwrap_or_default();
        Self::with_tracks(tracks, &thumbs)
    }

    /// Focus (the first `FOCUS_LEN` tracks), Evening (the rest) and the main
    /// playlist (all of them); every second one is liked.
    fn with_tracks(tracks: Vec<Track>, thumbnails: &[model::Thumbnail]) -> Self {
        let search: SearchResults =
            serde_json::from_str(SEARCH_JSON).expect("fixtures/search.json");
        let split = FOCUS_LEN.min(tracks.len());
        let demo = |id: &str, name: &str, tracks: &[Track]| Playlist {
            summary: PlaylistSummary {
                id: id.to_owned(),
                name: name.to_owned(),
                owner: Some("You".to_owned()),
                track_count: Some(tracks.len() as u64),
                thumbnails: thumbnails.to_vec(),
            },
            tracks: tracks.to_vec(),
            entry_ids: (0..tracks.len())
                .map(|i| Some(format!("{id}-entry-{i}")))
                .collect(),
        };
        let mut playlists = vec![
            demo("PLdemo_focus", "Focus", &tracks[..split]),
            demo("PLdemo_evening", "Evening", &tracks[split..]),
        ];
        let liked = playlists
            .iter()
            .flat_map(|p| &p.tracks)
            .step_by(2)
            .map(|t| t.id.clone())
            .collect();
        // The demo's main playlist: every track, in order.
        playlists.push(demo(DEMO_MAIN, "Everything", &tracks));
        Self {
            search,
            data: Mutex::new(Data {
                playlists,
                liked,
                next_entry: 0,
            }),
        }
    }

    fn data(&self) -> MutexGuard<'_, Data> {
        self.data.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every known track: search results first, then playlist tracks.
    fn find_track(&self, data: &Data, id: &str) -> Option<Track> {
        self.search
            .tracks
            .iter()
            .chain(data.playlists.iter().flat_map(|p| &p.tracks))
            .find(|t| t.id == id)
            .cloned()
    }

    /// Known tracks matching `keep`, each id once, search results first.
    fn unique_tracks(&self, data: &Data, keep: impl Fn(&Track) -> bool) -> Vec<Track> {
        let mut seen = std::collections::HashSet::new();
        self.search
            .tracks
            .iter()
            .chain(data.playlists.iter().flat_map(|p| &p.tracks))
            .filter(|t| keep(t) && seen.insert(t.id.clone()))
            .cloned()
            .collect()
    }

    fn known(&self, data: &Data, id: &str) -> Result<Track> {
        self.find_track(data, id)
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))
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
                .chain(self.data().playlists.iter().map(|p| &p.summary))
                .filter(|p| hit(&p.name))
                .cloned()
                .collect(),
        })
    }

    async fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        Ok(self
            .data()
            .playlists
            .iter()
            .map(|p| p.summary.clone())
            .collect())
    }

    async fn playlist(&self, id: &str) -> Result<Playlist> {
        self.data()
            .playlists
            .iter()
            .find(|p| p.summary.id == id)
            .cloned()
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))
    }

    async fn liked_tracks(&self) -> Result<Vec<Track>> {
        let data = self.data();
        Ok(data
            .liked
            .iter()
            .filter_map(|id| self.find_track(&data, id))
            .collect())
    }

    async fn stream(&self, track_id: &str) -> Result<StreamSource> {
        let track = self.known(&self.data(), track_id)?;
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

    async fn like(&self, track_id: &str) -> Result<()> {
        let mut data = self.data();
        self.known(&data, track_id)?;
        if !data.liked.iter().any(|id| id == track_id) {
            data.liked.insert(0, track_id.to_owned());
        }
        Ok(())
    }

    async fn unlike(&self, track_id: &str) -> Result<()> {
        self.data().liked.retain(|id| id != track_id);
        Ok(())
    }

    async fn add_to_playlist(
        &self,
        playlist_id: &str,
        track_ids: &[TrackId],
    ) -> Result<Vec<Option<String>>> {
        let mut data = self.data();
        let tracks = track_ids
            .iter()
            .map(|id| self.known(&data, id))
            .collect::<Result<Vec<_>>>()?;
        let Data {
            playlists,
            next_entry,
            ..
        } = &mut *data;
        let p = playlists
            .iter_mut()
            .find(|p| p.summary.id == playlist_id)
            .ok_or_else(|| BackendError::NotFound(playlist_id.to_owned()))?;
        let mut ids = Vec::with_capacity(tracks.len());
        for t in tracks {
            *next_entry += 1;
            let entry = Some(format!("fixture-added-{next_entry}"));
            p.tracks.push(t);
            p.entry_ids.push(entry.clone());
            ids.push(entry);
        }
        p.summary.track_count = Some(p.tracks.len() as u64);
        Ok(ids)
    }

    async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        entries: &[(TrackId, String)],
    ) -> Result<()> {
        let mut data = self.data();
        let p = data
            .playlists
            .iter_mut()
            .find(|p| p.summary.id == playlist_id)
            .ok_or_else(|| BackendError::NotFound(playlist_id.to_owned()))?;
        for (track_id, entry_id) in entries {
            let at = p
                .tracks
                .iter()
                .zip(&p.entry_ids)
                .position(|(t, e)| t.id == *track_id && e.as_deref() == Some(entry_id))
                .ok_or_else(|| BackendError::NotFound(entry_id.clone()))?;
            p.tracks.remove(at);
            p.entry_ids.remove(at);
        }
        p.summary.track_count = Some(p.tracks.len() as u64);
        Ok(())
    }

    async fn radio(&self, track_id: &str) -> Result<Vec<Track>> {
        let data = self.data();
        self.known(&data, track_id)?;
        // Deterministic: the playlist tracks that follow the seed, wrapping.
        let mut seen = std::collections::HashSet::new();
        let all: Vec<&Track> = data
            .playlists
            .iter()
            .flat_map(|p| &p.tracks)
            .filter(|t| seen.insert(t.id.as_str()))
            .collect();
        let start = all.iter().position(|t| t.id == track_id).unwrap_or(0);
        Ok(all
            .iter()
            .cycle()
            .skip(start + 1)
            .take(all.len())
            .filter(|t| t.id != track_id)
            .take(RADIO_LEN)
            .map(|t| (*t).clone())
            .collect())
    }

    async fn clear_cookies(&self) -> Result<()> {
        Ok(())
    }

    async fn album(&self, id: &str) -> Result<Album> {
        let data = self.data();
        let tracks = self.unique_tracks(&data, |t| t.album.as_ref().is_some_and(|a| a.id == id));
        let first = tracks
            .first()
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))?;
        let summary = album_summary(first).expect("matched on album");
        Ok(Album { summary, tracks })
    }

    async fn artist(&self, id: &str) -> Result<Artist> {
        let data = self.data();
        let by = |t: &Track| t.artists.iter().any(|a| a.id.as_deref() == Some(id));
        let tracks = self.unique_tracks(&data, by);
        let first = tracks
            .first()
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))?;
        let name = first
            .artists
            .iter()
            .find(|a| a.id.as_deref() == Some(id))
            .map(|a| a.name.clone())
            .unwrap_or_default();
        let mut albums: Vec<AlbumSummary> = Vec::new();
        for summary in tracks.iter().filter_map(album_summary) {
            if !albums.iter().any(|a| a.id == summary.id) {
                albums.push(summary);
            }
        }
        // Deterministic split: albums with one track here count as singles.
        let (singles, albums) = albums.into_iter().partition(|a| {
            tracks
                .iter()
                .filter(|t| t.album.as_ref().is_some_and(|b| b.id == a.id))
                .count()
                == 1
        });
        Ok(Artist {
            summary: ArtistSummary {
                id: id.to_owned(),
                name,
                thumbnails: first.thumbnails.clone(),
            },
            top_songs: tracks.into_iter().take(10).collect(),
            albums,
            singles,
        })
    }

    async fn lyrics(&self, track_id: &str) -> Result<Option<Lyrics>> {
        let data = self.data();
        self.known(&data, track_id)?;
        // Synced placeholder lines for the demo's first track; none for the
        // rest. Not the song's real lyrics.
        let first = data
            .playlists
            .iter()
            .find(|p| p.summary.id == DEMO_MAIN)
            .and_then(|p| p.tracks.first());
        if first.is_none_or(|t| t.id != track_id) {
            return Ok(None);
        }
        let lines = (0..24)
            .map(|i| LyricLine {
                start: Some(std::time::Duration::from_secs(8 + i * 6)),
                text: format!("Demo lyric line {} for Rustle", i + 1),
            })
            .collect();
        Ok(Some(Lyrics {
            lines,
            synced: true,
            source: Some("Source: Rustle demo".to_owned()),
        }))
    }
}

/// The album a track belongs to, as a summary with the track's art.
fn album_summary(t: &Track) -> Option<AlbumSummary> {
    t.album.as_ref().map(|a| AlbumSummary {
        id: a.id.clone(),
        name: a.name.clone(),
        artists: t.artists.clone(),
        year: None,
        thumbnails: t.thumbnails.clone(),
        album_type: AlbumType::Unknown,
    })
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
        assert_eq!(ids, ["PLdemo_focus", "PLdemo_evening", DEMO_MAIN]);
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
    fn large_library_is_generated_with_local_art() {
        let b = FixtureBackend::large(1535, "http://127.0.0.1:8000/");
        let main = block_on(b.playlist(DEMO_MAIN)).unwrap();
        assert_eq!(main.tracks.len(), 1535);
        let focus = block_on(b.playlist("PLdemo_focus")).unwrap();
        assert_eq!(focus.tracks.len(), FOCUS_LEN);
        let t = &main.tracks[7];
        let urls: Vec<&str> = t.thumbnails.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "http://127.0.0.1:8000/7-60.jpg",
                "http://127.0.0.1:8000/7-120.jpg",
                "http://127.0.0.1:8000/7-226.jpg",
                "http://127.0.0.1:8000/7-544.jpg",
            ]
        );
        let ids: std::collections::HashSet<_> = main.tracks.iter().map(|t| &t.id).collect();
        assert_eq!(ids.len(), 1535);
        let albums: std::collections::HashSet<_> = main
            .tracks
            .iter()
            .filter_map(|t| t.album.as_ref())
            .collect();
        assert!(albums.len() > 200, "{} albums", albums.len());
        // Deterministic.
        let again =
            block_on(FixtureBackend::large(1535, "http://127.0.0.1:8000").playlist(DEMO_MAIN));
        assert_eq!(again.unwrap(), main);
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
    #[test]
    fn like_and_unlike_change_liked_songs() {
        let b = FixtureBackend::new();
        let before = block_on(b.liked_tracks()).unwrap();
        let lib = block_on(b.library_playlists()).unwrap();
        let pl = block_on(b.playlist(&lib[0].id)).unwrap();
        // Liked songs are every second playlist track, so track 1 is not liked.
        let id = pl.tracks[1].id.clone();
        assert!(before.iter().all(|t| t.id != id));

        block_on(b.like(&id)).unwrap();
        let liked = block_on(b.liked_tracks()).unwrap();
        assert_eq!(liked[0].id, id);
        assert_eq!(liked.len(), before.len() + 1);
        block_on(b.like(&id)).unwrap();
        assert_eq!(block_on(b.liked_tracks()).unwrap().len(), before.len() + 1);

        block_on(b.unlike(&id)).unwrap();
        assert_eq!(block_on(b.liked_tracks()).unwrap(), before);
        assert!(matches!(
            block_on(b.like("nonexistent")),
            Err(BackendError::NotFound(_))
        ));
    }

    #[test]
    fn add_and_remove_playlist_entries() {
        let b = FixtureBackend::new();
        let focus = block_on(b.playlist("PLdemo_focus")).unwrap();
        assert_eq!(focus.entry_ids.len(), focus.tracks.len());
        let evening = block_on(b.playlist("PLdemo_evening")).unwrap();
        let add: Vec<TrackId> = evening.tracks[..2].iter().map(|t| t.id.clone()).collect();

        let ids = block_on(b.add_to_playlist("PLdemo_focus", &add)).unwrap();
        assert_eq!(ids.len(), 2);
        let grown = block_on(b.playlist("PLdemo_focus")).unwrap();
        assert_eq!(grown.tracks.len(), focus.tracks.len() + 2);
        assert_eq!(grown.summary.track_count, Some(grown.tracks.len() as u64));
        assert_eq!(grown.entry_ids[focus.tracks.len()..], ids[..]);

        let entry = ids[0].clone().unwrap();
        block_on(b.remove_from_playlist("PLdemo_focus", &[(add[0].clone(), entry.clone())]))
            .unwrap();
        let shrunk = block_on(b.playlist("PLdemo_focus")).unwrap();
        assert_eq!(shrunk.tracks.len(), focus.tracks.len() + 1);
        assert_eq!(shrunk.tracks.last().unwrap().id, add[1]);
        assert!(matches!(
            block_on(b.remove_from_playlist("PLdemo_focus", &[(add[0].clone(), entry)])),
            Err(BackendError::NotFound(_))
        ));
        assert!(matches!(
            block_on(b.add_to_playlist("PLnope", &add)),
            Err(BackendError::NotFound(_))
        ));
    }

    #[test]
    fn radio_is_deterministic_and_skips_the_seed() {
        let b = FixtureBackend::new();
        let seed = block_on(b.playlist("PLdemo_focus")).unwrap().tracks[3].clone();
        let radio = block_on(b.radio(&seed.id)).unwrap();
        assert!(!radio.is_empty() && radio.len() <= RADIO_LEN);
        assert!(radio.iter().all(|t| t.id != seed.id));
        assert_eq!(block_on(b.radio(&seed.id)).unwrap(), radio);
        assert!(block_on(b.clear_cookies()).is_ok());
    }
}
