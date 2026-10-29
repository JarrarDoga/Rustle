//! The screens. Views read the state and push [`Action`]s; the app applies
//! them after drawing, so no view mutates app state while it is borrowed.
//!
//! Direction D (`docs/design/README.md`): the main playlist is the app.
//! `app.rs` lays out the shell (rail, browse panel, content, spine, player
//! bar, queue drawer) and calls one `show` per region. Each file here is
//! owned by one UI-batch worker; shared UI state lives in [`View`].

use std::sync::Arc;

use egui::{RichText, ScrollArea, Ui};
use model::Track;

use crate::state::{Command, Loadable, Scope};
use crate::theme;

pub mod album;
pub mod artist;
pub mod banners;
pub mod browse;
pub mod collection;
pub mod first_run;
pub mod library;
pub mod login;
pub mod lyrics;
pub mod player_bar;
pub mod playlist;
pub mod queue;
pub mod rail;
pub mod search;
pub mod settings;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Route {
    /// The main playlist ("Everything"). The app opens here.
    #[default]
    Collection,
    Search,
    /// Other playlists and liked songs. Rarely used, so it is one rail icon.
    Library,
    Liked,
    Playlist(String),
    Album(String),
    Artist(String),
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchTab {
    #[default]
    Songs,
    Albums,
    Artists,
    Playlists,
}

pub enum Action {
    Cmd(Command),
    Go(Route),
    /// `true`: browse without logging in. `false`: show the login screen.
    Guest(bool),
}

/// UI-only state: where the user is and what they are doing. Never sent to
/// the backend; the reducer does not see it.
#[allow(dead_code)] // ui-batch: fields the workers read
pub struct View {
    pub route: Route,
    pub search_text: String,
    pub search_tab: SearchTab,
    pub focus_search: bool,
    /// The queue drawer is open (D-Queue).
    pub show_queue: bool,
    /// Seek bar value while the user drags it, in seconds.
    pub seek_drag: Option<f32>,
    /// Volume to restore when unmuting.
    pub unmuted_volume: f32,
    /// Selected row: list id and the index into that table's `tracks`
    /// (not the display row), so a filter change keeps the same song.
    /// Transient: set it with [`View::select`]; [`View::follow_playback`]
    /// clears it when playback moves on, and Escape or Locate clear it.
    pub selected: Option<(egui::Id, usize)>,
    /// The selected row's track id, so playback moving on can tell the
    /// track the user just played from that row apart from any other.
    pub selected_track: Option<String>,
    /// The playing track's id as of the last [`View::follow_playback`].
    pub playing: Option<String>,
    pub collection: CollectionView,
    pub browse_tab: BrowseTab,
    pub settings: settings::SettingsView,
}

impl Default for View {
    fn default() -> Self {
        Self {
            route: Route::Collection,
            search_text: String::new(),
            search_tab: SearchTab::Songs,
            focus_search: false,
            show_queue: false,
            seek_drag: None,
            unmuted_volume: 0.8,
            selected: None,
            selected_track: None,
            playing: None,
            collection: CollectionView::default(),
            browse_tab: BrowseTab::Artists,
            settings: settings::SettingsView::default(),
        }
    }
}

/// The Collection screen's state, shared by the collection, browse panel,
/// spine and player bar. Positions are 0-based indices into the main
/// playlist's tracks (shown 1-based, "#412").
#[allow(dead_code)] // ui-batch: fields the workers read
#[derive(Debug, Clone, Default)]
pub struct CollectionView {
    /// Free text in the filter field: matches title, artist and album.
    pub filter_text: String,
    /// Removable chips in the filter field (D-Filter). Browse adds them.
    pub chips: Vec<Chip>,
    /// The "Not heard in 3 months" quick filter is on.
    pub not_heard: bool,
    pub sort: Sort,
    /// A spine range, `from..=to` (D-Range). Set by the spine.
    pub range: Option<(usize, usize)>,
    /// Positions the table shows, in display order: the filter, quick
    /// filter, range and sort applied. Kept current by
    /// [`collection::update`] before anything draws; read by the spine
    /// (filter ticks) and the scope.
    pub visible: Arc<Vec<usize>>,
    /// The scope the header buttons act on (`Shuffle 41`), kept current by
    /// [`collection::update`] with `visible`.
    pub scope: Option<Scope>,
    /// First and last positions on screen, written by the collection from
    /// the table; read by the spine (viewport box). One frame behind.
    pub on_screen: Option<(usize, usize)>,
    /// Scroll the table to this position on the next frame (Locate, spine
    /// click). The collection clears it once done.
    pub scroll_to: Option<usize>,
    /// Focus the filter field on the next frame (Ctrl+F in Batch 4).
    pub focus_filter: bool,
}

#[allow(dead_code)] // ui-batch: variants the workers construct
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chip {
    Artist(String),
    Album(String),
}

#[allow(dead_code)] // ui-batch: variants the workers construct
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// The order songs were added: the spine's eras.
    #[default]
    PlaylistOrder,
    Title,
    Artist,
    Album,
    LastPlayed,
    Time,
}

#[allow(dead_code)] // ui-batch: variants the workers construct
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowseTab {
    #[default]
    Artists,
    Albums,
}

impl View {
    /// Selects row `index` of table `list`, which shows `track`.
    pub fn select(&mut self, list: egui::Id, index: usize, track: &str) {
        self.selected = Some((list, index));
        self.selected_track = Some(track.to_owned());
    }

    pub fn deselect(&mut self) {
        self.selected = None;
        self.selected_track = None;
    }

    /// Called once a frame before drawing. When the playing track changes to
    /// anything but the selected row's track (the player advancing, next or
    /// previous from a key, the media keys or the socket), the selection
    /// goes: left in place it reads as a stale "now playing" marker. Playing
    /// the selected row itself (double-click, Enter) keeps it.
    pub fn follow_playback(&mut self, state: &crate::state::State) {
        let now = state.current_track().map(|t| t.id.as_str());
        if now == self.playing.as_deref() {
            return;
        }
        self.playing = now.map(str::to_owned);
        if self.selected_track.as_deref() != now {
            self.deselect();
        }
    }
}

pub type Out = Vec<Action>;

pub fn cmd(out: &mut Out, c: Command) {
    out.push(Action::Cmd(c));
}

pub fn open_playlist(out: &mut Out, id: &str) {
    cmd(out, Command::OpenPlaylist(id.to_owned()));
    out.push(Action::Go(Route::Playlist(id.to_owned())));
}

pub fn open_album(out: &mut Out, id: &str) {
    cmd(out, Command::OpenAlbum(id.to_owned()));
    out.push(Action::Go(Route::Album(id.to_owned())));
}

pub fn open_artist(out: &mut Out, id: &str) {
    cmd(out, Command::OpenArtist(id.to_owned()));
    out.push(Action::Go(Route::Artist(id.to_owned())));
}

/// Locate (player bar button, Ctrl+L): open the Collection scrolled to the
/// playing song. Drops whatever filter hides it, so it always lands there.
/// Does nothing when the song is not in the main playlist.
pub fn locate(state: &crate::state::State, view: &mut View, out: &mut Out) {
    let Some(pos) = state
        .current_track()
        .and_then(|t| state.main_position(&t.id))
    else {
        return;
    };
    out.push(Action::Go(Route::Collection));
    // The playing row is the one to look at now, not a selection elsewhere.
    view.deselect();
    let c = &mut view.collection;
    if !c.visible.contains(&pos) {
        c.filter_text.clear();
        c.chips.clear();
        c.not_heard = false;
        c.range = None;
    }
    c.scroll_to = Some(pos);
}

/// "A, B & C" style credit line.
pub fn artists(track: &Track) -> String {
    let names: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    names.join(", ")
}

/// The content region for every route except Collection, which `app.rs`
/// lays out itself (it needs the spine beside it).
pub fn page(ui: &mut Ui, state: &crate::state::State, view: &mut View, out: &mut Out) {
    match view.route.clone() {
        Route::Collection => collection::show(ui, state, view, out),
        Route::Search => search::show(ui, state, view, out),
        Route::Library => library::show(ui, state, out),
        Route::Liked => playlist::liked(ui, state, view, out),
        Route::Playlist(id) => playlist::show(ui, state, view, &id, out),
        Route::Album(id) => album::show(ui, state, view, &id, out),
        Route::Artist(id) => artist::show(ui, state, view, &id, out),
        Route::Settings => settings::show(ui, state, view, out),
    }
}

pub fn loading_or_error<T>(ui: &mut Ui, value: &Loadable<T>, what: &str) {
    let text = match value {
        Loadable::Failed(e) => format!("Could not load {what}: {e}"),
        _ => "Loading…".to_owned(),
    };
    ui.label(RichText::new(text).color(theme::pal().text_3));
}

/// A virtualized list of `n` rows of `height` px.
pub fn rows(ui: &mut Ui, salt: &str, height: f32, n: usize, mut row: impl FnMut(&mut Ui, usize)) {
    ScrollArea::vertical()
        .id_salt(salt)
        .auto_shrink([false, false])
        .show_rows(ui, height, n, |ui, range| {
            for i in range {
                row(ui, i);
            }
        });
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use model::PlaylistSummary;

    use super::*;
    use crate::queue::tests::track;
    use crate::state::{PlaylistView, State, apply};

    const IDS: [&str; 4] = ["a", "b", "c", "d"];

    fn list() -> egui::Id {
        egui::Id::NULL.with("table")
    }

    /// Plays `IDS` from `start`, as a double-click on that row does.
    fn play(s: &mut State, start: usize) {
        let tracks = Arc::new(IDS.iter().map(|id| track(id)).collect());
        let play = Command::PlayTracks {
            tracks,
            start,
            seed: 1,
        };
        apply(s, &play, Instant::now());
    }

    fn step(s: &mut State, view: &mut View, command: Command) {
        apply(s, &command, Instant::now());
        view.follow_playback(s);
    }

    #[test]
    fn double_click_play_keeps_the_row_until_playback_moves_on() {
        let mut s = State::default();
        let mut view = View::default();
        view.follow_playback(&s);
        view.select(list(), 1, "b");
        play(&mut s, 1);
        view.follow_playback(&s);
        assert_eq!(view.selected, Some((list(), 1)), "the row just played");

        step(&mut s, &mut view, Command::Next);
        assert_eq!(view.selected, None);
        assert_eq!(view.selected_track, None);

        // It stays clear as playback keeps moving.
        step(&mut s, &mut view, Command::Next);
        step(&mut s, &mut view, Command::Previous);
        assert_eq!(view.selected, None);
    }

    #[test]
    fn a_selected_row_goes_when_another_track_starts() {
        let mut s = State::default();
        play(&mut s, 0);
        let mut view = View::default();
        view.follow_playback(&s);
        view.select(list(), 3, "d");

        // Pausing leaves the selection alone.
        step(&mut s, &mut view, Command::TogglePlay);
        assert_eq!(view.selected, Some((list(), 3)));

        step(&mut s, &mut view, Command::Next);
        assert_eq!(view.selected, None);
    }

    #[test]
    fn playing_the_selected_row_with_enter_keeps_it() {
        let mut s = State::default();
        play(&mut s, 0);
        let mut view = View::default();
        view.follow_playback(&s);
        view.select(list(), 2, "c");
        play(&mut s, 2);
        view.follow_playback(&s);
        assert_eq!(view.selected, Some((list(), 2)));
    }

    #[test]
    fn locate_clears_the_selection() {
        let tracks: Vec<Arc<Track>> = IDS.iter().map(|id| track(id)).collect();
        let main = PlaylistView {
            summary: PlaylistSummary {
                id: "main".to_owned(),
                name: "Everything".to_owned(),
                owner: None,
                track_count: Some(tracks.len() as u64),
                thumbnails: Vec::new(),
            },
            entry_ids: Arc::new(vec![None; tracks.len()]),
            tracks: Arc::new(tracks),
        };
        let mut s = State {
            main: Some(("main".to_owned(), Loadable::Ready(Arc::new(main)))),
            ..State::default()
        };
        play(&mut s, 2);
        let mut view = View::default();
        view.follow_playback(&s);
        view.select(list(), 0, "a");
        let mut out = Vec::new();
        locate(&s, &mut view, &mut out);
        assert_eq!(view.collection.scroll_to, Some(2));
        assert_eq!(view.selected, None);
    }
}
