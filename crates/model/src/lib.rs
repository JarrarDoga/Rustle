//! Shared domain types. Every other crate speaks these; none of them leak
//! rustypipe, SQLite, or audio-library types across a crate boundary.
//!
//! Frozen after the scaffold commit: add fields only through the integrator.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// YouTube video id of a track (11 chars, e.g. `dQw4w9WgXcQ`).
pub type TrackId = String;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArtistRef {
    /// Channel id; `None` for unlinked credits.
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AlbumRef {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Option<Duration>,
    /// Smallest-first; pick the first one at least as large as the slot.
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumSummary {
    pub id: String,
    pub name: String,
    pub artists: Vec<ArtistRef>,
    pub year: Option<u16>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtistSummary {
    pub id: String,
    pub name: String,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistSummary {
    pub id: String,
    pub name: String,
    pub owner: Option<String>,
    pub track_count: Option<u64>,
    pub thumbnails: Vec<Thumbnail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Playlist {
    pub summary: PlaylistSummary,
    pub tracks: Vec<Track>,
    /// Each entry's YouTube Music `setVideoId`, parallel to `tracks`.
    /// Removing a song from a playlist needs it. Empty when the source did
    /// not provide entry ids; otherwise the same length as `tracks`.
    #[serde(default)]
    pub entry_ids: Vec<Option<String>>,
}

/// An album page: its summary and tracks in album order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub summary: AlbumSummary,
    pub tracks: Vec<Track>,
}

/// An artist page (D-Artist): top songs, then albums, then singles and EPs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artist {
    pub summary: ArtistSummary,
    pub top_songs: Vec<Track>,
    pub albums: Vec<AlbumSummary>,
    pub singles: Vec<AlbumSummary>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<AlbumSummary>,
    pub artists: Vec<ArtistSummary>,
    pub playlists: Vec<PlaylistSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioCodec {
    /// Opus in a WebM container (itag 251, Premium itag 774).
    Opus,
    /// AAC-LC in an MP4/M4A container (itag 140).
    Aac,
}

/// A resolved, directly fetchable audio stream. URLs expire after a few hours.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamSource {
    pub track_id: TrackId,
    pub url: String,
    pub codec: AudioCodec,
    pub bitrate_bps: u32,
    /// Total size in bytes. YouTube throttles single huge GETs, so fetch in
    /// HTTP Range chunks (about 1 MiB each) up to this length.
    pub content_length: u64,
    pub duration: Option<Duration>,
    /// Loudness in dB for normalization: gain = 10^(-loudness_db / 20).
    pub loudness_db: Option<f32>,
}
