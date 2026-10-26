//! Local SQLite cache: library, metadata, queue. The UI renders from here so
//! the library opens instantly, even offline.

use std::path::Path;

use model::{Playlist, PlaylistSummary, Track, TrackId};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    _private: (),
}

impl Store {
    /// Opens or creates the database and runs migrations.
    pub fn open(_path: &Path) -> Result<Self> {
        todo!()
    }

    pub fn open_in_memory() -> Result<Self> {
        todo!()
    }

    /// Insert or update track metadata.
    pub fn upsert_tracks(&self, _tracks: &[Track]) -> Result<()> {
        todo!()
    }

    pub fn track(&self, _id: &str) -> Result<Option<Track>> {
        todo!()
    }

    /// Replace the cached list of the user's saved playlists.
    pub fn set_library_playlists(&self, _playlists: &[PlaylistSummary]) -> Result<()> {
        todo!()
    }

    pub fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        todo!()
    }

    /// Cache one playlist with its full track order.
    pub fn save_playlist(&self, _playlist: &Playlist) -> Result<()> {
        todo!()
    }

    pub fn playlist(&self, _id: &str) -> Result<Option<Playlist>> {
        todo!()
    }

    pub fn set_liked(&self, _tracks: &[Track]) -> Result<()> {
        todo!()
    }

    pub fn liked(&self) -> Result<Vec<Track>> {
        todo!()
    }

    /// Persist the play queue and the index of the current track.
    pub fn save_queue(&self, _queue: &[TrackId], _current: Option<usize>) -> Result<()> {
        todo!()
    }

    pub fn load_queue(&self) -> Result<(Vec<Track>, Option<usize>)> {
        todo!()
    }
}
