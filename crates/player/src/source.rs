//! HTTP media source: Range requests of about 1 MiB, read lazily.
//!
//! YouTube throttles one big GET, so each request covers at most `CHUNK`
//! bytes. The body is read straight from the socket as the decoder asks for
//! it, so nothing beyond the decoder's own read buffer sits in memory.

use std::io::{self, Read, Seek, SeekFrom};
use std::time::Duration;

use symphonia::core::io::MediaSource;
use ureq::BodyReader;

/// Bytes per Range request.
const CHUNK: u64 = 1024 * 1024;
/// Forward seeks shorter than this read through the open body instead of
/// opening a new request.
const SKIP_LIMIT: u64 = 64 * 1024;
/// Attempts per chunk before a read fails.
const ATTEMPTS: u32 = 3;

pub(crate) fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .timeout_per_read(Some(Duration::from_secs(20)))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0")
        .build();
    ureq::Agent::new_with_config(config)
}

pub(crate) struct HttpSource {
    agent: ureq::Agent,
    url: String,
    len: u64,
    pos: u64,
    body: Option<OpenChunk>,
}

struct OpenChunk {
    reader: BodyReader<'static>,
    /// Absolute offset the next body byte belongs to.
    at: u64,
    /// Exclusive end of the requested range.
    end: u64,
}

impl HttpSource {
    /// `len` is the total size in bytes; 0 means unknown, in which case the
    /// first response's `Content-Range` supplies it.
    pub(crate) fn open(agent: ureq::Agent, url: String, len: u64) -> io::Result<Self> {
        let mut src = HttpSource {
            agent,
            url,
            len,
            pos: 0,
            body: None,
        };
        // Fail early on a dead URL, and learn the length if needed.
        src.open_chunk()?;
        Ok(src)
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> u64 {
        self.len
    }

    fn open_chunk(&mut self) -> io::Result<()> {
        self.body = None;
        let end = if self.len == 0 {
            self.pos + CHUNK
        } else {
            self.len.min(self.pos + CHUNK)
        };
        let resp = self
            .agent
            .get(&self.url)
            .header("Range", format!("bytes={}-{}", self.pos, end - 1))
            .call()
            .map_err(|e| io::Error::other(format!("GET range {}: {e}", self.pos)))?;
        let partial = resp.status().as_u16() == 206;
        if partial {
            if let Some(total) = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.parse::<u64>().ok())
            {
                self.len = total;
            }
        } else if self.len == 0 {
            self.len = resp
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
        }
        let mut chunk = OpenChunk {
            reader: resp.into_body().into_reader(),
            // A 200 reply ignores the Range header and starts at byte 0.
            at: if partial { self.pos } else { 0 },
            end: if partial || self.len == 0 {
                end
            } else {
                self.len
            },
        };
        if self.len != 0 {
            chunk.end = chunk.end.min(self.len);
        }
        if chunk.at < self.pos {
            let gap = self.pos - chunk.at;
            skip(&mut chunk, gap)?;
        }
        self.body = Some(chunk);
        Ok(())
    }

    fn read_once(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let chunk_done = self
            .body
            .as_ref()
            .is_none_or(|c| c.at != self.pos || c.at >= c.end);
        if chunk_done {
            self.open_chunk()?;
        }
        let chunk = self.body.as_mut().expect("chunk opened above");
        let want = buf.len().min((chunk.end - chunk.at) as usize);
        let n = chunk.reader.read(&mut buf[..want])?;
        if n == 0 && want > 0 {
            // Not `UnexpectedEof`: the decoder would take that for the end
            // of the track.
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "server closed the range early",
            ));
        }
        chunk.at += n as u64;
        self.pos += n as u64;
        Ok(n)
    }
}

fn skip(chunk: &mut OpenChunk, mut n: u64) -> io::Result<()> {
    let mut scratch = [0u8; 4096];
    while n > 0 {
        let want = n.min(scratch.len() as u64) as usize;
        let got = chunk.reader.read(&mut scratch[..want])?;
        if got == 0 {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "body ended while skipping",
            ));
        }
        chunk.at += got as u64;
        n -= got as u64;
    }
    Ok(())
}

impl Read for HttpSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || (self.len != 0 && self.pos >= self.len) {
            return Ok(0);
        }
        let mut last_err = None;
        for _ in 0..ATTEMPTS {
            match self.read_once(buf) {
                Ok(n) => return Ok(n),
                Err(e) => {
                    // Drop the connection and retry the range from `pos`.
                    self.body = None;
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.expect("at least one attempt"))
    }
}

impl Seek for HttpSource {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
        }
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before start"))?;
        if target == self.pos {
            return Ok(target);
        }
        // Short forward hop inside the open range: read through it.
        if let Some(chunk) = self.body.as_mut()
            && chunk.at == self.pos
            && target > self.pos
            && target - self.pos <= SKIP_LIMIT
            && target <= chunk.end
            && skip(chunk, target - self.pos).is_ok()
        {
            self.pos = target;
            return Ok(target);
        }
        self.body = None;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for HttpSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        (self.len != 0).then_some(self.len)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serves `data` on loopback with Range support. Returns the base URL and
    /// a counter of requests served.
    pub(crate) fn serve(data: Vec<u8>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let data = Arc::new(data);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let data = data.clone();
                let counter = counter.clone();
                std::thread::spawn(move || handle(stream, &data, &counter));
            }
        });
        (url, hits)
    }

    fn handle(stream: std::net::TcpStream, data: &[u8], hits: &AtomicUsize) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut out = stream;
        loop {
            let mut range = None;
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 {
                    return;
                }
                let h = h.trim_end();
                if h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("range: bytes=") {
                    let (a, b) = v.split_once('-').unwrap();
                    let a: usize = a.parse().unwrap();
                    let b: usize = b.parse::<usize>().unwrap().min(data.len() - 1);
                    range = Some((a, b));
                }
            }
            hits.fetch_add(1, Ordering::Relaxed);
            let (a, b) = range.unwrap_or((0, data.len() - 1));
            let head = format!(
                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{b}/{}\r\n\r\n",
                b + 1 - a,
                data.len()
            );
            if out.write_all(head.as_bytes()).is_err() || out.write_all(&data[a..=b]).is_err() {
                return;
            }
        }
    }

    #[test]
    fn reads_whole_file_in_chunks() {
        let data: Vec<u8> = (0..(2 * CHUNK + 12345)).map(|i| (i % 251) as u8).collect();
        let (url, hits) = serve(data.clone());
        let mut src = HttpSource::open(agent(), url, data.len() as u64).unwrap();
        let mut got = Vec::new();
        src.read_to_end(&mut got).unwrap();
        assert_eq!(got, data);
        assert_eq!(hits.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn learns_length_and_seeks() {
        let data: Vec<u8> = (0..(CHUNK + 999)).map(|i| (i % 7) as u8).collect();
        let (url, _) = serve(data.clone());
        let mut src = HttpSource::open(agent(), url, 0).unwrap();
        assert_eq!(src.len(), data.len() as u64);
        let mut buf = [0u8; 10];
        src.seek(SeekFrom::Start(CHUNK + 5)).unwrap();
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[(CHUNK + 5) as usize..(CHUNK + 15) as usize]);
        // Short forward skip and a backward seek.
        src.seek(SeekFrom::Current(100)).unwrap();
        src.read_exact(&mut buf).unwrap();
        assert_eq!(
            &buf[..],
            &data[(CHUNK + 115) as usize..(CHUNK + 125) as usize]
        );
        src.seek(SeekFrom::Start(3)).unwrap();
        src.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &data[3..13]);
        src.seek(SeekFrom::End(-4)).unwrap();
        let mut tail = Vec::new();
        src.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &data[data.len() - 4..]);
    }
}
