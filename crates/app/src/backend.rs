//! The backend task: owns the authoritative [`State`], the music backend and
//! the playback, and publishes a snapshot after every batch of inputs.
//!
//! The UI thread never waits on it. It sends commands through [`Client`] and
//! picks up the newest snapshot when the `notify` callback wakes it.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use player::PlayerEvent;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, watch};
use ytm_core::MusicBackend;

use crate::playback::Playback;
use crate::state::{Command, Effect, Event, State, apply, on_event, sync_position};

/// Called after each new snapshot; the app passes `ctx.request_repaint()`.
pub type Notify = Box<dyn Fn() + Send + Sync>;

enum Input {
    Command(u64, Command),
    Event(Event),
}

type Inputs = mpsc::UnboundedSender<Input>;

pub struct Client {
    tx: Inputs,
    snapshots: watch::Receiver<Arc<State>>,
}

impl Client {
    pub fn send(&self, seq: u64, command: Command) {
        // Fails only if the backend task is gone, i.e. during shutdown.
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

/// Starts the backend task on `rt` and returns the UI's handle to it.
pub fn spawn<B: MusicBackend, P: Playback>(
    rt: &Handle,
    backend: B,
    playback: P,
    notify: Notify,
) -> Client {
    let (tx, rx) = mpsc::unbounded_channel();
    let (snap_tx, snap_rx) = watch::channel(Arc::new(State::default()));

    // The player reports on a std channel; one thread forwards into ours.
    let events = playback.events();
    let forward = tx.clone();
    std::thread::Builder::new()
        .name("player-events".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let event = match event {
                    PlayerEvent::TrackEnded { track_id } => Event::TrackEnded { track_id },
                    PlayerEvent::Error(message) => Event::PlayerError(message),
                    PlayerEvent::StatusChanged(_) => continue,
                };
                if forward.send(Input::Event(event)).is_err() {
                    return;
                }
            }
        })
        .expect("spawn player-events thread");

    rt.spawn(run(
        Arc::new(backend),
        playback,
        rx,
        tx.clone(),
        snap_tx,
        notify,
    ));
    Client {
        tx,
        snapshots: snap_rx,
    }
}

async fn run<B: MusicBackend, P: Playback>(
    backend: Arc<B>,
    playback: P,
    mut rx: mpsc::UnboundedReceiver<Input>,
    tx: Inputs,
    snapshots: watch::Sender<Arc<State>>,
    notify: Notify,
) {
    let mut state = State::default();
    playback.set_volume(state.playback.volume);
    while let Some(first) = rx.recv().await {
        // Handle everything already queued, then publish once.
        let mut next = Some(first);
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
                        playback.load(source);
                        if !state.playback.is_playing() {
                            playback.pause();
                        }
                    }
                    Vec::new()
                }
                Input::Event(event) => on_event(&mut state, event, now),
            };
            for effect in effects {
                execute(effect, &backend, &playback, &tx);
            }
            next = rx.try_recv().ok();
        }
        sync_position(&mut state, &playback.state(), Instant::now());
        snapshots.send_replace(Arc::new(state.clone()));
        notify();
    }
}

fn execute<B: MusicBackend, P: Playback>(
    effect: Effect,
    backend: &Arc<B>,
    playback: &P,
    tx: &Inputs,
) {
    match effect {
        Effect::FetchLibrary => fetch(backend, tx, |b| async move {
            Event::Library(b.library_playlists().await)
        }),
        Effect::FetchLiked => fetch(backend, tx, |b| async move {
            Event::Liked(b.liked_tracks().await)
        }),
        Effect::Search(query) => fetch(backend, tx, |b| async move {
            let result = b.search(&query).await;
            Event::Search { query, result }
        }),
        Effect::FetchPlaylist(id) => fetch(backend, tx, |b| async move {
            let result = b.playlist(&id).await;
            Event::Playlist { id, result }
        }),
        Effect::Load { uid, track_id } => {
            // Silence the old track now; the new one starts once resolved.
            playback.stop();
            fetch(backend, tx, move |b| async move {
                match b.stream(&track_id).await {
                    Ok(source) => Event::StreamReady { uid, source },
                    Err(error) => Event::StreamFailed { uid, error },
                }
            });
        }
        Effect::Play => playback.play(),
        Effect::Pause => playback.pause(),
        Effect::Stop => playback.stop(),
        Effect::Seek(to) => playback.seek(to),
        Effect::Volume(v) => playback.set_volume(v),
    }
}

fn fetch<B, F, Fut>(backend: &Arc<B>, tx: &Inputs, job: F)
where
    B: MusicBackend,
    F: FnOnce(Arc<B>) -> Fut,
    Fut: Future<Output = Event> + Send + 'static,
{
    let future = job(Arc::clone(backend));
    let tx = tx.clone();
    tokio::spawn(async move {
        let _ = tx.send(Input::Event(future.await));
    });
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    use player::PlaybackStatus;
    use ytm_core::FixtureBackend;

    use super::*;
    use crate::playback::SimulatedPlayback;
    use crate::state::Loadable;

    struct Harness {
        _rt: tokio::runtime::Runtime,
        client: Client,
        woken: std_mpsc::Receiver<()>,
    }

    impl Harness {
        fn new() -> Self {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .build()
                .unwrap();
            let (wake, woken) = std_mpsc::channel();
            let wake = std::sync::Mutex::new(wake);
            let client = spawn(
                rt.handle(),
                FixtureBackend::new(),
                SimulatedPlayback::new(),
                Box::new(move || {
                    let _ = wake.lock().unwrap().send(());
                }),
            );
            Self {
                _rt: rt,
                client,
                woken,
            }
        }

        /// Waits for a snapshot that satisfies `done`.
        fn wait_for(&mut self, what: &str, done: impl Fn(&State) -> bool) -> Arc<State> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(s) = self.client.latest()
                    && done(&s)
                {
                    return s;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if self.woken.recv_timeout(left).is_err() {
                    panic!("timed out waiting for {what}");
                }
            }
        }
    }

    #[test]
    fn demo_backend_loads_library_and_auto_advances() {
        let mut h = Harness::new();
        h.client.send(1, Command::Refresh);
        let s = h.wait_for("library", |s| {
            s.library.ready().is_some() && s.liked.ready().is_some()
        });
        assert_eq!(s.acked, 1);
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
        let fixture = ytm_core::FixtureBackend::new();
        let want_search = rt
            .block_on(ytm_core::MusicBackend::search(&fixture, "daft punk"))
            .unwrap();
        let want_focus = rt
            .block_on(ytm_core::MusicBackend::playlist(&fixture, "PLdemo_focus"))
            .unwrap();
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
}
