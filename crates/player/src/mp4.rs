//! The MP4 edit list, which symphonia 0.6.1 parses but does not apply.
//!
//! An AAC encoder emits priming samples before the real audio. The edit list
//! (`moov/trak/edts/elst`) says how many to skip and how long the track
//! plays. Without it every position runs ahead of the audio by the priming.

use std::io::{self, Read, Seek, SeekFrom};

use symphonia::core::io::MediaSource;

/// Bytes from the start of the file kept in memory while scanning, so the
/// demuxer can read them again without a second HTTP request.
const HEAD_MAX: usize = 256 * 1024;
/// Largest `moov` box read.
const MOOV_MAX: u64 = 4 * 1024 * 1024;
/// Top-level boxes looked at before giving up on `moov`.
const MAX_BOXES: usize = 16;

/// The audio track's edit, in seconds of media time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Edit {
    /// Media time of the first presented sample: the encoder priming.
    pub(crate) delay: f64,
    /// How long the track plays, if the file says.
    pub(crate) duration: Option<f64>,
}

/// A source that replays the bytes read from its start while `recording`.
pub(crate) struct Head {
    inner: Box<dyn MediaSource>,
    head: Vec<u8>,
    recording: bool,
    pos: u64,
    inner_pos: u64,
}

impl Head {
    /// `inner` must be at offset 0.
    pub(crate) fn new(inner: Box<dyn MediaSource>) -> Self {
        Head {
            inner,
            head: Vec::new(),
            recording: true,
            pos: 0,
            inner_pos: 0,
        }
    }

    /// Reads the edit of the first audio track, then rewinds to 0. Returns
    /// `None` when the file has no edit list or cannot be parsed.
    pub(crate) fn scan_edit(&mut self) -> Option<Edit> {
        let edit = self.find_moov().ok().flatten().and_then(|m| parse_moov(&m));
        self.recording = false;
        self.pos = 0;
        edit
    }

    fn find_moov(&mut self) -> io::Result<Option<Vec<u8>>> {
        let len = self.inner.byte_len();
        let mut at = 0u64;
        for _ in 0..MAX_BOXES {
            if len.is_some_and(|l| at + 8 > l) {
                return Ok(None);
            }
            self.seek(SeekFrom::Start(at))?;
            let mut hdr = [0u8; 8];
            self.read_exact(&mut hdr)?;
            let mut size = u64::from(u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]));
            let mut hdr_len = 8;
            if size == 1 {
                let mut big = [0u8; 8];
                self.read_exact(&mut big)?;
                size = u64::from_be_bytes(big);
                hdr_len = 16;
            } else if size == 0 {
                // Runs to the end of the file.
                size = len.map_or(0, |l| l - at);
            }
            if size < hdr_len {
                return Ok(None);
            }
            if &hdr[4..] == b"moov" {
                let body = size - hdr_len;
                if body > MOOV_MAX {
                    return Ok(None);
                }
                let mut moov = vec![0u8; body as usize];
                self.read_exact(&mut moov)?;
                return Ok(Some(moov));
            }
            at += size;
        }
        Ok(None)
    }
}

impl Read for Head {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let held = self.head.len() as u64;
        if self.pos < held {
            let at = self.pos as usize;
            let n = buf.len().min(self.head.len() - at);
            buf[..n].copy_from_slice(&self.head[at..at + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        if self.inner_pos != self.pos {
            self.inner.seek(SeekFrom::Start(self.pos))?;
            self.inner_pos = self.pos;
        }
        let n = self.inner.read(buf)?;
        if self.recording && self.pos == held && self.head.len() + n <= HEAD_MAX {
            self.head.extend_from_slice(&buf[..n]);
        }
        self.pos += n as u64;
        self.inner_pos += n as u64;
        Ok(n)
    }
}

impl Seek for Head {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self
                .inner
                .byte_len()
                .ok_or_else(|| io::Error::new(io::ErrorKind::Unsupported, "unknown length"))?
                .checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for Head {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}

/// Child boxes of `data` as (type, body).
fn boxes(mut data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    std::iter::from_fn(move || {
        if data.len() < 8 {
            return None;
        }
        let mut size = u32::from_be_bytes(data[..4].try_into().ok()?) as usize;
        let kind = &data[4..8];
        let mut hdr = 8;
        if size == 1 {
            size = usize::try_from(u64::from_be_bytes(data.get(8..16)?.try_into().ok()?)).ok()?;
            hdr = 16;
        } else if size == 0 {
            size = data.len();
        }
        if size < hdr || size > data.len() {
            return None;
        }
        let body = &data[hdr..size];
        data = &data[size..];
        Some((kind, body))
    })
}

fn child<'a>(data: &'a [u8], kind: &[u8]) -> Option<&'a [u8]> {
    boxes(data).find(|(k, _)| *k == kind).map(|(_, b)| b)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Timescale of an `mvhd` or `mdhd` body.
fn timescale(full_box: &[u8]) -> Option<u32> {
    let at = if *full_box.first()? == 1 { 20 } else { 12 };
    be32(full_box, at).filter(|&t| t > 0)
}

fn parse_moov(moov: &[u8]) -> Option<Edit> {
    let movie_scale = child(moov, b"mvhd").and_then(timescale);
    for (_, trak) in boxes(moov).filter(|(k, _)| *k == b"trak") {
        let Some(mdia) = child(trak, b"mdia") else {
            continue;
        };
        let sound = child(mdia, b"hdlr").is_none_or(|h| h.get(8..12) == Some(b"soun"));
        if !sound {
            continue;
        }
        let elst = child(trak, b"edts").and_then(|e| child(e, b"elst"))?;
        let media_scale = child(mdia, b"mdhd").and_then(timescale)?;
        let (segment, media_time) = first_edit(elst)?;
        let duration = movie_scale
            .filter(|_| segment > 0)
            .map(|s| segment as f64 / f64::from(s));
        return Some(Edit {
            delay: media_time as f64 / f64::from(media_scale),
            duration,
        });
    }
    None
}

/// The first non-empty edit: (segment duration, media time).
fn first_edit(elst: &[u8]) -> Option<(u64, u64)> {
    let version = *elst.first()?;
    let count = be32(elst, 4)? as usize;
    let size = if version == 1 { 20 } else { 12 };
    (0..count.min(16)).find_map(|i| {
        let at = 8 + i * size;
        let (segment, media_time) = if version == 1 {
            (be64(elst, at)?, be64(elst, at + 8)? as i64)
        } else {
            (
                u64::from(be32(elst, at)?),
                i64::from(be32(elst, at + 4)? as i32),
            )
        };
        // -1 marks an empty edit (a gap before the media starts).
        u64::try_from(media_time).ok().map(|m| (segment, m))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn scan(data: Vec<u8>) -> (Option<Edit>, Head) {
        let mut head = Head::new(Box::new(std::io::Cursor::new(data)));
        (head.scan_edit(), head)
    }

    fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend_from_slice(kind);
        b.extend_from_slice(body);
        b
    }

    #[test]
    fn reads_ffmpeg_priming_with_moov_first_and_last() {
        for name in ["onset2s.m4a", "sine3s.m4a"] {
            let data = fixture(name);
            let (edit, mut head) = scan(data.clone());
            let edit = edit.unwrap();
            assert!((edit.delay - 1024.0 / 44_100.0).abs() < 1e-9, "{name}");
            assert!(edit.duration.unwrap() > 1.9, "{name}");
            // The scan rewinds; the bytes read back match the file.
            let mut all = Vec::new();
            head.read_to_end(&mut all).unwrap();
            assert_eq!(all, data, "{name}");
        }
    }

    #[test]
    fn no_edit_list_means_none() {
        // A fragmented-MP4 header like YouTube's itag 140: no `edts`.
        let mut mdhd = vec![0u8; 24];
        mdhd[12..16].copy_from_slice(&44_100u32.to_be_bytes());
        let mdia = mp4_box(b"mdia", &mp4_box(b"mdhd", &mdhd));
        let moov = mp4_box(b"moov", &mp4_box(b"trak", &mdia));
        let mut file = mp4_box(b"ftyp", b"dash\0\0\0\0");
        file.extend_from_slice(&moov);
        file.extend_from_slice(&mp4_box(b"mdat", &[0; 32]));
        assert_eq!(scan(file).0, None);
        assert_eq!(scan(b"not an mp4 file".to_vec()).0, None);
    }

    #[test]
    fn skips_empty_edits_and_reads_version_1() {
        let mut elst = vec![1, 0, 0, 0, 0, 0, 0, 2];
        for (seg, time) in [(500u64, -1i64), (3_000, 2_112)] {
            elst.extend_from_slice(&seg.to_be_bytes());
            elst.extend_from_slice(&time.to_be_bytes());
            elst.extend_from_slice(&[0, 1, 0, 0]);
        }
        assert_eq!(first_edit(&elst), Some((3_000, 2_112)));
    }
}
