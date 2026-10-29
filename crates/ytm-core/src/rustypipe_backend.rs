use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use model::{
    Album, AlbumRef, AlbumSummary, AlbumType, Artist, ArtistRef, ArtistSummary, LyricLine, Lyrics,
    Playlist, PlaylistSummary, SearchResults, StreamSource, Thumbnail, Track, TrackId,
};
use rustypipe::cache::{CacheStorage, FileStorage};
use rustypipe::client::{ClientType, RustyPipe, RustyPipeQuery};
use rustypipe::error::{Error as RpError, ExtractionError};
use rustypipe::model as rp;

use crate::innertube;
use crate::rank;
use crate::stream_cache::StreamCache;
use crate::visionos::{self, VisionOsError};
use crate::{BackendError, MusicBackend, Result};

/// Upper bound on items fetched when following a paginator to the end.
const MAX_ITEMS: usize = 5_000;

/// Radio tracks to return when YouTube has that many.
const RADIO_MIN: usize = 20;

/// Stream formats we accept, best first: Premium Opus, Opus, AAC.
pub(crate) const ITAG_PREFERENCE: [u32; 3] = [774, 251, 140];

/// A stream plus whether it came from the URL cache, for diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamInfo {
    pub source: StreamSource,
    /// True when served from the in-memory URL cache.
    pub cached: bool,
}

/// The real backend, built on rustypipe. Streams come from the visionOS
/// client only; the fallbacks it used to have are described in
/// `docs/design/stream-fallbacks.md`.
pub struct RustyPipeBackend {
    rp: RustyPipe,
    streams: Mutex<StreamCache>,
    /// One lock per track id being resolved, so a play request that arrives
    /// during the prefetch waits for it instead of resolving again.
    resolving: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl RustyPipeBackend {
    /// Builds the client with its cache under `~/.config/rustle/`.
    pub fn new() -> Result<Self> {
        let dir =
            config_dir().ok_or_else(|| BackendError::Extraction("no home directory".to_owned()))?;
        Self::with_storage_dir(&dir)
    }

    fn with_storage_dir(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| BackendError::Extraction(format!("create {}: {e}", dir.display())))?;

        let rp = RustyPipe::builder()
            .storage_dir(dir)
            .storage(Box::new(NoSecretsStorage(FileStorage::new(
                dir.join("rustypipe_cache.json"),
            ))))
            // Error reports dump raw responses to disk, including ones made
            // with the user's cookies.
            .no_reporter()
            // PO tokens are only for rustypipe's own player requests, which
            // Rustle no longer makes. Without this, rustypipe runs any
            // `rustypipe-botguard` on PATH at startup to probe it.
            .no_botguard()
            .build()
            .map_err(map_error)?;
        Ok(Self {
            rp,
            streams: Mutex::new(StreamCache::default()),
            resolving: Mutex::new(HashMap::new()),
        })
    }

    fn logged_in(&self) -> bool {
        self.rp.query().auth_enabled(ClientType::DesktopMusic)
    }

    /// An authenticated query for a write, or `Auth` before any request when
    /// no cookies are set.
    fn write_query(&self) -> Result<RustyPipeQuery> {
        if self.logged_in() {
            Ok(self.rp.query().authenticated())
        } else {
            Err(BackendError::Auth)
        }
    }

    /// [`MusicBackend::stream`], plus whether the stream came from the URL
    /// cache.
    pub async fn stream_info(&self, track_id: &str) -> Result<StreamInfo> {
        if let Some(hit) = self.cached_stream(track_id) {
            return Ok(hit);
        }
        let id_lock = lock(&self.resolving)
            .entry(track_id.to_owned())
            .or_default()
            .clone();
        let res = {
            let _resolving = id_lock.lock().await;
            match self.cached_stream(track_id) {
                Some(hit) => Ok(hit),
                None => self.resolve_stream(track_id).await.map(|source| {
                    lock(&self.streams).insert(&source, SystemTime::now());
                    StreamInfo {
                        source,
                        cached: false,
                    }
                }),
            }
        };
        let mut resolving = lock(&self.resolving);
        // Only the map and this call hold it: nobody else is waiting.
        if Arc::strong_count(&id_lock) == 2 {
            resolving.remove(track_id);
        }
        res
    }

    fn cached_stream(&self, track_id: &str) -> Option<StreamInfo> {
        let source = lock(&self.streams).get(track_id, SystemTime::now())?;
        Some(StreamInfo {
            source,
            cached: true,
        })
    }

    /// The visionOS client with rustypipe's cached visitor id.
    async fn resolve_stream(&self, track_id: &str) -> Result<StreamSource> {
        let q = self.rp.query();
        resolve_visionos(
            track_id,
            |fresh| q.get_visitor_data(fresh),
            |vd| q.remove_visitor_data(vd),
            |vd| async move { visionos::stream(&vd, track_id).await },
        )
        .await
    }
}

/// Resolves `track_id` through the visionOS client, the only stream path.
///
/// `visitor_id(fresh)` hands out a visitor id, a new one when `fresh`;
/// `drop_id` forgets a burnt one; `player` sends the request with an id. A
/// bot check (`LOGIN_REQUIRED`) means YouTube stopped trusting the id, so it
/// is dropped and the request retried once with a fresh id. A second bot
/// check, or any other failure, is returned for the UI to show.
///
/// Plain closures returning futures rather than async closures: the
/// `MusicBackend` futures must be `Send`, and rustc cannot yet prove that for
/// an `AsyncFnMut` call.
async fn resolve_visionos<V, P>(
    track_id: &str,
    mut visitor_id: impl FnMut(bool) -> V,
    mut drop_id: impl FnMut(&str),
    mut player: impl FnMut(String) -> P,
) -> Result<StreamSource>
where
    V: Future<Output = std::result::Result<String, RpError>>,
    P: Future<Output = std::result::Result<StreamSource, VisionOsError>>,
{
    let vd = visitor_id(false).await.map_err(map_error)?;
    let mut res = player(vd.clone()).await;
    if let Err(VisionOsError::BotCheck(reason)) = &res {
        tracing::warn!(
            "visionOS bot check for {track_id} ({reason}); retrying with a fresh visitor id"
        );
        drop_id(&vd);
        let vd = visitor_id(true).await.map_err(map_error)?;
        res = player(vd.clone()).await;
        if let Err(VisionOsError::BotCheck(_)) = &res {
            drop_id(&vd);
        }
    }
    res.map_err(|e| match e {
        VisionOsError::NotFound => BackendError::NotFound(track_id.to_owned()),
        VisionOsError::Network(m) => BackendError::Network(m),
        // Not `Auth`: the login is fine, YouTube distrusts the anonymous
        // visitor id, and `Auth` would log the user out.
        VisionOsError::BotCheck(reason) => BackendError::Extraction(format!(
            "YouTube asked to confirm this is not a bot, also with a fresh visitor id ({reason})"
        )),
        VisionOsError::Unusable(m) => BackendError::Extraction(m),
    })
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl MusicBackend for RustyPipeBackend {
    async fn set_cookies(&self, cookies: &str) -> Result<()> {
        // Both calls fetch youtube.com with the cookies and fail with
        // `AuthError::NoLogin` if YouTube does not see a logged-in session, so
        // a successful return means the cookies were checked.
        let res = match parse_cookie_input(cookies) {
            CookieInput::Header(h) => self.rp.user_auth_set_cookie(h).await,
            CookieInput::NetscapeTxt(t) => self.rp.user_auth_set_cookie_txt(t).await,
        };
        res.map_err(|e| match map_error(e) {
            BackendError::Network(m) => BackendError::Network(m),
            _ => BackendError::Auth,
        })?;
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<SearchResults> {
        // One request per type instead of `music_search_main`: rustypipe
        // 0.11.4 misparses the main page's top-result card (artist "Song",
        // no duration or album) and it returns only a few items per type.
        let q = self.rp.query();
        let (tracks, albums, artists, playlists) = tokio::try_join!(
            q.music_search_tracks(query),
            q.music_search_albums(query),
            q.music_search_artists(query),
            q.music_search_playlists(query, false),
        )
        .map_err(map_error)?;
        Ok(map_search(
            query,
            tracks.items.items,
            albums.items.items,
            artists.items.items,
            playlists.items.items,
        ))
    }

    async fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let q = self.rp.query().authenticated();
        let mut p = q.music_saved_playlists().await.map_err(map_error)?;
        p.extend_limit(&q, MAX_ITEMS).await.map_err(map_error)?;
        p.items.truncate(MAX_ITEMS);
        Ok(p.items.into_iter().map(map_playlist_item).collect())
    }

    async fn playlist(&self, id: &str) -> Result<Playlist> {
        let mut q = self.rp.query();
        // Private playlists need the cookies; public ones work either way.
        let logged_in = self.logged_in();
        if logged_in {
            q = q.authenticated();
        }
        let tracks = async {
            let mut pl = q.music_playlist(id).await.map_err(map_error)?;
            pl.tracks
                .extend_limit(&q, MAX_ITEMS)
                .await
                .map_err(map_error)?;
            Ok(pl)
        };
        // rustypipe does not parse entry ids (`setVideoId`), which removing a
        // song needs, so a second walk over the raw pages collects them. Only
        // owned playlists have them; for others it stops after one request.
        // Best effort: without them the playlist still loads, read-only.
        let raw = async {
            if !logged_in {
                return innertube::RawPlaylist::default();
            }
            innertube::playlist_entries(&q, id, MAX_ITEMS)
                .await
                .unwrap_or_else(|e| {
                    tracing::warn!("entry ids for playlist {id}: {e}");
                    innertube::RawPlaylist::default()
                })
        };
        let (pl, raw) = tokio::join!(tracks, raw);
        let pl = match pl {
            Ok(pl) => pl,
            // rustypipe 0.11.4 cannot read the header YouTube sends for a
            // playlist the user owns (`musicEditablePlaylistDetailHeaderRenderer`),
            // takes it for an album and fails ("album playlist containing
            // items from different albums"). The raw walk read every row.
            Err(BackendError::Extraction(e)) if raw.entries.iter().any(|r| r.track.is_some()) => {
                tracing::warn!("playlist {id}: rustypipe failed ({e}); using the raw pages");
                return Ok(playlist_from_raw(id, raw));
            }
            Err(e) => return Err(e),
        };
        let entries = raw.entries;
        let mut playlist = map_playlist(pl);
        let ids: Vec<&str> = playlist.tracks.iter().map(|t| t.id.as_str()).collect();
        playlist.entry_ids = innertube::align_entry_ids(&ids, &entries);
        if playlist.entry_ids.is_empty() && entries.iter().any(|e| e.set_video_id.is_some()) {
            tracing::warn!(
                "entry ids for playlist {id}: {} rows do not match {} tracks",
                entries.len(),
                playlist.tracks.len()
            );
        }
        Ok(playlist)
    }

    async fn liked_tracks(&self) -> Result<Vec<Track>> {
        let q = self.rp.query().authenticated();
        let mut pl = q.music_liked_tracks().await.map_err(map_error)?;
        pl.tracks
            .extend_limit(&q, MAX_ITEMS)
            .await
            .map_err(map_error)?;
        Ok(map_playlist(pl).tracks)
    }

    async fn stream(&self, track_id: &str) -> Result<StreamSource> {
        self.stream_info(track_id).await.map(|i| i.source)
    }

    async fn like(&self, track_id: &str) -> Result<()> {
        innertube::rate(&self.write_query()?, track_id, true).await
    }

    async fn unlike(&self, track_id: &str) -> Result<()> {
        innertube::rate(&self.write_query()?, track_id, false).await
    }

    async fn add_to_playlist(
        &self,
        playlist_id: &str,
        track_ids: &[TrackId],
    ) -> Result<Vec<Option<String>>> {
        innertube::add_videos(&self.write_query()?, playlist_id, track_ids).await
    }

    async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        entries: &[(TrackId, String)],
    ) -> Result<()> {
        innertube::remove_entries(&self.write_query()?, playlist_id, entries).await
    }

    async fn radio(&self, track_id: &str) -> Result<Vec<Track>> {
        let mut q = self.rp.query();
        if self.logged_in() {
            q = q.authenticated();
        }
        let mut p = q.music_radio_track(track_id).await.map_err(map_error)?;
        if p.items.iter().filter(|t| t.id != track_id).count() < RADIO_MIN {
            // The first page usually holds ~50 tracks; a short one gets one
            // more page. A failure there still leaves the first page.
            if let Err(e) = p.extend_limit(&q, RADIO_MIN + 10).await {
                tracing::warn!("radio for {track_id}: next page failed: {e}");
            }
        }
        Ok(map_radio(track_id, p.items))
    }

    async fn clear_cookies(&self) -> Result<()> {
        match self.rp.user_auth_remove_cookie().await {
            // Nothing stored is already logged out.
            Ok(()) | Err(RpError::Auth(_)) => {}
            Err(e) => return Err(map_error(e)),
        }
        Ok(())
    }

    async fn album(&self, id: &str) -> Result<Album> {
        // Album and artist pages are public, and authenticated reads got
        // HTTP 403 (2026-10-26), so these never send the cookies. rustypipe
        // also fetches the album's playlist when some rows are music videos
        // or the page lists fewer tracks than the album has.
        let q = self.rp.query().unauthenticated();
        let album = q.music_album(id).await.map_err(map_error)?;
        Ok(map_music_album(album))
    }

    async fn artist(&self, id: &str) -> Result<Artist> {
        // One request: the albums and singles shown on the page, not the
        // full discography behind "More".
        let q = self.rp.query().unauthenticated();
        let artist = q.music_artist(id, false).await.map_err(map_error)?;
        Ok(map_music_artist(artist))
    }

    async fn lyrics(&self, track_id: &str) -> Result<Option<Lyrics>> {
        // Lyrics are public; like album pages, these reads never send the
        // cookies.
        let q = self.rp.query().unauthenticated();
        let details = q.music_details(track_id).await.map_err(map_error)?;
        let ua = q.user_agent(ClientType::DesktopMusic);
        fetch_lyrics(
            track_id,
            details.lyrics_id,
            async |id| innertube::timed_lyrics(&ua, id).await,
            async |id| q.music_lyrics(id).await,
        )
        .await
    }
}

/// Wraps rustypipe's JSON cache file and drops the login cookie and OAuth
/// token from it. Cookies live in the keyring; the app passes them in through
/// `set_cookies` on every start.
struct NoSecretsStorage(FileStorage);

impl CacheStorage for NoSecretsStorage {
    fn write(&self, data: &str) {
        if let Some(clean) = strip_secrets(data) {
            self.0.write(&clean);
        }
    }

    fn read(&self) -> Option<String> {
        self.0.read().and_then(|d| strip_secrets(&d))
    }
}

/// Returns the cache JSON without `auth_cookie` and `oauth_token`, or `None`
/// if it is not a JSON object (so nothing unsanitized reaches the disk).
fn strip_secrets(data: &str) -> Option<String> {
    match serde_json::from_str::<serde_json::Value>(data).ok()? {
        serde_json::Value::Object(mut map) => {
            map.remove("auth_cookie");
            map.remove("oauth_token");
            Some(serde_json::Value::Object(map).to_string())
        }
        _ => None,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME/rustle`, else `~/.config/rustle` (Linux only for now).
fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home_dir().map(|h| h.join(".config")))?;
    Some(base.join("rustle"))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CookieInput<'a> {
    /// Value of a `Cookie:` request header.
    Header(&'a str),
    /// Body of a Netscape cookies.txt file.
    NetscapeTxt(&'a str),
}

pub(crate) fn parse_cookie_input(input: &str) -> CookieInput<'_> {
    let input = input.trim();
    // cookies.txt lines are tab-separated; a header value never contains tabs.
    if input.starts_with('#') || input.lines().any(|l| l.split('\t').count() >= 7) {
        return CookieInput::NetscapeTxt(input);
    }
    let header = input
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("cookie:"))
        .map_or(input, |_| input[7..].trim_start());
    CookieInput::Header(header)
}

pub(crate) fn map_error(e: RpError) -> BackendError {
    match e {
        RpError::Auth(_) | RpError::HttpStatus(401, _) => BackendError::Auth,
        RpError::Extraction(ExtractionError::NotFound { id, .. }) => BackendError::NotFound(id),
        RpError::Http(_) | RpError::HttpStatus(..) => BackendError::Network(e.to_string()),
        // YouTube's throttling answers with an empty body, which serde
        // reports as EOF; a body cut short does too.
        RpError::Extraction(ExtractionError::InvalidData(ref m))
            if m.starts_with("EOF while parsing") =>
        {
            BackendError::Network(format!("empty or truncated response from YouTube: {m}"))
        }
        RpError::Extraction(_) | RpError::Other(_) => BackendError::Extraction(e.to_string()),
    }
}

/// Timed lyrics from the `ANDROID_MUSIC` client first, then the web client's
/// plain text when the Android answer fails or has an unknown shape.
async fn fetch_lyrics(
    track_id: &str,
    lyrics_id: Option<String>,
    timed: impl AsyncFnOnce(&str) -> Result<innertube::TimedLyrics>,
    plain: impl AsyncFnOnce(&str) -> std::result::Result<rp::Lyrics, RpError>,
) -> Result<Option<Lyrics>> {
    let Some(lyrics_id) = lyrics_id else {
        return Ok(None);
    };
    match timed(&lyrics_id).await {
        Ok(innertube::TimedLyrics::Found(l)) => return Ok(Some(l)),
        Ok(innertube::TimedLyrics::Unavailable) => return Ok(None),
        Ok(innertube::TimedLyrics::Unknown) => {}
        Err(e) => tracing::warn!("timed lyrics for {track_id} failed, trying plain: {e}"),
    }
    match plain(&lyrics_id).await {
        Ok(l) => Ok(map_plain_lyrics(l)),
        Err(RpError::Extraction(ExtractionError::NotFound { .. })) => Ok(None),
        Err(e) => Err(map_error(e)),
    }
}

/// The web client's lyrics: one line per body line, blank lines kept as
/// stanza breaks. `None` when the body is empty.
fn map_plain_lyrics(l: rp::Lyrics) -> Option<Lyrics> {
    let lines: Vec<LyricLine> = l
        .body
        .trim()
        .lines()
        .map(|t| LyricLine {
            start: None,
            text: t.trim_end().to_owned(),
        })
        .collect();
    if lines.iter().all(|l| l.text.is_empty()) {
        return None;
    }
    let source = l.footer.trim();
    Some(Lyrics {
        lines,
        synced: false,
        source: (!source.is_empty()).then(|| source.to_owned()),
    })
}

fn map_thumbnails(thumbs: Vec<rp::Thumbnail>) -> Vec<Thumbnail> {
    with_smaller_variants(
        thumbs
            .into_iter()
            .map(|t| Thumbnail {
                url: t.url,
                width: t.width,
                height: t.height,
            })
            .collect(),
    )
}

/// `thumbs` plus [`smaller_variants`], smallest first.
fn with_smaller_variants(mut out: Vec<Thumbnail>) -> Vec<Thumbnail> {
    let extra = smaller_variants(&out);
    out.extend(extra);
    out.sort_by_key(|t| (t.width, t.height));
    out
}

/// Sizes added for resizable Google image URLs. The art slots are 24, 32,
/// 40 and 112 px (D-Tokens), drawn at 1x or 2x; the app picks the smallest
/// thumbnail that covers the slot.
const RESIZED_SIDES: [u32; 4] = [40, 60, 120, 226];

/// Fixed-size frames every video has on i.ytimg.com: `default.jpg` and
/// `mqdefault.jpg`. YouTube Music lists videos with a 400 px or larger frame.
const YTIMG_FRAMES: [(&str, u32, u32); 2] = [("default", 120, 90), ("mqdefault", 320, 180)];

/// Smaller copies of the art, for URL forms checked to accept a size
/// (2026-10-26): `lh3`/`yt3.googleusercontent.com/...=wN-hN[-flags]` scales
/// to any square size, and `i.ytimg.com/vi/<id>/` serves the fixed frames
/// above. Other forms get nothing. Never adds a size that is already there or
/// larger than the largest one YouTube gave.
fn smaller_variants(thumbs: &[Thumbnail]) -> Vec<Thumbnail> {
    let has = |w: u32, h: u32| thumbs.iter().any(|t| t.width == w && t.height == h);
    let mut extra = Vec::new();
    let resizable = thumbs
        .iter()
        .filter_map(|t| resizable_parts(&t.url).filter(|p| p.1 == t.width && t.width == t.height))
        .max_by_key(|p| p.1);
    if let Some((prefix, largest, flags)) = resizable {
        for side in RESIZED_SIDES {
            if side < largest && !has(side, side) {
                extra.push(Thumbnail {
                    url: format!("{prefix}=w{side}-h{side}{flags}"),
                    width: side,
                    height: side,
                });
            }
        }
    }
    let frame = thumbs
        .iter()
        .filter_map(|t| ytimg_video_id(&t.url).map(|id| (id, t.width.min(t.height))))
        .max_by_key(|f| f.1);
    if let Some((id, largest_side)) = frame {
        for (name, w, h) in YTIMG_FRAMES {
            if h < largest_side && !has(w, h) {
                extra.push(Thumbnail {
                    url: format!("https://i.ytimg.com/vi/{id}/{name}.jpg"),
                    width: w,
                    height: h,
                });
            }
        }
    }
    extra
}

/// `(prefix, side, flags)` of `https://{lh3,yt3}.googleusercontent.com/X=wN-hN<flags>`
/// with N == N.
fn resizable_parts(url: &str) -> Option<(&str, u32, &str)> {
    if !(url.starts_with("https://lh3.googleusercontent.com/")
        || url.starts_with("https://yt3.googleusercontent.com/"))
        || url.contains('?')
    {
        return None;
    }
    let (prefix, params) = url.rsplit_once('=')?;
    let (w, rest) = params.strip_prefix('w')?.split_once('-')?;
    let rest = rest.strip_prefix('h')?;
    let h_end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (h, flags) = rest.split_at(h_end);
    if !(flags.is_empty() || flags.starts_with('-')) {
        return None;
    }
    let side: u32 = w.parse().ok()?;
    (h.parse::<u32>().ok()? == side).then_some((prefix, side, flags))
}

/// The video id in `https://i.ytimg.com/vi/<id>/...` or `/vi_webp/<id>/...`.
fn ytimg_video_id(url: &str) -> Option<&str> {
    let rest = url
        .strip_prefix("https://i.ytimg.com/vi/")
        .or_else(|| url.strip_prefix("https://i.ytimg.com/vi_webp/"))?;
    let id = rest.split('/').next()?;
    is_video_id(id).then_some(id)
}

fn map_artists(artists: Vec<rp::ArtistId>) -> Vec<ArtistRef> {
    artists
        .into_iter()
        .map(|a| ArtistRef {
            id: a.id,
            name: a.name,
        })
        .collect()
}

fn map_track(t: rp::TrackItem) -> Track {
    Track {
        id: t.id,
        title: t.name,
        artists: map_artists(t.artists),
        album: t.album.map(|a| AlbumRef {
            id: a.id,
            name: a.name,
        }),
        duration: t.duration.map(|s| Duration::from_secs(s.into())),
        thumbnails: map_thumbnails(t.cover),
    }
}

fn map_album_type(t: rp::AlbumType) -> AlbumType {
    match t {
        rp::AlbumType::Album => AlbumType::Album,
        rp::AlbumType::Ep => AlbumType::Ep,
        rp::AlbumType::Single => AlbumType::Single,
        _ => AlbumType::Unknown,
    }
}

fn map_album(a: rp::AlbumItem) -> AlbumSummary {
    AlbumSummary {
        id: a.id,
        name: a.name,
        artists: map_artists(a.artists),
        year: a.year,
        thumbnails: map_thumbnails(a.cover),
        album_type: map_album_type(a.album_type),
    }
}

fn map_artist(a: rp::ArtistItem) -> ArtistSummary {
    ArtistSummary {
        id: a.id,
        name: a.name,
        thumbnails: map_thumbnails(a.avatar),
    }
}

fn playlist_owner(channel: Option<rp::ChannelId>, from_ytm: bool) -> Option<String> {
    match channel {
        Some(c) => Some(c.name),
        // Editorial playlists have no channel.
        None if from_ytm => Some("YouTube Music".to_owned()),
        None => None,
    }
}

fn map_playlist_item(p: rp::MusicPlaylistItem) -> PlaylistSummary {
    PlaylistSummary {
        id: p.id,
        name: p.name,
        owner: playlist_owner(p.channel, p.from_ytm),
        track_count: p.track_count,
        thumbnails: map_thumbnails(p.thumbnail),
    }
}

/// A playlist built from the raw pages alone (see `MusicBackend::playlist`):
/// rows without a title are skipped, as rustypipe skips them.
fn playlist_from_raw(id: &str, raw: innertube::RawPlaylist) -> Playlist {
    let header = raw.header.unwrap_or_default();
    let (tracks, entry_ids): (Vec<Track>, Vec<Option<String>>) = raw
        .entries
        .into_iter()
        .filter_map(|e| {
            let mut t = e.track?;
            t.thumbnails = with_smaller_variants(std::mem::take(&mut t.thumbnails));
            Some((t, e.set_video_id))
        })
        .unzip();
    Playlist {
        summary: PlaylistSummary {
            id: id.to_owned(),
            name: if header.name.is_empty() {
                id.to_owned()
            } else {
                header.name
            },
            owner: header.owner,
            track_count: header.track_count.or(Some(tracks.len() as u64)),
            thumbnails: with_smaller_variants(header.thumbnails),
        },
        tracks,
        entry_ids,
    }
}

fn map_playlist(mut p: rp::MusicPlaylist) -> Playlist {
    p.tracks.items.truncate(MAX_ITEMS);
    Playlist {
        summary: PlaylistSummary {
            id: p.id,
            name: p.name,
            owner: playlist_owner(p.channel, p.from_ytm),
            track_count: p.track_count,
            thumbnails: map_thumbnails(p.thumbnail),
        },
        tracks: p.tracks.items.into_iter().map(map_track).collect(),
        // Filled by `MusicBackend::playlist` from the raw pages.
        entry_ids: Vec::new(),
    }
}

/// The four search pages, with albums re-ranked by the songs (see
/// `rank::rank_albums`).
fn map_search(
    query: &str,
    tracks: Vec<rp::TrackItem>,
    albums: Vec<rp::AlbumItem>,
    artists: Vec<rp::ArtistItem>,
    playlists: Vec<rp::MusicPlaylistItem>,
) -> SearchResults {
    let tracks: Vec<Track> = tracks.into_iter().map(map_track).collect();
    let mut albums: Vec<AlbumSummary> = albums.into_iter().map(map_album).collect();
    rank::rank_albums(&mut albums, &tracks, query);
    SearchResults {
        tracks,
        albums,
        artists: artists.into_iter().map(map_artist).collect(),
        playlists: playlists.into_iter().map(map_playlist_item).collect(),
    }
}

/// A YouTube video id: 11 chars of `[A-Za-z0-9_-]`. Unavailable rows have
/// none.
fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// An album page. Tracks keep the page order (track numbers restart on each
/// disc); rows without a video id are skipped. Album rows carry no art and
/// sometimes no artists, so they get the album's.
fn map_music_album(a: rp::MusicAlbum) -> Album {
    let album_ref = AlbumRef {
        id: a.id.clone(),
        name: a.name.clone(),
    };
    let summary = AlbumSummary {
        id: a.id,
        name: a.name,
        artists: map_artists(a.artists),
        year: a.year,
        thumbnails: map_thumbnails(a.cover),
        album_type: map_album_type(a.album_type),
    };
    let tracks = a
        .tracks
        .into_iter()
        .filter(|t| is_video_id(&t.id))
        .map(|t| {
            let mut track = map_track(t);
            track.album = Some(album_ref.clone());
            if track.artists.is_empty() {
                track.artists.clone_from(&summary.artists);
            }
            if track.thumbnails.is_empty() {
                track.thumbnails.clone_from(&summary.thumbnails);
            }
            track
        })
        .collect();
    Album { summary, tracks }
}

/// An artist page. rustypipe puts the "Songs" shelf and the "Videos"
/// carousel into one track list; shelf rows name their album and carousel
/// videos never do, so only tracks with an album count as top songs.
/// Singles and EPs go to `singles`, everything else to `albums`.
fn map_music_artist(a: rp::MusicArtist) -> Artist {
    let top_songs = a
        .tracks
        .into_iter()
        .filter(|t| t.album.is_some() && is_video_id(&t.id))
        .map(map_track)
        .collect();
    let mut seen = std::collections::HashSet::new();
    let (singles, albums): (Vec<rp::AlbumItem>, Vec<rp::AlbumItem>) = a
        .albums
        .into_iter()
        .filter(|al| seen.insert(al.id.clone()))
        .partition(|al| matches!(al.album_type, rp::AlbumType::Single | rp::AlbumType::Ep));
    Artist {
        summary: ArtistSummary {
            id: a.id,
            name: a.name,
            // The wide header image: no square sizes exist for it.
            thumbnails: map_thumbnails(a.header_image),
        },
        top_songs,
        albums: albums.into_iter().map(map_album).collect(),
        singles: singles.into_iter().map(map_album).collect(),
    }
}

/// Radio tracks without the seed and without repeats, in YouTube's order.
fn map_radio(seed: &str, items: Vec<rp::TrackItem>) -> Vec<Track> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|t| t.id != seed && seen.insert(t.id.clone()))
        .map(map_track)
        .collect()
}

#[cfg(test)]
mod tests {
    use rustypipe::error::{AuthError, UnavailabilityReason};
    use serde_json::json;

    use super::*;

    #[test]
    fn playlist_from_raw_pages() {
        let page = innertube::parse_entry_page(
            &serde_json::from_str(include_str!("../fixtures/browse_owned_playlist_real.json"))
                .unwrap(),
        )
        .unwrap();
        let raw = innertube::RawPlaylist {
            header: page.header,
            entries: page.entries,
        };
        let p = playlist_from_raw("PLownedreal", raw);
        assert_eq!(p.summary.id, "PLownedreal");
        assert_eq!(p.summary.name, "LikedSongsMigration");
        assert_eq!(p.summary.track_count, Some(1535));
        assert_eq!(p.tracks.len(), 3);
        assert_eq!(p.entry_ids.len(), 3);
        assert_eq!(p.entry_ids[0].as_deref(), Some("56B44F6D10557CC6"));
        // Smaller copies of the resizable art, smallest first, as for
        // rustypipe's playlists.
        let sides: Vec<u32> = p.tracks[0].thumbnails.iter().map(|t| t.width).collect();
        assert_eq!(sides, [40, 60, 120]);

        // No header: the id stands in for the name, the rows for the count.
        let bare = playlist_from_raw(
            "PLx",
            innertube::RawPlaylist {
                header: None,
                entries: p
                    .tracks
                    .iter()
                    .map(|t| innertube::RawEntry {
                        video_id: t.id.clone(),
                        set_video_id: None,
                        track: Some(t.clone()),
                    })
                    .collect(),
            },
        );
        assert_eq!(bare.summary.name, "PLx");
        assert_eq!(bare.summary.track_count, Some(3));
    }

    #[test]
    fn auth_errors_map_to_auth() {
        assert!(matches!(
            map_error(RpError::Auth(AuthError::NoLogin)),
            BackendError::Auth
        ));
        assert!(matches!(
            map_error(RpError::HttpStatus(401, "unauthorized".into())),
            BackendError::Auth
        ));
    }

    #[test]
    fn not_found_keeps_id() {
        let e = RpError::Extraction(ExtractionError::NotFound {
            id: "PLnope".to_owned(),
            msg: "404".into(),
        });
        match map_error(e) {
            BackendError::NotFound(id) => assert_eq!(id, "PLnope"),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn http_errors_map_to_network() {
        assert!(matches!(
            map_error(RpError::Http("connection reset".into())),
            BackendError::Network(_)
        ));
        assert!(matches!(
            map_error(RpError::HttpStatus(503, "unavailable".into())),
            BackendError::Network(_)
        ));
    }

    #[test]
    fn empty_bodies_map_to_network() {
        // What rustypipe returns for a throttled 2xx with an empty body.
        let empty = serde_json::from_str::<serde_json::Value>("").unwrap_err();
        let e = RpError::Extraction(ExtractionError::from(empty));
        assert!(matches!(map_error(e), BackendError::Network(_)));
        let cut = serde_json::from_str::<serde_json::Value>("{\"contents\": [").unwrap_err();
        let e = RpError::Extraction(ExtractionError::from(cut));
        assert!(matches!(map_error(e), BackendError::Network(_)));
        // Valid JSON of the wrong shape is still an extraction failure.
        let wrong = serde_json::from_str::<Vec<u8>>("{}").unwrap_err();
        let e = RpError::Extraction(ExtractionError::from(wrong));
        assert!(matches!(map_error(e), BackendError::Extraction(_)));
    }

    #[test]
    fn other_errors_map_to_extraction() {
        let unavailable = RpError::Extraction(ExtractionError::Unavailable {
            reason: UnavailabilityReason::Geoblocked,
            msg: "not in your country".to_owned(),
        });
        for e in [
            unavailable,
            RpError::Extraction(ExtractionError::InvalidData("bad json".into())),
            RpError::Extraction(ExtractionError::Botguard("crashed".into())),
            RpError::Other("no clients".into()),
        ] {
            assert!(matches!(map_error(e), BackendError::Extraction(_)));
        }
    }

    #[test]
    fn cookie_input_detection() {
        assert_eq!(
            parse_cookie_input("SID=abc; HSID=def"),
            CookieInput::Header("SID=abc; HSID=def")
        );
        assert_eq!(
            parse_cookie_input("Cookie: SID=abc; HSID=def\n"),
            CookieInput::Header("SID=abc; HSID=def")
        );
        let txt = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n";
        assert_eq!(
            parse_cookie_input(txt),
            CookieInput::NetscapeTxt(txt.trim())
        );
        let no_header = ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc";
        assert_eq!(
            parse_cookie_input(no_header),
            CookieInput::NetscapeTxt(no_header)
        );
    }

    #[test]
    fn cache_file_never_holds_cookies() {
        let data = json!({
            "clients": {"desktop": {"data": {"version": "2.2025"}}},
            "auth_cookie": {"cookie": "SID=secret"},
            "oauth_token": {"access_token": "secret"},
        })
        .to_string();
        let clean = strip_secrets(&data).unwrap();
        assert!(!clean.contains("secret"));
        assert!(clean.contains("2.2025"));
        assert_eq!(strip_secrets("not json"), None);
    }

    const VISIONOS_PLAYER: &str = include_str!("../fixtures/visionos_player.json");

    fn player_status(status: &str, reason: &str) -> String {
        json!({ "playabilityStatus": { "status": status, "reason": reason } }).to_string()
    }

    fn bot_check() -> String {
        player_status("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot")
    }

    /// What one `resolve_visionos` run did.
    struct Run {
        result: Result<StreamSource>,
        /// Visitor ids asked for, `true` for a fresh one.
        asked: Vec<bool>,
        dropped: Vec<String>,
        /// The visitor id each `player` request carried.
        sent: Vec<String>,
    }

    /// Runs `resolve_visionos` with the recorded `player` bodies, one per
    /// request; visitor ids are `vd1`, `vd2`, ... in the order asked.
    async fn resolve_with(bodies: Vec<String>) -> Run {
        let (mut asked, mut dropped, mut sent) = (Vec::new(), Vec::new(), Vec::new());
        let mut bodies = bodies.into_iter();
        let result = resolve_visionos(
            "jvm6DpqqbLk",
            |fresh| {
                asked.push(fresh);
                std::future::ready(Ok(format!("vd{}", asked.len())))
            },
            |vd| dropped.push(vd.to_owned()),
            |vd| {
                sent.push(vd);
                let body = bodies.next().expect("one request too many");
                std::future::ready(visionos::parse_player("jvm6DpqqbLk", &body))
            },
        )
        .await;
        Run {
            result,
            asked,
            dropped,
            sent,
        }
    }

    #[tokio::test]
    async fn visionos_resolves_with_the_cached_visitor_id() {
        let run = resolve_with(vec![VISIONOS_PLAYER.to_owned()]).await;
        let s = run.result.unwrap();
        assert_eq!(s.codec, model::AudioCodec::Opus);
        assert!(s.url.contains("itag=251"), "{}", s.url);
        assert_eq!(run.asked, [false]);
        assert!(run.dropped.is_empty());
        assert_eq!(run.sent, ["vd1"]);
    }

    #[tokio::test]
    async fn visionos_bot_check_retries_once_with_a_fresh_visitor_id() {
        let run = resolve_with(vec![bot_check(), VISIONOS_PLAYER.to_owned()]).await;
        assert!(run.result.unwrap().url.contains("itag=251"));
        assert_eq!(run.asked, [false, true]);
        assert_eq!(run.dropped, ["vd1"]);
        assert_eq!(run.sent, ["vd1", "vd2"]);
    }

    #[tokio::test]
    async fn visionos_second_bot_check_is_a_clear_error() {
        let run = resolve_with(vec![bot_check(), bot_check()]).await;
        // Not `Auth`, which would log the user out.
        match run.result {
            Err(BackendError::Extraction(m)) => {
                assert!(
                    m.contains("not a bot") && m.contains("fresh visitor id"),
                    "{m}"
                );
            }
            other => panic!("got {other:?}"),
        }
        // Both ids are burnt; no third request.
        assert_eq!(run.asked, [false, true]);
        assert_eq!(run.dropped, ["vd1", "vd2"]);
        assert_eq!(run.sent.len(), 2);
    }

    #[tokio::test]
    async fn visionos_other_failures_do_not_retry() {
        let run = resolve_with(vec![player_status("ERROR", "Video unavailable")]).await;
        assert!(matches!(run.result, Err(BackendError::NotFound(ref id)) if id == "jvm6DpqqbLk"));
        assert_eq!(run.asked, [false]);
        let run = resolve_with(vec![player_status("UNPLAYABLE", "This video is private")]).await;
        assert!(
            matches!(run.result, Err(BackendError::Extraction(ref m)) if m.contains("private"))
        );
        assert!(run.dropped.is_empty());

        let r = resolve_visionos(
            "jvm6DpqqbLk",
            |_| std::future::ready(Ok("vd1".to_owned())),
            |_| panic!("nothing to drop"),
            |_| std::future::ready(Err(VisionOsError::Network("timed out".to_owned()))),
        )
        .await;
        assert!(matches!(r, Err(BackendError::Network(_))));
        // No visitor id, no request.
        let r = resolve_visionos(
            "jvm6DpqqbLk",
            |_| std::future::ready(Err(RpError::Http("connection reset".into()))),
            |_| panic!("nothing to drop"),
            |_: String| -> std::future::Ready<std::result::Result<StreamSource, VisionOsError>> {
                panic!("request sent without a visitor id")
            },
        )
        .await;
        assert!(matches!(r, Err(BackendError::Network(_))));
    }

    fn thumb(url: &str, side: u32) -> Thumbnail {
        Thumbnail {
            url: url.to_owned(),
            width: side,
            height: side,
        }
    }

    #[test]
    fn resizable_art_gets_smaller_sizes() {
        let base = "https://lh3.googleusercontent.com/abc";
        let out: Vec<rp::Thumbnail> = serde_json::from_value(json!([
            {"url": format!("{base}=w544-h544-l90-rj"), "width": 544, "height": 544},
            {"url": format!("{base}=w60-h60-l90-rj"), "width": 60, "height": 60},
        ]))
        .unwrap();
        let out = map_thumbnails(out);
        let got: Vec<(u32, &str)> = out.iter().map(|t| (t.width, t.url.as_str())).collect();
        assert_eq!(
            got,
            [
                (40, "https://lh3.googleusercontent.com/abc=w40-h40-l90-rj"),
                (60, "https://lh3.googleusercontent.com/abc=w60-h60-l90-rj"),
                (
                    120,
                    "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"
                ),
                (
                    226,
                    "https://lh3.googleusercontent.com/abc=w226-h226-l90-rj"
                ),
                (
                    544,
                    "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj"
                ),
            ]
        );
        // Flags between the size and the end stay; nothing larger is added.
        let yt3 = smaller_variants(&[thumb(
            "https://yt3.googleusercontent.com/x=w120-h120-p-l90-rj",
            120,
        )]);
        let urls: Vec<&str> = yt3.iter().map(|t| t.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://yt3.googleusercontent.com/x=w40-h40-p-l90-rj",
                "https://yt3.googleusercontent.com/x=w60-h60-p-l90-rj",
            ]
        );
    }

    #[test]
    fn video_frames_get_small_fixed_frames() {
        let hq = Thumbnail {
            url: "https://i.ytimg.com/vi/D8K90hX4PrE/hqdefault.jpg?sqp=x&rs=y".to_owned(),
            width: 400,
            height: 225,
        };
        let extra = smaller_variants(std::slice::from_ref(&hq));
        let got: Vec<(&str, u32, u32)> = extra
            .iter()
            .map(|t| (t.url.as_str(), t.width, t.height))
            .collect();
        assert_eq!(
            got,
            [
                ("https://i.ytimg.com/vi/D8K90hX4PrE/default.jpg", 120, 90),
                ("https://i.ytimg.com/vi/D8K90hX4PrE/mqdefault.jpg", 320, 180),
            ]
        );
    }

    #[test]
    fn unknown_url_forms_are_left_alone() {
        for t in [
            thumb("https://lh3.googleusercontent.com/abc=s544", 544),
            thumb(
                "https://lh3.googleusercontent.com/abc=w544-h300-l90-rj",
                544,
            ),
            thumb("https://lh3.googleusercontent.com/abc=w544-h544?x=1", 544),
            thumb("https://example.com/abc=w544-h544", 544),
            thumb("https://i.ytimg.com/vi/short/hqdefault.jpg", 360),
            thumb("https://i.ytimg.com/vi/D8K90hX4PrE/default.jpg", 90),
        ] {
            assert!(
                smaller_variants(std::slice::from_ref(&t)).is_empty(),
                "{}",
                t.url
            );
        }
    }

    #[test]
    fn radio_drops_seed_and_repeats() {
        let p: rp::paginator::Paginator<rp::TrackItem> =
            serde_json::from_str(include_str!("../fixtures/rp_radio.json")).unwrap();
        let seed = p.items[0].id.clone();
        let tracks = map_radio(&seed, p.items);
        assert!(tracks.len() >= RADIO_MIN, "{}", tracks.len());
        assert!(tracks.iter().all(|t| t.id != seed));
        let mut ids: Vec<&str> = tracks.iter().map(|t| t.id.as_str()).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[tokio::test]
    async fn writes_without_cookies_are_auth_errors_offline() {
        let dir = std::env::temp_dir().join(format!("rustle-ytm-write-{}", std::process::id()));
        let b = RustyPipeBackend::with_storage_dir(&dir).unwrap();
        let ids = ["D8K90hX4PrE".to_owned()];
        let entries = [("D8K90hX4PrE".to_owned(), "1F3E5D7C9B0A2468".to_owned())];
        assert!(matches!(
            b.like("D8K90hX4PrE").await,
            Err(BackendError::Auth)
        ));
        assert!(matches!(
            b.unlike("D8K90hX4PrE").await,
            Err(BackendError::Auth)
        ));
        assert!(matches!(
            b.add_to_playlist("PLx", &ids).await,
            Err(BackendError::Auth)
        ));
        assert!(matches!(
            b.remove_from_playlist("PLx", &entries).await,
            Err(BackendError::Auth)
        ));
        // Logging out twice is fine.
        b.clear_cookies().await.unwrap();
        b.clear_cookies().await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn sorted(thumbs: &[Thumbnail]) -> bool {
        thumbs
            .windows(2)
            .all(|w| (w[0].width, w[0].height) <= (w[1].width, w[1].height))
    }

    /// `album.json` and `artist.json` are the mapping of the recorded pages.
    /// Run with `UPDATE_FIXTURES=1` after `spike record-pages`.
    #[test]
    fn recorded_pages_map_to_fixtures() {
        let rp_album: rp::MusicAlbum =
            serde_json::from_str(include_str!("../fixtures/rp_album.json")).unwrap();
        let rp_artist: rp::MusicArtist =
            serde_json::from_str(include_str!("../fixtures/rp_artist.json")).unwrap();
        let rp_covers = rp_album.cover.len();
        let rp_tracks = rp_album.tracks.len();
        let track_count = usize::from(rp_album.track_count);
        let album = map_music_album(rp_album.clone());
        let artist = map_music_artist(rp_artist.clone());

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            let write = |name: &str, json: String| {
                std::fs::write(dir.join(name), json + "\n").unwrap();
            };
            write("album.json", serde_json::to_string_pretty(&album).unwrap());
            write(
                "artist.json",
                serde_json::to_string_pretty(&artist).unwrap(),
            );
            return;
        }
        let want_album: Album =
            serde_json::from_str(include_str!("../fixtures/album.json")).unwrap();
        let want_artist: Artist =
            serde_json::from_str(include_str!("../fixtures/artist.json")).unwrap();
        assert_eq!(album, want_album);
        assert_eq!(artist, want_artist);

        // Album: every track, in page order, pointing at this album.
        let s = &album.summary;
        assert_eq!(s.id, "MPREb_OgeEnoHTsCm");
        assert_eq!(s.name, "Random Access Memories (Drumless Edition)");
        assert_eq!(s.year, Some(2023));
        assert_eq!(s.artists[0].name, "Daft Punk");
        assert_eq!(album.tracks.len(), track_count);
        assert_eq!(album.tracks.len(), rp_tracks);
        let ids: Vec<&str> = album.tracks.iter().map(|t| t.id.as_str()).collect();
        let rp_ids: Vec<&str> = rp_album.tracks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, rp_ids);
        assert_eq!(ids.first(), Some(&"asPohQE-tB4"));
        assert_eq!(ids.last(), Some(&"gPdua5JqBgw"));
        let album_ref = AlbumRef {
            id: s.id.clone(),
            name: s.name.clone(),
        };
        // The 24, 32 and 40 px slots get a 40 px copy of the cover.
        assert!(s.thumbnails.len() > rp_covers);
        assert_eq!(s.thumbnails[0].width, 40);
        assert!(sorted(&s.thumbnails));
        for t in &album.tracks {
            assert_eq!(t.album.as_ref(), Some(&album_ref), "{t:?}");
            assert!(!t.artists.is_empty(), "{t:?}");
            assert!(t.duration.is_some(), "{t:?}");
            assert_eq!(t.thumbnails, s.thumbnails, "{t:?}");
        }

        // Artist: the five shelf songs, then albums and singles split by type.
        assert_eq!(artist.summary.id, "UCRr1xG_2WIDs18a6cIiCxeA");
        assert_eq!(artist.summary.name, "Daft Punk");
        assert!(!artist.summary.thumbnails.is_empty());
        assert!(sorted(&artist.summary.thumbnails));
        let top: Vec<&str> = artist.top_songs.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            top,
            [
                "khnokW3Mw24",
                "Rgrt_8mXrK8",
                "wU26xVT_vBU",
                "Jb6gcoR266U",
                "qe8Q7mjxjig"
            ]
        );
        for t in &artist.top_songs {
            assert!(t.album.is_some(), "{t:?}");
            assert!(sorted(&t.thumbnails), "{t:?}");
        }
        let type_of = |id: &str| {
            rp_artist
                .albums
                .iter()
                .find(|a| a.id == id)
                .map(|a| a.album_type)
                .unwrap()
        };
        assert_eq!(artist.albums.len(), 10);
        assert_eq!(artist.singles.len(), 10);
        assert_eq!(artist.albums[1].name, "Random Access Memories");
        assert_eq!(artist.albums[1].year, Some(2013));
        for a in &artist.albums {
            assert!(!matches!(
                type_of(&a.id),
                rp::AlbumType::Single | rp::AlbumType::Ep
            ));
        }
        for a in &artist.singles {
            assert!(matches!(
                type_of(&a.id),
                rp::AlbumType::Single | rp::AlbumType::Ep
            ));
        }
        for a in artist.albums.iter().chain(&artist.singles) {
            assert!(a.year.is_some(), "{a:?}");
            assert!(sorted(&a.thumbnails), "{a:?}");
        }
    }

    #[test]
    fn album_skips_rows_without_a_video_id() {
        let mut a: rp::MusicAlbum =
            serde_json::from_str(include_str!("../fixtures/rp_album.json")).unwrap();
        let n = a.tracks.len();
        a.tracks[1].id = String::new();
        let album = map_music_album(a);
        assert_eq!(album.tracks.len(), n - 1);
        assert_eq!(album.tracks[1].id, "CAoX8vchMqU");
    }

    #[test]
    fn artist_dedupes_albums_and_puts_eps_with_singles() {
        let mut a: rp::MusicArtist =
            serde_json::from_str(include_str!("../fixtures/rp_artist.json")).unwrap();
        let mut ep = a.albums[0].clone();
        ep.id = "MPREb_epepepepepep".to_owned();
        ep.album_type = rp::AlbumType::Ep;
        let repeat = a.albums[0].clone();
        a.albums.push(ep);
        a.albums.push(repeat);
        let artist = map_music_artist(a);
        assert_eq!(artist.albums.len(), 10);
        assert_eq!(artist.singles.len(), 11);
        assert_eq!(artist.singles[10].id, "MPREb_epepepepepep");
    }

    /// "heaven knows" (`rp_search_heaven_knows.json`): YouTube's album page
    /// is twenty "Heaven Knows" releases, mostly unknown singles, and none by
    /// the artists of the songs page.
    #[test]
    fn heaven_knows_albums_rerank() {
        #[derive(serde::Deserialize)]
        struct RecordedSearch {
            query: String,
            tracks: Vec<rp::TrackItem>,
            albums: Vec<rp::AlbumItem>,
        }
        let s: RecordedSearch =
            serde_json::from_str(include_str!("../fixtures/rp_search_heaven_knows.json")).unwrap();
        let line = |a: &AlbumSummary| (a.artists[0].name.clone(), a.album_type);
        let youtube: Vec<_> = s.albums.iter().cloned().map(map_album).collect();
        assert_eq!(line(&youtube[5]), ("Warn Torn".into(), AlbumType::Single));

        // Recorded as is: full albums named after the query first, then the
        // EP, then the singles in YouTube's order, then other names.
        let res = map_search(&s.query, s.tracks.clone(), s.albums.clone(), vec![], vec![]);
        assert_eq!(res.albums.len(), youtube.len());
        let top: Vec<_> = res.albums.iter().take(5).map(line).collect();
        assert_eq!(
            top,
            [
                ("PinkPantheress".into(), AlbumType::Album),
                ("Hilux Vibez".into(), AlbumType::Album),
                ("Jim Croegaert".into(), AlbumType::Album),
                ("Jonathan Sim".into(), AlbumType::Ep),
                ("Peaceful Rhythms".into(), AlbumType::Single),
            ]
        );
        let singles: Vec<&str> = youtube
            .iter()
            .filter(|a| a.album_type == AlbumType::Single && a.name == "Heaven Knows")
            .map(|a| a.id.as_str())
            .collect();
        let ranked: Vec<&str> = res.albums[4..].iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ranked[..singles.len()], singles);
        assert!(
            res.albums
                .last()
                .unwrap()
                .name
                .starts_with("Heaven Knows / ")
        );

        // Albums by the songs' artists come first, in song order, wherever
        // YouTube put them: song #1 is Robert Plant, song #2 The Pretty
        // Reckless.
        assert_eq!(res.tracks[0].artists[0].name, "Robert Plant");
        assert_eq!(res.tracks[1].artists[0].name, "The Pretty Reckless");
        let by = |id: &str, track: usize| {
            let mut a = s.albums[1].clone();
            a.id = id.to_owned();
            a.artists = s.tracks[track].artists.clone();
            a
        };
        let mut albums = s.albums.clone();
        albums.push(by("MPREb_reckless", 1));
        albums.insert(3, by("MPREb_plant", 0));
        let res = map_search(&s.query, s.tracks, albums, vec![], vec![]);
        let ids: Vec<&str> = res.albums.iter().take(3).map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["MPREb_plant", "MPREb_reckless", &youtube[0].id]);
    }

    /// The model JSON fixtures are the mapping of the recorded rustypipe
    /// responses. Run with `UPDATE_FIXTURES=1` after re-recording.
    #[test]
    fn recorded_responses_map_to_fixtures() {
        /// Written by `spike record`: one page per search type.
        #[derive(serde::Deserialize)]
        struct RecordedSearch {
            tracks: Vec<rp::TrackItem>,
            albums: Vec<rp::AlbumItem>,
            artists: Vec<rp::ArtistItem>,
            playlists: Vec<rp::MusicPlaylistItem>,
        }
        let s: RecordedSearch =
            serde_json::from_str(include_str!("../fixtures/rp_search.json")).unwrap();
        let playlist: rp::MusicPlaylist =
            serde_json::from_str(include_str!("../fixtures/rp_playlist.json")).unwrap();
        let search = map_search("daft punk", s.tracks, s.albums, s.artists, s.playlists);
        let playlist = map_playlist(playlist);

        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        if std::env::var_os("UPDATE_FIXTURES").is_some() {
            let write = |name: &str, json: String| {
                std::fs::write(dir.join(name), json + "\n").unwrap();
            };
            write(
                "search.json",
                serde_json::to_string_pretty(&search).unwrap(),
            );
            write(
                "playlist.json",
                serde_json::to_string_pretty(&playlist).unwrap(),
            );
            return;
        }
        let want_search: SearchResults =
            serde_json::from_str(include_str!("../fixtures/search.json")).unwrap();
        let want_playlist: Playlist =
            serde_json::from_str(include_str!("../fixtures/playlist.json")).unwrap();
        assert_eq!(search, want_search);
        assert_eq!(playlist, want_playlist);

        assert!(!search.tracks.is_empty());
        assert!(!search.albums.is_empty());
        assert!(!search.artists.is_empty());
        assert!(playlist.tracks.len() > 10);
        for t in search.tracks.iter().chain(&playlist.tracks) {
            assert_eq!(t.id.len(), 11, "{t:?}");
            assert!(!t.artists.is_empty(), "{t:?}");
            assert!(t.thumbnails.windows(2).all(|w| w[0].width <= w[1].width));
        }
    }

    fn plain_fixture() -> rp::Lyrics {
        serde_json::from_str(include_str!("../fixtures/rp_lyrics_plain.json")).unwrap()
    }

    fn synced_fixture() -> Lyrics {
        match innertube::parse_timed_lyrics(include_str!("../fixtures/lyrics_timed_synced.json")) {
            Ok(innertube::TimedLyrics::Found(l)) => l,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn lyrics_prefer_timed() {
        let got = fetch_lyrics(
            "v",
            Some("MPLYt_x".to_owned()),
            async |id| {
                assert_eq!(id, "MPLYt_x");
                Ok(innertube::TimedLyrics::Found(synced_fixture()))
            },
            async |_| panic!("plain lyrics fetched although timed ones exist"),
        )
        .await
        .unwrap();
        assert_eq!(got, Some(synced_fixture()));
    }

    #[tokio::test]
    async fn lyrics_fall_back_to_plain() {
        for timed in [
            Ok(innertube::TimedLyrics::Unknown),
            Err(BackendError::Network("HTTP 429".to_owned())),
        ] {
            let got = fetch_lyrics(
                "v",
                Some("MPLYt_x".to_owned()),
                async |_| timed,
                async |_| Ok(plain_fixture()),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(!got.synced);
            assert_eq!(got.source.as_deref(), Some("Source: LyricFind"));
            assert_eq!(got.lines[0].text, "Rock, robot rock");
            assert!(got.lines.iter().all(|l| l.start.is_none()));
            // CRLF line ends are gone; blank lines stay as stanza breaks.
            assert!(got.lines.iter().all(|l| !l.text.contains('\r')));
            assert_eq!(got.lines[4].text, "");
        }
    }

    #[tokio::test]
    async fn no_lyrics() {
        // No lyrics id: nothing is fetched.
        let got = fetch_lyrics(
            "v",
            None,
            async |_| panic!("timed lyrics fetched without an id"),
            async |_| panic!("plain lyrics fetched without an id"),
        )
        .await
        .unwrap();
        assert_eq!(got, None);
        // YouTube says "not available": no plain request either.
        let got = fetch_lyrics(
            "v",
            Some("MPLYt_x".to_owned()),
            async |_| Ok(innertube::TimedLyrics::Unavailable),
            async |_| panic!("plain lyrics fetched after \"not available\""),
        )
        .await
        .unwrap();
        assert_eq!(got, None);
        // An empty plain body.
        let mut empty = plain_fixture();
        empty.body = " \r\n".to_owned();
        assert_eq!(map_plain_lyrics(empty), None);
    }

    #[tokio::test]
    async fn plain_lyrics_errors_map() {
        let got = fetch_lyrics(
            "v",
            Some("MPLYt_x".to_owned()),
            async |_| Ok(innertube::TimedLyrics::Unknown),
            async |_| Err(RpError::HttpStatus(503, "".into())),
        )
        .await;
        assert!(matches!(got, Err(BackendError::Network(_))));
    }
}
