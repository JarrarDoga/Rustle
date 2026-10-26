//! App state and the reducer that changes it.
//!
//! [`apply`] is pure and runs twice for every command: once on the UI thread,
//! right away, so a control shows its result the moment it is used; and once
//! in the backend task, which also carries out the returned [`Effect`]s. Each
//! snapshot the backend publishes says which command it has seen last
//! (`acked`), and the UI re-applies the commands that came after it, so a
//! lagging snapshot never undoes a click.

use std::sync::Arc;
use std::time::{Duration, Instant};

use model::{Playlist, PlaylistSummary, SearchResults, StreamSource, Track};
use player::{PlaybackStatus, PlayerState};
use ytm_core::BackendError;

use crate::queue::{Queue, Repeat};

/// Previous restarts the track instead when it has played longer than this.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);

pub const SESSION_EXPIRED: &str = "Session expired, paste new cookies";
pub const COOKIES_REJECTED: &str = "those cookies were rejected";

#[derive(Debug, Clone, Default)]
pub enum Loadable<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Loadable<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Loadable::Ready(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Playback {
    pub status: PlaybackStatus,
    /// Position at `anchor` (or now, when paused or stopped).
    pub position: Duration,
    /// When `status` is `Playing`: the instant `position` was taken.
    pub anchor: Option<Instant>,
    /// 0.0 to 1.0.
    pub volume: f32,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            position: Duration::ZERO,
            anchor: None,
            volume: 0.8,
        }
    }
}

impl Playback {
    /// Where playback is at `now`, extrapolated while playing.
    pub fn position_at(&self, now: Instant) -> Duration {
        match self.anchor {
            Some(at) => self.position + now.saturating_duration_since(at),
            None => self.position,
        }
    }

    pub fn is_playing(&self) -> bool {
        matches!(
            self.status,
            PlaybackStatus::Playing | PlaybackStatus::Buffering
        )
    }

    fn start_track(&mut self, now: Instant, playing: bool) {
        self.position = Duration::ZERO;
        if playing {
            self.status = PlaybackStatus::Playing;
            self.anchor = Some(now);
        } else {
            self.status = PlaybackStatus::Paused;
            self.anchor = None;
        }
    }

    fn stop(&mut self) {
        self.status = PlaybackStatus::Stopped;
        self.position = Duration::ZERO;
        self.anchor = None;
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Sequence number of the last UI command this state includes.
    pub acked: u64,
    pub library: Loadable<Arc<Vec<PlaylistSummary>>>,
    pub liked: Loadable<Arc<Vec<Arc<Track>>>>,
    pub search_query: String,
    pub search: Loadable<Arc<SearchView>>,
    /// The playlist page that is open, by id.
    pub playlist: Option<(String, Loadable<Arc<PlaylistView>>)>,
    pub queue: Queue,
    pub playback: Playback,
    /// One message at the top of the window, e.g. "session expired".
    pub banner: Option<String>,
    pub session: Session,
}

/// Login state. Demo mode starts `LoggedIn`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Session {
    /// Checking the keyring for saved cookies at startup.
    #[default]
    Starting,
    LoggedOut,
    /// Cookies sent to the backend, waiting for `set_cookies`.
    LoggingIn,
    LoggedIn,
    /// The last login attempt failed; the message is shown on the login screen.
    Failed(String),
}

/// Search results with tracks behind `Arc`s, so playing from them is cheap.
#[derive(Debug, Clone, Default)]
pub struct SearchView {
    pub tracks: Arc<Vec<Arc<Track>>>,
    pub results: SearchResults,
}

#[derive(Debug, Clone)]
pub struct PlaylistView {
    pub summary: PlaylistSummary,
    pub tracks: Arc<Vec<Arc<Track>>>,
}

impl State {
    pub fn current_track(&self) -> Option<&Arc<Track>> {
        self.queue.current().map(|e| &e.track)
    }

    pub fn duration(&self) -> Option<Duration> {
        self.current_track().and_then(|t| t.duration)
    }
}

/// What the UI asks for.
#[derive(Debug, Clone)]
pub enum Command {
    /// Fetch the library playlists and liked songs.
    Refresh,
    Search(String),
    OpenPlaylist(String),
    /// Replace the queue with `tracks` and play `tracks[start]`.
    PlayTracks {
        tracks: Arc<Vec<Arc<Track>>>,
        start: usize,
        seed: u64,
    },
    PlayNow(Arc<Track>),
    PlayNext(Arc<Track>),
    AddToQueue(Arc<Track>),
    /// Play the queue entry at this index.
    JumpTo(usize),
    MoveEntry {
        from: usize,
        to: usize,
    },
    RemoveEntry(usize),
    TogglePlay,
    Next,
    Previous,
    Seek(Duration),
    SetVolume(f32),
    SetShuffle {
        on: bool,
        seed: u64,
    },
    CycleRepeat,
    DismissBanner,
    /// Log in with a pasted Cookie header or cookies.txt body.
    #[allow(dead_code)] // sent by the batch 2 login screen
    Login(String),
    #[allow(dead_code)] // sent by the batch 2 login screen
    Logout,
}

/// Work the backend does after a reducer step. The UI ignores these.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    FetchLibrary,
    FetchLiked,
    Search(String),
    FetchPlaylist(String),
    /// Resolve the stream for queue entry `uid` and load it into the player.
    Load {
        uid: u64,
        track_id: String,
    },
    Play,
    Pause,
    Stop,
    Seek(Duration),
    Volume(f32),
    /// Validate cookies with `MusicBackend::set_cookies`, then save them.
    Login(String),
    /// Forget the saved cookies.
    Logout,
}

/// What the backend learns from YouTube Music and the player.
#[derive(Debug)]
pub enum Event {
    Library(Result<Vec<PlaylistSummary>, BackendError>),
    Liked(Result<Vec<Track>, BackendError>),
    Search {
        query: String,
        result: Result<SearchResults, BackendError>,
    },
    Playlist {
        id: String,
        result: Result<Playlist, BackendError>,
    },
    StreamReady {
        uid: u64,
        source: StreamSource,
    },
    StreamFailed {
        uid: u64,
        error: BackendError,
    },
    TrackEnded {
        track_id: String,
    },
    PlayerError(String),
    /// The keyring holds cookies. The session counts as logged in at once so
    /// the cached library shows; `Login { saved: true, .. }` follows.
    SavedLogin,
    /// The keyring holds no cookies.
    NoSavedLogin,
    /// Cookies were checked: the saved ones (`saved`) or pasted ones.
    Login {
        saved: bool,
        result: Result<(), BackendError>,
    },
    /// The backend could not start; shown on the login screen.
    StartFailed(String),
    /// A message for the banner.
    Notice(String),
    /// Read from the local store at startup, before the network answers.
    CachedLibrary(Vec<PlaylistSummary>),
    CachedLiked(Vec<Track>),
    CachedQueue {
        tracks: Vec<Track>,
        current: Option<usize>,
    },
    CachedPlaylist(Playlist),
}

fn load_current(state: &mut State, now: Instant, playing: bool) -> Vec<Effect> {
    match state.queue.current() {
        Some(e) => {
            let effect = Effect::Load {
                uid: e.uid,
                track_id: e.track.id.clone(),
            };
            state.playback.start_track(now, playing);
            vec![effect]
        }
        None => {
            state.playback.stop();
            vec![Effect::Stop]
        }
    }
}

pub fn apply(state: &mut State, cmd: &Command, now: Instant) -> Vec<Effect> {
    let pb = &mut state.playback;
    match cmd {
        Command::Refresh => refresh(state),
        Command::Search(query) => {
            let query = query.trim();
            state.search_query = query.to_owned();
            if query.is_empty() {
                state.search = Loadable::Idle;
                return Vec::new();
            }
            state.search = Loadable::Loading;
            vec![Effect::Search(query.to_owned())]
        }
        Command::OpenPlaylist(id) => {
            state.playlist = Some((id.clone(), Loadable::Loading));
            vec![Effect::FetchPlaylist(id.clone())]
        }
        Command::PlayTracks {
            tracks,
            start,
            seed,
        } => {
            state.queue.play_list(tracks, *start, *seed);
            load_current(state, now, true)
        }
        Command::PlayNow(track) => {
            state.queue.play_now(Arc::clone(track));
            load_current(state, now, true)
        }
        Command::PlayNext(track) => {
            state.queue.play_next(Arc::clone(track));
            Vec::new()
        }
        Command::AddToQueue(track) => {
            state.queue.add(Arc::clone(track));
            Vec::new()
        }
        Command::JumpTo(index) => {
            if state.queue.jump(*index).is_none() {
                return Vec::new();
            }
            load_current(state, now, true)
        }
        Command::MoveEntry { from, to } => {
            state.queue.move_entry(*from, *to);
            Vec::new()
        }
        Command::RemoveEntry(index) => {
            if !state.queue.remove(*index) {
                return Vec::new();
            }
            if state.queue.current().is_some() && state.playback.status != PlaybackStatus::Stopped {
                let playing = state.playback.is_playing();
                load_current(state, now, playing)
            } else {
                state.playback.stop();
                vec![Effect::Stop]
            }
        }
        Command::TogglePlay => match pb.status {
            PlaybackStatus::Playing | PlaybackStatus::Buffering => {
                pb.position = pb.position_at(now);
                pb.anchor = None;
                pb.status = PlaybackStatus::Paused;
                vec![Effect::Pause]
            }
            PlaybackStatus::Paused => {
                pb.anchor = Some(now);
                pb.status = PlaybackStatus::Playing;
                vec![Effect::Play]
            }
            PlaybackStatus::Stopped => {
                if state.queue.current().is_none() && state.queue.advance(false).is_none() {
                    return Vec::new();
                }
                load_current(state, now, true)
            }
        },
        Command::Next => {
            let playing = pb.status != PlaybackStatus::Paused;
            if state.queue.advance(false).is_some() {
                load_current(state, now, playing)
            } else {
                state.playback.stop();
                vec![Effect::Stop]
            }
        }
        Command::Previous => {
            let at_start =
                state.queue.current_index() == Some(0) && state.queue.repeat() != Repeat::All;
            if pb.position_at(now) > RESTART_THRESHOLD || at_start {
                return restart(state, now);
            }
            let playing = pb.status != PlaybackStatus::Paused;
            if state.queue.back().is_none() {
                return Vec::new();
            }
            load_current(state, now, playing)
        }
        Command::Seek(to) => {
            if state.queue.current().is_none() {
                return Vec::new();
            }
            let to = state.duration().map_or(*to, |d| (*to).min(d));
            let pb = &mut state.playback;
            pb.position = to;
            if pb.anchor.is_some() {
                pb.anchor = Some(now);
            }
            vec![Effect::Seek(to)]
        }
        Command::SetVolume(v) => {
            let v = v.clamp(0.0, 1.0);
            pb.volume = v;
            vec![Effect::Volume(v)]
        }
        Command::SetShuffle { on, seed } => {
            state.queue.set_shuffle(*on, *seed);
            Vec::new()
        }
        Command::CycleRepeat => {
            let next = state.queue.repeat().cycle();
            state.queue.set_repeat(next);
            Vec::new()
        }
        Command::Login(cookies) => {
            state.session = Session::LoggingIn;
            vec![Effect::Login(cookies.clone())]
        }
        Command::Logout => {
            state.session = Session::LoggedOut;
            state.library = Loadable::Idle;
            state.liked = Loadable::Idle;
            state.playlist = None;
            state.banner = None;
            vec![Effect::Logout]
        }
        Command::DismissBanner => {
            state.banner = None;
            Vec::new()
        }
    }
}

fn restart(state: &mut State, now: Instant) -> Vec<Effect> {
    match state.playback.status {
        PlaybackStatus::Stopped => load_current(state, now, true),
        _ => apply(state, &Command::Seek(Duration::ZERO), now),
    }
}

/// Fetches the library and liked songs. Cached data stays on screen while
/// the fetch runs. Logged out, there is no library to fetch.
fn refresh(state: &mut State) -> Vec<Effect> {
    if state.session != Session::LoggedIn {
        return Vec::new();
    }
    if state.library.ready().is_none() {
        state.library = Loadable::Loading;
    }
    if state.liked.ready().is_none() {
        state.liked = Loadable::Loading;
    }
    vec![Effect::FetchLibrary, Effect::FetchLiked]
}

/// An auth error while logged in ends the session with one banner. Network
/// errors never log the user out.
fn describe(error: &BackendError, state: &mut State) -> String {
    if matches!(error, BackendError::Auth) && state.session == Session::LoggedIn {
        state.banner = Some(SESSION_EXPIRED.to_owned());
        state.session = Session::LoggedOut;
    }
    error.to_string()
}

/// A fetch result for a slot. A failed refresh keeps the data already shown.
fn settle<T>(slot: &mut Loadable<T>, result: Result<T, BackendError>, state: &mut State) {
    match result {
        Ok(v) => *slot = Loadable::Ready(v),
        Err(e) => {
            let message = describe(&e, state);
            if slot.ready().is_none() {
                *slot = Loadable::Failed(message);
            }
        }
    }
}

fn on_login(state: &mut State, saved: bool, result: Result<(), BackendError>) -> Vec<Effect> {
    let expected = if saved {
        Session::LoggedIn
    } else {
        Session::LoggingIn
    };
    // The user has moved on (logged out, or started another attempt).
    if state.session != expected {
        return Vec::new();
    }
    match result {
        Ok(()) => {
            state.session = Session::LoggedIn;
            if !saved {
                state.banner = None;
            }
            return refresh(state);
        }
        // Offline at startup: keep showing the cached library. The backend
        // checks the saved cookies again before each library fetch.
        Err(BackendError::Network(_)) if saved => {
            state.banner = Some("Offline: could not reach YouTube Music".to_owned());
            return refresh(state);
        }
        Err(_) if saved => {
            state.session = Session::LoggedOut;
            state.banner = Some(SESSION_EXPIRED.to_owned());
        }
        Err(BackendError::Network(m)) => {
            state.session = Session::Failed(format!("Could not reach YouTube Music: {m}"));
        }
        Err(BackendError::Auth) => state.session = Session::Failed(COOKIES_REJECTED.to_owned()),
        Err(e) => state.session = Session::Failed(e.to_string()),
    }
    Vec::new()
}

fn arc_tracks(tracks: Vec<Track>) -> Arc<Vec<Arc<Track>>> {
    Arc::new(tracks.into_iter().map(Arc::new).collect())
}

fn playlist_view(p: Playlist) -> Arc<PlaylistView> {
    Arc::new(PlaylistView {
        summary: p.summary,
        tracks: arc_tracks(p.tracks),
    })
}

/// Backend side only: folds in a result or a player event.
pub fn on_event(state: &mut State, event: Event, now: Instant) -> Vec<Effect> {
    match event {
        Event::Library(r) => {
            let mut slot = std::mem::take(&mut state.library);
            settle(&mut slot, r.map(Arc::new), state);
            state.library = slot;
        }
        Event::Liked(r) => {
            let mut slot = std::mem::take(&mut state.liked);
            settle(&mut slot, r.map(arc_tracks), state);
            state.liked = slot;
        }
        Event::Search { query, result } => {
            // A newer query has replaced this one; its answer is stale.
            if query != state.search_query {
                return Vec::new();
            }
            state.search = match result {
                Ok(mut results) => Loadable::Ready(Arc::new(SearchView {
                    tracks: arc_tracks(std::mem::take(&mut results.tracks)),
                    results,
                })),
                Err(e) => Loadable::Failed(describe(&e, state)),
            };
        }
        Event::Playlist { id, result } => {
            let Some((open, mut page)) = state.playlist.take() else {
                return Vec::new();
            };
            if open == id {
                settle(&mut page, result.map(playlist_view), state);
            }
            state.playlist = Some((open, page));
        }
        // The backend loads the source itself; nothing in the state changes.
        Event::StreamReady { .. } => {}
        Event::StreamFailed { uid, error } => {
            let message = describe(&error, state);
            if state.queue.current().map(|e| e.uid) == Some(uid) {
                state.banner = Some(format!("Could not play this track: {message}"));
                state.playback.stop();
                return vec![Effect::Stop];
            }
        }
        Event::TrackEnded { track_id } => {
            if state.current_track().map(|t| t.id.as_str()) != Some(track_id.as_str()) {
                return Vec::new();
            }
            if state.queue.advance(true).is_some() {
                return load_current(state, now, true);
            }
            state.playback.stop();
            return vec![Effect::Stop];
        }
        Event::PlayerError(message) | Event::Notice(message) => state.banner = Some(message),
        Event::Login { saved, result } => return on_login(state, saved, result),
        Event::SavedLogin => {
            if state.session == Session::Starting {
                state.session = Session::LoggedIn;
            }
        }
        Event::NoSavedLogin => {
            if state.session == Session::Starting {
                state.session = Session::LoggedOut;
            }
        }
        Event::StartFailed(message) => state.session = Session::Failed(message),
        // Cached data fills only what the network has not filled yet.
        Event::CachedLibrary(v) => {
            if state.library.ready().is_none() {
                state.library = Loadable::Ready(Arc::new(v));
            }
        }
        Event::CachedLiked(v) => {
            if state.liked.ready().is_none() {
                state.liked = Loadable::Ready(arc_tracks(v));
            }
        }
        Event::CachedQueue { tracks, current } => {
            // Restore it stopped; the user presses play to resume.
            if state.queue.is_empty() {
                state
                    .queue
                    .play_list(&arc_tracks(tracks), current.unwrap_or(0), 0);
            }
        }
        Event::CachedPlaylist(p) => {
            if let Some((open, page)) = &mut state.playlist
                && *open == p.summary.id
                && page.ready().is_none()
            {
                *page = Loadable::Ready(playlist_view(p));
            }
        }
    }
    Vec::new()
}

/// Backend side only: takes the player's position when it is playing the
/// current track, so the shown position follows the real one.
pub fn sync_position(state: &mut State, player: &PlayerState, now: Instant) {
    let Some(track) = state.current_track() else {
        return;
    };
    if player.track_id.as_deref() != Some(track.id.as_str()) {
        return;
    }
    let pb = &mut state.playback;
    match (pb.status, player.status) {
        (PlaybackStatus::Playing, PlaybackStatus::Playing) => {
            pb.position = player.position;
            pb.anchor = Some(now);
        }
        (PlaybackStatus::Paused, PlaybackStatus::Paused) => pb.position = player.position,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::tests::track;

    fn list(ids: &[&str]) -> Arc<Vec<Arc<Track>>> {
        Arc::new(ids.iter().map(|id| track(id)).collect())
    }

    fn playing(ids: &[&str], start: usize, now: Instant) -> State {
        let mut s = State::default();
        apply(
            &mut s,
            &Command::PlayTracks {
                tracks: list(ids),
                start,
                seed: 1,
            },
            now,
        );
        s
    }

    fn current(s: &State) -> &str {
        &s.current_track().unwrap().id
    }

    #[test]
    fn play_tracks_loads_and_plays_at_once() {
        let now = Instant::now();
        let mut s = State::default();
        let fx = apply(
            &mut s,
            &Command::PlayTracks {
                tracks: list(&["a", "b"]),
                start: 1,
                seed: 1,
            },
            now,
        );
        assert_eq!(current(&s), "b");
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        let uid = s.queue.current().unwrap().uid;
        assert_eq!(
            fx,
            [Effect::Load {
                uid,
                track_id: "b".into()
            }]
        );
    }

    #[test]
    fn toggle_play_freezes_and_resumes_position() {
        let t0 = Instant::now();
        let mut s = playing(&["a"], 0, t0);
        let t1 = t0 + Duration::from_secs(10);
        assert_eq!(apply(&mut s, &Command::TogglePlay, t1), [Effect::Pause]);
        assert_eq!(s.playback.status, PlaybackStatus::Paused);
        let t2 = t1 + Duration::from_secs(60);
        assert_eq!(s.playback.position_at(t2), Duration::from_secs(10));
        assert_eq!(apply(&mut s, &Command::TogglePlay, t2), [Effect::Play]);
        assert_eq!(
            s.playback.position_at(t2 + Duration::from_secs(5)),
            Duration::from_secs(15)
        );
    }

    #[test]
    fn track_end_advances_and_stops_at_queue_end() {
        let now = Instant::now();
        let mut s = playing(&["a", "b"], 0, now);
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            now,
        );
        assert_eq!(current(&s), "b");
        assert!(matches!(fx[..], [Effect::Load { .. }]));
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "b".into(),
            },
            now,
        );
        assert_eq!(fx, [Effect::Stop]);
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);
    }

    #[test]
    fn track_end_respects_repeat_one_and_all() {
        let now = Instant::now();
        let mut s = playing(&["a", "b"], 1, now);
        apply(&mut s, &Command::CycleRepeat, now); // all
        on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "b".into(),
            },
            now,
        );
        assert_eq!(current(&s), "a");
        apply(&mut s, &Command::CycleRepeat, now); // one
        on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            now,
        );
        assert_eq!(current(&s), "a");
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
    }

    #[test]
    fn stale_track_end_is_ignored() {
        let now = Instant::now();
        let mut s = playing(&["a", "b", "c"], 0, now);
        apply(&mut s, &Command::Next, now);
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            now,
        );
        assert!(fx.is_empty());
        assert_eq!(current(&s), "b");
    }

    #[test]
    fn next_while_paused_stays_paused() {
        let now = Instant::now();
        let mut s = playing(&["a", "b"], 0, now);
        apply(&mut s, &Command::TogglePlay, now);
        apply(&mut s, &Command::Next, now);
        assert_eq!(current(&s), "b");
        assert_eq!(s.playback.status, PlaybackStatus::Paused);
    }

    #[test]
    fn previous_restarts_after_three_seconds() {
        let t0 = Instant::now();
        let mut s = playing(&["a", "b"], 1, t0);
        let fx = apply(&mut s, &Command::Previous, t0 + Duration::from_secs(10));
        assert_eq!(fx, [Effect::Seek(Duration::ZERO)]);
        assert_eq!(current(&s), "b");
        let fx = apply(&mut s, &Command::Previous, t0 + Duration::from_secs(11));
        assert!(matches!(fx[..], [Effect::Load { .. }]));
        assert_eq!(current(&s), "a");
    }

    #[test]
    fn removing_current_entry_plays_the_next() {
        let now = Instant::now();
        let mut s = playing(&["a", "b"], 0, now);
        let fx = apply(&mut s, &Command::RemoveEntry(0), now);
        assert_eq!(current(&s), "b");
        assert!(matches!(fx[..], [Effect::Load { .. }]));
        let fx = apply(&mut s, &Command::RemoveEntry(0), now);
        assert_eq!(fx, [Effect::Stop]);
        assert!(s.queue.is_empty());
    }

    #[test]
    fn seek_clamps_to_duration() {
        let now = Instant::now();
        let mut s = playing(&["a"], 0, now);
        let fx = apply(&mut s, &Command::Seek(Duration::from_secs(500)), now);
        assert_eq!(fx, [Effect::Seek(Duration::from_secs(100))]);
    }

    #[test]
    fn stale_search_results_are_dropped() {
        let now = Instant::now();
        let mut s = State::default();
        apply(&mut s, &Command::Search("old".into()), now);
        apply(&mut s, &Command::Search("new".into()), now);
        on_event(
            &mut s,
            Event::Search {
                query: "old".into(),
                result: Ok(SearchResults::default()),
            },
            now,
        );
        assert!(matches!(s.search, Loadable::Loading));
    }

    #[test]
    fn auth_error_raises_one_banner() {
        let now = Instant::now();
        let mut s = logged_in();
        apply(&mut s, &Command::Refresh, now);
        on_event(&mut s, Event::Library(Err(BackendError::Auth)), now);
        assert!(matches!(s.library, Loadable::Failed(_)));
        assert_eq!(s.banner.as_deref(), Some(SESSION_EXPIRED));
        assert_eq!(s.session, Session::LoggedOut);
    }

    fn logged_in() -> State {
        State {
            session: Session::LoggedIn,
            ..State::default()
        }
    }

    fn summary(id: &str) -> PlaylistSummary {
        PlaylistSummary {
            id: id.into(),
            name: id.into(),
            owner: None,
            track_count: None,
            thumbnails: Vec::new(),
        }
    }

    fn login(s: &mut State, saved: bool, result: Result<(), BackendError>) -> Vec<Effect> {
        on_event(s, Event::Login { saved, result }, Instant::now())
    }

    #[test]
    fn pasted_cookies_log_in_and_refresh() {
        let now = Instant::now();
        let mut s = State::default();
        on_event(&mut s, Event::NoSavedLogin, now);
        assert_eq!(s.session, Session::LoggedOut);
        assert!(
            apply(&mut s, &Command::Refresh, now).is_empty(),
            "no library logged out"
        );
        let fx = apply(&mut s, &Command::Login("SID=1".into()), now);
        assert_eq!(fx, [Effect::Login("SID=1".into())]);
        assert_eq!(s.session, Session::LoggingIn);
        let fx = login(&mut s, false, Ok(()));
        assert_eq!(s.session, Session::LoggedIn);
        assert_eq!(fx, [Effect::FetchLibrary, Effect::FetchLiked]);
    }

    #[test]
    fn pasted_cookie_failures_stay_on_the_login_screen() {
        let now = Instant::now();
        let mut s = State::default();
        apply(&mut s, &Command::Login("x".into()), now);
        login(&mut s, false, Err(BackendError::Auth));
        assert_eq!(s.session, Session::Failed(COOKIES_REJECTED.into()));
        assert!(s.banner.is_none());

        apply(&mut s, &Command::Login("x".into()), now);
        login(
            &mut s,
            false,
            Err(BackendError::Network("timed out".into())),
        );
        let Session::Failed(message) = &s.session else {
            panic!("{:?}", s.session);
        };
        assert!(message.contains("timed out"), "{message}");
    }

    #[test]
    fn saved_cookies_log_in_at_once_and_are_checked_after() {
        let now = Instant::now();
        let mut s = State::default();
        on_event(&mut s, Event::SavedLogin, now);
        assert_eq!(s.session, Session::LoggedIn);
        // Rejected: back to the login screen with one banner.
        login(&mut s, true, Err(BackendError::Auth));
        assert_eq!(s.session, Session::LoggedOut);
        assert_eq!(s.banner.as_deref(), Some(SESSION_EXPIRED));
    }

    #[test]
    fn offline_start_keeps_the_session_and_cached_library() {
        let now = Instant::now();
        let mut s = State::default();
        on_event(&mut s, Event::SavedLogin, now);
        on_event(&mut s, Event::CachedLibrary(vec![summary("PLa")]), now);
        let fx = login(&mut s, true, Err(BackendError::Network("down".into())));
        assert_eq!(s.session, Session::LoggedIn);
        assert_eq!(fx, [Effect::FetchLibrary, Effect::FetchLiked]);
        assert_eq!(
            s.library.ready().unwrap()[0].id,
            "PLa",
            "refresh keeps cache"
        );
        // Network errors never log out or blank the cached data.
        on_event(
            &mut s,
            Event::Library(Err(BackendError::Network("down".into()))),
            now,
        );
        assert_eq!(s.session, Session::LoggedIn);
        assert_eq!(s.library.ready().unwrap()[0].id, "PLa");
    }

    #[test]
    fn stale_login_results_are_dropped() {
        let now = Instant::now();
        let mut s = State::default();
        apply(&mut s, &Command::Login("x".into()), now);
        apply(&mut s, &Command::Logout, now);
        assert!(login(&mut s, false, Ok(())).is_empty());
        assert_eq!(s.session, Session::LoggedOut);
    }

    #[test]
    fn logout_forgets_the_library() {
        let now = Instant::now();
        let mut s = logged_in();
        on_event(&mut s, Event::CachedLibrary(vec![summary("PLa")]), now);
        apply(&mut s, &Command::Logout, now);
        assert_eq!(s.session, Session::LoggedOut);
        assert!(matches!(s.library, Loadable::Idle));
    }

    #[test]
    fn network_results_replace_cached_data_but_cache_never_replaces_them() {
        let now = Instant::now();
        let mut s = logged_in();
        apply(&mut s, &Command::Refresh, now);
        on_event(&mut s, Event::Library(Ok(vec![summary("PLnet")])), now);
        on_event(&mut s, Event::CachedLibrary(vec![summary("PLold")]), now);
        assert_eq!(s.library.ready().unwrap()[0].id, "PLnet");
    }

    #[test]
    fn cached_queue_restores_stopped_unless_the_user_started_one() {
        let now = Instant::now();
        let cached = || Event::CachedQueue {
            tracks: vec![Track::clone(&track("a")), Track::clone(&track("b"))],
            current: Some(1),
        };
        let mut s = State::default();
        on_event(&mut s, cached(), now);
        assert_eq!(current(&s), "b");
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);
        assert!(matches!(
            apply(&mut s, &Command::TogglePlay, now)[..],
            [Effect::Load { .. }]
        ));

        let mut s = playing(&["z"], 0, now);
        on_event(&mut s, cached(), now);
        assert_eq!(current(&s), "z");
        assert_eq!(s.queue.len(), 1);
    }

    /// The UI's optimistic replay: a snapshot that has not seen the latest
    /// command must not undo it once pending commands are re-applied.
    #[test]
    fn replaying_pending_commands_over_a_lagging_snapshot() {
        let now = Instant::now();
        let cmds = [
            Command::PlayTracks {
                tracks: list(&["a", "b", "c"]),
                start: 0,
                seed: 1,
            },
            Command::AddToQueue(track("z")),
            Command::Next,
        ];
        let mut ui = State::default();
        for c in &cmds {
            apply(&mut ui, c, now);
        }
        // The backend has only seen the first command.
        let mut snapshot = State::default();
        apply(&mut snapshot, &cmds[0], now);
        snapshot.acked = 1;
        let mut rebuilt = snapshot.clone();
        for c in &cmds[snapshot.acked as usize..] {
            apply(&mut rebuilt, c, now);
        }
        assert_eq!(current(&rebuilt), current(&ui));
        let ids = |s: &State| -> Vec<String> {
            s.queue
                .entries()
                .iter()
                .map(|e| e.track.id.clone())
                .collect()
        };
        assert_eq!(ids(&rebuilt), ids(&ui));
        assert_eq!(
            rebuilt.queue.current().unwrap().uid,
            ui.queue.current().unwrap().uid
        );
    }

    #[test]
    fn sync_position_follows_player_for_current_track_only() {
        let now = Instant::now();
        let mut s = playing(&["a"], 0, now);
        let mut ps = PlayerState {
            status: PlaybackStatus::Playing,
            track_id: Some("other".into()),
            position: Duration::from_secs(42),
            duration: None,
            volume: 1.0,
        };
        sync_position(&mut s, &ps, now);
        assert_eq!(s.playback.position_at(now), Duration::ZERO);
        ps.track_id = Some("a".into());
        sync_position(&mut s, &ps, now);
        assert_eq!(s.playback.position_at(now), Duration::from_secs(42));
    }
}
