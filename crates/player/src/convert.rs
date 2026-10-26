//! Channel mapping and linear resampling from the decoder's format to the
//! output device's format. Resampling runs only when the device cannot open
//! at the source rate.

/// Maps `input` (interleaved, `from` channels) to `to` channels, appending
/// to `out`.
pub(crate) fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    for frame in input.chunks_exact(from) {
        if to == 1 {
            out.push(frame.iter().sum::<f32>() / from as f32);
        } else if from == 1 {
            out.extend(std::iter::repeat_n(frame[0], to));
        } else {
            out.extend((0..to).map(|c| frame.get(c).copied().unwrap_or(0.0)));
        }
    }
}

/// Streaming linear resampler over interleaved frames.
pub(crate) struct Resampler {
    channels: usize,
    /// Input frames per output frame.
    step: f64,
    /// Read position, in input frames, relative to `prev` (index 0).
    pos: f64,
    /// Last frame of the previous block.
    prev: Vec<f32>,
}

impl Resampler {
    pub(crate) fn new(from_rate: u32, to_rate: u32, channels: usize) -> Self {
        Resampler {
            channels,
            step: f64::from(from_rate) / f64::from(to_rate),
            pos: 1.0,
            prev: vec![0.0; channels],
        }
    }

    pub(crate) fn is_identity(&self) -> bool {
        self.step == 1.0
    }

    /// Clears history, for a seek.
    pub(crate) fn reset(&mut self) {
        self.pos = 1.0;
        self.prev.iter_mut().for_each(|s| *s = 0.0);
    }

    /// Resamples `input`, appending to `out`.
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        let ch = self.channels;
        if self.is_identity() {
            out.extend_from_slice(input);
            return;
        }
        let frames = input.len() / ch;
        if frames == 0 {
            return;
        }
        // Virtual frame i: i == 0 is `prev`, i >= 1 is input frame i - 1.
        let frame = |i: usize, c: usize| {
            if i == 0 {
                self.prev[c]
            } else {
                input[(i - 1) * ch + c]
            }
        };
        while self.pos < frames as f64 {
            let i = self.pos as usize;
            let t = (self.pos - i as f64) as f32;
            for c in 0..ch {
                let a = frame(i, c);
                let b = frame(i + 1, c);
                out.push(a + (b - a) * t);
            }
            self.pos += self.step;
        }
        self.pos -= frames as f64;
        self.prev
            .copy_from_slice(&input[(frames - 1) * ch..frames * ch]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_mono_to_stereo_and_back() {
        let mut out = Vec::new();
        map_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
        let mut down = Vec::new();
        map_channels(&[1.0, 0.0, 0.5, 0.5], 2, 1, &mut down);
        assert_eq!(down, [0.5, 0.5]);
    }

    #[test]
    fn resamples_44k1_to_48k_keeping_length_and_pitch() {
        let rate_in = 44_100u32;
        let input: Vec<f32> = (0..rate_in)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / rate_in as f32).sin())
            .collect();
        let mut r = Resampler::new(rate_in, 48_000, 1);
        let mut out = Vec::new();
        // Feed in uneven blocks to exercise the carried state.
        for block in input.chunks(1_000) {
            r.process(block, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "len {}", out.len());
        // Compare against an ideal 48 kHz sine.
        let err = out
            .iter()
            .enumerate()
            .skip(10)
            .map(|(i, &s)| {
                let t = i as f32 / 48_000.0;
                (s - (t * 440.0 * std::f32::consts::TAU).sin()).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(err < 0.01, "max error {err}");
    }
}
