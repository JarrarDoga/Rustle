//! cpal output fed through a lock-free SPSC ring buffer.
//!
//! The audio callback only does atomic loads and stores, ring reads, and
//! arithmetic: it never allocates, locks, or blocks.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::Sender;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{
    Device, FromSample, OutputCallbackInfo, Sample, SampleFormat, SizedSample, StreamConfig,
    SupportedStreamConfig,
};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::{PlayerError, PlayerEvent};

/// Seconds of decoded audio the ring holds. Covers the request latency at
/// each 1 MiB chunk boundary.
const RING_SECS: usize = 2;

/// Volume and loudness gain, shared by the `Player`, engine, and callback.
pub(crate) struct Gain {
    volume: AtomicU32,
    norm: AtomicU32,
}

impl Gain {
    pub(crate) fn new() -> Self {
        Gain {
            volume: AtomicU32::new(1.0f32.to_bits()),
            norm: AtomicU32::new(1.0f32.to_bits()),
        }
    }

    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub(crate) fn set_volume(&self, v: f32) {
        let v = if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        self.volume.store(v.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn set_loudness(&self, loudness_db: Option<f32>) {
        self.norm
            .store(norm_gain(loudness_db).to_bits(), Ordering::Relaxed);
    }

    fn total(&self) -> f32 {
        self.volume() * f32::from_bits(self.norm.load(Ordering::Relaxed))
    }
}

/// Loudness normalization: 10^(-db/20), never above 1.0.
pub(crate) fn norm_gain(loudness_db: Option<f32>) -> f32 {
    match loudness_db {
        Some(db) if db.is_finite() => 10f32.powf(-db / 20.0).min(1.0),
        _ => 1.0,
    }
}

/// Counters shared between one output stream and its callback.
pub(crate) struct Ctl {
    /// Samples the callback has taken from the ring, discarded ones included.
    popped: AtomicU64,
    /// The callback discards every sample with an index below this.
    discard_to: AtomicU64,
    /// Software pause, for devices that cannot pause in hardware.
    paused: AtomicBool,
}

impl Ctl {
    pub(crate) fn popped(&self) -> u64 {
        self.popped.load(Ordering::Acquire)
    }
}

pub(crate) struct Output {
    stream: cpal::Stream,
    producer: Producer<f32>,
    pub(crate) rate: u32,
    pub(crate) channels: usize,
    pushed: u64,
    pub(crate) ctl: Arc<Ctl>,
    playing: bool,
    /// Whether the stream itself runs: while playing, and while paused
    /// until the callback has dropped the samples of the last flush.
    running: bool,
}

/// Picks a stream config: the source rate if the device supports it, f32
/// stereo preferred; else the device default.
pub(crate) fn pick_config(
    device: &Device,
    rate: u32,
) -> Result<SupportedStreamConfig, PlayerError> {
    let supported =
        |f: SampleFormat| matches!(f, SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32);
    if let Ok(ranges) = device.supported_output_configs() {
        let best = ranges
            .filter(|r| supported(r.sample_format()))
            .filter(|r| r.min_sample_rate() <= rate && rate <= r.max_sample_rate())
            .min_by_key(|r| {
                (
                    r.channels() != 2,
                    r.sample_format() != SampleFormat::F32,
                    r.channels(),
                )
            });
        if let Some(r) = best {
            return Ok(r.with_sample_rate(rate));
        }
    }
    device
        .default_output_config()
        .map_err(|e| PlayerError::Device(e.to_string()))
}

impl Output {
    /// Opens a stream for `config`. The stream starts paused.
    pub(crate) fn open(
        device: &Device,
        config: SupportedStreamConfig,
        gain: Arc<Gain>,
        events: Sender<PlayerEvent>,
    ) -> Result<Self, PlayerError> {
        let format = config.sample_format();
        let config: StreamConfig = config.config();
        let channels = usize::from(config.channels);
        let rate = config.sample_rate;
        let (producer, consumer) = RingBuffer::new(rate as usize * channels * RING_SECS);
        let ctl = Arc::new(Ctl {
            popped: AtomicU64::new(0),
            discard_to: AtomicU64::new(0),
            paused: AtomicBool::new(true),
        });
        let stream = match format {
            SampleFormat::F32 => build::<f32>(device, config, consumer, &ctl, gain, events),
            SampleFormat::I16 => build::<i16>(device, config, consumer, &ctl, gain, events),
            SampleFormat::I32 => build::<i32>(device, config, consumer, &ctl, gain, events),
            other => Err(PlayerError::Device(format!(
                "unsupported sample format {other}"
            ))),
        }?;
        // Some hosts start streams on creation.
        let _ = stream.pause();
        Ok(Output {
            stream,
            producer,
            rate,
            channels,
            pushed: 0,
            ctl,
            playing: false,
            running: false,
        })
    }

    /// Free space in samples, rounded down to whole frames.
    pub(crate) fn free(&self) -> usize {
        let n = self.producer.slots();
        n - n % self.channels
    }

    /// Pushes as many whole frames of `samples` as fit; returns the number
    /// of samples pushed.
    pub(crate) fn push(&mut self, samples: &[f32]) -> usize {
        let n = self.free().min(samples.len());
        let n = n - n % self.channels;
        if n == 0 {
            return 0;
        }
        let Ok(chunk) = self.producer.write_chunk_uninit(n) else {
            return 0;
        };
        let written = chunk.fill_from_iter(samples[..n].iter().copied());
        self.pushed += written as u64;
        written
    }

    /// Marks everything pushed so far for discard. Returns the sample index
    /// the next pushed sample will have.
    pub(crate) fn flush(&mut self) -> u64 {
        self.ctl.discard_to.store(self.pushed, Ordering::Release);
        self.sync();
        self.pushed
    }

    /// Samples of the last flush still in the ring.
    fn has_stale(&self) -> bool {
        self.ctl.popped() < self.ctl.discard_to.load(Ordering::Acquire)
    }

    /// No room for another frame of new audio.
    pub(crate) fn full(&self) -> bool {
        self.free() == 0 && !self.has_stale()
    }

    /// Paused, but the stream still runs to drop flushed samples.
    pub(crate) fn settling(&self) -> bool {
        self.running && !self.playing
    }

    /// Starts or stops the stream to match `playing` and the flush state.
    /// The engine calls this on each pass, so a settling stream stops soon
    /// after the callback has caught up.
    pub(crate) fn sync(&mut self) {
        let run = self.playing || self.has_stale();
        if run != self.running {
            let _ = if run {
                self.stream.play()
            } else {
                self.stream.pause()
            };
            self.running = run;
        }
    }

    /// Samples pushed but not yet played.
    pub(crate) fn queued(&self) -> u64 {
        let done = self
            .ctl
            .popped()
            .max(self.ctl.discard_to.load(Ordering::Relaxed));
        self.pushed.saturating_sub(done)
    }

    pub(crate) fn drained(&self) -> bool {
        self.ctl.popped() >= self.pushed
    }

    pub(crate) fn is_playing(&self) -> bool {
        self.playing
    }

    pub(crate) fn play(&mut self) {
        self.ctl.paused.store(false, Ordering::Relaxed);
        self.playing = true;
        self.sync();
    }

    /// Pauses in hardware where possible, so the callback thread sleeps; the
    /// flag covers devices that cannot, and the time spent dropping flushed
    /// samples.
    pub(crate) fn pause(&mut self) {
        self.ctl.paused.store(true, Ordering::Relaxed);
        self.playing = false;
        self.sync();
    }
}

fn build<T>(
    device: &Device,
    config: StreamConfig,
    consumer: Consumer<f32>,
    ctl: &Arc<Ctl>,
    gain: Arc<Gain>,
    events: Sender<PlayerEvent>,
) -> Result<cpal::Stream, PlayerError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut render = Renderer {
        consumer,
        ctl: ctl.clone(),
        gain,
        channels,
        popped: 0,
        last_gain: f32::NAN,
    };
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &OutputCallbackInfo| render.fill(data),
            move |err| {
                use cpal::ErrorKind;
                if matches!(
                    err.kind(),
                    ErrorKind::DeviceNotAvailable | ErrorKind::DeviceChanged
                ) {
                    let _ = events.send(PlayerEvent::Error(format!("audio output: {err}")));
                }
            },
            None,
        )
        .map_err(|e| PlayerError::Device(e.to_string()))
}

/// The callback's state. Owned by the callback closure.
struct Renderer {
    consumer: Consumer<f32>,
    ctl: Arc<Ctl>,
    gain: Arc<Gain>,
    channels: usize,
    popped: u64,
    last_gain: f32,
}

impl Renderer {
    fn fill<T: Sample + FromSample<f32>>(&mut self, data: &mut [T]) {
        // Drop samples queued before the last flush, paused or not.
        let target = self.ctl.discard_to.load(Ordering::Acquire);
        if self.popped < target {
            let n = ((target - self.popped) as usize).min(self.consumer.slots());
            if let Ok(chunk) = self.consumer.read_chunk(n) {
                chunk.commit_all();
                self.popped += n as u64;
            }
        }
        if self.ctl.paused.load(Ordering::Relaxed) {
            data.fill(T::EQUILIBRIUM);
            self.ctl.popped.store(self.popped, Ordering::Release);
            return;
        }

        let want = self.gain.total();
        let from = if self.last_gain.is_nan() {
            want
        } else {
            self.last_gain
        };
        self.last_gain = want;

        let n = self.consumer.slots().min(data.len());
        let n = n - n % self.channels;
        if let Ok(chunk) = self.consumer.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            let frames = (n / self.channels).max(1) as f32;
            for (i, (out, &s)) in data.iter_mut().zip(a.iter().chain(b)).enumerate() {
                // Ramp the gain across the block to avoid clicks.
                let t = (i / self.channels + 1) as f32 / frames;
                *out = T::from_sample(s * (from + (want - from) * t));
            }
            chunk.commit_all();
            self.popped += n as u64;
        }
        data[n..].fill(T::EQUILIBRIUM);
        self.ctl.popped.store(self.popped, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn renderer(cap: usize) -> (Producer<f32>, Renderer, Arc<Gain>) {
        let (p, c) = RingBuffer::new(cap);
        let gain = Arc::new(Gain::new());
        let ctl = Arc::new(Ctl {
            popped: AtomicU64::new(0),
            discard_to: AtomicU64::new(0),
            paused: AtomicBool::new(false),
        });
        let r = Renderer {
            consumer: c,
            ctl,
            gain: gain.clone(),
            channels: 2,
            popped: 0,
            last_gain: f32::NAN,
        };
        (p, r, gain)
    }

    #[test]
    fn norm_gain_never_boosts() {
        assert_eq!(norm_gain(None), 1.0);
        assert!((norm_gain(Some(6.0)) - 0.501).abs() < 0.001);
        assert_eq!(norm_gain(Some(-3.0)), 1.0);
        assert_eq!(norm_gain(Some(0.0)), 1.0);
    }

    #[test]
    fn renders_with_gain_and_pads_underrun() {
        let (mut p, mut r, gain) = renderer(16);
        gain.set_volume(0.5);
        for _ in 0..4 {
            p.push(1.0).unwrap();
        }
        let mut out = [9.0f32; 8];
        r.fill(&mut out);
        assert_eq!(out, [0.5, 0.5, 0.5, 0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(r.ctl.popped(), 4);
    }

    #[test]
    fn flush_discards_old_samples_only() {
        let (mut p, mut r, _) = renderer(16);
        for _ in 0..6 {
            p.push(1.0).unwrap();
        }
        // Flush at index 6, then queue new samples before the callback runs.
        r.ctl.discard_to.store(6, Ordering::Release);
        for _ in 0..2 {
            p.push(0.25).unwrap();
        }
        let mut out = [9.0f32; 4];
        r.fill(&mut out);
        assert_eq!(out, [0.25, 0.25, 0.0, 0.0]);
        assert_eq!(r.ctl.popped(), 8);
    }

    #[test]
    fn paused_still_drops_flushed_samples() {
        let (mut p, mut r, _) = renderer(8);
        for _ in 0..4 {
            p.push(1.0).unwrap();
        }
        r.ctl.discard_to.store(2, Ordering::Release);
        r.ctl.paused.store(true, Ordering::Relaxed);
        let mut out = [9.0f32; 2];
        r.fill(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        assert_eq!(r.ctl.popped(), 2);
        assert_eq!(r.consumer.slots(), 2);
    }

    #[test]
    fn paused_outputs_silence_without_consuming() {
        let (mut p, mut r, _) = renderer(8);
        p.push(1.0).unwrap();
        p.push(1.0).unwrap();
        r.ctl.paused.store(true, Ordering::Relaxed);
        let mut out = [9.0f32; 2];
        r.fill(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        assert_eq!(r.consumer.slots(), 2);
    }
}
