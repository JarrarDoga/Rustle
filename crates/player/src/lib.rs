//! Audio playback: HTTP Range fetch, symphonia demux, Opus/AAC decode, cpal out.
//!
//! The player owns its own threads. Callers never block on audio work: every
//! method returns at once, and state comes back as snapshots and events.
//! `Player` is `Send`; the output stream stays on the engine thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait};

mod convert;
mod decode;
mod engine;
mod mp4;
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
}

// The app moves the player onto a tokio worker.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Player>();
};

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
        let engine_shared = shared.clone();
        // The engine, and with it the output stream, is built on its own
        // thread and never leaves it.
        std::thread::Builder::new()
            .name("rustle-player".into())
            .spawn(move || Engine::new(device, rx, events_tx, engine_shared).run())
            .map_err(|e| PlayerError::Device(format!("spawn player thread: {e}")))?;
        Ok(Player {
            tx,
            shared,
            events: Mutex::new(Some(events_rx)),
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
    /// Returns at once: the engine thread stops on its own, and a decoder
    /// thread stuck in a network read exits when the read returns.
    fn drop(&mut self) {
        self.send(Cmd::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    use model::AudioCodec;

    use crate::source::tests::{serve, serve_stalling};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    /// A silent player and its events, or `None` without an output device.
    fn player() -> Option<(Player, Receiver<PlayerEvent>)> {
        match Player::new() {
            Ok(p) => {
                p.set_volume(0.0);
                let events = p.events();
                Some((p, events))
            }
            Err(e) => {
                eprintln!("skipped: {e}");
                None
            }
        }
    }

    fn source(url: String, codec: AudioCodec, len: usize, secs: f64) -> StreamSource {
        StreamSource {
            track_id: "t".into(),
            url,
            codec,
            bitrate_bps: 0,
            content_length: len as u64,
            duration: Some(Duration::from_secs_f64(secs)),
            loudness_db: None,
        }
    }

    /// The next status change within `within`; panics on an error event.
    fn next_status(events: &Receiver<PlayerEvent>, within: Duration) -> PlaybackStatus {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(PlayerEvent::StatusChanged(s)) => return s,
                Ok(PlayerEvent::Error(e)) => panic!("player error: {e}"),
                Ok(PlayerEvent::TrackEnded { .. }) => {}
                Err(e) => panic!("no status change within {within:?}: {e}"),
            }
        }
    }

    fn wait_for_end(events: &Receiver<PlayerEvent>, within: Duration) {
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(PlayerEvent::TrackEnded { .. }) => return,
                Ok(PlayerEvent::Error(e)) => panic!("player error: {e}"),
                Ok(_) => {}
                Err(e) => panic!("track did not end within {within:?}: {e}"),
            }
        }
    }

    #[test]
    fn plays_seeks_and_ends() {
        let Some((p, events)) = player() else { return };
        for (name, codec) in [
            ("sine3s.webm", AudioCodec::Opus),
            ("sine3s.m4a", AudioCodec::Aac),
        ] {
            let data = fixture(name);
            let (url, _) = serve(data.clone());
            p.load(source(url, codec, data.len(), 3.0));
            let secs = Duration::from_secs(5);
            assert_eq!(next_status(&events, secs), PlaybackStatus::Buffering);
            assert_eq!(next_status(&events, secs), PlaybackStatus::Playing);
            p.seek(Duration::from_millis(2_500));
            assert_eq!(next_status(&events, secs), PlaybackStatus::Buffering);
            assert_eq!(next_status(&events, secs), PlaybackStatus::Playing);
            let pos = p.state().position.as_secs_f64();
            assert!((2.5..2.7).contains(&pos), "{name}: at {pos} s after seek");
            wait_for_end(&events, secs);
            assert_eq!(p.state().status, PlaybackStatus::Stopped);
        }
    }

    #[test]
    fn reports_buffering_while_the_fetch_stalls() {
        let Some((p, events)) = player() else { return };
        // Cues before the clusters, as in YouTube's WebM, so opening the
        // stream reads only the head of the file.
        let data = fixture("sine3s-cues-front.webm");
        // About 1.5 s of audio arrives, then nothing until 2.5 s in.
        let stall = Some((data.len() / 2, Duration::from_millis(2_500)));
        let (url, _) = serve_stalling(data.clone(), stall);
        p.load(source(url, AudioCodec::Opus, data.len(), 3.0));
        let secs = Duration::from_secs(5);
        assert_eq!(next_status(&events, secs), PlaybackStatus::Buffering);
        assert_eq!(next_status(&events, secs), PlaybackStatus::Playing);

        assert_eq!(next_status(&events, secs), PlaybackStatus::Buffering);
        let held = p.state();
        assert_eq!(held.status, PlaybackStatus::Buffering);
        let at = held.position.as_secs_f64();
        assert!((0.8..1.6).contains(&at), "stalled at {at} s");
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            p.state().position,
            held.position,
            "position moved while stalled"
        );

        assert_eq!(next_status(&events, secs), PlaybackStatus::Playing);
        std::thread::sleep(Duration::from_millis(300));
        assert!(p.state().position > held.position);
        wait_for_end(&events, secs);
    }

    #[test]
    fn drop_returns_at_once_while_a_load_is_stalled() {
        let Some((p, events)) = player() else { return };
        // Accepts connections and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().collect();
            drop(held);
        });
        p.load(source(url.clone(), AudioCodec::Opus, 0, 3.0));
        let quick = Duration::from_millis(500);
        assert_eq!(next_status(&events, quick), PlaybackStatus::Buffering);
        // Commands still go through while the fetch hangs.
        std::thread::sleep(Duration::from_millis(100));
        p.stop();
        assert_eq!(next_status(&events, quick), PlaybackStatus::Stopped);
        p.load(source(url, AudioCodec::Opus, 0, 3.0));
        assert_eq!(next_status(&events, quick), PlaybackStatus::Buffering);

        let t0 = Instant::now();
        drop(p);
        assert!(
            t0.elapsed() < Duration::from_millis(50),
            "drop took {:?}",
            t0.elapsed()
        );
        // The engine thread exits soon after, hanging up the event channel.
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("engine thread still running"),
            }
        }
    }
}
