//! The fetch/decode thread. It owns the output stream, the decoder, and the
//! ring's producer side, and takes commands from `Player` over a channel.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::Device;
use model::StreamSource;

use crate::convert::{Resampler, map_channels};
use crate::decode::{Decode, Demuxed, Silence};
use crate::output::{Ctl, Gain, Output, pick_config};
use crate::source::{HttpSource, agent};
use crate::{PlaybackStatus, PlayerError, PlayerEvent, PlayerState};

/// Audio queued before playback starts after a load.
const PREBUFFER: Duration = Duration::from_millis(250);
/// Sleep between checks while the ring is full or draining.
const POLL: Duration = Duration::from_millis(100);

pub(crate) enum Cmd {
    Load(StreamSource),
    Play,
    Pause,
    Stop,
    Seek(Duration),
    Shutdown,
}

/// State readable from any thread.
pub(crate) struct Shared {
    pub(crate) gain: Arc<Gain>,
    snap: Mutex<Snap>,
}

#[derive(Default)]
struct Snap {
    status: PlaybackStatus,
    track_id: Option<String>,
    duration: Option<Duration>,
    /// Media time of the sample with index `base_index`.
    base: Duration,
    base_index: u64,
    /// Output samples per second (rate × channels).
    samples_per_sec: f64,
    ctl: Option<Arc<Ctl>>,
}

impl Shared {
    pub(crate) fn new() -> Self {
        Shared {
            gain: Arc::new(Gain::new()),
            snap: Mutex::new(Snap::default()),
        }
    }

    pub(crate) fn state(&self) -> PlayerState {
        let snap = self.snap.lock().unwrap_or_else(|e| e.into_inner());
        let position = match (&snap.ctl, snap.status) {
            (_, PlaybackStatus::Stopped) => Duration::ZERO,
            (Some(ctl), _) if snap.samples_per_sec > 0.0 => {
                let played = ctl.popped().saturating_sub(snap.base_index);
                snap.base + Duration::from_secs_f64(played as f64 / snap.samples_per_sec)
            }
            _ => snap.base,
        };
        let position = snap.duration.map_or(position, |d| position.min(d));
        PlayerState {
            status: snap.status,
            track_id: snap.track_id.clone(),
            position,
            duration: snap.duration,
            volume: self.gain.volume(),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Snap) -> R) -> R {
        f(&mut self.snap.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

struct Track {
    source: StreamSource,
    dec: Box<dyn Decode>,
    resampler: Resampler,
    /// Converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_at: usize,
    eof: bool,
    started: bool,
}

pub(crate) struct Engine {
    device: Device,
    rx: Receiver<Cmd>,
    events: Sender<PlayerEvent>,
    shared: Arc<Shared>,
    agent: ureq::Agent,
    out: Option<Output>,
    cur: Option<Track>,
    last: Option<StreamSource>,
    paused: bool,
    block: Vec<f32>,
    mapped: Vec<f32>,
}

enum Wait {
    Poll,
    For(Duration),
    Block,
}

impl Engine {
    pub(crate) fn new(
        device: Device,
        rx: Receiver<Cmd>,
        events: Sender<PlayerEvent>,
        shared: Arc<Shared>,
    ) -> Self {
        Engine {
            device,
            rx,
            events,
            shared,
            agent: agent(),
            out: None,
            cur: None,
            last: None,
            paused: false,
            block: Vec::new(),
            mapped: Vec::new(),
        }
    }

    pub(crate) fn run(mut self) {
        loop {
            let cmd = match self.wait() {
                Wait::Poll => match self.rx.try_recv() {
                    Ok(c) => Some(c),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                },
                Wait::For(d) => match self.rx.recv_timeout(d) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                Wait::Block => match self.rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
            };
            match cmd {
                Some(Cmd::Shutdown) => return,
                Some(cmd) => self.handle(cmd),
                None => self.work(),
            }
        }
    }

    fn wait(&self) -> Wait {
        let (Some(t), Some(out)) = (&self.cur, &self.out) else {
            return Wait::Block;
        };
        let has_data = t.pending_at < t.pending.len() || !t.eof;
        if has_data && out.free() > 0 {
            return Wait::Poll;
        }
        if !t.started {
            // Prebuffer full (or track over) but not started: work() starts it.
            return Wait::Poll;
        }
        if out.is_playing() {
            Wait::For(POLL)
        } else {
            // Paused with a full ring: nothing to do until a command.
            Wait::Block
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Load(src) => self.load(src),
            Cmd::Play => {
                if self.cur.is_some() {
                    self.paused = false;
                    if self.cur.as_ref().is_some_and(|t| t.started) {
                        if let Some(out) = &mut self.out {
                            out.play();
                        }
                        self.set_status(PlaybackStatus::Playing);
                    } else {
                        self.set_status(PlaybackStatus::Buffering);
                    }
                } else if let Some(src) = self.last.clone() {
                    self.load(src);
                }
            }
            Cmd::Pause => {
                if self.cur.is_some() {
                    self.paused = true;
                    if let Some(out) = &mut self.out {
                        out.pause();
                    }
                    self.set_status(PlaybackStatus::Paused);
                }
            }
            Cmd::Stop => self.stop(),
            Cmd::Seek(to) => self.seek(to),
            Cmd::Shutdown => {}
        }
    }

    fn load(&mut self, src: StreamSource) {
        self.stop();
        self.last = Some(src.clone());
        self.paused = false;
        self.shared.with(|s| {
            s.track_id = Some(src.track_id.clone());
            s.duration = src.duration;
            s.base = Duration::ZERO;
        });
        self.set_status(PlaybackStatus::Buffering);

        let opened = open_decoder(&self.agent, &src).and_then(|dec| {
            self.ensure_output(dec.rate())?;
            Ok(dec)
        });
        let dec = match opened {
            Ok(d) => d,
            Err(e) => {
                self.fail(e);
                return;
            }
        };
        let out = self.out.as_mut().expect("ensure_output succeeded");
        self.shared.gain.set_loudness(src.loudness_db);
        let base_index = out.flush();
        let (rate, channels, ctl) = (out.rate, out.channels, out.ctl.clone());
        self.shared.with(|s| {
            s.base = Duration::ZERO;
            s.base_index = base_index;
            s.samples_per_sec = f64::from(rate) * channels as f64;
            s.ctl = Some(ctl);
        });
        self.cur = Some(Track {
            resampler: Resampler::new(dec.rate(), rate, channels),
            source: src,
            dec,
            pending: Vec::new(),
            pending_at: 0,
            eof: false,
            started: false,
        });
    }

    /// Makes sure an output stream exists, at `rate` when the device allows.
    fn ensure_output(&mut self, rate: u32) -> Result<(), PlayerError> {
        let config = pick_config(&self.device, rate)?;
        if let Some(out) = &self.out
            && out.rate == config.sample_rate()
            && out.channels == usize::from(config.channels())
        {
            return Ok(());
        }
        self.out = None;
        self.out = Some(Output::open(
            &self.device,
            config,
            self.shared.gain.clone(),
            self.events.clone(),
        )?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(out) = &mut self.out {
            out.flush();
            out.pause();
        }
        self.cur = None;
        self.shared.with(|s| s.base = Duration::ZERO);
        self.set_status(PlaybackStatus::Stopped);
    }

    fn seek(&mut self, to: Duration) {
        let (Some(t), Some(out)) = (&mut self.cur, &mut self.out) else {
            return;
        };
        let to = t.source.duration.map_or(to, |d| to.min(d));
        if let Err(e) = t.dec.seek(to) {
            self.fail(e);
            return;
        }
        t.pending.clear();
        t.pending_at = 0;
        t.resampler.reset();
        t.eof = false;
        let base_index = out.flush();
        self.shared.with(|s| {
            s.base = to;
            s.base_index = base_index;
        });
    }

    /// Decodes one block and queues what fits; starts playback once enough
    /// is queued; reports the end once the ring drains.
    fn work(&mut self) {
        let (Some(t), Some(out)) = (&mut self.cur, &mut self.out) else {
            return;
        };
        if t.pending_at >= t.pending.len() && !t.eof {
            t.pending.clear();
            t.pending_at = 0;
            match t.dec.next(&mut self.block) {
                Ok(true) => {
                    let block = if t.dec.channels() == out.channels {
                        &self.block
                    } else {
                        self.mapped.clear();
                        map_channels(
                            &self.block,
                            t.dec.channels(),
                            out.channels,
                            &mut self.mapped,
                        );
                        &self.mapped
                    };
                    t.resampler.process(block, &mut t.pending);
                }
                Ok(false) => t.eof = true,
                Err(e) => {
                    self.fail(e);
                    return;
                }
            }
        }
        if t.pending_at < t.pending.len() {
            t.pending_at += out.push(&t.pending[t.pending_at..]);
        }

        let ended = t.eof && t.pending_at >= t.pending.len();
        if !t.started {
            let prebuffer =
                (PREBUFFER.as_secs_f64() * f64::from(out.rate)) as u64 * out.channels as u64;
            if ended || out.queued() >= prebuffer || out.free() == 0 {
                t.started = true;
                if !self.paused {
                    out.play();
                    self.set_status(PlaybackStatus::Playing);
                }
            }
            return;
        }
        if ended && out.drained() {
            let track_id = t.source.track_id.clone();
            out.pause();
            self.cur = None;
            self.shared.with(|s| s.base = Duration::ZERO);
            self.set_status(PlaybackStatus::Stopped);
            let _ = self.events.send(PlayerEvent::TrackEnded { track_id });
        }
    }

    fn fail(&mut self, e: PlayerError) {
        let _ = self.events.send(PlayerEvent::Error(e.to_string()));
        self.stop();
    }

    fn set_status(&self, status: PlaybackStatus) {
        let changed = self
            .shared
            .with(|s| std::mem::replace(&mut s.status, status) != status);
        if changed {
            let _ = self.events.send(PlayerEvent::StatusChanged(status));
        }
    }
}

fn open_decoder(agent: &ureq::Agent, src: &StreamSource) -> Result<Box<dyn Decode>, PlayerError> {
    if src.url.starts_with("demo://") {
        return Ok(Box::new(Silence::new(src.duration.unwrap_or_default())));
    }
    let http = HttpSource::open(agent.clone(), src.url.clone(), src.content_length)
        .map_err(|e| PlayerError::Fetch(e.to_string()))?;
    Ok(Box::new(Demuxed::open(Box::new(http), src.codec)?))
}
