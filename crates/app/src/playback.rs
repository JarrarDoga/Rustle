//! The audio output the backend drives, behind a small trait.
//!
//! [`Playback`] mirrors the public API of `player::Player`, which implements
//! it with one-line forwarding methods. Demo mode uses
//! [`SimulatedPlayback`], which plays silence on a clock and reports the end
//! of each track like the real player would.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use model::StreamSource;
use player::{PlaybackStatus, Player, PlayerEvent, PlayerState};

/// Every method returns at once; state comes back through `state` and `events`.
pub trait Playback: Send + 'static {
    /// Stops the current track and starts playing `source`.
    fn load(&self, source: StreamSource);
    fn play(&self);
    fn pause(&self);
    fn stop(&self);
    fn seek(&self, to: Duration);
    /// 0.0 to 1.0.
    fn set_volume(&self, volume: f32);
    fn state(&self) -> PlayerState;
    /// Single consumer. Call once.
    fn events(&self) -> Receiver<PlayerEvent>;
}

/// The real audio output.
#[allow(dead_code)] // used by the batch 2 wiring worker
pub type SystemPlayback = Player;

/// Opens the system audio output. The wiring worker calls this for non-demo
/// runs.
#[allow(dead_code)] // used by the batch 2 wiring worker
pub fn system_playback() -> Result<SystemPlayback, String> {
    Player::new().map_err(|e| e.to_string())
}

impl Playback for Player {
    fn load(&self, source: StreamSource) {
        Player::load(self, source);
    }

    fn play(&self) {
        Player::play(self);
    }

    fn pause(&self) {
        Player::pause(self);
    }

    fn stop(&self) {
        Player::stop(self);
    }

    fn seek(&self, to: Duration) {
        Player::seek(self, to);
    }

    fn set_volume(&self, volume: f32) {
        Player::set_volume(self, volume);
    }

    fn state(&self) -> PlayerState {
        Player::state(self)
    }

    fn events(&self) -> Receiver<PlayerEvent> {
        Player::events(self)
    }
}

/// Used when a stream reports no duration.
const FALLBACK_DURATION: Duration = Duration::from_secs(180);

/// The clock behind [`SimulatedPlayback`]. Takes `now` explicitly so tests
/// can run it without waiting.
#[derive(Debug, Clone)]
struct Clock {
    status: PlaybackStatus,
    track_id: Option<String>,
    duration: Duration,
    position: Duration,
    /// Set while playing: the instant `position` was taken.
    anchor: Option<Instant>,
    volume: f32,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            track_id: None,
            duration: Duration::ZERO,
            position: Duration::ZERO,
            anchor: None,
            volume: 1.0,
        }
    }
}

impl Clock {
    fn position_at(&self, now: Instant) -> Duration {
        let p = match self.anchor {
            Some(at) => self.position + now.saturating_duration_since(at),
            None => self.position,
        };
        p.min(self.duration)
    }

    fn load(&mut self, source: &StreamSource, now: Instant) {
        self.track_id = Some(source.track_id.clone());
        self.duration = source.duration.unwrap_or(FALLBACK_DURATION);
        self.position = Duration::ZERO;
        self.status = PlaybackStatus::Playing;
        self.anchor = Some(now);
    }

    fn play(&mut self, now: Instant) {
        if self.status == PlaybackStatus::Paused {
            self.status = PlaybackStatus::Playing;
            self.anchor = Some(now);
        }
    }

    fn pause(&mut self, now: Instant) {
        if self.status == PlaybackStatus::Playing {
            self.position = self.position_at(now);
            self.anchor = None;
            self.status = PlaybackStatus::Paused;
        }
    }

    fn stop(&mut self) {
        self.status = PlaybackStatus::Stopped;
        self.track_id = None;
        self.position = Duration::ZERO;
        self.anchor = None;
    }

    fn seek(&mut self, to: Duration, now: Instant) {
        if self.track_id.is_none() {
            return;
        }
        self.position = to.min(self.duration);
        if self.anchor.is_some() {
            self.anchor = Some(now);
        }
    }

    /// When the current track will end, if it is playing.
    fn end_at(&self) -> Option<Instant> {
        let at = self.anchor?;
        Some(at + self.duration.saturating_sub(self.position))
    }

    /// Ends the track if its time is up and returns the events to send.
    fn poll(&mut self, now: Instant) -> Vec<PlayerEvent> {
        if self.status != PlaybackStatus::Playing || self.position_at(now) < self.duration {
            return Vec::new();
        }
        let track_id = self.track_id.take().unwrap_or_default();
        self.stop();
        vec![
            PlayerEvent::TrackEnded { track_id },
            PlayerEvent::StatusChanged(PlaybackStatus::Stopped),
        ]
    }

    fn snapshot(&self, now: Instant) -> PlayerState {
        PlayerState {
            status: self.status,
            track_id: self.track_id.clone(),
            position: self.position_at(now),
            duration: self.track_id.as_ref().map(|_| self.duration),
            volume: self.volume,
        }
    }
}

struct Shared {
    clock: Mutex<(Clock, bool)>,
    wake: Condvar,
}

/// Plays nothing, but keeps time and reports track ends. One timer thread
/// that sleeps until the current track's end and is woken on every change.
pub struct SimulatedPlayback {
    shared: Arc<Shared>,
    events: Mutex<Option<Receiver<PlayerEvent>>>,
}

impl SimulatedPlayback {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            clock: Mutex::new((Clock::default(), false)),
            wake: Condvar::new(),
        });
        let (tx, rx) = mpsc::channel();
        let timer = Arc::clone(&shared);
        thread::Builder::new()
            .name("sim-playback".into())
            .spawn(move || run_timer(&timer, &tx))
            .expect("spawn playback timer");
        Self {
            shared,
            events: Mutex::new(Some(rx)),
        }
    }

    fn with(&self, f: impl FnOnce(&mut Clock, Instant)) {
        let mut guard = self.shared.clock.lock().expect("clock lock");
        f(&mut guard.0, Instant::now());
        self.shared.wake.notify_one();
    }
}

impl Default for SimulatedPlayback {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SimulatedPlayback {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.shared.clock.lock() {
            guard.1 = true;
        }
        self.shared.wake.notify_one();
    }
}

fn run_timer(shared: &Shared, tx: &Sender<PlayerEvent>) {
    let mut guard = shared.clock.lock().expect("clock lock");
    loop {
        if guard.1 {
            return;
        }
        let now = Instant::now();
        for event in guard.0.poll(now) {
            if tx.send(event).is_err() {
                return;
            }
        }
        guard = match guard.0.end_at() {
            Some(end) => {
                let wait = end.saturating_duration_since(now);
                shared.wake.wait_timeout(guard, wait).expect("clock lock").0
            }
            None => shared.wake.wait(guard).expect("clock lock"),
        };
    }
}

impl Playback for SimulatedPlayback {
    fn load(&self, source: StreamSource) {
        self.with(|c, now| c.load(&source, now));
    }

    fn play(&self) {
        self.with(|c, now| c.play(now));
    }

    fn pause(&self) {
        self.with(|c, now| c.pause(now));
    }

    fn stop(&self) {
        self.with(|c, _| c.stop());
    }

    fn seek(&self, to: Duration) {
        self.with(|c, now| c.seek(to, now));
    }

    fn set_volume(&self, volume: f32) {
        self.with(|c, _| c.volume = volume.clamp(0.0, 1.0));
    }

    fn state(&self) -> PlayerState {
        let guard = self.shared.clock.lock().expect("clock lock");
        guard.0.snapshot(Instant::now())
    }

    fn events(&self) -> Receiver<PlayerEvent> {
        self.events
            .lock()
            .expect("events lock")
            .take()
            .unwrap_or_else(|| mpsc::channel().1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::AudioCodec;

    fn source(id: &str, secs: f32) -> StreamSource {
        StreamSource {
            track_id: id.to_owned(),
            url: "demo://silence".to_owned(),
            codec: AudioCodec::Opus,
            bitrate_bps: 0,
            content_length: 0,
            duration: Some(Duration::from_secs_f32(secs)),
            loudness_db: None,
        }
    }

    #[test]
    fn clock_advances_pauses_and_ends() {
        let t0 = Instant::now();
        let s = |n: u64| t0 + Duration::from_secs(n);
        let mut c = Clock::default();
        c.load(&source("a", 10.0), t0);
        assert_eq!(c.position_at(s(4)), Duration::from_secs(4));
        c.pause(s(4));
        assert_eq!(c.position_at(s(100)), Duration::from_secs(4));
        assert!(c.poll(s(100)).is_empty());
        c.play(s(100));
        c.seek(Duration::from_secs(9), s(100));
        assert_eq!(c.end_at(), Some(s(101)));
        assert!(c.poll(s(100)).is_empty());
        let events = c.poll(s(101));
        assert!(matches!(
            &events[..],
            [PlayerEvent::TrackEnded { track_id }, PlayerEvent::StatusChanged(PlaybackStatus::Stopped)]
                if track_id == "a"
        ));
        assert_eq!(c.status, PlaybackStatus::Stopped);
    }

    #[test]
    fn timer_thread_reports_track_end() {
        let p = SimulatedPlayback::new();
        let events = p.events();
        p.load(source("short", 0.05));
        assert_eq!(p.state().status, PlaybackStatus::Playing);
        match events.recv_timeout(Duration::from_secs(5)) {
            Ok(PlayerEvent::TrackEnded { track_id }) => assert_eq!(track_id, "short"),
            other => panic!("expected TrackEnded, got {other:?}"),
        }
        assert_eq!(p.state().status, PlaybackStatus::Stopped);
    }

    /// Serves `data` on loopback, answering Range requests.
    fn serve(data: Vec<u8>) -> String {
        use std::io::{BufRead, BufReader, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/track", listener.local_addr().unwrap());
        let data = Arc::new(data);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let data = Arc::clone(&data);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut out = stream;
                    loop {
                        let mut range = (0, data.len() - 1);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                return;
                            }
                            let h = line.trim_end().to_ascii_lowercase();
                            if h.is_empty() {
                                break;
                            }
                            if let Some((a, b)) = h
                                .strip_prefix("range: bytes=")
                                .and_then(|v| v.split_once('-'))
                            {
                                let b = b.parse().unwrap_or(usize::MAX);
                                range = (a.parse().unwrap(), b.min(data.len() - 1));
                            }
                        }
                        let (a, b) = range;
                        let head = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{b}/{}\r\n\r\n",
                            b + 1 - a,
                            data.len()
                        );
                        if out.write_all(head.as_bytes()).is_err()
                            || out.write_all(&data[a..=b]).is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        url
    }

    fn status_within(events: &Receiver<PlayerEvent>, want: PlaybackStatus) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(PlayerEvent::StatusChanged(s)) if s == want => return,
                Ok(PlayerEvent::Error(e)) => panic!("player error: {e}"),
                Ok(_) => {}
                Err(e) => panic!("no {want:?} within 5 s: {e}"),
            }
        }
    }

    /// Drives the real player through the trait, silently, on a local file.
    #[test]
    fn system_playback_plays_through_the_trait() {
        let p: Box<dyn Playback> = match system_playback() {
            Ok(p) => Box::new(p),
            Err(e) => {
                eprintln!("skipped: no audio output: {e}");
                return;
            }
        };
        p.set_volume(0.0);
        let events = p.events();
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../player/tests/fixtures/sine3s.webm"
        );
        let data = std::fs::read(path).unwrap();
        let len = data.len() as u64;
        p.load(StreamSource {
            track_id: "local".to_owned(),
            url: serve(data),
            codec: AudioCodec::Opus,
            bitrate_bps: 0,
            content_length: len,
            duration: Some(Duration::from_secs(3)),
            loudness_db: None,
        });
        status_within(&events, PlaybackStatus::Playing);
        assert_eq!(p.state().track_id.as_deref(), Some("local"));
        assert_eq!(p.state().volume, 0.0);

        p.pause();
        status_within(&events, PlaybackStatus::Paused);
        p.seek(Duration::from_millis(2_600));
        thread::sleep(Duration::from_millis(100));
        let pos = p.state().position;
        assert_eq!(
            pos,
            Duration::from_millis(2_600),
            "paused seek moved to {pos:?}"
        );
        p.play();
        status_within(&events, PlaybackStatus::Playing);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left) {
                Ok(PlayerEvent::TrackEnded { track_id }) => {
                    assert_eq!(track_id, "local");
                    break;
                }
                Ok(PlayerEvent::Error(e)) => panic!("player error: {e}"),
                Ok(_) => {}
                Err(e) => panic!("track did not end: {e}"),
            }
        }
        assert_eq!(p.state().status, PlaybackStatus::Stopped);
    }

    #[test]
    fn events_has_a_single_consumer() {
        let p = SimulatedPlayback::new();
        let _first = p.events();
        let second = p.events();
        assert!(second.try_recv().is_err());
    }
}
