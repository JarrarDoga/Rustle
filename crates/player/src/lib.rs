//! Audio playback: HTTP Range fetch, symphonia demux, Opus/AAC decode, cpal out.
//!
//! The player owns its own threads. Callers never block on audio work: every
//! method returns at once, and state comes back as snapshots and events.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};

mod convert;
mod decode;
mod engine;
mod output;
mod source;

use engine::{Cmd, Engine, Shared};

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
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    events: Mutex<Option<Receiver<PlayerEvent>>>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    /// Opens the default output device. Fails only if there is none.
    pub fn new() -> Result<Self, PlayerError> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| PlayerError::Device("no default output device".into()))?;
        device
            .default_output_config()
            .map_err(|e| PlayerError::Device(e.to_string()))?;
        let (tx, rx) = mpsc::channel();
        let (events_tx, events_rx) = mpsc::channel();
        let shared = Arc::new(Shared::new());
        let engine = Engine::new(device, rx, events_tx, shared.clone());
        let thread = std::thread::Builder::new()
            .name("rustle-player".into())
            .spawn(move || engine.run())
            .map_err(|e| PlayerError::Device(format!("spawn player thread: {e}")))?;
        Ok(Player {
            tx,
            shared,
            events: Mutex::new(Some(events_rx)),
            thread: Some(thread),
        })
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Stops the current track and starts fetching and playing `source`.
    /// A `demo://` URL plays silence for `source.duration`.
    pub fn load(&self, source: StreamSource) {
        self.send(Cmd::Load(source));
    }

    pub fn play(&self) {
        self.send(Cmd::Play);
    }

    pub fn pause(&self) {
        self.send(Cmd::Pause);
    }

    pub fn stop(&self) {
        self.send(Cmd::Stop);
    }

    pub fn seek(&self, to: Duration) {
        self.send(Cmd::Seek(to));
    }

    /// 0.0 to 1.0.
    pub fn set_volume(&self, volume: f32) {
        self.shared.gain.set_volume(volume);
    }

    pub fn state(&self) -> PlayerState {
        self.shared.state()
    }

    /// Single consumer. Call once; later calls may return a dead receiver.
    pub fn events(&self) -> Receiver<PlayerEvent> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .unwrap_or_else(|| mpsc::channel().1)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
