//! YouTube Music access. The only crate that knows rustypipe exists.
//!
//! The app talks to [`MusicBackend`] and nothing else, so when YouTube changes
//! something, only this crate gets patched.

use std::future::Future;

pub use model;
use model::{Playlist, PlaylistSummary, SearchResults, StreamSource, Track};

mod fixture;
mod rustypipe_backend;
mod stream_cache;
mod ytdlp;

pub use fixture::FixtureBackend;
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
}
