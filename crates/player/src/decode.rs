//! Demux and decode to interleaved f32 at the source's own rate.
//!
//! symphonia reads the container (WebM or MP4). AAC decodes through
//! symphonia; Opus packets go to libopus, since symphonia has no Opus
//! decoder.

use std::time::Duration;

use model::AudioCodec;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase};

use crate::PlayerError;
use crate::mp4::Head;

/// Priming of YouTube's AAC (itag 140), whose fragmented MP4 has no edit
/// list: 1600 samples at 44.1 kHz. Measured, not from a spec: the AAC output
/// ran 1600 samples behind the Opus (itag 251) output of the same video, on
/// each of four videos checked.
const YOUTUBE_AAC_DELAY: f64 = 1600.0 / 44_100.0;

/// A source of decoded audio.
pub(crate) trait Decode: Send {
    fn rate(&self) -> u32;
    fn channels(&self) -> usize;
    /// Replaces `out` with the next block of interleaved samples. Returns
    /// `false` at the end of the stream.
    fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, PlayerError>;
    /// Moves to `to`. The next block starts at `to` (or ends the stream if
    /// `to` is past the end).
    fn seek(&mut self, to: Duration) -> Result<(), PlayerError>;
}

/// Silence for `demo://` URLs.
pub(crate) struct Silence {
    left: u64,
    total: u64,
}

const SILENCE_RATE: u32 = 48_000;
const SILENCE_BLOCK: u64 = 4_800;

impl Silence {
    pub(crate) fn new(duration: Duration) -> Self {
        let total = (duration.as_secs_f64() * f64::from(SILENCE_RATE)) as u64;
        Silence { left: total, total }
    }
}

impl Decode for Silence {
    fn rate(&self) -> u32 {
        SILENCE_RATE
    }

    fn channels(&self) -> usize {
        2
    }

    fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, PlayerError> {
        out.clear();
        if self.left == 0 {
            return Ok(false);
        }
        let n = self.left.min(SILENCE_BLOCK);
        self.left -= n;
        out.resize(n as usize * 2, 0.0);
        Ok(true)
    }

    fn seek(&mut self, to: Duration) -> Result<(), PlayerError> {
        let at = (to.as_secs_f64() * f64::from(SILENCE_RATE)) as u64;
        self.left = self.total.saturating_sub(at);
        Ok(())
    }
}

enum Codec {
    Opus {
        dec: opus::Decoder,
        /// Decode scratch: 120 ms at 48 kHz, the Opus maximum.
        buf: Vec<f32>,
    },
    Other {
        dec: Box<dyn AudioDecoder>,
    },
}

/// A demuxed, decoded container stream.
pub(crate) struct Demuxed {
    /// `None` only while being re-probed.
    format: Option<Box<dyn FormatReader>>,
    hint: Hint,
    track_id: u32,
    time_base: TimeBase,
    codec: Codec,
    rate: u32,
    channels: usize,
    /// Drop decoded frames before this time (seconds). Covers Opus pre-skip
    /// (the WebM reader shifts the first packets below zero) and seeks.
    trim_until: f64,
    /// Opus pre-skip still to drop, for containers that did not shift
    /// timestamps by the codec delay.
    preskip: usize,
    /// Media time of presentation time 0 (seconds): the AAC encoder priming.
    delay: f64,
    /// Media time where the track stops playing, from the MP4 edit list.
    end: Option<f64>,
    first_packet: bool,
    ended: bool,
    /// Past `end`, but the reader has not hit the end of the stream.
    past_end: bool,
}

/// Audio decoded and dropped before a seek target. 80 ms is the Opus
/// pre-roll RFC 7845 asks for after a decoder reset.
const PREROLL: f64 = 0.08;
/// Opus decoders always run at 48 kHz.
const OPUS_RATE: u32 = 48_000;
const OPUS_MAX_FRAMES: usize = 5_760;

impl Demuxed {
    pub(crate) fn open(
        source: Box<dyn MediaSource>,
        codec: AudioCodec,
    ) -> Result<Self, PlayerError> {
        let is_aac = codec == AudioCodec::Aac;
        let (source, edit): (Box<dyn MediaSource>, _) = match codec {
            AudioCodec::Aac => {
                let mut head = Head::new(source);
                let edit = head.scan_edit();
                (Box::new(head), edit)
            }
            AudioCodec::Opus => (source, None),
        };
        let mss = MediaSourceStream::new(
            source,
            MediaSourceStreamOptions {
                buffer_len: 64 * 1024,
            },
        );
        let mut hint = Hint::new();
        hint.with_extension(match codec {
            AudioCodec::Opus => "webm",
            AudioCodec::Aac => "m4a",
        });
        let format = probe(&hint, mss)?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| PlayerError::Decode("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| PlayerError::Decode("no audio codec parameters".into()))?;
        let track_id = track.id;
        let time_base = track.time_base.unwrap_or_else(|| {
            TimeBase::try_from_recip(params.sample_rate.unwrap_or(OPUS_RATE)).unwrap_or_default()
        });

        let (codec, rate, channels, preskip) = if params.codec == CODEC_ID_OPUS {
            let head = params.extra_data.as_deref().unwrap_or(&[]);
            let (channels, preskip) = parse_opus_head(head)
                .unwrap_or((params.channels.as_ref().map_or(2, |c| c.count()), 0));
            let ch = match channels {
                1 => opus::Channels::Mono,
                2 => opus::Channels::Stereo,
                _ => {
                    return Err(PlayerError::Decode(
                        "only mono and stereo Opus are supported".into(),
                    ));
                }
            };
            let dec = opus::Decoder::new(OPUS_RATE, ch)
                .map_err(|e| PlayerError::Decode(format!("libopus: {e}")))?;
            let buf = vec![0.0; OPUS_MAX_FRAMES * channels];
            (Codec::Opus { dec, buf }, OPUS_RATE, channels, preskip)
        } else {
            let dec = symphonia::default::get_codecs()
                .make_audio_decoder(params, &AudioDecoderOptions::default())
                .map_err(map_err)?;
            let rate = params
                .sample_rate
                .ok_or_else(|| PlayerError::Decode("unknown sample rate".into()))?;
            let channels = params
                .channels
                .as_ref()
                .map(|c| c.count())
                .ok_or_else(|| PlayerError::Decode("unknown channel count".into()))?;
            (Codec::Other { dec }, rate, channels, 0)
        };
        let (delay, end) = match edit {
            Some(e) => (e.delay, e.duration.map(|d| e.delay + d)),
            None if is_aac => (YOUTUBE_AAC_DELAY, None),
            None => (0.0, None),
        };

        Ok(Demuxed {
            format: Some(format),
            hint,
            track_id,
            time_base,
            codec,
            rate,
            channels,
            trim_until: delay,
            preskip,
            delay,
            end,
            first_packet: true,
            ended: false,
            past_end: false,
        })
    }
}

/// Channel count and pre-skip from an `OpusHead` block. Channel mapping
/// families other than 0 (mono/stereo) are reported as 0 channels.
fn parse_opus_head(head: &[u8]) -> Option<(usize, usize)> {
    if head.len() < 19 || &head[..8] != b"OpusHead" {
        return None;
    }
    let channels = usize::from(head[9]);
    let preskip = usize::from(u16::from_le_bytes([head[10], head[11]]));
    let family = head[18];
    Some((if family == 0 { channels } else { 0 }, preskip))
}

fn probe(
    hint: &Hint,
    mss: MediaSourceStream<'static>,
) -> Result<Box<dyn FormatReader>, PlayerError> {
    symphonia::default::get_probe()
        .probe(
            hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(map_err)
}

fn map_err(e: SymError) -> PlayerError {
    match e {
        SymError::IoError(e) => PlayerError::Fetch(e.to_string()),
        other => PlayerError::Decode(other.to_string()),
    }
}

impl Decode for Demuxed {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> usize {
        self.channels
    }

    fn next(&mut self, out: &mut Vec<f32>) -> Result<bool, PlayerError> {
        out.clear();
        loop {
            if self.ended || self.past_end {
                return Ok(false);
            }
            let format = self.format.as_mut().expect("format present");
            let packet = match format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => {
                    self.ended = true;
                    return Ok(false);
                }
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    self.ended = true;
                    return Ok(false);
                }
                Err(e) => return Err(map_err(e)),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let start = self
                .time_base
                .calc_time(packet.pts)
                .map_or(0.0, |t| t.as_secs_f64());
            if self.first_packet {
                self.first_packet = false;
                if start < 0.0 {
                    // The container already moved pre-skip below zero.
                    self.preskip = 0;
                }
            }
            match &mut self.codec {
                Codec::Opus { dec, buf } => {
                    match dec.decode_float(&packet.data, buf, false) {
                        Ok(frames) => out.extend_from_slice(&buf[..frames * self.channels]),
                        // A corrupt packet: skip it, as symphonia does.
                        Err(_) => continue,
                    }
                }
                Codec::Other { dec } => match dec.decode(&packet) {
                    Ok(decoded) => {
                        let at = out.len();
                        out.resize(at + decoded.samples_interleaved(), 0.0);
                        decoded.copy_to_slice_interleaved(&mut out[at..]);
                    }
                    Err(SymError::DecodeError(_)) => continue,
                    Err(e) => return Err(map_err(e)),
                },
            }

            if let Some(end) = self.end {
                let keep = ((end - start) * f64::from(self.rate)).round().max(0.0) as usize;
                if keep == 0 {
                    out.clear();
                    self.past_end = true;
                    return Ok(false);
                }
                out.truncate(keep * self.channels);
            }

            let mut drop_frames = 0;
            if self.preskip > 0 {
                drop_frames = self.preskip;
            }
            if start < self.trim_until {
                let behind = ((self.trim_until - start) * f64::from(self.rate)).round() as usize;
                drop_frames = drop_frames.max(behind);
            }
            let frames = out.len() / self.channels;
            let dropped = drop_frames.min(frames);
            self.preskip = self.preskip.saturating_sub(dropped);
            if dropped > 0 {
                out.drain(..dropped * self.channels);
            }
            if !out.is_empty() {
                return Ok(true);
            }
        }
    }

    fn seek(&mut self, to: Duration) -> Result<(), PlayerError> {
        let secs = to.as_secs_f64() + self.delay;
        // Land early and decode up to `secs`: the MKV reader's accurate seek
        // lands on the first packet after the target, and a reset decoder
        // needs pre-roll.
        let time = Time::try_from_secs_f64((secs - PREROLL).max(0.0))
            .ok_or_else(|| PlayerError::Decode("seek target out of range".into()))?;
        // symphonia 0.6.1's MKV and MP4 readers cannot seek once they have
        // hit the end of the stream: re-probe the same source first.
        if self.ended {
            self.reprobe()?;
        }
        let to = SeekTo::Time {
            time,
            track_id: Some(self.track_id),
        };
        let format = self.format.as_mut().expect("format present");
        let res = format.seek(SeekMode::Accurate, to);
        match &mut self.codec {
            Codec::Opus { dec, .. } => {
                let _ = dec.reset_state();
            }
            Codec::Other { dec } => dec.reset(),
        }
        self.preskip = 0;
        self.first_packet = false;
        self.past_end = false;
        self.trim_until = secs;
        match res {
            Ok(_) => {
                self.ended = false;
                Ok(())
            }
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => {
                self.ended = true;
                Ok(())
            }
            Err(e) => Err(map_err(e)),
        }
    }
}

impl Demuxed {
    fn reprobe(&mut self) -> Result<(), PlayerError> {
        use std::io::{Seek, SeekFrom};
        let format = self.format.take().expect("format present");
        let mut mss = format.into_inner();
        mss.seek(SeekFrom::Start(0))
            .map_err(|e| PlayerError::Fetch(e.to_string()))?;
        self.format = Some(probe(&self.hint, mss)?);
        self.ended = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{HttpSource, agent, tests::serve};

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn open_file(name: &str, codec: AudioCodec) -> Demuxed {
        let file = std::io::Cursor::new(fixture(name));
        Demuxed::open(Box::new(file), codec).unwrap()
    }

    /// Decodes to the end; returns (interleaved samples, first block).
    fn drain(d: &mut dyn Decode) -> Vec<f32> {
        let mut all = Vec::new();
        let mut block = Vec::new();
        while d.next(&mut block).unwrap() {
            all.extend_from_slice(&block);
        }
        all
    }

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    }

    /// Counts upward zero crossings on channel 0 to estimate frequency.
    fn freq(s: &[f32], channels: usize, rate: u32) -> f32 {
        let ch0: Vec<f32> = s.iter().step_by(channels).copied().collect();
        let ups = ch0.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        ups as f32 / (ch0.len() as f32 / rate as f32)
    }

    fn check_sine(d: &mut Demuxed, secs: f32) {
        let (rate, ch) = (d.rate(), d.channels());
        let all = drain(d);
        let got = all.len() as f32 / ch as f32 / rate as f32;
        assert!((got - secs).abs() < 0.06, "decoded {got} s, want {secs} s");
        let mid = &all[all.len() / 4..all.len() * 3 / 4];
        // The fixture's sine has an RMS of 0.0625.
        assert!((rms(mid) - 0.0625).abs() < 0.01, "rms {}", rms(mid));
        let f = freq(mid, ch, rate);
        assert!((f - 440.0).abs() < 5.0, "freq {f}");
    }

    #[test]
    fn decodes_opus_webm() {
        let mut d = open_file("sine3s.webm", AudioCodec::Opus);
        assert_eq!((d.rate(), d.channels()), (48_000, 2));
        check_sine(&mut d, 3.0);
    }

    #[test]
    fn decodes_aac_m4a() {
        let mut d = open_file("sine3s.m4a", AudioCodec::Aac);
        assert_eq!((d.rate(), d.channels()), (44_100, 2));
        check_sine(&mut d, 3.0);
    }

    fn check_seek(name: &str, codec: AudioCodec) {
        let mut d = open_file(name, codec);
        let mut block = Vec::new();
        // Read a little, then jump forward, then back.
        d.next(&mut block).unwrap();
        d.seek(Duration::from_secs(2)).unwrap();
        let rest = drain(&mut d);
        let secs = rest.len() as f32 / d.channels() as f32 / d.rate() as f32;
        assert!(
            (secs - 1.0).abs() < 0.06,
            "{name}: {secs} s left after seek to 2 s"
        );
        d.seek(Duration::from_millis(500)).unwrap();
        let rest = drain(&mut d);
        let secs = rest.len() as f32 / d.channels() as f32 / d.rate() as f32;
        assert!(
            (secs - 2.5).abs() < 0.06,
            "{name}: {secs} s left after seek to 0.5 s"
        );
        // Back again after hitting the end.
        d.seek(Duration::ZERO).unwrap();
        let rest = drain(&mut d);
        let secs = rest.len() as f32 / d.channels() as f32 / d.rate() as f32;
        assert!(
            (secs - 3.0).abs() < 0.06,
            "{name}: {secs} s left after seek to 0"
        );
        // Past the end: the stream ends.
        d.seek(Duration::from_secs(60)).unwrap();
        assert!(!d.next(&mut block).unwrap());
    }

    #[test]
    fn seeks_opus() {
        check_seek("sine3s.webm", AudioCodec::Opus);
    }

    #[test]
    fn seeks_aac() {
        check_seek("sine3s.m4a", AudioCodec::Aac);
    }

    #[test]
    fn decodes_over_http() {
        for (name, codec) in [
            ("sine3s.webm", AudioCodec::Opus),
            ("sine3s.m4a", AudioCodec::Aac),
        ] {
            let data = fixture(name);
            let (url, _) = serve(data.clone());
            let src = HttpSource::open(agent(), url, data.len() as u64).unwrap();
            let mut d = Demuxed::open(Box::new(src), codec).unwrap();
            check_sine(&mut d, 3.0);
        }
    }

    /// Seconds from the first output sample to the tone onset.
    fn onset(d: &mut Demuxed) -> f64 {
        let all = drain(d);
        let ch = d.channels();
        let at = all
            .iter()
            .step_by(ch)
            .position(|x| x.abs() > 0.35)
            .expect("tone found");
        at as f64 / f64::from(d.rate())
    }

    /// The `onset2s` fixtures are 1 s of silence, then a tone. Positions are
    /// right when the tone starts 1 s after the start, and `1 - t` after a
    /// seek to `t`.
    #[test]
    fn onset_lands_on_time_from_start_and_after_seek() {
        for (name, codec) in [
            ("onset2s.m4a", AudioCodec::Aac),
            ("onset2s.webm", AudioCodec::Opus),
        ] {
            for seek in [0.0, 0.5, 0.9] {
                let mut d = open_file(name, codec);
                if seek > 0.0 {
                    d.seek(Duration::from_secs_f64(seek)).unwrap();
                }
                let got = onset(&mut d);
                let want = 1.0 - seek;
                assert!(
                    (got - want).abs() < 0.003,
                    "{name} seek {seek}: tone at {got:.4} s, want {want:.4} s"
                );
            }
        }
    }

    #[test]
    fn aac_edit_list_trims_priming_and_padding() {
        let mut d = open_file("sine3s.m4a", AudioCodec::Aac);
        let frames = drain(&mut d).len() / d.channels();
        assert_eq!(frames, 3 * 44_100);
    }

    #[test]
    fn silence_lasts_duration_and_seeks() {
        let mut s = Silence::new(Duration::from_millis(1500));
        let all = drain(&mut s);
        assert_eq!(all.len(), 72_000 * 2);
        assert!(all.iter().all(|&x| x == 0.0));
        s.seek(Duration::from_secs(1)).unwrap();
        assert_eq!(drain(&mut s).len(), 24_000 * 2);
    }
}
