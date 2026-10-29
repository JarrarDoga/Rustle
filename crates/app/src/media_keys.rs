//! MPRIS media keys and now-playing info, through souvlaki's libdbus backend.
//!
//! souvlaki's `attach` opens the session bus and requests the name
//! synchronously, and dropping its controls joins its service thread, so a
//! small `rustle-mpris` thread owns them. The UI thread only sends owned
//! [`Update`]s down a channel and drains key presses from another.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use dbus::blocking::{BlockingSender, Connection};
use player::PlaybackStatus;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::state::{Command, Playback, State};

/// A seek key without an amount moves this far.
const SEEK_STEP: Duration = Duration::from_secs(5);
/// A position this far from where the last publish predicts counts as a seek.
/// `sync_position` nudges the shown position by less than this.
const JUMP_TOLERANCE: Duration = Duration::from_secs(1);
const BUS_NAME: &str = "org.mpris.MediaPlayer2.rustle";

pub struct MediaKeys {
    ctx: egui::Context,
    events: Receiver<MediaControlEvent>,
    /// `None` once the MPRIS thread has gone, e.g. no session bus.
    updates: Option<Sender<Update>>,
    tracker: Tracker,
    /// Playback and duration as of the last `sync`, to turn relative seeks
    /// and play/pause into commands.
    last: Playback,
    duration: Option<Duration>,
}

impl MediaKeys {
    /// `ctx` lets a key press wake the UI with `request_repaint`.
    pub fn new(ctx: egui::Context) -> Self {
        let (event_tx, events) = mpsc::channel();
        let (updates, update_rx) = mpsc::channel();
        let wake = ctx.clone();
        let spawned = thread::Builder::new()
            .name("rustle-mpris".into())
            .spawn(move || serve(event_tx, update_rx, wake));
        let updates = match spawned {
            Ok(_) => Some(updates),
            Err(e) => {
                eprintln!("rustle: media keys off: {e}");
                None
            }
        };
        Self {
            ctx,
            events,
            updates,
            tracker: Tracker::default(),
            last: Playback::default(),
            duration: None,
        }
    }

    /// Commands from media keys since the last call. Called once per frame.
    pub fn poll(&mut self) -> Vec<Command> {
        let mut out = Vec::new();
        let mut playing = self.last.is_playing();
        while let Ok(event) = self.events.try_recv() {
            let now = Instant::now();
            let command = match event {
                MediaControlEvent::Toggle => {
                    playing = !playing;
                    Some(Command::TogglePlay)
                }
                MediaControlEvent::Play if !playing => {
                    playing = true;
                    Some(Command::TogglePlay)
                }
                MediaControlEvent::Pause | MediaControlEvent::Stop if playing => {
                    playing = false;
                    Some(Command::TogglePlay)
                }
                MediaControlEvent::Next => Some(Command::Next),
                MediaControlEvent::Previous => Some(Command::Previous),
                MediaControlEvent::Seek(dir) => Some(self.seek_by(dir, SEEK_STEP, now)),
                MediaControlEvent::SeekBy(dir, by) => Some(self.seek_by(dir, by, now)),
                MediaControlEvent::SetPosition(MediaPosition(to)) => Some(Command::Seek(to)),
                MediaControlEvent::SetVolume(v) => Some(Command::SetVolume(v as f32)),
                MediaControlEvent::Raise => {
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    None
                }
                MediaControlEvent::Quit => {
                    self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    None
                }
                _ => None,
            };
            out.extend(command);
        }
        out
    }

    fn seek_by(&self, dir: SeekDirection, by: Duration, now: Instant) -> Command {
        let at = self.last.position_at(now);
        let to = match dir {
            SeekDirection::Forward => at + by,
            SeekDirection::Backward => at.saturating_sub(by),
        };
        Command::Seek(self.duration.map_or(to, |d| to.min(d)))
    }

    /// Publish the current track and position. Called once per frame; must be
    /// cheap when nothing changed.
    pub fn sync(&mut self, state: &State) {
        self.last = state.playback.clone();
        self.duration = state.duration();
        let Some(tx) = &self.updates else {
            return;
        };
        for update in self.tracker.diff(state, Instant::now()) {
            if tx.send(update).is_err() {
                // The thread has logged why it stopped.
                self.updates = None;
                return;
            }
        }
    }
}

/// Runs on the `rustle-mpris` thread until `MediaKeys` is dropped.
fn serve(events: Sender<MediaControlEvent>, updates: Receiver<Update>, ctx: egui::Context) {
    let config = PlatformConfig {
        dbus_name: "rustle",
        display_name: "Rustle",
        hwnd: None,
    };
    let attached = MediaControls::new(config).and_then(|mut controls| {
        // Runs on souvlaki's thread; nothing here may panic (panic = "abort").
        controls.attach(move |event| {
            events.send(event).ok();
            ctx.request_repaint();
        })?;
        Ok(controls)
    });
    let mut controls = match attached {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rustle: media keys off: {e}");
            return;
        }
    };
    // souvlaki's service loop takes one update per wake-up, and a wake-up can
    // wait a second for D-Bus traffic, so send only the newest of each kind
    // and ping the service after each one to wake it at once.
    let waker = Connection::new_session().ok();
    while let Ok(first) = updates.recv() {
        for update in coalesce(first, updates.try_iter()) {
            let result = match update {
                Update::Metadata(m) => controls.set_metadata(MediaMetadata {
                    title: m.title.as_deref(),
                    album: m.album.as_deref(),
                    artist: m.artist.as_deref(),
                    cover_url: m.cover_url.as_deref(),
                    duration: m.duration,
                }),
                Update::Playback(p) => controls.set_playback(p),
                Update::Volume(v) => controls.set_volume(v),
            };
            if let Err(e) = result {
                eprintln!("rustle: media keys off: {e}");
                return;
            }
            if let Some(conn) = &waker {
                wake(conn);
            }
        }
    }
}

/// Sends souvlaki's service a call so its `process` returns and it drains
/// the update just queued. The reply (an unknown-interface error) is read
/// and dropped so nothing piles up on this connection.
fn wake(conn: &Connection) {
    let path = "/org/mpris/MediaPlayer2";
    if let Ok(ping) =
        dbus::Message::new_method_call(BUS_NAME, path, "org.freedesktop.DBus.Peer", "Ping")
    {
        conn.send_with_reply_and_block(ping, Duration::from_millis(200))
            .ok();
    }
}

/// The newest update of each kind, metadata first.
fn coalesce(first: Update, rest: impl Iterator<Item = Update>) -> Vec<Update> {
    let mut slots: [Option<Update>; 3] = [None, None, None];
    for update in std::iter::once(first).chain(rest) {
        let slot = match update {
            Update::Metadata(_) => 0,
            Update::Playback(_) => 1,
            Update::Volume(_) => 2,
        };
        slots[slot] = Some(update);
    }
    slots.into_iter().flatten().collect()
}

/// One change to publish over MPRIS.
#[derive(Debug, Clone, PartialEq)]
enum Update {
    Metadata(Metadata),
    Playback(MediaPlayback),
    Volume(f64),
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Metadata {
    title: Option<String>,
    album: Option<String>,
    artist: Option<String>,
    cover_url: Option<String>,
    duration: Option<Duration>,
}

fn metadata(state: &State) -> Metadata {
    let Some(track) = state.current_track() else {
        return Metadata::default();
    };
    let artists: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    Metadata {
        title: Some(track.title.clone()),
        album: track.album.as_ref().map(|a| a.name.clone()),
        artist: (!artists.is_empty()).then(|| artists.join(", ")),
        cover_url: track
            .thumbnails
            .iter()
            .max_by_key(|t| u64::from(t.width) * u64::from(t.height))
            .map(|t| t.url.clone()),
        duration: track.duration,
    }
}

/// MPRIS knows no buffering; it counts as playing. A track that is still
/// loading counts as paused: MPRIS clients run their own clock while
/// playing, and it would start before the audio does.
fn mpris_status(pb: &Playback) -> PlaybackStatus {
    match pb.status {
        PlaybackStatus::Buffering if pb.loading => PlaybackStatus::Paused,
        PlaybackStatus::Buffering => PlaybackStatus::Playing,
        s => s,
    }
}

fn playback(pb: &Playback, now: Instant) -> MediaPlayback {
    let progress = Some(MediaPosition(pb.position_at(now)));
    match mpris_status(pb) {
        PlaybackStatus::Playing => MediaPlayback::Playing { progress },
        PlaybackStatus::Paused => MediaPlayback::Paused { progress },
        _ => MediaPlayback::Stopped,
    }
}

/// What was last published, to send only what changed.
#[derive(Debug, Default)]
struct Tracker {
    /// `None` until the first `diff`; then the current track id, if any.
    track: Option<Option<String>>,
    playback: Option<Playback>,
    volume: Option<f32>,
}

impl Tracker {
    fn diff(&mut self, state: &State, now: Instant) -> Vec<Update> {
        let mut out = Vec::new();
        let id = state.current_track().map(|t| t.id.as_str());
        let track_changed = self.track.as_ref().is_none_or(|t| t.as_deref() != id);
        if track_changed {
            self.track = Some(id.map(str::to_owned));
            out.push(Update::Metadata(metadata(state)));
        }

        let pb = &state.playback;
        let playback_changed = track_changed
            || self.playback.as_ref().is_none_or(|old| {
                mpris_status(old) != mpris_status(pb)
                    || old.position_at(now).abs_diff(pb.position_at(now)) > JUMP_TOLERANCE
            });
        if playback_changed {
            self.playback = Some(pb.clone());
            out.push(Update::Playback(playback(pb, now)));
        }

        if self.volume.is_none_or(|v| (v - pb.volume).abs() > 0.001) {
            self.volume = Some(pb.volume);
            out.push(Update::Volume(f64::from(pb.volume)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::track;
    use crate::state::apply;
    use crate::state::tests::started;
    use std::sync::Arc;

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

    fn primed(s: &State, now: Instant) -> Tracker {
        let mut t = Tracker::default();
        assert_eq!(t.diff(s, now).len(), 3, "first sync publishes everything");
        t
    }

    #[test]
    fn nothing_changed_publishes_nothing() {
        let t0 = Instant::now();
        let s = playing(&["a", "b"], t0);
        let mut t = primed(&s, t0);
        for secs in [0, 1, 30, 90] {
            assert!(t.diff(&s, t0 + Duration::from_secs(secs)).is_empty());
        }
    }

    #[test]
    fn first_sync_publishes_track_metadata() {
        let t0 = Instant::now();
        let s = playing(&["a"], t0);
        let out = Tracker::default().diff(&s, t0);
        let Update::Metadata(m) = &out[0] else {
            panic!("expected metadata first, got {out:?}");
        };
        assert_eq!(m.title.as_deref(), Some("A"));
        assert_eq!(m.duration, Some(Duration::from_secs(100)));
        assert!(matches!(
            out[1],
            Update::Playback(MediaPlayback::Playing { .. })
        ));
    }

    #[test]
    fn track_change_publishes_metadata_and_playback() {
        let t0 = Instant::now();
        let mut s = playing(&["a", "b"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        apply(&mut s, &Command::Next, t1);
        let out = t.diff(&s, t1);
        assert_eq!(out.len(), 2, "{out:?}");
        assert!(matches!(&out[0], Update::Metadata(m) if m.title.as_deref() == Some("B")));
        assert!(matches!(out[1], Update::Playback(_)));
        assert!(t.diff(&s, t1).is_empty());
    }

    #[test]
    fn status_change_publishes_playback_only() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        apply(&mut s, &Command::TogglePlay, t1);
        let out = t.diff(&s, t1);
        let progress = Some(MediaPosition(Duration::from_secs(10)));
        assert_eq!(out, [Update::Playback(MediaPlayback::Paused { progress })]);
        assert!(t.diff(&s, t1 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn buffering_counts_as_playing() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        s.playback.status = PlaybackStatus::Buffering;
        assert!(t.diff(&s, t0).is_empty());
    }

    #[test]
    fn a_loading_track_counts_as_paused_until_it_starts() {
        let t0 = Instant::now();
        let mut s = playing(&["a", "b"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        apply(&mut s, &Command::Next, t1);
        let at_start = Some(MediaPosition(Duration::ZERO));
        let out = t.diff(&s, t1);
        assert_eq!(
            out[1],
            Update::Playback(MediaPlayback::Paused { progress: at_start })
        );
        // MPRIS clients get no clock to run while the stream loads.
        let t2 = t1 + Duration::from_secs(2);
        assert!(t.diff(&s, t2).is_empty());
        started(&mut s, t2);
        assert_eq!(
            t.diff(&s, t2),
            [Update::Playback(MediaPlayback::Playing {
                progress: at_start
            })]
        );
    }

    #[test]
    fn seek_publishes_new_position() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(5);
        apply(&mut s, &Command::Seek(Duration::from_secs(60)), t1);
        let progress = Some(MediaPosition(Duration::from_secs(60)));
        assert_eq!(
            t.diff(&s, t1),
            [Update::Playback(MediaPlayback::Playing { progress })]
        );
    }

    #[test]
    fn small_position_corrections_are_not_seeks() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        let t1 = t0 + Duration::from_secs(10);
        // The player reports slightly behind the UI clock.
        s.playback.position = Duration::from_millis(9_700);
        s.playback.anchor = Some(t1);
        assert!(t.diff(&s, t1).is_empty());
    }

    #[test]
    fn volume_change_publishes_volume() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        apply(&mut s, &Command::SetVolume(0.25), t0);
        assert_eq!(t.diff(&s, t0), [Update::Volume(0.25)]);
    }

    #[test]
    fn queue_end_publishes_stopped() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], t0);
        let mut t = primed(&s, t0);
        apply(&mut s, &Command::Next, t0);
        assert_eq!(t.diff(&s, t0), [Update::Playback(MediaPlayback::Stopped)]);
    }

    #[test]
    fn coalesce_keeps_newest_of_each_kind() {
        let stopped = Update::Playback(MediaPlayback::Stopped);
        let paused = Update::Playback(MediaPlayback::Paused { progress: None });
        let out = coalesce(
            stopped,
            [Update::Volume(0.1), paused.clone(), Update::Volume(0.2)].into_iter(),
        );
        assert_eq!(out, [paused, Update::Volume(0.2)]);
    }
}
