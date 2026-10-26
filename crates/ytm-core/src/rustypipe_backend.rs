use model::{Playlist, PlaylistSummary, SearchResults, StreamSource, Track};

use crate::{MusicBackend, Result};

/// The real backend, built on rustypipe + rustypipe-botguard.
pub struct RustyPipeBackend {
    _private: (),
}

impl RustyPipeBackend {
    pub fn new() -> Result<Self> {
        todo!("build a rustypipe::client::RustyPipe with storage under the config dir")
    }
}

impl MusicBackend for RustyPipeBackend {
    async fn set_cookies(&self, _cookies: &str) -> Result<()> {
        todo!()
    }

    async fn search(&self, _query: &str) -> Result<SearchResults> {
        todo!()
    }

    async fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        todo!()
    }

    async fn playlist(&self, _id: &str) -> Result<Playlist> {
        todo!()
    }

    async fn liked_tracks(&self) -> Result<Vec<Track>> {
        todo!()
    }

    async fn stream(&self, _track_id: &str) -> Result<StreamSource> {
        todo!()
    }
}
