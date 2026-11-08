//! Discord Rich Presence: the playing song as a "Listening to" activity on
//! the user's Discord profile.
//!
//! Discord's desktop app listens on a local Unix socket (`discord-ipc-N`).
//! A small `rustle-discord` thread owns that socket. The UI thread only sends
//! owned [`Activity`] values down a channel, and only when what Discord
//! should show has changed. While paused or stopped, the activity is cleared.
//!
//! When Discord is not running, the thread tries again at most every
//! [`RETRY`], and only while there is a song to show.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use player::PlaybackStatus;
use serde_json::{Value, json};

use crate::state::State;

/// The "Rustle" application in the Discord developer portal. Discord shows
/// its name on the card. Client IDs are public.
const CLIENT_ID: &str = "1569033941820768367";
/// How often to look for Discord again while it is not running.
const RETRY: Duration = Duration::from_secs(15);
/// Discord takes about 5 activity updates per 20 s; this keeps well under.
const MIN_GAP: Duration = Duration::from_secs(2);
/// How long one read or write on the socket may take.
const IO_TIMEOUT: Duration = Duration::from_secs(5);
/// A start time this far from the last one sent counts as a seek.
const JUMP_TOLERANCE_MS: i64 = 1_500;
/// Discord rejects `details`, `state` and `large_text` outside 2 to 128
/// characters, and asset keys or URLs over 256.
const TEXT_MAX: usize = 128;
const URL_MAX: usize = 256;
/// The art Discord shows is at most about 120 px, so any thumbnail at least
/// this big is sharp.
const ART_MIN: u32 = 240;

pub struct Discord {
    /// `None` when presence is off for this run (`--demo`) or the thread
    /// has gone.
    updates: Option<Sender<Option<Activity>>>,
    tracker: Tracker,
}

impl Discord {
    /// `active` false (the `--demo` run) never touches Discord, so fixture
    /// songs do not end up on the user's profile.
    pub fn new(active: bool) -> Self {
        let updates = active.then(|| {
            let (tx, rx) = mpsc::channel();
            thread::Builder::new()
                .name("rustle-discord".into())
                .spawn(move || serve(&rx))
                .map(|_| tx)
                .map_err(|e| eprintln!("rustle: discord presence off: {e}"))
                .ok()
        });
        Self {
            updates: updates.flatten(),
            tracker: Tracker::default(),
        }
    }

    /// Publishes the playing song. Called once per frame; cheap when
    /// nothing changed.
    pub fn sync(&mut self, state: &State) {
        let Some(tx) = &self.updates else {
            return;
        };
        let Some(update) = self.tracker.diff(state, Instant::now(), SystemTime::now()) else {
            return;
        };
        if tx.send(update).is_err() {
            self.updates = None;
        }
    }
}

/// What the card shows: one song, playing since `start_ms`.
#[derive(Debug, Clone, PartialEq)]
struct Activity {
    track_id: String,
    title: String,
    artist: Option<String>,
    album: Option<String>,
    art_url: Option<String>,
    /// Unix time in ms at which the song would have started, had it played
    /// without pauses. Discord draws the progress bar from it.
    start_ms: i64,
    end_ms: Option<i64>,
}

/// What was last sent, to send only what changed.
#[derive(Debug, Default)]
struct Tracker {
    /// `None` until the first `diff`.
    shown: Option<Shown>,
}

#[derive(Debug, Clone, PartialEq)]
enum Shown {
    Nothing,
    Song { track_id: String, start_ms: i64 },
}

impl Tracker {
    /// The new activity to send (`Some(None)` clears it), or `None` when
    /// Discord already shows the right thing.
    fn diff(&mut self, state: &State, now: Instant, wall: SystemTime) -> Option<Option<Activity>> {
        let pb = &state.playback;
        let track = state.current_track().filter(|_| state.settings.discord);
        let want = match (track, pb.status) {
            (Some(track), PlaybackStatus::Playing) => {
                let position = pb.position_at(now);
                let start_ms = unix_ms(wall) - millis(position);
                Some((track, start_ms))
            }
            // A stream that is loading or buffering has no running clock.
            // Keep what Discord shows until it plays or pauses.
            (Some(_), PlaybackStatus::Buffering) if self.shown.is_some() => return None,
            _ => None,
        };
        let next = match &want {
            Some((track, start_ms)) => Shown::Song {
                track_id: track.id.clone(),
                start_ms: *start_ms,
            },
            None => Shown::Nothing,
        };
        let same = match (&self.shown, &next) {
            (Some(Shown::Nothing), Shown::Nothing) => true,
            (
                Some(Shown::Song { track_id, start_ms }),
                Shown::Song {
                    track_id: id,
                    start_ms: start,
                },
            ) => track_id == id && (start_ms - start).abs() <= JUMP_TOLERANCE_MS,
            _ => false,
        };
        if same {
            return None;
        }
        // Nothing was shown before the first sync, so there is nothing to clear.
        let first = self.shown.is_none();
        self.shown = Some(next);
        match want {
            Some((track, start_ms)) => Some(Some(activity(track, start_ms))),
            None if first => None,
            None => Some(None),
        }
    }
}

fn activity(track: &model::Track, start_ms: i64) -> Activity {
    let artists: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    Activity {
        track_id: track.id.clone(),
        title: track.title.clone(),
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        album: track.album.as_ref().map(|a| a.name.clone()),
        art_url: art_url(&track.thumbnails),
        start_ms,
        end_ms: track.duration.map(|d| start_ms + millis(d)),
    }
}

/// The smallest thumbnail at least [`ART_MIN`] on its short side, else the
/// largest. Discord fetches and proxies the URL itself.
fn art_url(thumbs: &[model::Thumbnail]) -> Option<String> {
    let usable = thumbs
        .iter()
        .filter(|t| t.url.starts_with("https://") && t.url.len() <= URL_MAX);
    let big_enough = usable
        .clone()
        .filter(|t| t.width.min(t.height) >= ART_MIN)
        .min_by_key(|t| u64::from(t.width) * u64::from(t.height));
    big_enough
        .or_else(|| usable.max_by_key(|t| u64::from(t.width) * u64::from(t.height)))
        .map(|t| t.url.clone())
}

fn unix_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, millis)
}

fn millis(d: Duration) -> i64 {
    i64::try_from(d.as_millis()).unwrap_or(i64::MAX)
}

/// Text Discord accepts: 2 to 128 characters.
fn fit(text: &str) -> String {
    let mut out: String = if text.chars().count() > TEXT_MAX {
        let mut cut: String = text.chars().take(TEXT_MAX - 1).collect();
        cut.push('\u{2026}');
        cut
    } else {
        text.to_owned()
    };
    // A zero-width space pads one-character titles such as "X".
    while out.chars().count() < 2 {
        out.push('\u{200b}');
    }
    out
}

/// The `activity` argument of `SET_ACTIVITY`.
fn activity_json(a: &Activity) -> Value {
    let watch = format!("https://music.youtube.com/watch?v={}", a.track_id);
    let mut timestamps = json!({ "start": a.start_ms });
    if let Some(end) = a.end_ms {
        timestamps["end"] = json!(end);
    }
    let mut out = json!({
        // 2: "Listening to".
        "type": 2,
        // 2: the member list says "Listening to <title>", not "to Rustle".
        "status_display_type": 2,
        "details": fit(&a.title),
        "details_url": watch,
        "timestamps": timestamps,
        "buttons": [{ "label": "Play on YouTube Music", "url": watch }],
    });
    if let Some(artist) = &a.artist {
        out["state"] = json!(fit(artist));
    }
    let mut assets = serde_json::Map::new();
    if let Some(url) = &a.art_url {
        assets.insert("large_image".into(), json!(url));
    }
    if let Some(album) = &a.album {
        assets.insert("large_text".into(), json!(fit(album)));
    }
    if !assets.is_empty() {
        out["assets"] = Value::Object(assets);
    }
    out
}

/// Runs on the `rustle-discord` thread until `Discord` is dropped.
fn serve(updates: &Receiver<Option<Activity>>) {
    let mut conn: Option<Ipc> = None;
    let mut want: Option<Activity> = None;
    // Whether Discord shows `want` already.
    let mut done = true;
    let mut sent_at: Option<Instant> = None;
    loop {
        // Wait without a timeout unless a song is waiting for Discord.
        let next = if done {
            updates.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            updates.recv_timeout(RETRY)
        };
        match next {
            Ok(first) => want = first,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Skipping through tracks sends only where it lands: wait out the
        // gap since the last send, keeping only the newest update.
        if let Some(at) = sent_at {
            let until = at + MIN_GAP;
            while let Some(left) = until.checked_duration_since(Instant::now()) {
                match updates.recv_timeout(left) {
                    Ok(newer) => want = newer,
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }
        want = updates.try_iter().last().unwrap_or(want);
        // With nothing to show and no connection, Discord shows nothing.
        if want.is_none() && conn.is_none() {
            done = true;
            continue;
        }
        done = publish(&mut conn, want.as_ref());
        if done {
            sent_at = Some(Instant::now());
        }
    }
}

/// Sends `activity` on `conn`, connecting first if needed and once more if
/// the connection has gone stale (Discord restarted). True when sent.
fn publish(conn: &mut Option<Ipc>, activity: Option<&Activity>) -> bool {
    for _ in 0..2 {
        if conn.is_none() {
            *conn = Ipc::connect().ok();
        }
        let Some(c) = conn else {
            return false;
        };
        match c.set_activity(activity) {
            Ok(()) => return true,
            Err(_) => *conn = None,
        }
    }
    false
}

/// One connection to Discord's IPC socket. Frames are a little-endian u32
/// opcode, a little-endian u32 length, then that many bytes of JSON.
struct Ipc {
    stream: UnixStream,
    nonce: u64,
}

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;
/// A reply larger than this is not from Discord.
const FRAME_MAX: usize = 64 * 1024;

impl Ipc {
    fn connect() -> io::Result<Self> {
        let mut last = io::Error::from(io::ErrorKind::NotFound);
        for path in socket_paths() {
            match UnixStream::connect(&path) {
                Ok(stream) => match Self::handshake(stream) {
                    Ok(ipc) => return Ok(ipc),
                    Err(e) => last = e,
                },
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn handshake(stream: UnixStream) -> io::Result<Self> {
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let mut ipc = Self { stream, nonce: 0 };
        ipc.write(OP_HANDSHAKE, &json!({ "v": 1, "client_id": CLIENT_ID }))?;
        // Discord answers with a READY dispatch, or closes with an error.
        let reply = ipc.read_reply()?;
        if reply["evt"] == "READY" {
            Ok(ipc)
        } else {
            Err(io::Error::other(format!("discord handshake: {reply}")))
        }
    }

    fn set_activity(&mut self, activity: Option<&Activity>) -> io::Result<()> {
        self.nonce += 1;
        let payload = json!({
            "cmd": "SET_ACTIVITY",
            "args": {
                "pid": std::process::id(),
                "activity": activity.map(activity_json),
            },
            "nonce": self.nonce.to_string(),
        });
        self.write(OP_FRAME, &payload)?;
        let reply = self.read_reply()?;
        if reply["evt"] == "ERROR" {
            // Discord refused this activity; the connection still works.
            eprintln!("rustle: discord refused the activity: {}", reply["data"]);
        }
        Ok(())
    }

    fn write(&mut self, op: u32, payload: &Value) -> io::Result<()> {
        let body = payload.to_string();
        let len = u32::try_from(body.len()).map_err(io::Error::other)?;
        let mut frame = Vec::with_capacity(8 + body.len());
        frame.extend_from_slice(&op.to_le_bytes());
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(body.as_bytes());
        self.stream.write_all(&frame)
    }

    /// The next frame's JSON, answering pings on the way.
    fn read_reply(&mut self) -> io::Result<Value> {
        loop {
            let mut header = [0u8; 8];
            self.stream.read_exact(&mut header)?;
            let [o0, o1, o2, o3, l0, l1, l2, l3] = header;
            let op = u32::from_le_bytes([o0, o1, o2, o3]);
            let len = u32::from_le_bytes([l0, l1, l2, l3]) as usize;
            if len > FRAME_MAX {
                return Err(io::Error::other("discord frame too large"));
            }
            let mut body = vec![0u8; len];
            self.stream.read_exact(&mut body)?;
            let value: Value = serde_json::from_slice(&body).map_err(io::Error::other)?;
            match op {
                OP_PING => self.write(OP_PONG, &value)?,
                OP_CLOSE => {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        format!("discord closed: {value}"),
                    ));
                }
                _ => return Ok(value),
            }
        }
    }
}

/// Where Discord's socket can be: the runtime or temp dir, directly or in
/// the Flatpak or Snap sandbox's own dir.
fn socket_paths() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .collect();
    roots.push(PathBuf::from("/tmp"));
    roots.dedup();
    let sandboxes = [
        "",
        "app/com.discordapp.Discord",
        "app/com.discordapp.DiscordCanary",
        "app/dev.vencord.Vesktop",
        "snap.discord",
    ];
    let mut out = Vec::new();
    for root in &roots {
        for sandbox in sandboxes {
            for i in 0..10 {
                out.push(root.join(sandbox).join(format!("discord-ipc-{i}")));
            }
        }
    }
    out.retain(|p| p.exists());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::track;
    use crate::state::tests::started;
    use crate::state::{Command, apply};
    use std::sync::Arc;

    const WALL_SECS: u64 = 1_800_000_000;

    fn wall(t0: Instant, now: Instant) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(WALL_SECS) + now.duration_since(t0)
    }

    fn playing(ids: &[&str], now: Instant) -> State {
        let mut s = State::default();
        let tracks = Arc::new(ids.iter().map(|id| track(id)).collect());
        apply(
            &mut s,
            &Command::PlayTracks {
                tracks,
                start: 0,
                seed: 1,
            },
            now,
        );
        started(&mut s, now);
        s
    }

    fn diff(t: &mut Tracker, s: &State, t0: Instant, now: Instant) -> Option<Option<Activity>> {
        t.diff(s, now, wall(t0, now))
    }

    fn primed(s: &State, t0: Instant) -> Tracker {
        let mut t = Tracker::default();
        assert!(diff(&mut t, s, t0, t0).is_some());
        t
    }

    #[test]
    fn first_sync_shows_the_song_with_its_progress() {
        let t0 = Instant::now();
        let s = playing(&["a"], t0);
        let Some(Some(a)) = diff(&mut Tracker::default(), &s, t0, t0) else {
            panic!("expected an activity");
        };
        let start = i64::try_from(WALL_SECS).unwrap() * 1000;
        assert_eq!(a.title, "A");
        assert_eq!(a.start_ms, start);
        assert_eq!(a.end_ms, Some(start + 100_000));
    }

    #[test]
    fn steady_playback_sends_nothing() {
        let t0 = Instant::now();
        let s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        for secs in [1, 30, 90] {
            assert_eq!(diff(&mut t, &s, t0, t0 + Duration::from_secs(secs)), None);
        }
    }

    #[test]
    fn pause_clears_and_resume_shows_again() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        apply(&mut s, &Command::TogglePlay, t1);
        assert_eq!(diff(&mut t, &s, t0, t1), Some(None));
        assert_eq!(diff(&mut t, &s, t0, t1 + Duration::from_secs(5)), None);
        // Resumed 20 s later at 0:10: the song "started" 30 s after t0.
        let t2 = t1 + Duration::from_secs(20);
        apply(&mut s, &Command::TogglePlay, t2);
        let Some(Some(a)) = diff(&mut t, &s, t0, t2) else {
            panic!("expected an activity");
        };
        let start = i64::try_from(WALL_SECS).unwrap() * 1000 + 20_000;
        assert_eq!(a.start_ms, start);
    }

    #[test]
    fn seek_moves_the_start() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(5);
        apply(&mut s, &Command::Seek(Duration::from_secs(60)), t1);
        let Some(Some(a)) = diff(&mut t, &s, t0, t1) else {
            panic!("expected an activity");
        };
        let start = i64::try_from(WALL_SECS).unwrap() * 1000 + 5_000 - 60_000;
        assert_eq!(a.start_ms, start);
    }

    #[test]
    fn a_loading_track_keeps_the_card_until_it_plays() {
        let t0 = Instant::now();
        let mut s = playing(&["a", "b"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        apply(&mut s, &Command::Next, t1);
        assert_eq!(diff(&mut t, &s, t0, t1), None);
        assert_eq!(diff(&mut t, &s, t0, t1 + Duration::from_secs(3)), None);
        let t2 = t1 + Duration::from_secs(3);
        started(&mut s, t2);
        let Some(Some(a)) = diff(&mut t, &s, t0, t2) else {
            panic!("expected an activity");
        };
        assert_eq!(a.title, "B");
    }

    #[test]
    fn switching_the_setting_off_clears() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        s.settings.discord = false;
        assert_eq!(diff(&mut t, &s, t0, t0), Some(None));
        assert_eq!(diff(&mut t, &s, t0, t0), None);
    }

    #[test]
    fn nothing_playing_at_start_sends_nothing() {
        let t0 = Instant::now();
        let s = State::default();
        assert_eq!(diff(&mut Tracker::default(), &s, t0, t0), None);
    }

    #[test]
    fn fit_pads_and_truncates() {
        assert_eq!(fit("X").chars().count(), 2);
        assert_eq!(fit("Wayside"), "Wayside");
        let long = "a".repeat(300);
        let out = fit(&long);
        assert_eq!(out.chars().count(), TEXT_MAX);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn art_prefers_the_smallest_sharp_thumbnail() {
        let thumb = |w, h| model::Thumbnail {
            url: format!("https://i.ytimg.com/{w}x{h}.jpg"),
            width: w,
            height: h,
        };
        let thumbs = [
            thumb(60, 60),
            thumb(226, 226),
            thumb(544, 544),
            thumb(1200, 1200),
        ];
        assert_eq!(
            art_url(&thumbs).as_deref(),
            Some("https://i.ytimg.com/544x544.jpg")
        );
        assert_eq!(
            art_url(&thumbs[..2]).as_deref(),
            Some("https://i.ytimg.com/226x226.jpg")
        );
        assert_eq!(art_url(&[]), None);
    }

    #[test]
    fn activity_json_shape() {
        let a = Activity {
            track_id: "abc".into(),
            title: "Wayside".into(),
            artist: Some("Ninajirachi".into()),
            album: None,
            art_url: Some("https://i.ytimg.com/a.jpg".into()),
            start_ms: 1000,
            end_ms: Some(136_000),
        };
        let v = activity_json(&a);
        assert_eq!(v["type"], 2);
        assert_eq!(v["details"], "Wayside");
        assert_eq!(v["state"], "Ninajirachi");
        assert_eq!(v["timestamps"]["end"], 136_000);
        assert_eq!(v["assets"]["large_image"], "https://i.ytimg.com/a.jpg");
        assert!(v["assets"].get("large_text").is_none());
        assert_eq!(
            v["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=abc"
        );
    }
}
