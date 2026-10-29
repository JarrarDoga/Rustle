//! App state and the reducer that changes it.
//!
//! [`apply`] is pure and runs twice for every command: once on the UI thread,
//! right away, so a control shows its result the moment it is used; and once
//! in the backend task, which also carries out the returned [`Effect`]s. Each
//! snapshot the backend publishes says which command it has seen last
//! (`acked`), and the UI re-applies the commands that came after it, so a
//! lagging snapshot never undoes a click.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use model::{
    Album, Artist, Lyrics, Playlist, PlaylistSummary, SearchResults, StreamSource, Track, TrackId,
};
use player::{PlaybackStatus, PlayerState};
use ytm_core::BackendError;

use crate::queue::{Queue, Repeat, scope_label};

/// Previous restarts the track instead when it has played longer than this.
const RESTART_THRESHOLD: Duration = Duration::from_secs(3);
/// A play is recorded once the track has played this long, or half its
/// length if that comes first.
const PLAY_THRESHOLD: Duration = Duration::from_secs(30);
/// `Buffering` shows only after this much continuous buffering, so a seek
/// within buffered audio does not flicker.
pub const BUFFERING_DELAY: Duration = Duration::from_millis(250);

pub const SESSION_EXPIRED: &str = "Session expired, paste new cookies";
pub const COOKIES_REJECTED: &str = "those cookies were rejected";
pub const OFFLINE: &str = "Offline: could not reach YouTube Music";
/// What a guest sees where the library and liked songs would be (see
/// [`State::needs_login`]).
#[allow(dead_code)] // shown by the UI batch
pub const NEEDS_LOGIN: &str = "Sign in to see your library";
const SIGN_IN_TO_EDIT: &str = "Sign in to change your library";
const NO_MAIN: &str = "Choose a main playlist first";
const MAIN_LOADING: &str = "The main playlist is still loading";
const LIKED_LOADING: &str = "Liked songs are still loading";
const UNKNOWN_ENTRY: &str = "Cannot remove that song yet: refresh the main playlist and try again";
/// The id of the user's liked songs as a playlist. YouTube Music edits it
/// only by liking and unliking, never as a playlist.
pub const LIKED_MUSIC: &str = "LM";

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
    /// The player has not started the current track yet: its stream is
    /// resolving or prebuffering. The clock stays at `position` until the
    /// player reports the track playing or paused, and `status` is
    /// `Buffering` while the user wants it to play.
    pub loading: bool,
    /// 0.0 to 1.0.
    pub volume: f32,
    /// Backend side: when the player started buffering while `status` is
    /// still `Playing`. `status` turns `Buffering` after [`BUFFERING_DELAY`].
    pub buffering_since: Option<Instant>,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            position: Duration::ZERO,
            anchor: None,
            loading: false,
            volume: 0.8,
            buffering_since: None,
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

    /// The player has nothing to say about a new track yet, so its clock
    /// waits at 0:00 for the player (see [`sync_position`]).
    fn start_track(&mut self, playing: bool) {
        self.position = Duration::ZERO;
        self.anchor = None;
        self.buffering_since = None;
        self.loading = true;
        self.status = if playing {
            PlaybackStatus::Buffering
        } else {
            PlaybackStatus::Paused
        };
    }

    fn stop(&mut self) {
        self.buffering_since = None;
        self.loading = false;
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
    /// The main playlist ("Everything"), by id, once one is chosen. When the
    /// playlist page shows the same id, both hold the same view.
    pub main: Option<(String, Loadable<Arc<PlaylistView>>)>,
    /// Album-art accent colors (sRGB) by track id, for the spine and accent.
    pub art_colors: Arc<HashMap<TrackId, [u8; 3]>>,
    /// The album page that is open, by browse id.
    pub album: Option<(String, Loadable<Arc<Album>>)>,
    /// The artist page that is open, by channel id.
    pub artist: Option<(String, Loadable<Arc<Artist>>)>,
    /// When each main-playlist track was last played (unix seconds), from
    /// the local play history. Missing means never played here. Feeds the
    /// "Last played" column and "Not heard in 3 months".
    pub last_played: Arc<HashMap<TrackId, i64>>,
    /// Saved user settings. Loaded from the store at startup.
    pub settings: Settings,
    /// The lyrics drawer is open. In the state, not the view, so the reducer
    /// can fetch lyrics when the track changes while it is open.
    pub lyrics_open: bool,
    /// Lyrics for this track id, fetched while the drawer is open.
    pub lyrics: Option<(TrackId, Loadable<Option<Arc<Lyrics>>>)>,
    /// Optimistic edits waiting for YouTube Music's answer, to roll back.
    pub(crate) pending: Vec<Pending>,
    /// The seed of the radio fetch in flight, so it goes out once.
    pub(crate) radio_for: Option<TrackId>,
    /// The queue ran out while waiting for radio: play the first radio
    /// track once it arrives.
    pub(crate) resume_on_radio: bool,
    /// Counts track starts; a play is recorded once per start.
    pub(crate) play_serial: u64,
    /// The `play_serial` last recorded as a play.
    pub(crate) recorded_serial: u64,
}

/// An optimistic edit and what undoes it.
///
/// One track has at most one like or unlike on the wire (`sent`), and at
/// most one of the other kind queued behind it: YouTube may apply two
/// requests in flight at once in either order (issue #6).
#[derive(Debug, Clone)]
pub(crate) enum Pending {
    /// `inserted`: the track was put at the top of the liked list;
    /// `in_main`: also at the top of the main playlist, which is
    /// [`LIKED_MUSIC`].
    Like {
        track_id: TrackId,
        inserted: bool,
        in_main: bool,
        sent: bool,
    },
    /// The track was taken out of the liked list at `at`, and out of the
    /// main playlist (when it is [`LIKED_MUSIC`]) at `main`, with its entry
    /// id.
    Unlike {
        track_id: TrackId,
        at: Option<usize>,
        main: Option<(usize, Option<String>)>,
        track: Arc<Track>,
        sent: bool,
    },
    /// Appended to the main playlist with no entry id yet.
    Add {
        playlist_id: String,
        track_ids: Vec<TrackId>,
    },
    /// Taken out of the main playlist: `(index, track, entry id)`.
    Remove {
        playlist_id: String,
        entries: Vec<(TrackId, String)>,
        removed: Vec<(usize, Arc<Track>, Option<String>)>,
    },
}

impl Pending {
    /// For a like or unlike: its track, whether it likes, and whether it
    /// is on the wire.
    fn like_edit(&self) -> Option<(&TrackId, bool, bool)> {
        match self {
            Pending::Like { track_id, sent, .. } => Some((track_id, true, *sent)),
            Pending::Unlike { track_id, sent, .. } => Some((track_id, false, *sent)),
            _ => None,
        }
    }

    /// The request for a like or unlike.
    fn like_effect(&self) -> Option<Effect> {
        match self.like_edit()? {
            (id, true, _) => Some(Effect::Like(id.clone())),
            (id, false, _) => Some(Effect::Unlike(id.clone())),
        }
    }

    /// Folds in the undo of a later edit of the same kind and track, so
    /// undoing this one undoes both.
    fn absorb(&mut self, later: Pending) {
        match (self, later) {
            (
                Pending::Like {
                    inserted, in_main, ..
                },
                Pending::Like {
                    inserted: i,
                    in_main: m,
                    ..
                },
            ) => {
                *inserted |= i;
                *in_main |= m;
            }
            (Pending::Unlike { at, main, .. }, Pending::Unlike { at: a, main: m, .. }) => {
                *at = at.or(a);
                *main = main.take().or(m);
            }
            _ => {}
        }
    }
}

/// Data that is cached in the store and refreshed from YouTube Music.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feed {
    Library,
    Liked,
    Playlist(String),
}

impl Feed {
    /// The store's `fetched_at` key.
    pub fn key(&self) -> String {
        match self {
            Feed::Library => "library".to_owned(),
            Feed::Liked => "liked".to_owned(),
            Feed::Playlist(id) => format!("playlist:{id}"),
        }
    }

    /// The network fetch.
    pub fn fetch(&self) -> Effect {
        match self {
            Feed::Library => Effect::FetchLibrary,
            Feed::Liked => Effect::FetchLiked,
            Feed::Playlist(id) => Effect::FetchPlaylist(id.clone()),
        }
    }
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
    /// Each entry's id (`setVideoId`), parallel to `tracks`; `None` where
    /// unknown (e.g. an add YouTube has not confirmed yet).
    pub entry_ids: Arc<Vec<Option<String>>>,
}

impl PlaylistView {
    fn insert(&mut self, at: usize, track: Arc<Track>, entry: Option<String>) {
        let at = at.min(self.tracks.len());
        Arc::make_mut(&mut self.tracks).insert(at, track);
        Arc::make_mut(&mut self.entry_ids).insert(at, entry);
        self.summary.track_count = Some(self.tracks.len() as u64);
    }

    fn remove(&mut self, at: usize) -> (Arc<Track>, Option<String>) {
        let track = Arc::make_mut(&mut self.tracks).remove(at);
        let entry = Arc::make_mut(&mut self.entry_ids).remove(at);
        self.summary.track_count = Some(self.tracks.len() as u64);
        (track, entry)
    }

    fn position(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    fn to_playlist(&self) -> Playlist {
        let mut summary = self.summary.clone();
        summary.track_count = Some(self.tracks.len() as u64);
        Playlist {
            summary,
            tracks: self.tracks.iter().map(|t| Track::clone(t)).collect(),
            entry_ids: self.entry_ids.to_vec(),
        }
    }
}

impl State {
    pub fn current_track(&self) -> Option<&Arc<Track>> {
        self.queue.current().map(|e| &e.track)
    }

    pub fn duration(&self) -> Option<Duration> {
        self.current_track().and_then(|t| t.duration)
    }

    /// The loaded main playlist.
    pub fn main_view(&self) -> Option<&Arc<PlaylistView>> {
        self.main.as_ref().and_then(|(_, page)| page.ready())
    }

    /// The main playlist as a store `Playlist`, for writing back edits.
    pub fn main_playlist(&self) -> Option<Playlist> {
        self.main_view().map(|v| v.to_playlist())
    }

    /// Logged out (a guest, or a session that ended): there is no library
    /// to load. Slots with nothing cached are `Idle`; the UI shows
    /// [`NEEDS_LOGIN`] for them instead of "Loading…".
    #[allow(dead_code)] // read by the UI batch
    pub fn needs_login(&self) -> bool {
        matches!(self.session, Session::LoggedOut | Session::Failed(_))
    }

    #[allow(dead_code)] // read by the UI batch (heart buttons)
    pub fn is_liked(&self, id: &str) -> bool {
        self.liked
            .ready()
            .is_some_and(|l| l.iter().any(|t| t.id == id))
    }

    /// The track's 0-based position in the main playlist, for the "In
    /// Everything" column. A linear scan: call it for visible rows only.
    #[allow(dead_code)] // the UI batch's pages read it
    pub fn main_position(&self, id: &str) -> Option<usize> {
        self.main_view()?.tracks.iter().position(|t| t.id == id)
    }

    /// A track the app already knows, by id.
    fn find_track(&self, id: &str) -> Option<Arc<Track>> {
        let page = self.playlist.as_ref().and_then(|(_, p)| p.ready());
        let lists = [
            self.main_view().map(|v| &v.tracks[..]),
            page.map(|v| &v.tracks[..]),
            self.liked.ready().map(|l| &l[..]),
            self.search.ready().map(|s| &s.tracks[..]),
        ];
        self.queue
            .entries()
            .iter()
            .map(|e| &e.track)
            .chain(lists.into_iter().flatten().flatten())
            .find(|t| t.id == id)
            .cloned()
            // A song unliked off the only list that showed it, liked again
            // before YouTube answered.
            .or_else(|| {
                self.pending.iter().find_map(|p| match p {
                    Pending::Unlike { track, .. } if track.id == id => Some(Arc::clone(track)),
                    _ => None,
                })
            })
    }

    /// The playlist page shows the main playlist's view when it is open.
    fn sync_page(&mut self) {
        if let (Some((main_id, main)), Some((open, page))) = (&self.main, &mut self.playlist)
            && main_id == open
            && main.ready().is_some()
        {
            *page = main.clone();
        }
    }
}

/// Saved user settings (the settings popover). The store keeps one key per
/// field as plain text (`settings::load` / `settings::save`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Radio follows when the queue runs out.
    pub autoplay: bool,
    /// Take the accent from the playing art; off means the palette's
    /// fallback accent, as with nothing playing.
    pub accent_from_art: bool,
    /// The palette's name (`theme::PALETTES`); unknown names fall back to
    /// the default.
    pub theme: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autoplay: true,
            accent_from_art: true,
            theme: "Graphite".to_owned(),
        }
    }
}

/// One settings change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingChange {
    Autoplay(bool),
    AccentFromArt(bool),
    Theme(String),
}

/// What the track table currently shows, and what Shuffle / Play in order
/// act on.
#[allow(dead_code)] // the UI batch sends it
#[derive(Debug, Clone, PartialEq)]
pub enum Scope {
    /// The whole main playlist.
    All,
    /// A filter over the main playlist: the label shown in "Playing from"
    /// (e.g. "Saint Juniper") and the matching track ids in playlist order.
    Filter {
        label: String,
        ids: Arc<Vec<TrackId>>,
    },
    /// Main-playlist positions `from..=to`, 0-based (shown 1-based, "#380").
    Range { from: usize, to: usize },
}

/// What the UI asks for.
#[derive(Debug, Clone)]
pub enum Command {
    /// Fetch the library playlists and liked songs.
    Refresh,
    /// Like `Refresh`, but each feed (library, liked, main playlist) is
    /// fetched only if the store's copy is older than 10 minutes. Sent when
    /// the window gains focus.
    RefreshIfStale,
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
    /// Remove the first upcoming queue entry (after the current one) that
    /// plays this track. The undo of one `AddToQueue`; does nothing when
    /// the track is not upcoming.
    RemoveFromQueue(TrackId),
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
    #[allow(dead_code)] // the UI batch sends it
    Like(TrackId),
    #[allow(dead_code)] // the UI batch sends it
    Unlike(TrackId),
    /// Append songs to the main playlist.
    #[allow(dead_code)] // the UI batch sends it
    AddToMain(Vec<Arc<Track>>),
    /// Remove songs from the main playlist, by track id.
    #[allow(dead_code)] // the UI batch sends it
    RemoveFromMain(Vec<TrackId>),
    /// Make this playlist the main playlist and open it.
    #[allow(dead_code)] // the UI batch sends it
    SetMainPlaylist(String),
    /// Replace the queue with the scope's tracks, shuffled or in order.
    #[allow(dead_code)] // the UI batch sends it
    PlayScope {
        scope: Scope,
        shuffle: bool,
        seed: u64,
    },
    #[allow(dead_code)] // the UI batch sends it
    SetAutoplay(bool),
    /// Open an album page by browse id.
    #[allow(dead_code)] // the UI batch sends it
    OpenAlbum(String),
    /// Open an artist page by channel id.
    #[allow(dead_code)] // the UI batch sends it
    OpenArtist(String),
    /// Open or close the lyrics drawer. Opening fetches the current track's
    /// lyrics; a track change while open fetches again.
    ShowLyrics(bool),
    /// Change one saved setting.
    SetSetting(SettingChange),
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
    Like(TrackId),
    Unlike(TrackId),
    AddToPlaylist {
        playlist_id: String,
        track_ids: Vec<TrackId>,
    },
    /// Each item is `(track_id, entry_id)`.
    RemoveFromPlaylist {
        playlist_id: String,
        entries: Vec<(TrackId, String)>,
    },
    /// Save the main playlist id in the store.
    SaveMainPlaylist(String),
    /// Fetch radio tracks that follow this track, for autoplay.
    FetchRadio(TrackId),
    /// Store one play in the play history.
    RecordPlay {
        track_id: TrackId,
        played_ms: u64,
    },
    /// Compute art colors for these tracks (no stored color yet) with
    /// `thumbs::request_colors`; results come back as `Event::ArtColors`.
    NeedArtColors(Vec<crate::thumbs::ColorJob>),
    /// Show the cached copy and fetch only if the store's `fetched_at` for
    /// the feed is missing or older than 10 minutes.
    RefreshIfStale(Feed),
    FetchAlbum(String),
    FetchArtist(String),
    FetchLyrics(TrackId),
    /// Write all settings to the store.
    SaveSettings(Settings),
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
        /// The "Playing from" label and per-entry radio marks
        /// (`Store::queue_extras`).
        label: Option<String>,
        radio: Vec<bool>,
    },
    CachedPlaylist(Playlist),
    LikeResult {
        track_id: TrackId,
        liked: bool,
        result: Result<(), BackendError>,
    },
    /// `result` holds the new entry ids, one per track id.
    Added {
        playlist_id: String,
        track_ids: Vec<TrackId>,
        result: Result<Vec<Option<String>>, BackendError>,
    },
    Removed {
        playlist_id: String,
        entries: Vec<(TrackId, String)>,
        result: Result<(), BackendError>,
    },
    Radio {
        seed: TrackId,
        result: Result<Vec<Track>, BackendError>,
    },
    /// Art accent colors computed by `thumbs::request_colors`; the backend
    /// stores them with `Store::set_art_colors`.
    ArtColors(Vec<(TrackId, [u8; 3])>),
    /// The main playlist id read from the store at startup.
    CachedMainPlaylist(String),
    /// Art colors read from the store.
    CachedArtColors(Vec<(TrackId, [u8; 3])>),
    Album {
        id: String,
        result: Result<Album, BackendError>,
    },
    Artist {
        id: String,
        result: Result<Artist, BackendError>,
    },
    /// Last-played times (unix seconds) read from the store at startup.
    CachedPlayStats(Vec<(TrackId, i64)>),
    /// A play was recorded at `at` (unix seconds).
    Played {
        track_id: TrackId,
        at: i64,
    },
    /// `Ok(None)`: the song has no lyrics.
    Lyrics {
        track_id: TrackId,
        result: Result<Option<Lyrics>, BackendError>,
    },
    /// Settings read from the store at startup.
    CachedSettings(Settings),
}

fn load_current(state: &mut State, playing: bool) -> Vec<Effect> {
    match state.queue.current() {
        Some(e) => {
            let effect = Effect::Load {
                uid: e.uid,
                track_id: e.track.id.clone(),
            };
            state.playback.start_track(playing);
            state.play_serial += 1;
            state.resume_on_radio = false;
            let mut effects = vec![effect];
            effects.extend(fetch_lyrics(state));
            effects
        }
        None => {
            state.playback.stop();
            vec![Effect::Stop]
        }
    }
}

/// Fetches the current track's lyrics while the drawer is open, once per
/// track.
fn fetch_lyrics(state: &mut State) -> Option<Effect> {
    if !state.lyrics_open {
        return None;
    }
    let id = state.current_track()?.id.clone();
    if state.lyrics.as_ref().is_some_and(|(have, _)| *have == id) {
        return None;
    }
    state.lyrics = Some((id.clone(), Loadable::Loading));
    Some(Effect::FetchLyrics(id))
}

/// Applies one settings change to the state; the caller saves.
fn change_setting(state: &mut State, change: &SettingChange) {
    match change {
        SettingChange::Autoplay(on) => {
            state.settings.autoplay = *on;
            state.queue.set_autoplay(*on);
        }
        SettingChange::AccentFromArt(on) => state.settings.accent_from_art = *on,
        SettingChange::Theme(name) => state.settings.theme.clone_from(name),
    }
}

pub fn apply(state: &mut State, cmd: &Command, now: Instant) -> Vec<Effect> {
    let mut effects = apply_command(state, cmd, now);
    effects.extend(autoplay(state));
    effects
}

fn apply_command(state: &mut State, cmd: &Command, now: Instant) -> Vec<Effect> {
    let pb = &mut state.playback;
    match cmd {
        Command::Refresh => refresh(state, false),
        Command::RefreshIfStale => refresh(state, true),
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
            state.sync_page();
            vec![Effect::FetchPlaylist(id.clone())]
        }
        Command::OpenAlbum(id) => {
            state.album = Some((id.clone(), Loadable::Loading));
            vec![Effect::FetchAlbum(id.clone())]
        }
        Command::OpenArtist(id) => {
            state.artist = Some((id.clone(), Loadable::Loading));
            vec![Effect::FetchArtist(id.clone())]
        }
        Command::PlayTracks {
            tracks,
            start,
            seed,
        } => {
            state.queue.play_list(tracks, *start, *seed);
            load_current(state, true)
        }
        Command::PlayNow(track) => {
            state.queue.play_now(Arc::clone(track));
            load_current(state, true)
        }
        Command::PlayNext(track) => {
            state.queue.play_next(Arc::clone(track));
            Vec::new()
        }
        Command::AddToQueue(track) => {
            state.queue.add(Arc::clone(track));
            Vec::new()
        }
        Command::RemoveFromQueue(id) => {
            // Never the current entry, so playback is untouched.
            state.queue.remove_upcoming_track(id);
            Vec::new()
        }
        Command::JumpTo(index) => {
            if state.queue.jump(*index).is_none() {
                return Vec::new();
            }
            load_current(state, true)
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
                load_current(state, playing)
            } else {
                state.playback.stop();
                vec![Effect::Stop]
            }
        }
        Command::TogglePlay => match pb.status {
            PlaybackStatus::Playing | PlaybackStatus::Buffering => {
                pb.position = pb.position_at(now);
                pb.anchor = None;
                pb.buffering_since = None;
                pb.status = PlaybackStatus::Paused;
                vec![Effect::Pause]
            }
            PlaybackStatus::Paused => {
                pb.buffering_since = None;
                if pb.loading {
                    // Nothing plays yet; the clock starts with the player.
                    pb.status = PlaybackStatus::Buffering;
                } else {
                    // Resuming a loaded track is instant.
                    pb.anchor = Some(now);
                    pb.status = PlaybackStatus::Playing;
                }
                vec![Effect::Play]
            }
            PlaybackStatus::Stopped => {
                if state.queue.current().is_none() && state.queue.advance(false).is_none() {
                    return Vec::new();
                }
                load_current(state, true)
            }
        },
        Command::Next => {
            let playing = pb.status != PlaybackStatus::Paused;
            if state.queue.advance(false).is_some() {
                load_current(state, playing)
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
            load_current(state, playing)
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
            state.main = None;
            state.art_colors = Arc::default();
            state.pending.clear();
            state.banner = None;
            vec![Effect::Logout]
        }
        Command::DismissBanner => {
            state.banner = None;
            Vec::new()
        }
        Command::Like(id) => like(state, id, None),
        Command::Unlike(id) => unlike(state, id),
        Command::AddToMain(tracks) => add_to_main(state, tracks),
        Command::RemoveFromMain(ids) => remove_from_main(state, ids),
        Command::SetMainPlaylist(id) => {
            let keep = state.main.as_ref().filter(|(main, _)| main == id);
            let page = keep.map_or(Loadable::Loading, |(_, page)| page.clone());
            state.main = Some((id.clone(), page));
            state.playlist = Some((id.clone(), Loadable::Loading));
            state.sync_page();
            vec![
                Effect::SaveMainPlaylist(id.clone()),
                Effect::FetchPlaylist(id.clone()),
            ]
        }
        Command::PlayScope {
            scope,
            shuffle,
            seed,
        } => {
            let Some(view) = state.main_view() else {
                return Vec::new();
            };
            let (tracks, label) = scope_tracks(view, scope);
            if state
                .queue
                .play_scope(&tracks, label, *shuffle, *seed)
                .is_none()
            {
                return Vec::new();
            }
            load_current(state, true)
        }
        Command::SetAutoplay(on) => {
            change_setting(state, &SettingChange::Autoplay(*on));
            vec![Effect::SaveSettings(state.settings.clone())]
        }
        Command::SetSetting(change) => {
            change_setting(state, change);
            vec![Effect::SaveSettings(state.settings.clone())]
        }
        Command::ShowLyrics(open) => {
            state.lyrics_open = *open;
            fetch_lyrics(state).into_iter().collect()
        }
    }
}

/// The scope's tracks, in main-playlist order, and its "Playing from" label.
fn scope_tracks(view: &PlaylistView, scope: &Scope) -> (Vec<Arc<Track>>, String) {
    let all = &view.tracks;
    let tracks: Vec<Arc<Track>> = match scope {
        Scope::All => all.to_vec(),
        Scope::Filter { ids, .. } => {
            let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
            all.iter()
                .filter(|t| ids.contains(t.id.as_str()))
                .cloned()
                .collect()
        }
        Scope::Range { from, to } => {
            let Some(last) = all.len().checked_sub(1) else {
                return (Vec::new(), String::new());
            };
            let to = (*to).min(last);
            all[(*from).min(to)..=to].to_vec()
        }
    };
    let name = match scope {
        Scope::All => view.summary.name.clone(),
        Scope::Filter { label, .. } => label.clone(),
        Scope::Range { from, to } => {
            let to = (*to).min(all.len().saturating_sub(1));
            format!("#{} to #{}", (*from).min(to) + 1, to + 1)
        }
    };
    let label = scope_label(&name, tracks.len());
    (tracks, label)
}

/// After every reducer step: fetch radio once when the queue is about to
/// run out with autoplay on. A stopped queue (e.g. restored at startup)
/// fetches nothing unless it stopped because it ran out.
fn autoplay(state: &mut State) -> Option<Effect> {
    if state.radio_for.is_some() || !state.queue.needs_radio() {
        return None;
    }
    if state.playback.status == PlaybackStatus::Stopped && !state.resume_on_radio {
        return None;
    }
    let seed = state.queue.entries().last()?.track.id.clone();
    state.radio_for = Some(seed.clone());
    Some(Effect::FetchRadio(seed))
}

/// Edits need a session; tells a guest so.
fn can_edit(state: &mut State) -> bool {
    if state.session == Session::LoggedIn {
        return true;
    }
    state.banner = Some(SIGN_IN_TO_EDIT.to_owned());
    false
}

/// `track` is the song when the caller has it; otherwise it is looked up.
fn like(state: &mut State, id: &TrackId, track: Option<Arc<Track>>) -> Vec<Effect> {
    if !can_edit(state) {
        return Vec::new();
    }
    // Without the liked list, a like could hit an already-liked song, which
    // moves it to the top of the user's ordered list.
    if state.liked.ready().is_none() {
        state.banner = Some(LIKED_LOADING.to_owned());
        return Vec::new();
    }
    if state.is_liked(id) {
        return Vec::new();
    }
    let track = track.or_else(|| state.find_track(id));
    if let (Loadable::Ready(list), Some(track)) = (&mut state.liked, &track) {
        Arc::make_mut(list).insert(0, Arc::clone(track));
    }
    // With Liked Music as the main playlist, the collection shows the same
    // list, so it changes too.
    let in_main = match (main_view_mut(state, LIKED_MUSIC), &track) {
        (Some(view), Some(track)) if view.position(id).is_none() => {
            view.insert(0, Arc::clone(track), None);
            true
        }
        _ => false,
    };
    state.sync_page();
    queue_like_edit(
        state,
        Pending::Like {
            track_id: id.clone(),
            inserted: track.is_some(),
            in_main,
            sent: false,
        },
    )
}

fn unlike(state: &mut State, id: &TrackId) -> Vec<Effect> {
    if !can_edit(state) {
        return Vec::new();
    }
    let Loadable::Ready(list) = &mut state.liked else {
        state.banner = Some(LIKED_LOADING.to_owned());
        return Vec::new();
    };
    let at = list.iter().position(|t| t.id == *id);
    let from_liked = at.map(|at| Arc::make_mut(list).remove(at));
    // The main playlist, when it is Liked Music, may hold a song a stale
    // liked list lacks; "Remove from" it still unlikes.
    let from_main = main_view_mut(state, LIKED_MUSIC)
        .and_then(|view| view.position(id).map(|i| (i, view.remove(i))));
    state.sync_page();
    let main = from_main
        .as_ref()
        .map(|(i, (_, entry))| (*i, entry.clone()));
    let Some(track) = from_liked.or(from_main.map(|(_, (track, _))| track)) else {
        return Vec::new();
    };
    queue_like_edit(
        state,
        Pending::Unlike {
            track_id: id.clone(),
            at,
            main,
            track,
            sent: false,
        },
    )
}

/// Records a like or unlike whose optimistic edit is on screen, and
/// returns the request to send now, if any. A track has one request on the
/// wire at a time; a toggle while the other kind is in flight waits behind
/// it, and a toggle back cancels the one waiting.
fn queue_like_edit(state: &mut State, mut edit: Pending) -> Vec<Effect> {
    let Some((id, liked, _)) = edit.like_edit() else {
        return Vec::new();
    };
    let id = id.clone();
    let of_track = |p: &Pending| p.like_edit().is_some_and(|(t, ..)| *t == id);
    let Some(last) = state.pending.iter().rposition(of_track) else {
        let effects = edit.like_effect().into_iter().collect();
        if let Pending::Like { sent, .. } | Pending::Unlike { sent, .. } = &mut edit {
            *sent = true;
        }
        state.pending.push(edit);
        return effects;
    };
    match state.pending[last].like_edit() {
        // The same edit is already coming, e.g. a refresh dropped its row.
        Some((_, l, _)) if l == liked => state.pending[last].absorb(edit),
        // Waiting behind the opposite one in flight: the two cancel, and
        // the one in flight is undone as if this had never happened.
        Some((_, _, false)) => {
            state.pending.remove(last);
            if let Some(sent) = state.pending.iter().rposition(of_track) {
                state.pending[sent].absorb(edit);
            }
        }
        // The opposite one is in flight: send this once it has answered.
        _ => state.pending.push(edit),
    }
    Vec::new()
}

/// The main playlist's id and view, ready to edit, or a banner saying why
/// not.
fn main_for_edit(state: &mut State) -> Option<(String, &mut PlaylistView)> {
    let message = match &state.main {
        None => NO_MAIN,
        Some((_, Loadable::Ready(_))) => "",
        Some(_) => MAIN_LOADING,
    };
    if !message.is_empty() {
        state.banner = Some(message.to_owned());
        return None;
    }
    let Some((id, Loadable::Ready(view))) = &mut state.main else {
        return None;
    };
    Some((id.clone(), Arc::make_mut(view)))
}

fn add_to_main(state: &mut State, tracks: &[Arc<Track>]) -> Vec<Effect> {
    if !can_edit(state) {
        return Vec::new();
    }
    let Some((playlist_id, view)) = main_for_edit(state) else {
        return Vec::new();
    };
    // Songs already in the playlist are skipped, as are repeats.
    let mut seen: HashSet<TrackId> = view.tracks.iter().map(|t| t.id.clone()).collect();
    let new: Vec<Arc<Track>> = tracks
        .iter()
        .filter(|t| seen.insert(t.id.clone()))
        .cloned()
        .collect();
    if new.is_empty() {
        return Vec::new();
    }
    // YouTube rejects playlist edits on Liked Music: adding is liking.
    if playlist_id == LIKED_MUSIC {
        return new
            .into_iter()
            .flat_map(|t| {
                let id = t.id.clone();
                like(state, &id, Some(t))
            })
            .collect();
    }
    let track_ids: Vec<TrackId> = new.iter().map(|t| t.id.clone()).collect();
    Arc::make_mut(&mut view.tracks).extend(new);
    Arc::make_mut(&mut view.entry_ids).extend(track_ids.iter().map(|_| None));
    view.summary.track_count = Some(view.tracks.len() as u64);
    state.pending.push(Pending::Add {
        playlist_id: playlist_id.clone(),
        track_ids: track_ids.clone(),
    });
    state.sync_page();
    vec![Effect::AddToPlaylist {
        playlist_id,
        track_ids,
    }]
}

fn remove_from_main(state: &mut State, ids: &[TrackId]) -> Vec<Effect> {
    if !can_edit(state) {
        return Vec::new();
    }
    let Some((playlist_id, view)) = main_for_edit(state) else {
        return Vec::new();
    };
    // Removing from Liked Music is unliking, by track id: it has no entry
    // ids to send.
    if playlist_id == LIKED_MUSIC {
        return ids.iter().flat_map(|id| unlike(state, id)).collect();
    }
    let want: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut entries = Vec::new();
    let mut removed = Vec::new();
    for (i, (track, entry)) in view.tracks.iter().zip(view.entry_ids.iter()).enumerate() {
        if !want.contains(track.id.as_str()) {
            continue;
        }
        // YouTube removes entries by entry id; without one, do not guess.
        let Some(entry) = entry else {
            state.banner = Some(UNKNOWN_ENTRY.to_owned());
            return Vec::new();
        };
        entries.push((track.id.clone(), entry.clone()));
        removed.push((i, Arc::clone(track), Some(entry.clone())));
    }
    if entries.is_empty() {
        return Vec::new();
    }
    let tracks = Arc::make_mut(&mut view.tracks);
    let entry_ids = Arc::make_mut(&mut view.entry_ids);
    for (i, _, _) in removed.iter().rev() {
        tracks.remove(*i);
        entry_ids.remove(*i);
    }
    view.summary.track_count = Some(tracks.len() as u64);
    state.pending.push(Pending::Remove {
        playlist_id: playlist_id.clone(),
        entries: entries.clone(),
        removed,
    });
    state.sync_page();
    vec![Effect::RemoveFromPlaylist {
        playlist_id,
        entries,
    }]
}

/// Takes the first pending edit `matches` picks.
fn take_pending(state: &mut State, matches: impl Fn(&Pending) -> bool) -> Option<Pending> {
    let at = state.pending.iter().position(matches)?;
    Some(state.pending.remove(at))
}

/// One banner for a failed edit. An expired session keeps its own banner.
fn edit_failed(state: &mut State, what: &str, error: &BackendError) {
    let message = describe(error, state);
    if state.banner.as_deref() != Some(SESSION_EXPIRED) {
        state.banner = Some(format!("{what}: {message}"));
    }
}

fn on_like_result(
    state: &mut State,
    track_id: TrackId,
    liked: bool,
    result: Result<(), BackendError>,
) -> Vec<Effect> {
    let pending = take_pending(state, |p| p.like_edit() == Some((&track_id, liked, true)));
    // The opposite edit waiting behind this one.
    let waiting = state
        .pending
        .iter()
        .position(|p| matches!(p.like_edit(), Some((id, _, false)) if *id == track_id));
    let error = match result {
        Ok(()) => {
            if let Some(i) = waiting {
                let next = &mut state.pending[i];
                if let Pending::Like { sent, .. } | Pending::Unlike { sent, .. } = next {
                    *sent = true;
                }
                return next.like_effect().into_iter().collect();
            }
            let mut effects = Vec::new();
            // The liked list never had the track: fetch it as YouTube has it.
            if matches!(
                pending,
                Some(Pending::Like {
                    inserted: false,
                    ..
                })
            ) {
                effects.push(Effect::FetchLiked);
            }
            // The main playlist is the liked list: once the last like or
            // unlike is in, fetch it for YouTube's positions. A fetch while
            // others are in flight would drop their optimistic rows.
            let settled = !state.pending.iter().any(|p| p.like_edit().is_some());
            if settled && main_is_liked(state) {
                effects.push(Effect::FetchPlaylist(LIKED_MUSIC.to_owned()));
            }
            return effects;
        }
        Err(e) => e,
    };
    // YouTube still has the song as before this edit. An edit waiting
    // behind it is moot: undo it first, then this one.
    let waiting = waiting.map(|i| state.pending.remove(i));
    let cancelled = waiting.is_some();
    undo_like_edit(state, &track_id, waiting);
    undo_like_edit(state, &track_id, pending);
    state.sync_page();
    if cancelled {
        // The song ends as the user last left it: no news but a session
        // that expired.
        describe(&error, state);
        return Vec::new();
    }
    let what = if liked {
        "Could not like that song"
    } else {
        "Could not remove that song from liked songs"
    };
    edit_failed(state, what, &error);
    Vec::new()
}

/// Undoes a like or unlike on both lists, by content: a refresh may have
/// replaced the lists meanwhile.
fn undo_like_edit(state: &mut State, track_id: &TrackId, pending: Option<Pending>) {
    if let Loadable::Ready(list) = &mut state.liked {
        match &pending {
            Some(Pending::Like { .. }) => Arc::make_mut(list).retain(|t| t.id != *track_id),
            Some(Pending::Unlike {
                at: Some(at),
                track,
                ..
            }) if !list.iter().any(|t| t.id == *track_id) => {
                let at = (*at).min(list.len());
                Arc::make_mut(list).insert(at, Arc::clone(track));
            }
            _ => {}
        }
    }
    if let Some(view) = main_view_mut(state, LIKED_MUSIC) {
        match pending {
            Some(Pending::Like { in_main: true, .. }) => {
                if let Some(i) = view.position(track_id) {
                    view.remove(i);
                }
            }
            Some(Pending::Unlike {
                main: Some((at, entry)),
                track,
                ..
            }) if view.position(track_id).is_none() => view.insert(at, track, entry),
            _ => {}
        }
    }
}

/// The main playlist is Liked Music.
fn main_is_liked(state: &State) -> bool {
    state.main.as_ref().is_some_and(|(id, _)| id == LIKED_MUSIC)
}

/// The open main playlist's view, if `id` is the main playlist.
fn main_view_mut<'a>(state: &'a mut State, id: &str) -> Option<&'a mut PlaylistView> {
    match &mut state.main {
        Some((main, Loadable::Ready(view))) if main == id => Some(Arc::make_mut(view)),
        _ => None,
    }
}

fn on_added(
    state: &mut State,
    playlist_id: String,
    track_ids: Vec<TrackId>,
    result: Result<Vec<Option<String>>, BackendError>,
) -> Vec<Effect> {
    take_pending(state, |p| {
        matches!(p, Pending::Add { playlist_id: pid, track_ids: ids }
            if *pid == playlist_id && *ids == track_ids)
    });
    // The optimistic copies are the last ones with no entry id.
    let unconfirmed = |view: &PlaylistView, id: &str| {
        view.tracks
            .iter()
            .zip(view.entry_ids.iter())
            .rposition(|(t, e)| t.id == id && e.is_none())
    };
    let mut effects = Vec::new();
    match result {
        Ok(entry_ids) => {
            if let Some(view) = main_view_mut(state, &playlist_id) {
                for (id, entry) in track_ids.iter().zip(entry_ids) {
                    if let Some(i) = unconfirmed(view, id) {
                        Arc::make_mut(&mut view.entry_ids)[i] = entry;
                    }
                }
            }
            let colors = Arc::clone(&state.art_colors);
            let uncolored: Vec<_> = track_ids
                .iter()
                .filter(|id| !colors.contains_key(*id))
                .filter_map(|id| state.find_track(id))
                .filter_map(|t| crate::thumbs::color_job(&t))
                .collect();
            if !uncolored.is_empty() {
                effects.push(Effect::NeedArtColors(uncolored));
            }
        }
        Err(e) => {
            if let Some(view) = main_view_mut(state, &playlist_id) {
                for id in &track_ids {
                    if let Some(i) = unconfirmed(view, id) {
                        Arc::make_mut(&mut view.tracks).remove(i);
                        Arc::make_mut(&mut view.entry_ids).remove(i);
                    }
                }
                view.summary.track_count = Some(view.tracks.len() as u64);
            }
            edit_failed(state, "Could not add to the main playlist", &e);
        }
    }
    state.sync_page();
    effects
}

fn on_removed(
    state: &mut State,
    playlist_id: String,
    entries: Vec<(TrackId, String)>,
    result: Result<(), BackendError>,
) {
    let pending = take_pending(state, |p| {
        matches!(p, Pending::Remove { playlist_id: pid, entries: e, .. }
            if *pid == playlist_id && *e == entries)
    });
    let Err(e) = result else {
        return;
    };
    if let (Some(Pending::Remove { removed, .. }), Some(view)) =
        (pending, main_view_mut(state, &playlist_id))
    {
        let tracks = Arc::make_mut(&mut view.tracks);
        let entry_ids = Arc::make_mut(&mut view.entry_ids);
        for (i, track, entry) in removed {
            let present = entry.is_some() && entry_ids.contains(&entry);
            if !present {
                let at = i.min(tracks.len());
                tracks.insert(at, track);
                entry_ids.insert(at, entry);
            }
        }
        view.summary.track_count = Some(tracks.len() as u64);
    }
    edit_failed(state, "Could not remove from the main playlist", &e);
    state.sync_page();
}

fn on_radio(
    state: &mut State,
    seed: TrackId,
    result: Result<Vec<Track>, BackendError>,
) -> Vec<Effect> {
    if state.radio_for.as_ref() == Some(&seed) {
        state.radio_for = None;
    }
    let tracks = match result {
        Ok(tracks) => tracks,
        // Radio is a bonus: only an expired session is worth a banner. An
        // empty extension stops it from firing again for this run-out.
        Err(e) => {
            if matches!(e, BackendError::Auth) {
                describe(&e, state);
            }
            Vec::new()
        }
    };
    let last = state.queue.entries().last().map(|e| e.track.id.as_str());
    // The queue changed since the fetch; `autoplay` asks again.
    if !state.queue.needs_radio() || last != Some(seed.as_str()) {
        return Vec::new();
    }
    state
        .queue
        .extend_radio(tracks.into_iter().map(Arc::new).collect());
    if std::mem::take(&mut state.resume_on_radio) && state.queue.advance(true).is_some() {
        return load_current(state, true);
    }
    Vec::new()
}

fn restart(state: &mut State, now: Instant) -> Vec<Effect> {
    match state.playback.status {
        PlaybackStatus::Stopped => load_current(state, true),
        _ => apply_command(state, &Command::Seek(Duration::ZERO), now),
    }
}

/// Fetches the library, liked songs and the main playlist. Cached data
/// stays on screen while the fetch runs. With `if_stale`, data the store
/// fetched in the last 10 minutes is shown from the cache instead. Logged
/// out, there is no library to fetch.
fn refresh(state: &mut State, if_stale: bool) -> Vec<Effect> {
    if state.session != Session::LoggedIn {
        return Vec::new();
    }
    if state.library.ready().is_none() {
        state.library = Loadable::Loading;
    }
    if state.liked.ready().is_none() {
        state.liked = Loadable::Loading;
    }
    let mut feeds = vec![Feed::Library, Feed::Liked];
    if let Some((id, _)) = &state.main {
        feeds.push(Feed::Playlist(id.clone()));
    }
    feeds
        .into_iter()
        .map(|f| {
            if if_stale {
                Effect::RefreshIfStale(f)
            } else {
                f.fetch()
            }
        })
        .collect()
}

/// Logged out: nothing is loading any more. Cached data stays.
fn needs_login(state: &mut State) {
    fn mark<T>(slot: &mut Loadable<T>) {
        if matches!(slot, Loadable::Loading) {
            *slot = Loadable::Idle;
        }
    }
    mark(&mut state.library);
    mark(&mut state.liked);
}

/// An auth error while logged in ends the session with one banner. Network
/// errors never log the user out.
fn describe(error: &BackendError, state: &mut State) -> String {
    if matches!(error, BackendError::Auth) && state.session == Session::LoggedIn {
        state.banner = Some(SESSION_EXPIRED.to_owned());
        state.session = Session::LoggedOut;
        needs_login(state);
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
            // Saved cookies mean a restart: the cache is likely fresh.
            return refresh(state, saved);
        }
        // Offline at startup: keep showing the cached library. The backend
        // checks the saved cookies again before each library fetch.
        Err(BackendError::Network(_)) if saved => {
            state.banner = Some(OFFLINE.to_owned());
            return refresh(state, true);
        }
        Err(_) if saved => {
            state.session = Session::LoggedOut;
            state.banner = Some(SESSION_EXPIRED.to_owned());
            needs_login(state);
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
    let mut entry_ids = p.entry_ids;
    // The store returns no entry ids when it has none; keep them parallel.
    entry_ids.resize(p.tracks.len(), None);
    Arc::new(PlaylistView {
        summary: p.summary,
        tracks: arc_tracks(p.tracks),
        entry_ids: Arc::new(entry_ids),
    })
}

/// The slots showing playlist `id`: the playlist page and the main playlist.
fn playlist_slots<'a>(
    state: &'a mut State,
    id: &str,
) -> impl Iterator<Item = &'a mut Loadable<Arc<PlaylistView>>> {
    [state.playlist.as_mut(), state.main.as_mut()]
        .into_iter()
        .flatten()
        .filter(move |(open, _)| open == id)
        .map(|(_, page)| page)
}

/// Backend side only: folds in a result or a player event.
pub fn on_event(state: &mut State, event: Event, now: Instant) -> Vec<Effect> {
    let mut effects = fold_event(state, event, now);
    effects.extend(autoplay(state));
    effects
}

fn fold_event(state: &mut State, event: Event, now: Instant) -> Vec<Effect> {
    match event {
        // Results that land after a logout belong to the old account.
        Event::Library(_) | Event::Liked(_) if state.session != Session::LoggedIn => {}
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
        Event::Playlist { id, result } => match result {
            Ok(p) => {
                let view = playlist_view(p);
                for page in playlist_slots(state, &id) {
                    *page = Loadable::Ready(Arc::clone(&view));
                }
            }
            Err(e) => {
                let message = describe(&e, state);
                for page in playlist_slots(state, &id) {
                    if page.ready().is_none() {
                        *page = Loadable::Failed(message.clone());
                    }
                }
            }
        },
        // The backend loads the source itself; nothing in the state changes.
        Event::StreamReady { .. } => {}
        Event::StreamFailed { uid, error } => {
            let message = describe(&error, state);
            if state.queue.current().map(|e| e.uid) == Some(uid) {
                // An expired session keeps its own banner.
                if state.banner.as_deref() != Some(SESSION_EXPIRED) {
                    state.banner = Some(format!("Could not play this track: {message}"));
                }
                state.playback.stop();
                return vec![Effect::Stop];
            }
        }
        Event::TrackEnded { track_id } => {
            if state.current_track().map(|t| t.id.as_str()) != Some(track_id.as_str()) {
                return Vec::new();
            }
            // A short track can end before the next tick.
            let mut effects = tick(state, now);
            if state.queue.advance(true).is_some() {
                effects.extend(load_current(state, true));
                return effects;
            }
            state.resume_on_radio = state.queue.needs_radio();
            state.playback.stop();
            effects.push(Effect::Stop);
            return effects;
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
                needs_login(state);
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
        Event::CachedQueue {
            tracks,
            current,
            label,
            radio,
        } => {
            // Restore it stopped; the user presses play to resume.
            if state.queue.is_empty() {
                state
                    .queue
                    .play_list(&arc_tracks(tracks), current.unwrap_or(0), 0);
                state.queue.restore_extras(label, &radio);
            }
        }
        Event::CachedPlaylist(p) => {
            let id = p.summary.id.clone();
            let view = playlist_view(p);
            for page in playlist_slots(state, &id) {
                if page.ready().is_none() {
                    *page = Loadable::Ready(Arc::clone(&view));
                }
            }
        }
        Event::LikeResult {
            track_id,
            liked,
            result,
        } => return on_like_result(state, track_id, liked, result),
        Event::Added {
            playlist_id,
            track_ids,
            result,
        } => return on_added(state, playlist_id, track_ids, result),
        Event::Removed {
            playlist_id,
            entries,
            result,
        } => on_removed(state, playlist_id, entries, result),
        Event::Radio { seed, result } => return on_radio(state, seed, result),
        Event::ArtColors(colors) | Event::CachedArtColors(colors) => {
            Arc::make_mut(&mut state.art_colors).extend(colors);
        }
        Event::Album { id, result } => {
            let message = result.as_ref().err().map(|e| describe(e, state));
            if let Some((open, page)) = &mut state.album
                && *open == id
            {
                *page = match (result, message) {
                    (Ok(album), _) => Loadable::Ready(Arc::new(album)),
                    (Err(_), m) => Loadable::Failed(m.unwrap_or_default()),
                };
            }
        }
        Event::Artist { id, result } => {
            let message = result.as_ref().err().map(|e| describe(e, state));
            if let Some((open, page)) = &mut state.artist
                && *open == id
            {
                *page = match (result, message) {
                    (Ok(artist), _) => Loadable::Ready(Arc::new(artist)),
                    (Err(_), m) => Loadable::Failed(m.unwrap_or_default()),
                };
            }
        }
        Event::Lyrics { track_id, result } => {
            let message = result.as_ref().err().map(|e| describe(e, state));
            if let Some((id, page)) = &mut state.lyrics
                && *id == track_id
            {
                *page = match (result, message) {
                    (Ok(lyrics), _) => Loadable::Ready(lyrics.map(Arc::new)),
                    (Err(_), m) => Loadable::Failed(m.unwrap_or_default()),
                };
            }
        }
        Event::CachedSettings(settings) => {
            state.queue.set_autoplay(settings.autoplay);
            state.settings = settings;
        }
        Event::CachedPlayStats(stats) => {
            Arc::make_mut(&mut state.last_played).extend(stats);
        }
        Event::Played { track_id, at } => {
            let last = Arc::make_mut(&mut state.last_played);
            let entry = last.entry(track_id).or_insert(at);
            *entry = (*entry).max(at);
        }
        Event::CachedMainPlaylist(id) => {
            // The user may have picked one already.
            if state.main.is_some() {
                return Vec::new();
            }
            let open = state.playlist.as_ref().filter(|(open, _)| *open == id);
            let page = open.map_or(Loadable::Loading, |(_, page)| page.clone());
            state.main = Some((id.clone(), page));
            if state.session == Session::LoggedIn {
                return vec![Effect::RefreshIfStale(Feed::Playlist(id))];
            }
        }
    }
    Vec::new()
}

/// How long the current track must play to count as a play.
fn play_threshold(track: &Track) -> Duration {
    track
        .duration
        .map_or(PLAY_THRESHOLD, |d| (d / 2).min(PLAY_THRESHOLD))
}

/// Backend side only: records the current play once it has played long
/// enough. Runs on a timer (see [`next_wake`]) and before a track ends.
pub fn tick(state: &mut State, now: Instant) -> Vec<Effect> {
    if state.recorded_serial == state.play_serial {
        return Vec::new();
    }
    let Some(track) = state.current_track() else {
        return Vec::new();
    };
    let played = state.playback.position_at(now);
    if state.playback.status == PlaybackStatus::Stopped || played < play_threshold(track) {
        return Vec::new();
    }
    let effect = Effect::RecordPlay {
        track_id: track.id.clone(),
        played_ms: played.as_millis() as u64,
    };
    state.recorded_serial = state.play_serial;
    vec![effect]
}

/// Backend side only: when [`tick`] or [`sync_position`] next has work: the
/// play threshold, or the end of the buffering delay.
pub fn next_wake(state: &State, now: Instant) -> Option<Instant> {
    let pb = &state.playback;
    if pb.status != PlaybackStatus::Playing {
        return None;
    }
    let record = state
        .current_track()
        .filter(|_| state.recorded_serial != state.play_serial)
        .map(|t| now + play_threshold(t).saturating_sub(pb.position_at(now)));
    let buffering = pb
        .buffering_since
        .map(|since| since + BUFFERING_DELAY)
        .filter(|&at| at > now);
    record.into_iter().chain(buffering).min()
}

/// Backend side only: takes the player's position when it is playing the
/// current track, so the shown position follows the real one. Buffering
/// shows only once it has lasted [`BUFFERING_DELAY`]. A loading track's
/// clock starts here, once the player reports it playing.
pub fn sync_position(state: &mut State, player: &PlayerState, now: Instant) {
    let Some(track) = state.current_track() else {
        return;
    };
    if player.track_id.as_deref() != Some(track.id.as_str()) {
        return;
    }
    let pb = &mut state.playback;
    match (pb.status, player.status) {
        (PlaybackStatus::Playing | PlaybackStatus::Buffering, PlaybackStatus::Playing) => {
            pb.status = PlaybackStatus::Playing;
            pb.buffering_since = None;
            pb.loading = false;
            pb.position = player.position;
            pb.anchor = Some(now);
        }
        (PlaybackStatus::Playing, PlaybackStatus::Buffering) => {
            let since = *pb.buffering_since.get_or_insert(now);
            if now.saturating_duration_since(since) >= BUFFERING_DELAY {
                pb.status = PlaybackStatus::Buffering;
                pb.position = player.position;
                pb.anchor = None;
            }
        }
        (PlaybackStatus::Buffering, PlaybackStatus::Buffering) => pb.position = player.position,
        (PlaybackStatus::Paused, PlaybackStatus::Paused) => {
            pb.loading = false;
            pb.position = player.position;
        }
        _ => pb.buffering_since = None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
        started(&mut s, now);
        s
    }

    fn current(s: &State) -> &str {
        &s.current_track().unwrap().id
    }

    /// The player reports the current track playing from 0:00 at `now`, as
    /// it does once its prebuffer is queued.
    pub(crate) fn started(s: &mut State, now: Instant) {
        let player = PlayerState {
            status: PlaybackStatus::Playing,
            track_id: Some(current(s).to_owned()),
            position: Duration::ZERO,
            duration: None,
            volume: 1.0,
        };
        sync_position(s, &player, now);
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
        assert!(s.playback.is_playing());
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
        assert!(s.playback.is_playing());
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
    fn remove_from_queue_takes_one_upcoming_copy() {
        let now = Instant::now();
        let mut s = playing(&["a", "b"], 0, now);
        for _ in 0..3 {
            assert!(apply(&mut s, &Command::AddToQueue(track("x")), now).is_empty());
        }
        assert_eq!(queue_ids(&s), ["a", "b", "x", "x", "x"]);
        let fx = apply(&mut s, &Command::RemoveFromQueue("x".into()), now);
        assert!(fx.is_empty());
        assert_eq!(queue_ids(&s), ["a", "b", "x", "x"]);
        assert_eq!(current(&s), "a");
    }

    #[test]
    fn remove_from_queue_leaves_current_and_unqueued_alone() {
        let now = Instant::now();
        let mut s = playing(&["a", "b", "c"], 1, now);
        let before = s.playback.status;
        for id in ["a", "b", "nope"] {
            let fx = apply(&mut s, &Command::RemoveFromQueue(id.into()), now);
            assert!(fx.is_empty());
        }
        assert_eq!(queue_ids(&s), ["a", "b", "c"]);
        assert_eq!(current(&s), "b");
        assert_eq!(s.playback.status, before);
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
        assert_eq!(s.banner.as_deref(), Some(OFFLINE));
        assert_eq!(
            fx,
            [
                Effect::RefreshIfStale(Feed::Library),
                Effect::RefreshIfStale(Feed::Liked)
            ]
        );
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
            label: None,
            radio: Vec::new(),
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

    // Batch 3 wiring.

    fn playlist(id: &str, ids: &[&str]) -> Playlist {
        Playlist {
            summary: PlaylistSummary {
                name: "Everything".into(),
                ..summary(id)
            },
            tracks: ids.iter().map(|i| Track::clone(&track(i))).collect(),
            entry_ids: ids.iter().map(|i| Some(format!("e-{i}"))).collect(),
        }
    }

    /// Logged in, liked songs `liked`, main playlist `PLmain` holding `main`.
    fn with_main(main: &[&str], liked: &[&str]) -> State {
        let now = Instant::now();
        let mut s = logged_in();
        s.liked = Loadable::Ready(list(liked));
        on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        on_event(&mut s, Event::CachedPlaylist(playlist("PLmain", main)), now);
        s
    }

    fn liked_ids(s: &State) -> Vec<String> {
        s.liked
            .ready()
            .unwrap()
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    fn main_ids(s: &State) -> Vec<String> {
        s.main_view()
            .unwrap()
            .tracks
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    fn main_entries(s: &State) -> Vec<Option<String>> {
        s.main_view().unwrap().entry_ids.to_vec()
    }

    fn queue_ids(s: &State) -> Vec<String> {
        s.queue
            .entries()
            .iter()
            .map(|e| e.track.id.clone())
            .collect()
    }

    fn net() -> BackendError {
        BackendError::Network("down".into())
    }

    #[test]
    fn like_is_optimistic_and_rolls_back_with_one_banner() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b"], &["b"]);
        let fx = apply(&mut s, &Command::Like("a".into()), now);
        assert_eq!(fx, [Effect::Like("a".into())]);
        assert_eq!(liked_ids(&s), ["a", "b"]);
        assert!(s.is_liked("a"));

        let fx = on_event(
            &mut s,
            Event::LikeResult {
                track_id: "a".into(),
                liked: true,
                result: Err(net()),
            },
            now,
        );
        assert!(fx.is_empty());
        assert_eq!(liked_ids(&s), ["b"]);
        let banner = s.banner.clone().unwrap();
        assert!(banner.starts_with("Could not like"), "{banner}");
        assert_eq!(
            s.session,
            Session::LoggedIn,
            "network errors keep the session"
        );
        assert!(s.pending.is_empty());
    }

    #[test]
    fn like_sends_nothing_for_a_liked_song_or_an_unknown_list() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &["a"]);
        assert!(apply(&mut s, &Command::Like("a".into()), now).is_empty());
        assert_eq!(liked_ids(&s), ["a"]);

        s.liked = Loadable::Loading;
        assert!(apply(&mut s, &Command::Like("a".into()), now).is_empty());
        assert_eq!(s.banner.as_deref(), Some(LIKED_LOADING));

        let mut guest = State::default();
        assert!(apply(&mut guest, &Command::Like("a".into()), now).is_empty());
        assert_eq!(guest.banner.as_deref(), Some(SIGN_IN_TO_EDIT));
    }

    #[test]
    fn liking_an_unseen_track_refetches_liked_songs() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &[]);
        assert_eq!(
            apply(&mut s, &Command::Like("zz".into()), now),
            [Effect::Like("zz".into())]
        );
        let fx = on_event(
            &mut s,
            Event::LikeResult {
                track_id: "zz".into(),
                liked: true,
                result: Ok(()),
            },
            now,
        );
        assert_eq!(fx, [Effect::FetchLiked]);
    }

    #[test]
    fn failed_unlike_restores_the_song_in_place() {
        let now = Instant::now();
        let mut s = with_main(&[], &["a", "b", "c"]);
        assert_eq!(
            apply(&mut s, &Command::Unlike("b".into()), now),
            [Effect::Unlike("b".into())]
        );
        assert_eq!(liked_ids(&s), ["a", "c"]);
        on_event(
            &mut s,
            Event::LikeResult {
                track_id: "b".into(),
                liked: false,
                result: Err(net()),
            },
            now,
        );
        assert_eq!(liked_ids(&s), ["a", "b", "c"]);
        assert!(s.banner.is_some());
    }

    #[test]
    fn auth_error_on_a_write_ends_the_session_with_its_own_banner() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &[]);
        apply(&mut s, &Command::Like("a".into()), now);
        on_event(
            &mut s,
            Event::LikeResult {
                track_id: "a".into(),
                liked: true,
                result: Err(BackendError::Auth),
            },
            now,
        );
        assert_eq!(s.banner.as_deref(), Some(SESSION_EXPIRED));
        assert!(s.needs_login());
    }

    /// Logged in, liked songs `liked`, main playlist Liked Music holding
    /// `main` with no entry ids, as YouTube serves it.
    fn with_liked_main(main: &[&str], liked: &[&str]) -> State {
        let now = Instant::now();
        let mut s = logged_in();
        s.liked = Loadable::Ready(list(liked));
        let mut p = playlist(LIKED_MUSIC, main);
        p.entry_ids = vec![None; main.len()];
        on_event(&mut s, Event::CachedMainPlaylist(LIKED_MUSIC.into()), now);
        on_event(&mut s, Event::CachedPlaylist(p), now);
        s
    }

    fn like_result(
        s: &mut State,
        id: &str,
        liked: bool,
        result: Result<(), BackendError>,
    ) -> Vec<Effect> {
        let event = Event::LikeResult {
            track_id: id.into(),
            liked,
            result,
        };
        on_event(s, event, Instant::now())
    }

    fn refetch_liked_main() -> Effect {
        Effect::FetchPlaylist(LIKED_MUSIC.into())
    }

    #[test]
    fn liked_music_main_adds_by_liking_into_both_lists() {
        let now = Instant::now();
        let mut s = with_liked_main(&["a", "b"], &["a", "b"]);
        let fx = apply(
            &mut s,
            &Command::AddToMain(vec![track("c"), track("a"), track("d")]),
            now,
        );
        assert_eq!(fx, [Effect::Like("c".into()), Effect::Like("d".into())]);
        assert_eq!(liked_ids(&s), ["d", "c", "a", "b"]);
        assert_eq!(main_ids(&s), ["d", "c", "a", "b"]);
        assert_eq!(main_entries(&s), [None, None, None, None]);
        assert_eq!(s.main_view().unwrap().summary.track_count, Some(4));

        // One fetch for YouTube's positions, once the last like is in.
        assert!(like_result(&mut s, "c", true, Ok(())).is_empty());
        assert_eq!(
            like_result(&mut s, "d", true, Ok(())),
            [refetch_liked_main()]
        );
        assert!(s.pending.is_empty());
        assert!(s.banner.is_none());
    }

    #[test]
    fn liked_music_main_rolls_back_both_lists() {
        let now = Instant::now();
        let mut s = with_liked_main(&["a", "b"], &["a", "b"]);
        apply(&mut s, &Command::AddToMain(vec![track("c")]), now);
        assert!(like_result(&mut s, "c", true, Err(net())).is_empty());
        assert_eq!(liked_ids(&s), ["a", "b"]);
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert_eq!(s.main_view().unwrap().summary.track_count, Some(2));
        let banner = s.banner.take().unwrap();
        assert!(banner.starts_with("Could not like"), "{banner}");

        let fx = apply(&mut s, &Command::RemoveFromMain(vec!["b".into()]), now);
        assert_eq!(fx, [Effect::Unlike("b".into())]);
        assert_eq!(liked_ids(&s), ["a"]);
        assert_eq!(main_ids(&s), ["a"]);
        assert!(like_result(&mut s, "b", false, Err(net())).is_empty());
        assert_eq!(liked_ids(&s), ["a", "b"]);
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert!(s.banner.is_some());
        assert!(s.pending.is_empty());
    }

    #[test]
    fn liked_music_main_removes_by_unliking_without_entry_ids() {
        let now = Instant::now();
        // The liked list is stale and lacks "a"; removing it still unlikes.
        let mut s = with_liked_main(&["a", "b", "c"], &["b", "c"]);
        let fx = apply(
            &mut s,
            &Command::RemoveFromMain(vec!["a".into(), "c".into()]),
            now,
        );
        assert_eq!(fx, [Effect::Unlike("a".into()), Effect::Unlike("c".into())]);
        assert!(s.banner.is_none(), "{:?}", s.banner);
        assert_eq!(liked_ids(&s), ["b"]);
        assert_eq!(main_ids(&s), ["b"]);
        assert!(like_result(&mut s, "a", false, Ok(())).is_empty());
        // A failed unlike puts the song back where it was in each list.
        assert!(like_result(&mut s, "c", false, Err(net())).is_empty());
        assert_eq!(liked_ids(&s), ["b", "c"]);
        assert_eq!(main_ids(&s), ["b", "c"]);
    }

    #[test]
    fn hearts_change_liked_music_main_too() {
        let now = Instant::now();
        let mut s = with_liked_main(&["a", "b"], &["a", "b"]);
        s.search = Loadable::Ready(Arc::new(SearchView {
            tracks: list(&["x"]),
            results: SearchResults::default(),
        }));
        assert_eq!(
            apply(&mut s, &Command::Like("x".into()), now),
            [Effect::Like("x".into())]
        );
        assert_eq!(main_ids(&s), ["x", "a", "b"]);
        assert_eq!(
            like_result(&mut s, "x", true, Ok(())),
            [refetch_liked_main()]
        );

        apply(&mut s, &Command::Unlike("a".into()), now);
        assert_eq!(liked_ids(&s), ["x", "b"]);
        assert_eq!(main_ids(&s), ["x", "b"]);
        assert_eq!(
            like_result(&mut s, "a", false, Ok(())),
            [refetch_liked_main()]
        );
    }

    #[test]
    fn likes_leave_another_main_playlist_alone() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b"], &["b"]);
        apply(&mut s, &Command::Like("a".into()), now);
        apply(&mut s, &Command::Unlike("b".into()), now);
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert!(like_result(&mut s, "a", true, Ok(())).is_empty());
        assert!(like_result(&mut s, "b", false, Err(net())).is_empty());
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert_eq!(
            apply(&mut s, &Command::AddToMain(vec![track("c")]), now),
            [Effect::AddToPlaylist {
                playlist_id: "PLmain".into(),
                track_ids: vec!["c".into()],
            }]
        );
    }

    /// Issue #6: a heart toggled back while its request is in flight waits
    /// for the answer, then goes out alone.
    #[test]
    fn toggle_toggle_sends_the_second_once_the_first_answers() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b", "c"], &["a", "b", "c"]);
        assert_eq!(
            apply(&mut s, &Command::Unlike("b".into()), now),
            [Effect::Unlike("b".into())]
        );
        assert!(apply(&mut s, &Command::Like("b".into()), now).is_empty());
        // The screen shows where the user left it at once.
        assert_eq!(liked_ids(&s), ["b", "a", "c"]);
        assert!(s.is_liked("b"));
        assert_eq!(
            like_result(&mut s, "b", false, Ok(())),
            [Effect::Like("b".into())]
        );
        assert!(like_result(&mut s, "b", true, Ok(())).is_empty());
        assert_eq!(liked_ids(&s), ["b", "a", "c"]);
        assert!(s.pending.is_empty());
        assert!(s.banner.is_none());
    }

    #[test]
    fn toggle_toggle_toggle_sends_one_request() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b", "x"], &["a", "b"]);
        // From liked: ends unliked.
        assert_eq!(
            apply(&mut s, &Command::Unlike("b".into()), now),
            [Effect::Unlike("b".into())]
        );
        assert!(apply(&mut s, &Command::Like("b".into()), now).is_empty());
        assert!(apply(&mut s, &Command::Unlike("b".into()), now).is_empty());
        assert_eq!(liked_ids(&s), ["a"]);
        // From unliked: ends liked.
        assert_eq!(
            apply(&mut s, &Command::Like("x".into()), now),
            [Effect::Like("x".into())]
        );
        assert!(apply(&mut s, &Command::Unlike("x".into()), now).is_empty());
        assert!(apply(&mut s, &Command::Like("x".into()), now).is_empty());
        assert_eq!(liked_ids(&s), ["x", "a"]);
        assert!(like_result(&mut s, "b", false, Ok(())).is_empty());
        assert!(like_result(&mut s, "x", true, Ok(())).is_empty());
        assert_eq!(liked_ids(&s), ["x", "a"]);
        assert!(s.pending.is_empty());
    }

    #[test]
    fn failure_mid_toggle_rolls_back_to_what_youtube_has() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b", "c"], &["a", "b", "c"]);
        // The first fails with the second waiting: YouTube still has the
        // song where it was, which is where the user left it.
        apply(&mut s, &Command::Unlike("b".into()), now);
        apply(&mut s, &Command::Like("b".into()), now);
        assert!(like_result(&mut s, "b", false, Err(net())).is_empty());
        assert_eq!(liked_ids(&s), ["a", "b", "c"]);
        assert!(s.pending.is_empty());
        assert!(s.banner.is_none(), "{:?}", s.banner);
        // The second fails: back to after the first.
        apply(&mut s, &Command::Unlike("b".into()), now);
        apply(&mut s, &Command::Like("b".into()), now);
        assert_eq!(
            like_result(&mut s, "b", false, Ok(())),
            [Effect::Like("b".into())]
        );
        assert!(like_result(&mut s, "b", true, Err(net())).is_empty());
        assert_eq!(liked_ids(&s), ["a", "c"]);
        assert!(!s.is_liked("b"));
        assert!(s.pending.is_empty());
        let banner = s.banner.clone().unwrap();
        assert!(banner.starts_with("Could not like"), "{banner}");
    }

    #[test]
    fn a_like_in_flight_is_not_sent_twice() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &["a"]);
        // Not in any list, so the heart stays empty until YouTube answers.
        assert_eq!(
            apply(&mut s, &Command::Like("zz".into()), now),
            [Effect::Like("zz".into())]
        );
        assert!(apply(&mut s, &Command::Like("zz".into()), now).is_empty());
        assert_eq!(
            like_result(&mut s, "zz", true, Ok(())),
            [Effect::FetchLiked]
        );
        assert!(s.pending.is_empty());
    }

    #[test]
    fn liked_music_main_toggles_one_at_a_time_and_refetches_once() {
        let now = Instant::now();
        let mut s = with_liked_main(&["a", "b"], &["a", "b"]);
        assert_eq!(
            apply(&mut s, &Command::RemoveFromMain(vec!["a".into()]), now),
            [Effect::Unlike("a".into())]
        );
        assert!(apply(&mut s, &Command::AddToMain(vec![track("a")]), now).is_empty());
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert_eq!(liked_ids(&s), ["a", "b"]);
        // No fetch while the second is still to go.
        assert_eq!(
            like_result(&mut s, "a", false, Ok(())),
            [Effect::Like("a".into())]
        );
        assert_eq!(
            like_result(&mut s, "a", true, Ok(())),
            [refetch_liked_main()]
        );
        assert!(s.pending.is_empty());

        // A failure with the second waiting puts both lists back.
        apply(&mut s, &Command::Unlike("b".into()), now);
        apply(&mut s, &Command::Like("b".into()), now);
        assert_eq!(main_ids(&s), ["b", "a"]);
        assert!(like_result(&mut s, "b", false, Err(net())).is_empty());
        assert_eq!(main_ids(&s), ["a", "b"]);
        assert_eq!(liked_ids(&s), ["a", "b"]);
        assert!(s.pending.is_empty());
    }

    #[test]
    fn add_and_remove_round_trip_with_entry_ids() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b"], &[]);
        // Songs already in the playlist are skipped.
        let fx = apply(
            &mut s,
            &Command::AddToMain(vec![track("b"), track("c"), track("c")]),
            now,
        );
        assert_eq!(
            fx,
            [Effect::AddToPlaylist {
                playlist_id: "PLmain".into(),
                track_ids: vec!["c".into()],
            }]
        );
        assert_eq!(main_ids(&s), ["a", "b", "c"]);
        assert_eq!(main_entries(&s)[2], None);
        assert_eq!(s.main_view().unwrap().summary.track_count, Some(3));

        // Removing before YouTube confirms has no entry id to send.
        let fx = apply(&mut s, &Command::RemoveFromMain(vec!["c".into()]), now);
        assert!(fx.is_empty());
        assert_eq!(s.banner.as_deref(), Some(UNKNOWN_ENTRY));
        assert_eq!(main_ids(&s), ["a", "b", "c"]);

        on_event(
            &mut s,
            Event::Added {
                playlist_id: "PLmain".into(),
                track_ids: vec!["c".into()],
                result: Ok(vec![Some("e-new".into())]),
            },
            now,
        );
        assert_eq!(main_entries(&s)[2].as_deref(), Some("e-new"));

        let fx = apply(
            &mut s,
            &Command::RemoveFromMain(vec!["a".into(), "c".into()]),
            now,
        );
        let entries = vec![
            ("a".to_owned(), "e-a".to_owned()),
            ("c".to_owned(), "e-new".to_owned()),
        ];
        assert_eq!(
            fx,
            [Effect::RemoveFromPlaylist {
                playlist_id: "PLmain".into(),
                entries: entries.clone(),
            }]
        );
        assert_eq!(main_ids(&s), ["b"]);
        on_event(
            &mut s,
            Event::Removed {
                playlist_id: "PLmain".into(),
                entries,
                result: Ok(()),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["b"]);
        assert!(s.pending.is_empty());
    }

    #[test]
    fn failed_add_and_remove_roll_back() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b", "c"], &[]);
        apply(&mut s, &Command::AddToMain(vec![track("d")]), now);
        on_event(
            &mut s,
            Event::Added {
                playlist_id: "PLmain".into(),
                track_ids: vec!["d".into()],
                result: Err(net()),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["a", "b", "c"]);
        assert!(s.banner.take().is_some());

        let fx = apply(&mut s, &Command::RemoveFromMain(vec!["b".into()]), now);
        let [Effect::RemoveFromPlaylist { entries, .. }] = &fx[..] else {
            panic!("{fx:?}");
        };
        assert_eq!(main_ids(&s), ["a", "c"]);
        on_event(
            &mut s,
            Event::Removed {
                playlist_id: "PLmain".into(),
                entries: entries.clone(),
                result: Err(net()),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["a", "b", "c"]);
        assert_eq!(main_entries(&s)[1].as_deref(), Some("e-b"));
        assert!(s.banner.is_some());
    }

    #[test]
    fn edits_need_a_loaded_main_playlist() {
        let now = Instant::now();
        let mut s = logged_in();
        assert!(apply(&mut s, &Command::AddToMain(vec![track("a")]), now).is_empty());
        assert_eq!(s.banner.as_deref(), Some(NO_MAIN));
        on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        assert!(apply(&mut s, &Command::RemoveFromMain(vec!["a".into()]), now).is_empty());
        assert_eq!(s.banner.as_deref(), Some(MAIN_LOADING));
    }

    #[test]
    fn set_main_playlist_saves_and_opens_it() {
        let now = Instant::now();
        let mut s = logged_in();
        let fx = apply(&mut s, &Command::SetMainPlaylist("PLx".into()), now);
        assert_eq!(
            fx,
            [
                Effect::SaveMainPlaylist("PLx".into()),
                Effect::FetchPlaylist("PLx".into())
            ]
        );
        on_event(
            &mut s,
            Event::Playlist {
                id: "PLx".into(),
                result: Ok(playlist("PLx", &["a", "b"])),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["a", "b"]);
        let Some((open, Loadable::Ready(page))) = &s.playlist else {
            panic!("{:?}", s.playlist);
        };
        assert_eq!(open, "PLx");
        assert!(Arc::ptr_eq(page, s.main_view().unwrap()));

        // An edit shows on the open page too.
        apply(&mut s, &Command::AddToMain(vec![track("c")]), now);
        let Some((_, Loadable::Ready(page))) = &s.playlist else {
            unreachable!()
        };
        assert_eq!(page.tracks.len(), 3);
    }

    #[test]
    fn cached_main_playlist_is_refreshed_once_logged_in() {
        let now = Instant::now();
        // Before the saved login is confirmed: cache only.
        let mut s = State::default();
        let fx = on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        assert!(fx.is_empty());
        on_event(
            &mut s,
            Event::CachedPlaylist(playlist("PLmain", &["a"])),
            now,
        );
        assert_eq!(main_ids(&s), ["a"]);
        on_event(&mut s, Event::SavedLogin, now);
        let fx = login(&mut s, true, Ok(()));
        assert_eq!(
            fx,
            [
                Effect::RefreshIfStale(Feed::Library),
                Effect::RefreshIfStale(Feed::Liked),
                Effect::RefreshIfStale(Feed::Playlist("PLmain".into())),
            ]
        );

        // Already logged in: refresh at once.
        let mut s = logged_in();
        let fx = on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        assert_eq!(
            fx,
            [Effect::RefreshIfStale(Feed::Playlist("PLmain".into()))]
        );

        // Window focus: every feed, each only if stale; logged out, nothing.
        let fx = apply(&mut s, &Command::RefreshIfStale, now);
        assert_eq!(
            fx,
            [
                Effect::RefreshIfStale(Feed::Library),
                Effect::RefreshIfStale(Feed::Liked),
                Effect::RefreshIfStale(Feed::Playlist("PLmain".into())),
            ]
        );
        let mut guest = State {
            session: Session::LoggedOut,
            ..State::default()
        };
        assert!(apply(&mut guest, &Command::RefreshIfStale, now).is_empty());
        // A pasted login refreshes without the staleness check.
        let mut s = State::default();
        on_event(&mut s, Event::NoSavedLogin, now);
        apply(&mut s, &Command::Login("x".into()), now);
        assert_eq!(
            login(&mut s, false, Ok(())),
            [Effect::FetchLibrary, Effect::FetchLiked]
        );
    }

    #[test]
    fn network_playlist_replaces_cached_main_but_errors_keep_it() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &[]);
        on_event(
            &mut s,
            Event::Playlist {
                id: "PLmain".into(),
                result: Err(net()),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["a"]);
        on_event(
            &mut s,
            Event::Playlist {
                id: "PLmain".into(),
                result: Ok(playlist("PLmain", &["a", "b"])),
            },
            now,
        );
        assert_eq!(main_ids(&s), ["a", "b"]);
        on_event(
            &mut s,
            Event::CachedPlaylist(playlist("PLmain", &["z"])),
            now,
        );
        assert_eq!(main_ids(&s), ["a", "b"]);
    }

    #[test]
    fn playlist_without_entry_ids_gets_unknown_ones() {
        let now = Instant::now();
        let mut s = logged_in();
        on_event(&mut s, Event::CachedMainPlaylist("PLmain".into()), now);
        let mut p = playlist("PLmain", &["a", "b"]);
        p.entry_ids.clear();
        on_event(&mut s, Event::CachedPlaylist(p), now);
        assert_eq!(main_entries(&s), [None, None]);
        assert_eq!(s.main_playlist().unwrap().entry_ids, [None, None]);
    }

    #[test]
    fn play_scope_builds_labels_and_keeps_playlist_order() {
        let now = Instant::now();
        let ids: Vec<String> = (0..1535).map(|i| format!("t{i}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut s = with_main(&refs, &[]);

        let fx = apply(
            &mut s,
            &Command::PlayScope {
                scope: Scope::All,
                shuffle: false,
                seed: 1,
            },
            now,
        );
        assert!(matches!(&fx[..], [Effect::Load { track_id, .. }] if track_id == "t0"));
        assert_eq!(s.queue.playing_from(), Some("Everything \u{b7} 1,535"));
        assert_eq!(s.queue.len(), 1535);
        assert!(s.playback.is_playing());

        let filter = Scope::Filter {
            label: "Saint Juniper".into(),
            ids: Arc::new(vec!["t9".into(), "t3".into(), "nope".into()]),
        };
        apply(
            &mut s,
            &Command::PlayScope {
                scope: filter,
                shuffle: false,
                seed: 1,
            },
            now,
        );
        assert_eq!(queue_ids(&s), ["t3", "t9"], "playlist order");
        assert_eq!(s.queue.playing_from(), Some("Saint Juniper \u{b7} 2"));

        apply(
            &mut s,
            &Command::PlayScope {
                scope: Scope::Range { from: 379, to: 519 },
                shuffle: true,
                seed: 7,
            },
            now,
        );
        assert_eq!(s.queue.playing_from(), Some("#380 to #520 \u{b7} 141"));
        let mut got = queue_ids(&s);
        got.sort();
        let mut want: Vec<String> = ids[379..=519].to_vec();
        want.sort();
        assert_eq!(got, want, "shuffle plays every track in the range once");

        // A range past the end is clamped.
        apply(
            &mut s,
            &Command::PlayScope {
                scope: Scope::Range {
                    from: 1530,
                    to: 9000,
                },
                shuffle: false,
                seed: 1,
            },
            now,
        );
        assert_eq!(s.queue.playing_from(), Some("#1531 to #1535 \u{b7} 5"));
    }

    #[test]
    fn play_scope_with_nothing_to_play_keeps_the_queue() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &[]);
        apply(
            &mut s,
            &Command::PlayTracks {
                tracks: list(&["x"]),
                start: 0,
                seed: 1,
            },
            now,
        );
        let empty = Scope::Filter {
            label: "Nobody".into(),
            ids: Arc::new(Vec::new()),
        };
        let fx = apply(
            &mut s,
            &Command::PlayScope {
                scope: empty,
                shuffle: true,
                seed: 1,
            },
            now,
        );
        assert!(fx.is_empty());
        assert_eq!(current(&s), "x");
    }

    fn autoplaying(ids: &[&str], start: usize, now: Instant) -> State {
        let mut s = State::default();
        apply(&mut s, &Command::SetAutoplay(true), now);
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

    fn fetches_radio(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::FetchRadio(_)))
            .count()
    }

    #[test]
    fn autoplay_fetches_radio_once_while_the_last_track_plays() {
        let now = Instant::now();
        let mut s = autoplaying(&["a", "b"], 0, now);
        assert!(s.queue.autoplay());
        assert!(s.radio_for.is_none(), "not the last track yet");
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            now,
        );
        assert_eq!(fx.last(), Some(&Effect::FetchRadio("b".into())));
        // Further steps do not ask again while the fetch is in flight.
        let fx = apply(&mut s, &Command::SetVolume(0.5), now);
        assert_eq!(fetches_radio(&fx), 0);

        let fx = on_event(
            &mut s,
            Event::Radio {
                seed: "b".into(),
                result: Ok(vec![Track::clone(&track("r1")), Track::clone(&track("r2"))]),
            },
            now,
        );
        assert_eq!(fetches_radio(&fx), 0);
        assert_eq!(queue_ids(&s), ["a", "b", "r1", "r2"]);
        assert!(s.queue.entries()[2].radio);
        assert_eq!(current(&s), "b", "radio waits for the current track");

        // The next run-out asks again, for the new last track.
        on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "b".into(),
            },
            now,
        );
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "r1".into(),
            },
            now,
        );
        assert_eq!(fx.last(), Some(&Effect::FetchRadio("r2".into())));
    }

    #[test]
    fn late_radio_resumes_a_queue_that_ran_out() {
        let now = Instant::now();
        let mut s = autoplaying(&["a"], 0, now);
        // The fetch went out when "a" started.
        assert_eq!(s.radio_for.as_deref(), Some("a"));
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            now,
        );
        assert!(fx.contains(&Effect::Stop));
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);
        let fx = on_event(
            &mut s,
            Event::Radio {
                seed: "a".into(),
                result: Ok(vec![Track::clone(&track("r1"))]),
            },
            now,
        );
        assert!(matches!(&fx[..], [Effect::Load { track_id, .. }, ..] if track_id == "r1"));
        assert!(s.playback.is_playing());
    }

    #[test]
    fn failed_radio_does_not_refire() {
        let now = Instant::now();
        let mut s = autoplaying(&["a"], 0, now);
        let fx = on_event(
            &mut s,
            Event::Radio {
                seed: "a".into(),
                result: Err(net()),
            },
            now,
        );
        assert!(fx.is_empty());
        assert!(s.banner.is_none(), "radio failures are quiet");
        assert_eq!(
            fetches_radio(&apply(&mut s, &Command::SetVolume(0.1), now)),
            0
        );
    }

    #[test]
    fn stale_radio_for_a_replaced_queue_is_dropped() {
        let now = Instant::now();
        let mut s = autoplaying(&["a"], 0, now);
        let fx = apply(
            &mut s,
            &Command::PlayTracks {
                tracks: list(&["x"]),
                start: 0,
                seed: 1,
            },
            now,
        );
        assert_eq!(fetches_radio(&fx), 0, "one fetch in flight at a time");
        let fx = on_event(
            &mut s,
            Event::Radio {
                seed: "a".into(),
                result: Ok(vec![Track::clone(&track("r1"))]),
            },
            now,
        );
        assert_eq!(queue_ids(&s), ["x"]);
        assert_eq!(fx, [Effect::FetchRadio("x".into())]);
    }

    #[test]
    fn restored_stopped_queue_fetches_no_radio() {
        let now = Instant::now();
        let mut s = State::default();
        apply(&mut s, &Command::SetAutoplay(true), now);
        let fx = on_event(
            &mut s,
            Event::CachedQueue {
                tracks: vec![Track::clone(&track("a"))],
                current: Some(0),
                label: None,
                radio: Vec::new(),
            },
            now,
        );
        assert!(fx.is_empty());
        assert!(s.queue.needs_radio());
        // Pressing play starts the radio fetch.
        let fx = apply(&mut s, &Command::TogglePlay, now);
        assert_eq!(fetches_radio(&fx), 1);
    }

    #[test]
    fn guest_sees_sign_in_not_loading() {
        let now = Instant::now();
        let mut s = State::default();
        assert!(!s.needs_login(), "still checking the keyring");
        on_event(&mut s, Event::NoSavedLogin, now);
        assert!(s.needs_login());
        assert!(matches!(s.library, Loadable::Idle));
        assert!(matches!(s.liked, Loadable::Idle));
        assert!(apply(&mut s, &Command::Refresh, now).is_empty());
        assert!(matches!(s.liked, Loadable::Idle), "nothing starts loading");

        // A session that expires mid-load stops loading too.
        let mut s = logged_in();
        apply(&mut s, &Command::Refresh, now);
        on_event(&mut s, Event::Library(Err(BackendError::Auth)), now);
        assert!(s.needs_login());
        assert!(matches!(s.liked, Loadable::Idle));
    }

    #[test]
    fn logout_clears_user_data() {
        let now = Instant::now();
        let mut s = with_main(&["a"], &["a"]);
        apply(&mut s, &Command::OpenPlaylist("PLmain".into()), now);
        on_event(&mut s, Event::ArtColors(vec![("a".into(), [1, 2, 3])]), now);
        apply(&mut s, &Command::Like("zz".into()), now);
        assert_eq!(apply(&mut s, &Command::Logout, now), [Effect::Logout]);
        assert!(s.needs_login());
        assert!(s.main.is_none() && s.playlist.is_none());
        assert!(matches!(s.liked, Loadable::Idle));
        assert!(s.art_colors.is_empty());
        assert!(s.pending.is_empty());
        // A fetch that was in flight belongs to the old account.
        on_event(&mut s, Event::Library(Ok(vec![summary("PLold")])), now);
        assert!(s.library.ready().is_none());
    }

    #[test]
    fn art_colors_collect_in_the_state() {
        let now = Instant::now();
        let mut s = with_main(&["a", "b"], &[]);
        on_event(
            &mut s,
            Event::CachedArtColors(vec![("a".into(), [1, 2, 3])]),
            now,
        );
        let before = Arc::clone(&s.art_colors);
        on_event(&mut s, Event::ArtColors(vec![("b".into(), [4, 5, 6])]), now);
        assert_eq!(s.art_colors.get("a"), Some(&[1, 2, 3]));
        assert_eq!(s.art_colors.get("b"), Some(&[4, 5, 6]));
        assert_eq!(before.len(), 1, "snapshots keep their own copy");

        // A confirmed add asks for the new song's color.
        let mut c = (*track("c")).clone();
        c.thumbnails.push(model::Thumbnail {
            url: "https://art/c-60".into(),
            width: 60,
            height: 60,
        });
        apply(&mut s, &Command::AddToMain(vec![Arc::new(c)]), now);
        let fx = on_event(
            &mut s,
            Event::Added {
                playlist_id: "PLmain".into(),
                track_ids: vec!["c".into()],
                result: Ok(vec![Some("e-c".into())]),
            },
            now,
        );
        assert!(
            matches!(&fx[..], [Effect::NeedArtColors(t)] if t[..] == [("c".into(), "https://art/c-60".into())])
        );
    }

    fn player(status: PlaybackStatus, secs: u64) -> PlayerState {
        PlayerState {
            status,
            track_id: Some("a".into()),
            position: Duration::from_secs(secs),
            duration: None,
            volume: 1.0,
        }
    }

    #[test]
    fn buffering_shows_only_after_250_ms() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut s = playing(&["a"], 0, t0);
        // A seek within buffered audio: a short blip, no Buffering.
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 10), ms(0));
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        assert_eq!(next_wake(&s, ms(0)).unwrap(), ms(250));
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 10), ms(200));
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        sync_position(&mut s, &player(PlaybackStatus::Playing, 10), ms(240));
        assert_eq!(s.playback.status, PlaybackStatus::Playing);

        // The timer restarts with the next buffering.
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 11), ms(300));
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 11), ms(500));
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 11), ms(550));
        assert_eq!(s.playback.status, PlaybackStatus::Buffering);
        // Frozen while buffering.
        assert_eq!(s.playback.position_at(ms(5000)), Duration::from_secs(11));
        sync_position(&mut s, &player(PlaybackStatus::Playing, 11), ms(600));
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
    }

    fn player_on(id: &str, status: PlaybackStatus, secs: u64) -> PlayerState {
        PlayerState {
            track_id: Some(id.into()),
            ..player(status, secs)
        }
    }

    #[test]
    fn a_new_track_holds_its_clock_until_the_player_starts() {
        let t0 = Instant::now();
        let secs = |n| t0 + Duration::from_secs(n);
        let mut s = State::default();
        let play = Command::PlayTracks {
            tracks: list(&["a"]),
            start: 0,
            seed: 1,
        };
        apply(&mut s, &play, t0);
        assert!(s.playback.loading);
        assert!(s.playback.is_playing(), "it is on its way");
        assert_eq!(s.playback.status, PlaybackStatus::Buffering);
        assert_eq!(s.playback.position_at(secs(6)), Duration::ZERO);
        assert_eq!(next_wake(&s, secs(6)), None);
        assert!(tick(&mut s, secs(60)).is_empty(), "nothing played yet");

        // The player has the stream but is still prebuffering.
        sync_position(&mut s, &player(PlaybackStatus::Buffering, 0), secs(5));
        assert!(s.playback.loading);
        assert_eq!(s.playback.position_at(secs(6)), Duration::ZERO);

        sync_position(&mut s, &player(PlaybackStatus::Playing, 0), secs(6));
        assert!(!s.playback.loading);
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        assert_eq!(s.playback.position_at(secs(7)), Duration::from_secs(1));
    }

    #[test]
    fn only_a_loaded_track_resumes_at_once() {
        let t0 = Instant::now();
        let secs = |n| t0 + Duration::from_secs(n);
        let mut s = playing(&["a", "b"], 0, t0);
        apply(&mut s, &Command::TogglePlay, secs(10));
        apply(&mut s, &Command::Next, secs(10));
        assert!(s.playback.loading);
        assert_eq!(s.playback.status, PlaybackStatus::Paused);

        // Play before the player has the track: wait for it.
        assert_eq!(
            apply(&mut s, &Command::TogglePlay, secs(11)),
            [Effect::Play]
        );
        assert_eq!(s.playback.status, PlaybackStatus::Buffering);
        assert_eq!(s.playback.position_at(secs(20)), Duration::ZERO);
        assert_eq!(
            apply(&mut s, &Command::TogglePlay, secs(12)),
            [Effect::Pause]
        );
        assert_eq!(s.playback.position_at(secs(20)), Duration::ZERO);

        // The player loaded it paused; from here a resume is instant.
        sync_position(&mut s, &player_on("b", PlaybackStatus::Paused, 0), secs(13));
        assert!(!s.playback.loading);
        assert_eq!(
            apply(&mut s, &Command::TogglePlay, secs(14)),
            [Effect::Play]
        );
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        assert_eq!(s.playback.position_at(secs(19)), Duration::from_secs(5));
    }

    #[test]
    fn a_seek_while_loading_holds_the_new_position() {
        let t0 = Instant::now();
        let secs = |n| t0 + Duration::from_secs(n);
        let mut s = State::default();
        let play = Command::PlayTracks {
            tracks: list(&["a"]),
            start: 0,
            seed: 1,
        };
        apply(&mut s, &play, t0);
        let to = Duration::from_secs(30);
        assert_eq!(
            apply(&mut s, &Command::Seek(to), secs(1)),
            [Effect::Seek(to)]
        );
        assert!(s.playback.loading);
        assert_eq!(s.playback.position_at(secs(5)), to);

        sync_position(&mut s, &player(PlaybackStatus::Playing, 30), secs(5));
        assert_eq!(s.playback.position_at(secs(7)), Duration::from_secs(32));
    }

    #[test]
    fn a_failed_stream_ends_the_load() {
        let now = Instant::now();
        let mut s = State::default();
        let play = Command::PlayTracks {
            tracks: list(&["a"]),
            start: 0,
            seed: 1,
        };
        apply(&mut s, &play, now);
        let uid = s.queue.current().unwrap().uid;
        let failed = Event::StreamFailed {
            uid,
            error: BackendError::Network("offline".into()),
        };
        assert_eq!(on_event(&mut s, failed, now), [Effect::Stop]);
        assert!(!s.playback.loading);
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);
    }

    fn record_count(fx: &[Effect]) -> usize {
        fx.iter()
            .filter(|e| matches!(e, Effect::RecordPlay { .. }))
            .count()
    }

    #[test]
    fn a_play_is_recorded_once_after_30_seconds() {
        let t0 = Instant::now();
        let secs = |n| t0 + Duration::from_secs(n);
        let mut s = playing(&["a", "b"], 0, t0);
        assert_eq!(next_wake(&s, t0), Some(secs(30)));
        assert!(tick(&mut s, secs(29)).is_empty());
        let fx = tick(&mut s, secs(30));
        assert_eq!(
            fx,
            [Effect::RecordPlay {
                track_id: "a".into(),
                played_ms: 30_000
            }]
        );
        assert!(tick(&mut s, secs(60)).is_empty(), "once per play");
        assert_eq!(next_wake(&s, secs(60)), None);
        // Seeking back is the same play.
        apply(&mut s, &Command::Seek(Duration::ZERO), secs(61));
        assert!(tick(&mut s, secs(95)).is_empty());
    }

    #[test]
    fn short_tracks_count_at_half_and_repeats_count_again() {
        let t0 = Instant::now();
        let mut s = State::default();
        let short = Arc::new(Track {
            duration: Some(Duration::from_secs(20)),
            ..Track::clone(&track("s"))
        });
        apply(&mut s, &Command::PlayNow(short), t0);
        started(&mut s, t0);
        apply(&mut s, &Command::CycleRepeat, t0); // all
        apply(&mut s, &Command::CycleRepeat, t0); // one
        assert_eq!(record_count(&tick(&mut s, t0 + Duration::from_secs(10))), 1);
        // Repeat-one replays the same entry: a new play.
        let end = t0 + Duration::from_secs(20);
        on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "s".into(),
            },
            end,
        );
        assert_eq!(current(&s), "s");
        started(&mut s, end);
        assert_eq!(
            record_count(&tick(&mut s, end + Duration::from_secs(10))),
            1
        );
    }

    #[test]
    fn a_track_that_ends_before_the_tick_is_still_recorded() {
        let t0 = Instant::now();
        let mut s = playing(&["a", "b"], 0, t0);
        let fx = on_event(
            &mut s,
            Event::TrackEnded {
                track_id: "a".into(),
            },
            t0 + Duration::from_secs(100),
        );
        assert!(matches!(
            &fx[..],
            [Effect::RecordPlay { track_id, .. }, Effect::Load { .. }] if track_id == "a"
        ));
        // Paused or stopped tracks set no timer.
        apply(&mut s, &Command::TogglePlay, t0);
        assert_eq!(next_wake(&s, t0), None);
    }
}
