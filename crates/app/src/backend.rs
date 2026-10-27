//! The backend task: owns the authoritative [`State`], the music backend, the
//! playback and the local store, and publishes a snapshot after every batch of
//! inputs.
//!
//! The UI thread never waits on it. It sends commands through [`Client`] and
//! picks up the newest snapshot when the `notify` callback wakes it.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use model::{StreamSource, Track};
use player::{PlaybackStatus, PlayerEvent};
use store::Store;
use tokio::runtime::Handle;
use tokio::sync::{Mutex, mpsc, watch};
use ytm_core::{BackendError, MusicBackend};

use crate::playback::Playback;
use crate::queue::Queue;
use crate::state::{Command, Effect, Event, Session, State, apply, on_event, sync_position};

/// Called after each new snapshot; the app passes `ctx.request_repaint()`.
pub type Notify = Box<dyn Fn() + Send + Sync>;

/// How long after the last queue change the queue is written to the store.
const QUEUE_SAVE_DELAY: Duration = Duration::from_secs(2);
/// A resolved stream URL is reused for this long. YouTube's last a few hours.
const STREAM_FRESH: Duration = Duration::from_secs(60 * 60);
/// Resolved streams kept for reuse.
const STREAM_CACHE_CAP: usize = 8;
/// How long dropping the [`Client`] waits for the store's last write.
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

enum Input {
    Command(u64, Command),
    Event(Event),
    /// The next queue entry's stream, resolved ahead of time.
    Prefetched(StreamSource),
    /// The app is closing: save what is pending and stop.
    Shutdown,
}

type Inputs = mpsc::UnboundedSender<Input>;

fn send_event(tx: &Inputs, event: Event) {
    // Fails only if the backend task is gone, i.e. during shutdown.
    let _ = tx.send(Input::Event(event));
}

/// The saved-cookie functions. Plain function pointers so tests can swap in
/// fakes; they may block (D-Bus), so they always run on the blocking pool.
#[derive(Clone, Copy)]
pub struct Keyring {
    pub load: fn() -> Option<String>,
    pub save: fn(&str) -> Result<(), String>,
    pub clear: fn(),
}

impl Keyring {
    pub const SYSTEM: Keyring = Keyring {
        load: crate::auth::load_cookies,
        save: crate::auth::save_cookies,
        clear: crate::auth::clear_cookies,
    };
}

pub type OpenStore = Box<dyn FnOnce() -> Result<Store, String> + Send>;

/// What the backend task starts with.
pub struct Setup<F> {
    /// The UI starts from the same state, so the first frame matches.
    pub initial: State,
    /// Builds the music backend and the playback. Runs on the blocking pool:
    /// the real backend reads files and runs `rustypipe-botguard`.
    pub make: F,
    /// Opens the local store. Runs on the store thread.
    pub store: OpenStore,
    /// `None` skips the keyring: demo mode and tests.
    pub keyring: Option<Keyring>,
}

/// `~/.config/rustle/library.db`, next to rustypipe's cache.
pub fn library_db_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rustle").join("library.db"))
}

/// Opens the store at [`library_db_path`], creating its directory.
pub fn open_library_db() -> Result<Store, String> {
    let path = library_db_path().ok_or("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    Store::open(&path).map_err(|e| format!("open {}: {e}", path.display()))
}

pub struct Client {
    tx: Inputs,
    snapshots: watch::Receiver<Arc<State>>,
    /// Disconnects when the store thread has finished its last write.
    store_done: std_mpsc::Receiver<()>,
}

impl Client {
    pub fn send(&self, seq: u64, command: Command) {
        let _ = self.tx.send(Input::Command(seq, command));
    }

    /// The newest snapshot, if one arrived since the last call.
    pub fn latest(&mut self) -> Option<Arc<State>> {
        if !self.snapshots.has_changed().unwrap_or(false) {
            return None;
        }
        Some(Arc::clone(&self.snapshots.borrow_and_update()))
    }
}

impl Drop for Client {
    /// Flushes the debounced queue save. Waits a moment at most: the window
    /// is already closed by now.
    fn drop(&mut self) {
        if self.tx.send(Input::Shutdown).is_ok() {
            let _ = self.store_done.recv_timeout(SHUTDOWN_WAIT);
        }
    }
}

/// Starts the backend task on `rt` and returns the UI's handle to it at once;
/// the backend itself is built on the runtime.
pub fn spawn<B, P, F>(rt: &Handle, setup: Setup<F>, notify: Notify) -> Client
where
    B: MusicBackend,
    P: Playback,
    F: FnOnce() -> Result<(B, P), String> + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let (snap_tx, snap_rx) = watch::channel(Arc::new(setup.initial.clone()));
    let Setup {
        initial,
        make,
        store,
        keyring,
    } = setup;
    let (store, store_done) = StoreHandle::start(store, tx.clone());
    let start = Start {
        initial,
        make,
        keyring,
        store,
    };
    rt.spawn(run(start, rx, tx.clone(), snap_tx, notify));
    Client {
        tx,
        snapshots: snap_rx,
        store_done,
    }
}

type Job = Box<dyn FnOnce(&Store) + Send>;

/// The store lives on its own thread (it is `Send` but not `Sync`) and runs
/// jobs in order.
struct StoreHandle(std_mpsc::Sender<Job>);

impl StoreHandle {
    /// Opens the store on a new thread and reads the cached library, liked
    /// songs and queue into `tx` first.
    fn start(open: OpenStore, tx: Inputs) -> (Self, std_mpsc::Receiver<()>) {
        let (jobs, rx) = std_mpsc::channel::<Job>();
        let (done_tx, done_rx) = std_mpsc::channel::<()>();
        let spawned = std::thread::Builder::new()
            .name("rustle-store".into())
            .spawn(move || {
                let _done = done_tx;
                let store = match open() {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("rustle: library cache unavailable: {e}");
                        return;
                    }
                };
                read_cache(&store, &tx);
                while let Ok(job) = rx.recv() {
                    job(&store);
                }
            });
        if let Err(e) = spawned {
            eprintln!("rustle: cannot start the store thread: {e}");
        }
        (Self(jobs), done_rx)
    }

    fn run(&self, job: impl FnOnce(&Store) + Send + 'static) {
        // Fails only when the store could not open; the cache is optional.
        let _ = self.0.send(Box::new(job));
    }
}

fn read_cache(store: &Store, tx: &Inputs) {
    let log = |what: &str, e: store::StoreError| eprintln!("rustle: read cached {what}: {e}");
    match store.library_playlists() {
        Ok(v) if !v.is_empty() => send_event(tx, Event::CachedLibrary(v)),
        Ok(_) => {}
        Err(e) => log("library", e),
    }
    match store.liked() {
        Ok(v) if !v.is_empty() => send_event(tx, Event::CachedLiked(v)),
        Ok(_) => {}
        Err(e) => log("liked songs", e),
    }
    // Sent even when empty: it tells the backend it may start saving the queue.
    let (tracks, current) = store.load_queue().unwrap_or_else(|e| {
        log("queue", e);
        (Vec::new(), None)
    });
    send_event(tx, Event::CachedQueue { tracks, current });
}

fn log_store_error(what: &str, r: store::Result<()>) {
    if let Err(e) = r {
        eprintln!("rustle: save {what}: {e}");
    }
}

/// Identifies the queue's order and current entry, to notice changes.
fn queue_fingerprint(queue: &Queue) -> u64 {
    let mut h = DefaultHasher::new();
    for e in queue.entries() {
        e.uid.hash(&mut h);
    }
    queue.current_index().hash(&mut h);
    h.finish()
}

/// The entry that plays when the current one ends, unless it is the same one.
fn next_up(queue: &Queue) -> Option<Arc<Track>> {
    let current = queue.current()?.uid;
    let mut ahead = queue.clone();
    let next = ahead.advance(true)?;
    (next.uid != current).then(|| Arc::clone(&next.track))
}

struct Ctx<B, P> {
    backend: Arc<B>,
    playback: P,
    tx: Inputs,
    store: StoreHandle,
    keyring: Option<Keyring>,
    /// Saved cookies not yet accepted by the backend because YouTube was
    /// unreachable. Library fetches retry them once first.
    unverified: Arc<Mutex<Option<String>>>,
    /// Resolved streams by track id, with when they were resolved.
    streams: HashMap<String, (StreamSource, Instant)>,
    /// The (queue, stopped) pair the last prefetch decision was made for.
    prefetch_key: Option<(u64, bool)>,
    /// The queue as of the last batch, and when to save it next. The store
    /// reads the saved queue before it runs any write, so a save can never
    /// overwrite the queue before it is restored.
    queue_seen: u64,
    save_at: Option<tokio::time::Instant>,
}

/// [`Setup`] with the store already started.
struct Start<F> {
    initial: State,
    make: F,
    keyring: Option<Keyring>,
    store: StoreHandle,
}

async fn run<B, P, F>(
    setup: Start<F>,
    mut rx: mpsc::UnboundedReceiver<Input>,
    tx: Inputs,
    snapshots: watch::Sender<Arc<State>>,
    notify: Notify,
) where
    B: MusicBackend,
    P: Playback,
    F: FnOnce() -> Result<(B, P), String> + Send + 'static,
{
    let mut state = setup.initial;
    let built = tokio::task::spawn_blocking(setup.make)
        .await
        .unwrap_or_else(|e| Err(format!("backend setup panicked: {e}")));
    let (backend, playback) = match built {
        Ok(b) => b,
        Err(message) => {
            eprintln!("rustle: {message}");
            on_event(&mut state, Event::StartFailed(message), Instant::now());
            snapshots.send_replace(Arc::new(state));
            notify();
            // Nothing works without a backend; wait for the app to close.
            while let Some(input) = rx.recv().await {
                if matches!(input, Input::Shutdown) {
                    break;
                }
            }
            return;
        }
    };
    forward_player_events(playback.events(), tx.clone());
    playback.set_volume(state.playback.volume);

    let mut ctx = Ctx {
        backend: Arc::new(backend),
        playback,
        tx,
        store: setup.store,
        keyring: setup.keyring,
        unverified: Arc::default(),
        streams: HashMap::new(),
        prefetch_key: None,
        queue_seen: queue_fingerprint(&state.queue),
        save_at: None,
    };

    let mut effects = Vec::new();
    match state.session {
        Session::Starting => ctx.restore_login(),
        Session::LoggedIn => effects = apply(&mut state, &Command::Refresh, Instant::now()),
        _ => {}
    }
    for effect in effects {
        ctx.execute(effect);
    }
    ctx.publish(&mut state, &snapshots, &notify);

    loop {
        let first = match ctx.save_at {
            Some(at) => tokio::select! {
                input = rx.recv() => input,
                () = tokio::time::sleep_until(at) => {
                    ctx.save_queue(&state);
                    continue;
                }
            },
            None => rx.recv().await,
        };
        let Some(first) = first else { break };
        // Handle everything already queued, then publish once.
        let mut next = Some(first);
        let mut shutdown = false;
        while let Some(input) = next.take() {
            let now = Instant::now();
            let effects = match input {
                Input::Command(seq, command) => {
                    state.acked = seq;
                    apply(&mut state, &command, now)
                }
                Input::Event(Event::StreamReady { uid, source }) => {
                    // Drop streams for entries the user has already left.
                    if state.queue.current().map(|e| e.uid) == Some(uid) {
                        ctx.remember_stream(&source);
                        ctx.playback.load(source);
                        if !state.playback.is_playing() {
                            ctx.playback.pause();
                        }
                    }
                    Vec::new()
                }
                Input::Event(event) => {
                    ctx.persist(&state, &event);
                    let restoring = matches!(event, Event::CachedQueue { .. });
                    let touched = !state.queue.is_empty();
                    let effects = on_event(&mut state, event, now);
                    // A restored queue is already saved. One the user filled
                    // before the store answered is not, and stays changed.
                    if restoring && !touched {
                        ctx.queue_seen = queue_fingerprint(&state.queue);
                    }
                    effects
                }
                Input::Prefetched(source) => {
                    ctx.remember_stream(&source);
                    Vec::new()
                }
                Input::Shutdown => {
                    shutdown = true;
                    Vec::new()
                }
            };
            for effect in effects {
                ctx.execute(effect);
            }
            next = rx.try_recv().ok();
        }
        if shutdown {
            if ctx.save_at.is_some() {
                ctx.save_queue(&state);
            }
            break;
        }
        ctx.publish(&mut state, &snapshots, &notify);
    }
}

/// The player reports on a std channel; one thread forwards into ours.
fn forward_player_events(events: std_mpsc::Receiver<PlayerEvent>, tx: Inputs) {
    let spawned = std::thread::Builder::new()
        .name("player-events".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let event = match event {
                    PlayerEvent::TrackEnded { track_id } => Event::TrackEnded { track_id },
                    PlayerEvent::Error(message) => Event::PlayerError(message),
                    PlayerEvent::StatusChanged(_) => continue,
                };
                if tx.send(Input::Event(event)).is_err() {
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("rustle: cannot start the player-events thread: {e}");
    }
}

/// Checks the saved cookies once if they are still unverified. Cookies the
/// backend rejects are dropped; on a network error they are kept for the
/// next try.
async fn verify<B: MusicBackend>(
    backend: &B,
    unverified: &Mutex<Option<String>>,
) -> Result<(), BackendError> {
    let mut slot = unverified.lock().await;
    let Some(cookies) = slot.as_deref() else {
        return Ok(());
    };
    let result = backend.set_cookies(cookies).await;
    if !matches!(result, Err(BackendError::Network(_))) {
        *slot = None;
    }
    result
}

impl<B: MusicBackend, P: Playback> Ctx<B, P> {
    fn publish(
        &mut self,
        state: &mut State,
        snapshots: &watch::Sender<Arc<State>>,
        notify: &Notify,
    ) {
        sync_position(state, &self.playback.state(), Instant::now());
        self.after_batch(state);
        snapshots.send_replace(Arc::new(state.clone()));
        notify();
    }

    /// Schedules the queue save and the next track's prefetch.
    fn after_batch(&mut self, state: &State) {
        let fp = queue_fingerprint(&state.queue);
        if fp != self.queue_seen {
            self.queue_seen = fp;
            self.save_at = Some(tokio::time::Instant::now() + QUEUE_SAVE_DELAY);
        }
        let stopped = state.playback.status == PlaybackStatus::Stopped;
        if self.prefetch_key == Some((fp, stopped)) {
            return;
        }
        self.prefetch_key = Some((fp, stopped));
        // A queue restored at startup sits stopped; do not touch the network.
        if stopped {
            return;
        }
        if let Some(track) = next_up(&state.queue)
            && self.fresh_stream(&track.id).is_none()
        {
            let id = track.id.clone();
            let backend = Arc::clone(&self.backend);
            let tx = self.tx.clone();
            tokio::spawn(async move {
                // A failure here is retried by the normal load.
                if let Ok(source) = backend.stream(&id).await {
                    let _ = tx.send(Input::Prefetched(source));
                }
            });
        }
    }

    fn fresh_stream(&self, track_id: &str) -> Option<&StreamSource> {
        let (source, at) = self.streams.get(track_id)?;
        (at.elapsed() < STREAM_FRESH).then_some(source)
    }

    fn remember_stream(&mut self, source: &StreamSource) {
        self.streams
            .retain(|_, (_, at)| at.elapsed() < STREAM_FRESH);
        if self.streams.len() >= STREAM_CACHE_CAP
            && let Some(oldest) = self
                .streams
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(id, _)| id.clone())
        {
            self.streams.remove(&oldest);
        }
        self.streams
            .entry(source.track_id.clone())
            .or_insert_with(|| (source.clone(), Instant::now()));
    }

    fn save_queue(&mut self, state: &State) {
        self.save_at = None;
        let tracks: Vec<Track> = state
            .queue
            .entries()
            .iter()
            .map(|e| Track::clone(&e.track))
            .collect();
        let current = state.queue.current_index();
        self.store.run(move |store| {
            // `load_queue` skips ids without cached metadata, so cache the
            // tracks first; search results are not cached anywhere else.
            let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
            let r = store
                .upsert_tracks(&tracks)
                .and_then(|()| store.save_queue(&ids, current));
            log_store_error("queue", r);
        });
    }

    /// Writes fetch results to the store and reads cached pages from it.
    fn persist(&self, state: &State, event: &Event) {
        // Results that land after a logout belong to the old account.
        let logged_in = state.session == Session::LoggedIn;
        match event {
            Event::Library(Ok(v)) if logged_in => {
                let v = v.clone();
                self.store
                    .run(move |s| log_store_error("library", s.set_library_playlists(&v)));
            }
            Event::Liked(Ok(v)) if logged_in => {
                let v = v.clone();
                self.store
                    .run(move |s| log_store_error("liked songs", s.set_liked(&v)));
            }
            Event::Playlist { result: Ok(p), .. } => {
                let p = p.clone();
                self.store
                    .run(move |s| log_store_error("playlist", s.save_playlist(&p)));
            }
            _ => {}
        }
    }

    /// Looks for saved cookies and, if there are any, logs in with them.
    fn restore_login(&self) {
        let backend = Arc::clone(&self.backend);
        let unverified = Arc::clone(&self.unverified);
        let keyring = self.keyring;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let saved = match keyring {
                Some(k) => tokio::task::spawn_blocking(k.load).await.ok().flatten(),
                None => None,
            };
            let Some(cookies) = saved else {
                send_event(&tx, Event::NoSavedLogin);
                return;
            };
            *unverified.lock().await = Some(cookies);
            send_event(&tx, Event::SavedLogin);
            let result = verify(&*backend, &unverified).await;
            send_event(
                &tx,
                Event::Login {
                    saved: true,
                    result,
                },
            );
        });
    }

    fn execute(&mut self, effect: Effect) {
        match effect {
            Effect::FetchLibrary => {
                let unverified = Arc::clone(&self.unverified);
                self.fetch(|b| async move {
                    let r = match verify(&*b, &unverified).await {
                        Ok(()) => b.library_playlists().await,
                        Err(e) => Err(e),
                    };
                    Event::Library(r)
                })
            }
            Effect::FetchLiked => {
                let unverified = Arc::clone(&self.unverified);
                self.fetch(|b| async move {
                    let r = match verify(&*b, &unverified).await {
                        Ok(()) => b.liked_tracks().await,
                        Err(e) => Err(e),
                    };
                    Event::Liked(r)
                })
            }
            Effect::Search(query) => self.fetch(|b| async move {
                let result = b.search(&query).await;
                Event::Search { query, result }
            }),
            Effect::FetchPlaylist(id) => {
                let tx = self.tx.clone();
                let cached_id = id.clone();
                self.store.run(move |s| match s.playlist(&cached_id) {
                    Ok(Some(p)) => send_event(&tx, Event::CachedPlaylist(p)),
                    Ok(None) => {}
                    Err(e) => eprintln!("rustle: read cached playlist: {e}"),
                });
                self.fetch(|b| async move {
                    let result = b.playlist(&id).await;
                    Event::Playlist { id, result }
                })
            }
            Effect::Load { uid, track_id } => {
                // Silence the old track now; the new one starts once resolved.
                self.playback.stop();
                if let Some(source) = self.fresh_stream(&track_id) {
                    send_event(
                        &self.tx,
                        Event::StreamReady {
                            uid,
                            source: source.clone(),
                        },
                    );
                    return;
                }
                self.fetch(move |b| async move {
                    match b.stream(&track_id).await {
                        Ok(source) => Event::StreamReady { uid, source },
                        Err(error) => Event::StreamFailed { uid, error },
                    }
                });
            }
            Effect::Play => self.playback.play(),
            Effect::Pause => self.playback.pause(),
            Effect::Stop => self.playback.stop(),
            Effect::Seek(to) => self.playback.seek(to),
            Effect::Volume(v) => self.playback.set_volume(v),
            Effect::Login(cookies) => {
                let backend = Arc::clone(&self.backend);
                let unverified = Arc::clone(&self.unverified);
                let keyring = self.keyring;
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    *unverified.lock().await = None;
                    let result = backend.set_cookies(&cookies).await;
                    let ok = result.is_ok();
                    send_event(
                        &tx,
                        Event::Login {
                            saved: false,
                            result,
                        },
                    );
                    let Some(k) = keyring.filter(|_| ok) else {
                        return;
                    };
                    let saved = tokio::task::spawn_blocking(move || (k.save)(&cookies)).await;
                    if let Ok(Err(e)) = saved {
                        send_event(
                            &tx,
                            Event::Notice(format!("Logged in, but the login was not saved: {e}")),
                        );
                    }
                });
            }
            Effect::Logout => {
                let unverified = Arc::clone(&self.unverified);
                let keyring = self.keyring;
                tokio::spawn(async move {
                    *unverified.lock().await = None;
                    if let Some(k) = keyring {
                        let _ = tokio::task::spawn_blocking(k.clear).await;
                    }
                });
                // The cached library belongs to the account that left.
                self.store.run(|s| {
                    let r = s.set_library_playlists(&[]).and_then(|()| s.set_liked(&[]));
                    log_store_error("cleared library", r);
                });
            }
            // b3-wiring
            Effect::Like(_)
            | Effect::Unlike(_)
            | Effect::AddToPlaylist { .. }
            | Effect::RemoveFromPlaylist { .. }
            | Effect::SaveMainPlaylist(_)
            | Effect::FetchRadio(_)
            | Effect::RecordPlay { .. }
            | Effect::NeedArtColors(_) => {}
        }
    }

    fn fetch<J, Fut>(&self, job: J)
    where
        J: FnOnce(Arc<B>) -> Fut,
        Fut: Future<Output = Event> + Send + 'static,
    {
        let future = job(Arc::clone(&self.backend));
        let tx = self.tx.clone();
        tokio::spawn(async move {
            send_event(&tx, future.await);
        });
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use model::{PlaylistSummary, SearchResults};
    use ytm_core::FixtureBackend;

    use super::*;
    use crate::playback::SimulatedPlayback;
    use crate::state::{COOKIES_REJECTED, Loadable, SESSION_EXPIRED};

    /// [`FixtureBackend`] with switchable failures and a log of stream lookups.
    struct TestBackend {
        inner: FixtureBackend,
        flags: Arc<Flags>,
    }

    #[derive(Default)]
    struct Flags {
        reject_cookies: AtomicBool,
        library_auth: AtomicBool,
        streams: StdMutex<Vec<String>>,
    }

    impl Flags {
        fn stream_lookups(&self, id: &str) -> usize {
            self.streams
                .lock()
                .unwrap()
                .iter()
                .filter(|s| *s == id)
                .count()
        }
    }

    impl MusicBackend for TestBackend {
        async fn set_cookies(&self, cookies: &str) -> ytm_core::Result<()> {
            if self.flags.reject_cookies.load(Ordering::SeqCst) {
                return Err(BackendError::Auth);
            }
            self.inner.set_cookies(cookies).await
        }
        async fn search(&self, query: &str) -> ytm_core::Result<SearchResults> {
            self.inner.search(query).await
        }
        async fn library_playlists(&self) -> ytm_core::Result<Vec<PlaylistSummary>> {
            if self.flags.library_auth.load(Ordering::SeqCst) {
                return Err(BackendError::Auth);
            }
            self.inner.library_playlists().await
        }
        async fn playlist(&self, id: &str) -> ytm_core::Result<model::Playlist> {
            self.inner.playlist(id).await
        }
        async fn liked_tracks(&self) -> ytm_core::Result<Vec<Track>> {
            self.inner.liked_tracks().await
        }
        async fn stream(&self, track_id: &str) -> ytm_core::Result<StreamSource> {
            self.flags.streams.lock().unwrap().push(track_id.to_owned());
            self.inner.stream(track_id).await
        }
        async fn like(&self, track_id: &str) -> ytm_core::Result<()> {
            self.inner.like(track_id).await
        }
        async fn unlike(&self, track_id: &str) -> ytm_core::Result<()> {
            self.inner.unlike(track_id).await
        }
        async fn add_to_playlist(
            &self,
            playlist_id: &str,
            track_ids: &[model::TrackId],
        ) -> ytm_core::Result<Vec<Option<String>>> {
            self.inner.add_to_playlist(playlist_id, track_ids).await
        }
        async fn remove_from_playlist(
            &self,
            playlist_id: &str,
            entries: &[(model::TrackId, String)],
        ) -> ytm_core::Result<()> {
            self.inner.remove_from_playlist(playlist_id, entries).await
        }
        async fn radio(&self, track_id: &str) -> ytm_core::Result<Vec<Track>> {
            self.inner.radio(track_id).await
        }
        async fn clear_cookies(&self) -> ytm_core::Result<()> {
            self.inner.clear_cookies().await
        }
    }

    fn test_backend() -> (TestBackend, Arc<Flags>) {
        let flags = Arc::new(Flags::default());
        let backend = TestBackend {
            inner: FixtureBackend::new(),
            flags: Arc::clone(&flags),
        };
        (backend, flags)
    }

    fn logged_in() -> State {
        State {
            session: Session::LoggedIn,
            ..State::default()
        }
    }

    fn memory_store() -> OpenStore {
        Box::new(|| Store::open_in_memory().map_err(|e| e.to_string()))
    }

    /// A database file of this test's own, removed now and when dropped.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("rustle-{}-{name}.db", std::process::id()));
            let db = Self(path);
            db.remove();
            db
        }

        fn remove(&self) {
            for suffix in ["", "-wal", "-shm"] {
                let mut p = self.0.clone().into_os_string();
                p.push(suffix);
                let _ = std::fs::remove_file(Path::new(&p));
            }
        }

        fn open(&self) -> Store {
            Store::open(&self.0).unwrap()
        }

        fn opener(&self) -> OpenStore {
            let path = self.0.clone();
            Box::new(move || Store::open(&path).map_err(|e| e.to_string()))
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            self.remove();
        }
    }

    struct Harness {
        // Dropped first, so the backend saves before the runtime goes.
        client: Client,
        woken: std_mpsc::Receiver<()>,
        _rt: tokio::runtime::Runtime,
    }

    impl Harness {
        fn new() -> Self {
            Self::with(FixtureBackend::new(), logged_in(), memory_store(), None)
        }

        fn with<B: MusicBackend>(
            backend: B,
            initial: State,
            store: OpenStore,
            keyring: Option<Keyring>,
        ) -> Self {
            Self::build(
                move || Ok((backend, SimulatedPlayback::new())),
                initial,
                store,
                keyring,
            )
        }

        fn build<B, F>(make: F, initial: State, store: OpenStore, keyring: Option<Keyring>) -> Self
        where
            B: MusicBackend,
            F: FnOnce() -> Result<(B, SimulatedPlayback), String> + Send + 'static,
        {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap();
            let (wake, woken) = std_mpsc::channel();
            let wake = std::sync::Mutex::new(wake);
            let setup = Setup {
                initial,
                make,
                store,
                keyring,
            };
            let client = spawn(
                rt.handle(),
                setup,
                Box::new(move || {
                    let _ = wake.lock().unwrap().send(());
                }),
            );
            Self {
                client,
                woken,
                _rt: rt,
            }
        }

        fn wait_for(&mut self, what: &str, done: impl Fn(&State) -> bool) -> Arc<State> {
            self.wait_long(what, Duration::from_secs(5), done)
        }

        /// Waits for a snapshot that satisfies `done`.
        fn wait_long(
            &mut self,
            what: &str,
            limit: Duration,
            done: impl Fn(&State) -> bool,
        ) -> Arc<State> {
            let deadline = Instant::now() + limit;
            let mut last = None;
            loop {
                if let Some(s) = self.client.latest() {
                    if done(&s) {
                        return s;
                    }
                    last = Some(s);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if self.woken.recv_timeout(left).is_err() {
                    panic!("timed out waiting for {what}; last state: {last:#?}");
                }
            }
        }
    }

    /// Polls `done` for up to five seconds.
    fn eventually(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn ids(tracks: &[Arc<Track>]) -> Vec<String> {
        tracks.iter().map(|t| t.id.clone()).collect()
    }

    #[test]
    fn demo_backend_loads_library_and_auto_advances() {
        let mut h = Harness::new();
        h.client.send(1, Command::Refresh);
        let s = h.wait_for("library", |s| {
            s.acked == 1 && s.library.ready().is_some() && s.liked.ready().is_some()
        });
        assert_eq!(s.library.ready().unwrap().len(), 2);
        let liked = Arc::clone(s.liked.ready().unwrap());

        h.client.send(
            2,
            Command::PlayTracks {
                tracks: Arc::clone(&liked),
                start: 0,
                seed: 1,
            },
        );
        let s = h.wait_for("playing", |s| s.acked == 2);
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
        assert_eq!(s.current_track().unwrap().id, liked[0].id);

        // Let the stream resolve, then jump to just before the end.
        std::thread::sleep(Duration::from_millis(500));
        let end = liked[0].duration.unwrap();
        h.client
            .send(3, Command::Seek(end - Duration::from_millis(50)));
        let s = h.wait_for("next track", |s| {
            s.current_track().map(|t| &t.id) == Some(&liked[1].id)
        });
        assert_eq!(s.playback.status, PlaybackStatus::Playing);
    }

    #[test]
    fn search_and_playlist_round_trip() {
        // Expected counts come from the fixtures themselves, so re-recording
        // them in ytm-core does not break this test.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let fixture = FixtureBackend::new();
        let want_search = rt.block_on(fixture.search("daft punk")).unwrap();
        let want_focus = rt.block_on(fixture.playlist("PLdemo_focus")).unwrap();
        assert!(!want_search.tracks.is_empty() && !want_focus.tracks.is_empty());

        let mut h = Harness::new();
        h.client.send(1, Command::Search("daft punk".into()));
        let s = h.wait_for("search", |s| s.search.ready().is_some());
        let view = s.search.ready().unwrap();
        assert_eq!(view.tracks.len(), want_search.tracks.len());
        assert_eq!(view.results.artists.len(), want_search.artists.len());

        h.client
            .send(2, Command::OpenPlaylist("PLdemo_focus".into()));
        let s = h.wait_for("playlist", |s| {
            matches!(&s.playlist, Some((_, Loadable::Ready(_))))
        });
        let Some((_, Loadable::Ready(p))) = &s.playlist else {
            unreachable!()
        };
        assert_eq!(p.tracks.len(), want_focus.tracks.len());
    }

    #[test]
    fn saved_cookies_log_in_and_fetch_the_library() {
        let keyring = Keyring {
            load: || Some("SID=saved".to_owned()),
            save: |_| Ok(()),
            clear: || {},
        };
        let mut h = Harness::with(
            FixtureBackend::new(),
            State::default(),
            memory_store(),
            Some(keyring),
        );
        let s = h.wait_for("library", |s| s.library.ready().is_some());
        assert_eq!(s.session, Session::LoggedIn);
        assert!(s.banner.is_none());
    }

    static SAVED: StdMutex<Option<String>> = StdMutex::new(None);
    static CLEARED: AtomicBool = AtomicBool::new(false);

    #[test]
    fn login_saves_cookies_and_logout_clears_them() {
        let keyring = Keyring {
            load: || None,
            save: |c| {
                *SAVED.lock().unwrap() = Some(c.to_owned());
                Ok(())
            },
            clear: || CLEARED.store(true, Ordering::SeqCst),
        };
        let mut h = Harness::with(
            FixtureBackend::new(),
            State::default(),
            memory_store(),
            Some(keyring),
        );
        let s = h.wait_for("logged out", |s| s.session == Session::LoggedOut);
        assert!(matches!(s.library, Loadable::Idle));

        h.client.send(1, Command::Login("SID=pasted".into()));
        let s = h.wait_for("library", |s| {
            s.session == Session::LoggedIn && s.library.ready().is_some()
        });
        assert_eq!(s.library.ready().unwrap().len(), 2);
        eventually("save", || SAVED.lock().unwrap().is_some());
        assert_eq!(SAVED.lock().unwrap().as_deref(), Some("SID=pasted"));

        h.client.send(2, Command::Logout);
        let s = h.wait_for("logout", |s| s.acked == 2);
        assert_eq!(s.session, Session::LoggedOut);
        assert!(matches!(s.library, Loadable::Idle));
        eventually("clear", || CLEARED.load(Ordering::SeqCst));
    }

    #[test]
    fn rejected_cookies_fail_the_login() {
        let (backend, flags) = test_backend();
        flags.reject_cookies.store(true, Ordering::SeqCst);
        let mut h = Harness::with(backend, State::default(), memory_store(), None);
        h.wait_for("logged out", |s| s.session == Session::LoggedOut);
        h.client.send(1, Command::Login("SID=bad".into()));
        let s = h.wait_for("failure", |s| matches!(s.session, Session::Failed(_)));
        assert_eq!(s.session, Session::Failed(COOKIES_REJECTED.into()));
    }

    #[test]
    fn rejected_saved_cookies_ask_for_new_ones() {
        let (backend, flags) = test_backend();
        flags.reject_cookies.store(true, Ordering::SeqCst);
        let keyring = Keyring {
            load: || Some("SID=old".to_owned()),
            save: |_| Ok(()),
            clear: || {},
        };
        let mut h = Harness::with(backend, State::default(), memory_store(), Some(keyring));
        let s = h.wait_for("banner", |s| s.banner.is_some());
        assert_eq!(s.session, Session::LoggedOut);
        assert_eq!(s.banner.as_deref(), Some(SESSION_EXPIRED));
    }

    #[test]
    fn auth_error_later_ends_the_session() {
        let (backend, flags) = test_backend();
        flags.library_auth.store(true, Ordering::SeqCst);
        let mut h = Harness::with(backend, logged_in(), memory_store(), None);
        let s = h.wait_for("logged out", |s| s.session == Session::LoggedOut);
        assert_eq!(s.banner.as_deref(), Some(SESSION_EXPIRED));
    }

    #[test]
    fn library_reads_through_the_store() {
        let fixture = FixtureBackend::new();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let want_library = rt.block_on(fixture.library_playlists()).unwrap();
        let want_liked = rt.block_on(fixture.liked_tracks()).unwrap();

        let db = TempDb::new("read-through");
        let cached = Track {
            id: "cached00001".into(),
            ..want_liked[0].clone()
        };
        {
            let store = db.open();
            let summary = PlaylistSummary {
                id: "PLcached".into(),
                ..want_library[0].clone()
            };
            store.set_library_playlists(&[summary]).unwrap();
            store.set_liked(std::slice::from_ref(&cached)).unwrap();
            store.upsert_tracks(&want_liked[..2]).unwrap();
            let queue = [want_liked[0].id.clone(), want_liked[1].id.clone()];
            store.save_queue(&queue, Some(1)).unwrap();
        }

        // Logged out: nothing is fetched, so what shows came from the store.
        let mut h = Harness::with(FixtureBackend::new(), State::default(), db.opener(), None);
        let s = h.wait_for("cache", |s| {
            s.session == Session::LoggedOut && s.library.ready().is_some() && s.queue.len() == 2
        });
        assert_eq!(s.library.ready().unwrap()[0].id, "PLcached");
        assert_eq!(s.liked.ready().unwrap()[0].id, cached.id);
        assert_eq!(s.queue.current_index(), Some(1));
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);

        // Logging in refreshes from YouTube and writes the results back.
        h.client.send(1, Command::Login("SID=1".into()));
        let s = h.wait_for("refresh", |s| {
            s.library.ready().is_some_and(|l| l[0].id != "PLcached")
                && s.liked.ready().is_some_and(|l| l[0].id != cached.id)
        });
        assert_eq!(s.library.ready().unwrap().len(), want_library.len());
        drop(h);

        let store = db.open();
        assert_eq!(store.library_playlists().unwrap(), want_library);
        assert_eq!(store.liked().unwrap(), want_liked);
    }

    #[test]
    fn queue_is_saved_and_restored_stopped() {
        let db = TempDb::new("queue");
        let mut h = Harness::with(FixtureBackend::new(), logged_in(), db.opener(), None);
        let s = h.wait_for("liked", |s| s.liked.ready().is_some());
        let liked = Arc::clone(s.liked.ready().unwrap());
        h.client.send(
            1,
            Command::PlayTracks {
                tracks: Arc::clone(&liked),
                start: 2,
                seed: 1,
            },
        );
        h.wait_for("playing", |s| s.acked == 1);
        // Closing the app writes the queue without waiting out the debounce.
        drop(h);

        let (tracks, current) = db.open().load_queue().unwrap();
        assert_eq!(tracks.len(), liked.len());
        assert_eq!(current, Some(2));

        let mut h = Harness::with(FixtureBackend::new(), logged_in(), db.opener(), None);
        let s = h.wait_for("restored", |s| !s.queue.is_empty());
        let restored: Vec<Arc<Track>> = s
            .queue
            .entries()
            .iter()
            .map(|e| Arc::clone(&e.track))
            .collect();
        assert_eq!(ids(&restored), ids(&liked));
        assert_eq!(s.queue.current_index(), Some(2));
        assert_eq!(s.playback.status, PlaybackStatus::Stopped);
    }

    #[test]
    fn queue_save_is_debounced() {
        let db = TempDb::new("debounce");
        let mut h = Harness::with(FixtureBackend::new(), logged_in(), db.opener(), None);
        let s = h.wait_for("liked", |s| s.liked.ready().is_some());
        let liked = Arc::clone(s.liked.ready().unwrap());
        h.client.send(1, Command::AddToQueue(Arc::clone(&liked[0])));
        h.wait_for("queued", |s| s.acked == 1);
        let saved = || db.open().load_queue().unwrap().0.len();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(saved(), 0, "saved before the debounce delay");
        eventually("debounced save", || saved() == 1);
    }

    #[test]
    fn next_stream_is_prefetched_and_reused() {
        let (backend, flags) = test_backend();
        let mut h = Harness::with(backend, logged_in(), memory_store(), None);
        let s = h.wait_for("liked", |s| s.liked.ready().is_some());
        let liked = Arc::clone(s.liked.ready().unwrap());
        assert!(liked.len() >= 3);
        // A restored, stopped queue fetches nothing.
        assert!(flags.streams.lock().unwrap().is_empty());

        h.client.send(
            1,
            Command::PlayTracks {
                tracks: Arc::clone(&liked),
                start: 0,
                seed: 1,
            },
        );
        eventually("prefetch", || flags.stream_lookups(&liked[1].id) == 1);
        assert_eq!(flags.stream_lookups(&liked[2].id), 0, "only the next track");

        h.client.send(2, Command::Next);
        h.wait_for("next", |s| {
            s.current_track().map(|t| &t.id) == Some(&liked[1].id)
        });
        eventually("next prefetch", || flags.stream_lookups(&liked[2].id) == 1);
        assert_eq!(
            flags.stream_lookups(&liked[1].id),
            1,
            "prefetched URL reused"
        );
    }

    /// Searches YouTube Music and plays the first result through the real
    /// backend. Playback is simulated until the real player is wired in.
    #[test]
    #[ignore = "network: talks to YouTube Music"]
    fn real_backend_searches_and_plays() {
        let make = || {
            let music = ytm_core::RustyPipeBackend::new().map_err(|e| e.to_string())?;
            Ok((music, SimulatedPlayback::new()))
        };
        let mut h = Harness::build(make, State::default(), memory_store(), None);
        h.wait_long("start", Duration::from_secs(20), |s| {
            s.session == Session::LoggedOut
        });
        h.client
            .send(1, Command::Search("daft punk around the world".into()));
        let s = h.wait_long("search", Duration::from_secs(30), |s| {
            !matches!(s.search, Loadable::Loading | Loadable::Idle)
        });
        let Loadable::Ready(view) = &s.search else {
            panic!("search failed: {:?}", s.search);
        };
        let track = Arc::clone(view.tracks.first().expect("a search result"));
        eprintln!("playing {} ({})", track.title, track.id);
        h.client.send(2, Command::PlayNow(track));
        // The position moves only once the player has loaded the stream.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut seq = 2;
        loop {
            seq += 1;
            h.client.send(seq, Command::SetVolume(0.5));
            let s = h.wait_long("snapshot", Duration::from_secs(5), |s| s.acked == seq);
            assert!(s.banner.is_none(), "banner: {:?}", s.banner);
            if s.playback.position > Duration::ZERO {
                break;
            }
            assert!(Instant::now() < deadline, "stream never loaded");
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}
