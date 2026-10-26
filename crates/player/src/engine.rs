//! The engine thread owns the output stream and the ring's producer side, and
//! takes commands from `Player` over a channel. Each track's fetch and decode
//! run on a decoder thread of their own, so a stalled network read never
//! holds up a command, and the engine sees the ring run dry while it waits.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cpal::Device;
use model::StreamSource;

use crate::convert::{Resampler, map_channels};
use crate::decode::{Decode, Demuxed, Silence};
use crate::output::{Ctl, Gain, Output, pick_config};
use crate::source::{HttpSource, agent};
use crate::{PlaybackStatus, PlayerError, PlayerEvent, PlayerState};

/// Audio queued before playback starts after a load or a seek.
const PREBUFFER: Duration = Duration::from_millis(250);
/// A playing track goes to `Buffering` when the ring holds less than this
/// before the end of the stream. More than `POLL` plus one audio callback,
/// so the engine sees it before the ring runs dry.
const LOW_WATER: Duration = Duration::from_millis(200);
/// After a stall, audio queued before playback resumes. Well above
/// `LOW_WATER`, so a slow link does not flap between states.
const REBUFFER: Duration = Duration::from_secs(1);
/// Sleep between checks while playing.
const POLL: Duration = Duration::from_millis(100);
/// Sleep between checks while waiting for the decoder to fill the ring.
const FAST_POLL: Duration = Duration::from_millis(20);
/// Decoded blocks the decoder may hold ready for the engine.
const BLOCKS_AHEAD: usize = 8;

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
    /// The current track's output; `None` until its stream is open.
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

/// From a decoder thread to the engine.
enum Msg {
    Opened {
        rate: u32,
        channels: usize,
    },
    /// Interleaved samples at the source rate, decoded after seek `epoch`.
    Block {
        epoch: u64,
        samples: Vec<f32>,
    },
    End {
        epoch: u64,
    },
    Failed(PlayerError),
}

struct Track {
    source: StreamSource,
    /// The latest seek; blocks decoded before it are dropped.
    epoch: u64,
    seeks: Sender<(u64, Duration)>,
    data: Receiver<Msg>,
    /// Set once the stream is open and the output matches it.
    conv: Option<Conv>,
    /// Converted samples waiting for ring space.
    pending: Vec<f32>,
    pending_at: usize,
    eof: bool,
    /// Holding playback until the ring has this much audio queued.
    waiting: Option<Duration>,
}

struct Conv {
    resampler: Resampler,
    channels: usize,
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
    mapped: Vec<f32>,
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
            mapped: Vec::new(),
        }
    }

    pub(crate) fn run(mut self) {
        loop {
            let cmd = match self.wait() {
                Some(d) => match self.rx.recv_timeout(d) {
                    Ok(c) => Some(c),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match self.rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
            };
            match cmd {
                Some(Cmd::Shutdown) => return,
                Some(cmd) => self.handle(cmd),
                None => {}
            }
            self.work();
        }
    }

    /// How long to sleep before the next `work`; `None` to wait for a command.
    fn wait(&self) -> Option<Duration> {
        if self.out.as_ref().is_some_and(Output::settling) {
            return Some(POLL);
        }
        let t = self.cur.as_ref()?;
        if t.conv.is_none() || t.waiting.is_some() {
            return Some(if self.paused { POLL } else { FAST_POLL });
        }
        if self.out.as_ref().is_some_and(Output::is_playing) {
            return Some(POLL);
        }
        // Paused: once the ring is full or the stream is over, nothing
        // changes until a command.
        if t.pending_at < t.pending.len() || t.eof {
            None
        } else {
            Some(POLL)
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Load(src) => self.load(src),
            Cmd::Play => {
                if let Some(t) = &self.cur {
                    self.paused = false;
                    if t.waiting.is_none()
                        && let Some(out) = &mut self.out
                    {
                        out.play();
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
        self.shared.gain.set_loudness(src.loudness_db);
        self.shared.with(|s| {
            s.track_id = Some(src.track_id.clone());
            s.duration = src.duration;
            s.base = Duration::ZERO;
            s.ctl = None;
        });
        self.set_status(PlaybackStatus::Buffering);

        let (seeks, seek_rx) = mpsc::channel();
        let (data_tx, data) = mpsc::sync_channel(BLOCKS_AHEAD);
        let (agent, thread_src) = (self.agent.clone(), src.clone());
        let spawned = std::thread::Builder::new()
            .name("rustle-decode".into())
            .spawn(move || decode(&agent, &thread_src, &seek_rx, &data_tx));
        if let Err(e) = spawned {
            self.fail(PlayerError::Decode(format!("spawn decoder thread: {e}")));
            return;
        }
        self.cur = Some(Track {
            source: src,
            epoch: 0,
            seeks,
            data,
            conv: None,
            pending: Vec::new(),
            pending_at: 0,
            eof: false,
            waiting: Some(PREBUFFER),
        });
    }

    /// The decoder opened the stream: match the output to it.
    fn opened(&mut self, rate: u32, channels: usize) -> Result<(), PlayerError> {
        self.ensure_output(rate)?;
        let (Some(t), Some(out)) = (&mut self.cur, &mut self.out) else {
            return Ok(());
        };
        let base_index = out.flush();
        t.conv = Some(Conv {
            resampler: Resampler::new(rate, out.rate, out.channels),
            channels,
        });
        let samples_per_sec = f64::from(out.rate) * out.channels as f64;
        let ctl = out.ctl.clone();
        self.shared.with(|s| {
            s.base_index = base_index;
            s.samples_per_sec = samples_per_sec;
            s.ctl = Some(ctl);
        });
        Ok(())
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
            out.pause();
            out.flush();
        }
        // Dropping the track hangs up on its decoder thread, which exits
        // when its current read returns.
        self.cur = None;
        self.shared.with(|s| s.base = Duration::ZERO);
        self.set_status(PlaybackStatus::Stopped);
    }

    fn seek(&mut self, to: Duration) {
        let Some(t) = &mut self.cur else {
            return;
        };
        let to = t.source.duration.map_or(to, |d| to.min(d));
        t.epoch += 1;
        let _ = t.seeks.send((t.epoch, to));
        t.pending.clear();
        t.pending_at = 0;
        t.eof = false;
        t.waiting = Some(PREBUFFER);
        let mut base_index = None;
        if let Some(conv) = &mut t.conv {
            conv.resampler.reset();
            if let Some(out) = &mut self.out {
                out.pause();
                base_index = Some(out.flush());
            }
        }
        self.shared.with(|s| {
            s.base = to;
            if let Some(i) = base_index {
                s.base_index = i;
            }
        });
        if !self.paused {
            self.set_status(PlaybackStatus::Buffering);
        }
    }

    /// Moves decoded audio into the ring; starts, holds, and resumes
    /// playback by how much is queued; reports the end once the ring drains.
    fn work(&mut self) {
        if let Some(out) = &mut self.out {
            out.sync();
        }
        if let Err(e) = self.pump() {
            self.fail(e);
            return;
        }
        let (Some(t), Some(out)) = (&mut self.cur, &mut self.out) else {
            return;
        };
        if t.conv.is_none() {
            return;
        }
        let (rate, channels) = (f64::from(out.rate), out.channels as u64);
        let samples = |d: Duration| (d.as_secs_f64() * rate) as u64 * channels;
        let ended = t.eof && t.pending_at >= t.pending.len();
        let mut status = None;
        if let Some(need) = t.waiting {
            if ended || out.queued() >= samples(need) || out.full() {
                t.waiting = None;
                if !self.paused {
                    out.play();
                    status = Some(PlaybackStatus::Playing);
                }
            }
        } else if !self.paused && !ended && out.queued() < samples(LOW_WATER) {
            // The fetch fell behind: hold the rest until the ring refills.
            t.waiting = Some(REBUFFER);
            out.pause();
            status = Some(PlaybackStatus::Buffering);
        } else if ended && out.drained() {
            let track_id = t.source.track_id.clone();
            out.pause();
            self.cur = None;
            self.shared.with(|s| s.base = Duration::ZERO);
            self.set_status(PlaybackStatus::Stopped);
            let _ = self.events.send(PlayerEvent::TrackEnded { track_id });
        }
        if let Some(s) = status {
            self.set_status(s);
        }
    }

    /// Takes decoder messages until the ring is full or none are ready.
    fn pump(&mut self) -> Result<(), PlayerError> {
        loop {
            let Some(t) = &mut self.cur else {
                return Ok(());
            };
            if t.pending_at < t.pending.len() {
                let Some(out) = &mut self.out else {
                    return Ok(());
                };
                t.pending_at += out.push(&t.pending[t.pending_at..]);
                if t.pending_at < t.pending.len() {
                    return Ok(());
                }
            }
            let msg = match t.data.try_recv() {
                Ok(m) => m,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) if t.eof => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    return Err(PlayerError::Decode("decoder thread stopped".into()));
                }
            };
            match msg {
                Msg::Opened { rate, channels } => self.opened(rate, channels)?,
                Msg::Block { epoch, samples } if epoch == t.epoch => {
                    let (Some(conv), Some(out)) = (&mut t.conv, &self.out) else {
                        continue;
                    };
                    let block = if conv.channels == out.channels {
                        &samples
                    } else {
                        self.mapped.clear();
                        map_channels(&samples, conv.channels, out.channels, &mut self.mapped);
                        &self.mapped
                    };
                    t.pending.clear();
                    t.pending_at = 0;
                    conv.resampler.process(block, &mut t.pending);
                }
                Msg::End { epoch } if epoch == t.epoch => t.eof = true,
                Msg::Failed(e) => return Err(e),
                // Decoded before the latest seek.
                Msg::Block { .. } | Msg::End { .. } => {}
            }
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

/// The decoder thread: opens `src`, then decodes ahead of the engine, at
/// most `BLOCKS_AHEAD` blocks, until the engine hangs up.
fn decode(
    agent: &ureq::Agent,
    src: &StreamSource,
    seeks: &Receiver<(u64, Duration)>,
    out: &SyncSender<Msg>,
) {
    let mut dec = match open_decoder(agent, src) {
        Ok(d) => d,
        Err(e) => {
            let _ = out.send(Msg::Failed(e));
            return;
        }
    };
    let opened = Msg::Opened {
        rate: dec.rate(),
        channels: dec.channels(),
    };
    if out.send(opened).is_err() {
        return;
    }
    let mut epoch = 0;
    let mut ended = false;
    let mut capacity = 0;
    loop {
        let seek = if ended {
            match seeks.recv() {
                Ok(s) => Some(s),
                Err(_) => return,
            }
        } else {
            match seeks.try_recv() {
                Ok(s) => Some(s),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return,
            }
        };
        if let Some(mut s) = seek {
            // Only the latest seek matters.
            while let Ok(later) = seeks.try_recv() {
                s = later;
            }
            epoch = s.0;
            ended = false;
            if let Err(e) = dec.seek(s.1) {
                let _ = out.send(Msg::Failed(e));
                return;
            }
        }
        let mut samples = Vec::with_capacity(capacity);
        let msg = match dec.next(&mut samples) {
            Ok(true) => {
                capacity = samples.len();
                Msg::Block { epoch, samples }
            }
            Ok(false) => {
                ended = true;
                Msg::End { epoch }
            }
            Err(e) => {
                let _ = out.send(Msg::Failed(e));
                return;
            }
        };
        if out.send(msg).is_err() {
            return;
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
