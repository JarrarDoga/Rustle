//! Audio playback: HTTP Range fetch, symphonia demux, Opus/AAC decode, cpal out.
//!
//! The player owns its own threads. Callers never block on audio work: every
//! method returns at once, and state comes back as snapshots and events.

use std::sync::mpsc::Receiver;
use std::time::Duration;

pub use model::StreamSource;

#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("no audio output device: {0}")]
    Device(String),
    #[error("stream fetch failed: {0}")]
    Fetch(String),
    #[error("decode failed: {0}")]
    Decode(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackStatus {
    #[default]
    Stopped,
    Buffering,
    Playing,
    Paused,
}

/// A cheap copy of the player's state, for the UI to read each frame.
#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    pub status: PlaybackStatus,
    pub track_id: Option<String>,
    pub position: Duration,
    pub duration: Option<Duration>,
    /// 0.0 to 1.0.
    pub volume: f32,
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    /// The current track played to its end. The app loads the next one.
    TrackEnded {
        track_id: String,
    },
    StatusChanged(PlaybackStatus),
    Error(String),
}

pub struct Player {
    _private: (),
}

impl Player {
    /// Opens the default output device. Fails only if there is none.
    pub fn new() -> Result<Self, PlayerError> {
        todo!()
    }

    /// Stops the current track and starts fetching and playing `source`.
    /// A `demo://` URL plays silence for `source.duration`.
    pub fn load(&self, _source: StreamSource) {
        todo!()
    }

    pub fn play(&self) {
        todo!()
    }

    pub fn pause(&self) {
        todo!()
    }

    pub fn stop(&self) {
        todo!()
    }

    pub fn seek(&self, _to: Duration) {
        todo!()
    }

    /// 0.0 to 1.0.
    pub fn set_volume(&self, _volume: f32) {
        todo!()
    }

    pub fn state(&self) -> PlayerState {
        todo!()
    }

    /// Single consumer. Call once; later calls may return a dead receiver.
    pub fn events(&self) -> Receiver<PlayerEvent> {
        todo!()
    }
}
