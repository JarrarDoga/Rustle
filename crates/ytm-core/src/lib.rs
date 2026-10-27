//! YouTube Music access. The only crate that knows rustypipe exists.
//!
//! The app talks to [`MusicBackend`] and nothing else, so when YouTube changes
//! something, only this crate gets patched.

use std::future::Future;

pub use model;
use model::{
    Album, Artist, Lyrics, Playlist, PlaylistSummary, SearchResults, StreamSource, Track, TrackId,
};

mod fixture;
mod innertube;
mod rustypipe_backend;
mod stream_cache;
mod ytdlp;

pub use fixture::{DEMO_MAIN, FixtureBackend, LARGE_ART_SIZES};
pub use rustypipe_backend::{RustyPipeBackend, StreamInfo, StreamOrigin};

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// Cookies missing, rejected, or expired. The UI shows the re-paste banner.
    #[error("not logged in or session expired")]
    Auth,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("network error: {0}")]
    Network(String),
    /// YouTube returned something we could not parse or extract.
    #[error("extraction failed: {0}")]
    Extraction(String),
}

pub type Result<T> = std::result::Result<T, BackendError>;

/// Everything the app may ask of YouTube Music.
///
/// Keep this small. A new method needs a UI feature that uses it.
pub trait MusicBackend: Send + Sync + 'static {
    /// Log in with a `Cookie:` header value or a Netscape cookies.txt body.
    fn set_cookies(&self, cookies: &str) -> impl Future<Output = Result<()>> + Send;

    fn search(&self, query: &str) -> impl Future<Output = Result<SearchResults>> + Send;

    /// The logged-in user's saved playlists.
    fn library_playlists(&self) -> impl Future<Output = Result<Vec<PlaylistSummary>>> + Send;

    fn playlist(&self, id: &str) -> impl Future<Output = Result<Playlist>> + Send;

    fn liked_tracks(&self) -> impl Future<Output = Result<Vec<Track>>> + Send;

    /// Best audio-only stream: Premium Opus, then Opus 251, then AAC 140.
    fn stream(&self, track_id: &str) -> impl Future<Output = Result<StreamSource>> + Send;

    /// Add a song to the user's liked songs.
    fn like(&self, track_id: &str) -> impl Future<Output = Result<()>> + Send;

    /// Remove a song from the user's liked songs.
    fn unlike(&self, track_id: &str) -> impl Future<Output = Result<()>> + Send;

    /// Append songs to a playlist the user owns. Returns the new entries'
    /// ids (`setVideoId`), one per track in order, `None` where YouTube did
    /// not report one.
    fn add_to_playlist(
        &self,
        playlist_id: &str,
        track_ids: &[TrackId],
    ) -> impl Future<Output = Result<Vec<Option<String>>>> + Send;

    /// Remove entries from a playlist. Each item is `(track_id, entry_id)`;
    /// the entry id says which copy to remove.
    fn remove_from_playlist(
        &self,
        playlist_id: &str,
        entries: &[(TrackId, String)],
    ) -> impl Future<Output = Result<()>> + Send;

    /// Radio (autoplay) tracks that follow `track_id`. The seed track itself
    /// is not included.
    fn radio(&self, track_id: &str) -> impl Future<Output = Result<Vec<Track>>> + Send;

    /// Log out: forget the cookies this backend holds.
    fn clear_cookies(&self) -> impl Future<Output = Result<()>> + Send;

    /// An album by its browse id, tracks in album order.
    fn album(&self, id: &str) -> impl Future<Output = Result<Album>> + Send;

    /// An artist by channel id: top songs, albums, singles.
    fn artist(&self, id: &str) -> impl Future<Output = Result<Artist>> + Send;

    /// The song's lyrics, timed when YouTube Music has timed ones.
    /// `Ok(None)` when the song has no lyrics.
    fn lyrics(&self, track_id: &str) -> impl Future<Output = Result<Option<Lyrics>>> + Send;
}
