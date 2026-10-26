//! Stream extraction through the `yt-dlp` CLI, for when rustypipe's web
//! clients fail. yt-dlp tracks YouTube's player changes within days, so it
//! still returns Opus while rustypipe's deobfuscator is broken.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model::{AudioCodec, StreamSource};
use serde::Deserialize;

use crate::rustypipe_backend::{CookieInput, find_bin, parse_cookie_input};

const BIN: &str = "yt-dlp";

/// Premium Opus, else Opus 251. AAC comes from rustypipe's iOS client.
const FORMAT: &str = "774/251";

pub(crate) const TIMEOUT: Duration = Duration::from_secs(15);

/// Longest stderr excerpt kept in an error message.
const MAX_STDERR: usize = 300;

#[derive(Debug, thiserror::Error)]
pub(crate) enum YtDlpError {
    #[error("yt-dlp not found on PATH or in ~/.local/bin")]
    Missing,
    #[error("invalid video id {0:?}")]
    InvalidId(String),
    #[error("yt-dlp timed out after {0:?}")]
    Timeout(Duration),
    #[error("yt-dlp: {0}")]
    Io(#[from] std::io::Error),
    #[error("yt-dlp exited with {status}: {stderr}")]
    Failed { status: String, stderr: String },
    #[error("yt-dlp output: {0}")]
    Parse(String),
}

pub(crate) struct YtDlp {
    bin: PathBuf,
    timeout: Duration,
    /// Where the cookie file for one call goes. `None` disables cookies.
    cookie_dir: Option<PathBuf>,
}

impl YtDlp {
    /// Looks on PATH and in `~/.local/bin` (a desktop launcher's PATH often
    /// lacks it). Cookie files go to `$XDG_RUNTIME_DIR`, a per-user tmpfs.
    pub(crate) fn find() -> Option<Self> {
        let cookie_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir());
        Some(Self::new(find_bin(BIN)?, TIMEOUT, cookie_dir))
    }

    pub(crate) fn new(bin: PathBuf, timeout: Duration, cookie_dir: Option<PathBuf>) -> Self {
        Self {
            bin,
            timeout,
            cookie_dir,
        }
    }

    /// Resolves the Opus stream for `track_id`. `cookies` is a Netscape
    /// cookies.txt body (see [`to_netscape`]); it lets yt-dlp return Premium
    /// itag 774.
    pub(crate) async fn stream(
        &self,
        track_id: &str,
        cookies: Option<&str>,
    ) -> Result<StreamSource, YtDlpError> {
        if !valid_video_id(track_id) {
            return Err(YtDlpError::InvalidId(track_id.to_owned()));
        }
        // Declared before the child so that it drops (and deletes the file)
        // after the child is killed.
        let cookie_file = match (cookies, &self.cookie_dir) {
            (Some(c), Some(dir)) => Some(CookieFile::create(dir, c)?),
            _ => None,
        };

        let mut cmd = tokio::process::Command::new(&self.bin);
        cmd.args(["-f", FORMAT, "-j", "--no-playlist", "--no-warnings"])
            .arg("--ignore-config");
        if let Some(f) = &cookie_file {
            cmd.arg("--cookies").arg(&f.path);
        }
        cmd.arg(format!("https://music.youtube.com/watch?v={track_id}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = cmd.spawn().map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => YtDlpError::Missing,
            _ => YtDlpError::Io(e),
        })?;
        // On timeout the future drops the child, and `kill_on_drop` kills it.
        let out = tokio::time::timeout(self.timeout, child.wait_with_output())
            .await
            .map_err(|_| YtDlpError::Timeout(self.timeout))??;
        drop(cookie_file);

        if !out.status.success() {
            return Err(YtDlpError::Failed {
                status: out.status.to_string(),
                stderr: stderr_excerpt(&out.stderr),
            });
        }
        parse_output(track_id, &out.stdout)
    }
}

/// The fields we read from `yt-dlp -j` for the one selected format.
#[derive(Deserialize)]
struct Output {
    url: String,
    format_id: String,
    acodec: Option<String>,
    vcodec: Option<String>,
    /// Audio bitrate in kbit/s.
    abr: Option<f64>,
    /// Total bitrate in kbit/s.
    tbr: Option<f64>,
    filesize: Option<u64>,
    /// Seconds, rounded by yt-dlp; the URL's `dur=` is more precise.
    duration: Option<f64>,
    #[serde(default)]
    has_drm: bool,
}

pub(crate) fn parse_output(track_id: &str, json: &[u8]) -> Result<StreamSource, YtDlpError> {
    let o: Output = serde_json::from_slice(json).map_err(|e| YtDlpError::Parse(e.to_string()))?;
    if o.vcodec.as_deref().is_some_and(|v| v != "none") {
        return Err(YtDlpError::Parse(format!(
            "format {} has video",
            o.format_id
        )));
    }
    if o.has_drm {
        return Err(YtDlpError::Parse(format!("format {} has DRM", o.format_id)));
    }
    let codec = match o.acodec.as_deref() {
        Some("opus") => AudioCodec::Opus,
        Some(a) if a.starts_with("mp4a") => AudioCodec::Aac,
        other => {
            return Err(YtDlpError::Parse(format!(
                "format {} has audio codec {other:?}",
                o.format_id
            )));
        }
    };
    let content_length = o
        .filesize
        .or_else(|| url_param(&o.url, "clen")?.parse().ok())
        .ok_or_else(|| YtDlpError::Parse("no filesize and no clen= in url".to_owned()))?;
    let duration = url_param(&o.url, "dur")
        .and_then(|d| d.parse::<f64>().ok())
        .or(o.duration)
        .and_then(|s| Duration::try_from_secs_f64(s).ok());
    let kbps = o.abr.or(o.tbr).unwrap_or(0.0);
    Ok(StreamSource {
        track_id: track_id.to_owned(),
        url: o.url,
        codec,
        bitrate_bps: (kbps * 1000.0).round() as u32,
        content_length,
        duration,
        // yt-dlp does not expose YouTube's loudnessDb.
        loudness_db: None,
    })
}

/// Value of query parameter `key` in `url`, undecoded.
pub(crate) fn url_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    let query = query.split_once('#').map_or(query, |(q, _)| q);
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

/// When a googlevideo URL stops working, from its `expire=` unix timestamp.
pub(crate) fn url_expiry(url: &str) -> Option<SystemTime> {
    let secs: u64 = url_param(url, "expire")?.parse().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(secs))
}

fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn stderr_excerpt(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let line = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("");
    match line.char_indices().nth(MAX_STDERR) {
        Some((i, _)) => format!("{}...", &line[..i]),
        None => line.to_owned(),
    }
}

/// Converts the input `set_cookies` accepted into a Netscape cookies.txt body
/// for yt-dlp's `--cookies`.
pub(crate) fn to_netscape(input: &str, now: SystemTime) -> String {
    const MAGIC: &str = "# Netscape HTTP Cookie File\n";
    match parse_cookie_input(input) {
        CookieInput::NetscapeTxt(t) if t.starts_with("# Netscape") || t.starts_with("# HTTP") => {
            format!("{t}\n")
        }
        CookieInput::NetscapeTxt(t) => format!("{MAGIC}{t}\n"),
        CookieInput::Header(h) => {
            // A header carries no expiry. yt-dlp drops expired cookies, so
            // give them one a day out; the file lives only for one call.
            let expires = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() + 86_400;
            let mut out = MAGIC.to_owned();
            for (name, value) in h
                .split(';')
                .filter_map(|c| c.trim().split_once('='))
                .filter(|(n, _)| !n.is_empty())
            {
                out.push_str(&format!(
                    ".youtube.com\tTRUE\t/\tTRUE\t{expires}\t{name}\t{value}\n"
                ));
            }
            out
        }
    }
}

/// A 0600 cookies.txt that is deleted on drop.
struct CookieFile {
    path: PathBuf,
}

impl CookieFile {
    fn create(dir: &Path, body: &str) -> std::io::Result<Self> {
        use std::io::Write;

        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = dir.join(format!(
            "rustle-ytdlp-{}-{}.txt",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        let mut f = opts.open(&path)?;
        // Construct the guard first so a failed write still deletes the file.
        let guard = Self { path };
        f.write_all(body.as_bytes())?;
        Ok(guard)
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/ytdlp_251.json");

    #[test]
    fn parses_recorded_output() {
        let s = parse_output("d7woYjGWs6k", FIXTURE.as_bytes()).unwrap();
        assert_eq!(s.track_id, "d7woYjGWs6k");
        assert_eq!(s.codec, AudioCodec::Opus);
        assert_eq!(url_param(&s.url, "itag"), Some("251"));
        assert_eq!(s.bitrate_bps, 124_379);
        assert_eq!(s.content_length, 5_110_440);
        assert_eq!(s.duration, Some(Duration::from_millis(328_701)));
        assert_eq!(s.loudness_db, None);
    }

    #[test]
    fn falls_back_to_clen_and_duration() {
        let json = serde_json::json!({
            "url": "https://example.invalid/videoplayback?itag=251&clen=1234",
            "format_id": "251", "acodec": "opus", "vcodec": "none",
            "tbr": 130.0, "filesize": null, "duration": 241,
        });
        let s = parse_output("abcdefghijk", json.to_string().as_bytes()).unwrap();
        assert_eq!(s.content_length, 1234);
        assert_eq!(s.bitrate_bps, 130_000);
        assert_eq!(s.duration, Some(Duration::from_secs(241)));
    }

    #[test]
    fn rejects_unusable_output() {
        let base = serde_json::json!({
            "url": "https://example.invalid/videoplayback?itag=251&clen=1234",
            "format_id": "251", "acodec": "opus", "vcodec": "none",
        });
        let with = |k: &str, v: serde_json::Value| {
            let mut j = base.clone();
            j[k] = v;
            j.to_string()
        };
        for bad in [
            with("vcodec", "vp9".into()),
            with("has_drm", true.into()),
            with("acodec", "none".into()),
            with(
                "url",
                "https://example.invalid/videoplayback?itag=251".into(),
            ),
            "not json".to_owned(),
        ] {
            assert!(
                matches!(parse_output("x", bad.as_bytes()), Err(YtDlpError::Parse(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn expire_param() {
        let url = "https://rr1.googlevideo.com/videoplayback?expire=1793059828&ei=x&itag=251";
        assert_eq!(
            url_expiry(url),
            Some(UNIX_EPOCH + Duration::from_secs(1_793_059_828))
        );
        assert_eq!(url_param(url, "ei"), Some("x"));
        // Only whole keys match.
        assert_eq!(
            url_param("https://x/?xexpire=5&expire=7", "expire"),
            Some("7")
        );
        assert_eq!(url_expiry("https://x/?itag=251"), None);
        assert_eq!(url_expiry("https://x/?expire=soon"), None);
        assert_eq!(url_expiry("no query"), None);
    }

    #[test]
    fn video_id_validation() {
        assert!(valid_video_id("d7woYjGWs6k"));
        assert!(valid_video_id("a-b_c-d_e-f"));
        assert!(!valid_video_id("d7woYjGWs6"));
        assert!(!valid_video_id("d7woYjGWs6k&list=PL1"));
        assert!(!valid_video_id("--exec=rm -"));
    }

    #[test]
    fn header_cookies_to_netscape() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let txt = to_netscape("Cookie: SID=abc; HSID=d=ef; ;bad", now);
        assert_eq!(
            txt,
            "# Netscape HTTP Cookie File\n\
             .youtube.com\tTRUE\t/\tTRUE\t87400\tSID\tabc\n\
             .youtube.com\tTRUE\t/\tTRUE\t87400\tHSID\td=ef\n"
        );
        let line = ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc";
        assert_eq!(
            to_netscape(line, now),
            format!("# Netscape HTTP Cookie File\n{line}\n")
        );
        let file = format!("# Netscape HTTP Cookie File\n{line}\n");
        assert_eq!(to_netscape(&file, now), file);
    }

    /// Serializes the tests that write and run fake scripts. Without it, a
    /// parallel test's fork can inherit the script's still-open write handle,
    /// and running the script fails with ETXTBSY ("Text file busy").
    static FAKE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A scratch directory with a fake `yt-dlp` shell script in it. Holds
    /// `FAKE_LOCK` for the test's lifetime.
    struct Fake {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Fake {
        fn new(name: &str, script: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let lock = FAKE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir =
                std::env::temp_dir().join(format!("ytm-core-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("yt-dlp");
            std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir, _lock: lock }
        }

        fn ytdlp(&self, timeout: Duration) -> YtDlp {
            YtDlp::new(self.dir.join("yt-dlp"), timeout, Some(self.dir.clone()))
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn runs_fake_ytdlp() {
        let fake = Fake::new("ok", "cat \"$(dirname \"$0\")/out.json\"");
        std::fs::write(fake.dir.join("out.json"), FIXTURE).unwrap();
        let s = fake
            .ytdlp(TIMEOUT)
            .stream("d7woYjGWs6k", None)
            .await
            .unwrap();
        assert_eq!(s.codec, AudioCodec::Opus);
        assert_eq!(s.content_length, 5_110_440);
    }

    #[tokio::test]
    async fn passes_private_cookie_file_and_deletes_it() {
        // Records the --cookies file's mode, path, and body, then prints the
        // fixture.
        let fake = Fake::new(
            "cookies",
            r#"d="$(dirname "$0")"
while [ $# -gt 0 ]; do
  if [ "$1" = --cookies ]; then
    stat -c %a "$2" > "$d/mode"; echo "$2" > "$d/path"; cp "$2" "$d/body"
  fi
  shift
done
cat "$d/out.json""#,
        );
        std::fs::write(fake.dir.join("out.json"), FIXTURE).unwrap();
        let cookies = to_netscape("SID=abc", SystemTime::now());
        fake.ytdlp(TIMEOUT)
            .stream("d7woYjGWs6k", Some(&cookies))
            .await
            .unwrap();
        let read = |n: &str| std::fs::read_to_string(fake.dir.join(n)).unwrap();
        assert_eq!(read("mode").trim(), "600");
        assert_eq!(read("body"), cookies);
        let path = PathBuf::from(read("path").trim());
        assert!(path.starts_with(&fake.dir));
        assert!(!path.exists(), "cookie file left at {}", path.display());
    }

    #[tokio::test]
    async fn timeout_kills_child_and_deletes_cookies() {
        let fake = Fake::new(
            "sleep",
            r#"d="$(dirname "$0")"
for a in "$@"; do [ -f "$a" ] && echo "$a" > "$d/path"; done
echo $$ > "$d/pid"
exec sleep 30"#,
        );
        let start = Instant::now();
        let err = fake
            .ytdlp(Duration::from_millis(500))
            .stream("d7woYjGWs6k", Some("# Netscape HTTP Cookie File\n"))
            .await
            .unwrap_err();
        assert!(matches!(err, YtDlpError::Timeout(_)), "{err}");
        assert!(start.elapsed() < Duration::from_secs(5));

        let pid = std::fs::read_to_string(fake.dir.join("pid")).unwrap();
        let stat = format!("/proc/{}/stat", pid.trim());
        // Gone, or a zombie until tokio reaps it.
        let dead = || {
            std::fs::read_to_string(&stat).map_or(true, |s| {
                s.rsplit(')')
                    .next()
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z')
            })
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        while !dead() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(dead(), "fake yt-dlp still running");

        let path = std::fs::read_to_string(fake.dir.join("path")).unwrap();
        assert!(!Path::new(path.trim()).exists());
    }

    #[tokio::test]
    async fn failure_keeps_last_stderr_line() {
        let fake = Fake::new(
            "fail",
            "echo 'WARNING: noise' >&2; echo 'ERROR: [youtube] x: Video unavailable' >&2; exit 1",
        );
        match fake.ytdlp(TIMEOUT).stream("d7woYjGWs6k", None).await {
            Err(YtDlpError::Failed { stderr, .. }) => {
                assert_eq!(stderr, "ERROR: [youtube] x: Video unavailable");
            }
            other => panic!("got {other:?}"),
        }
    }

    #[tokio::test]
    async fn missing_binary() {
        let y = YtDlp::new(PathBuf::from("/nonexistent/yt-dlp"), TIMEOUT, None);
        assert!(matches!(
            y.stream("d7woYjGWs6k", None).await,
            Err(YtDlpError::Missing)
        ));
    }
}
