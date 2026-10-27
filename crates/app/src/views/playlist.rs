//! A playlist page other than the main one, and Liked songs, on the shared
//! table with the "In Everything" column. No board: D-Album's header and
//! table language at a smaller cover.

use std::sync::Arc;

use egui::Ui;
use model::Track;

use crate::state::{Loadable, NEEDS_LOGIN, PlaylistView, State};
use crate::theme;
use crate::views::album::{Art, Header, count_and_length, header, memo, note, play_buttons};
use crate::views::{Out, View, loading_or_error};
use crate::widgets::track_table;

pub fn liked(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let tracks = match &state.liked {
        Loadable::Ready(t) => Some(Arc::clone(t)),
        _ => None,
    };
    let detail = tracks.as_ref().map_or_else(String::new, |t| {
        memo(
            ui,
            egui::Id::unique("liked-detail"),
            Arc::clone(t),
            Arc::ptr_eq,
            || Arc::<str>::from(count_and_length(t.iter().map(|t| &**t))),
        )
        .to_string()
    });
    header(
        ui,
        Header {
            key: "liked",
            art: Art::Liked,
            art_size: theme::ARTIST_AVATAR,
            caption: "Playlist",
            title: "Liked songs",
            detail: &detail,
            status: None,
        },
        |ui| {
            if let Some(t) = &tracks {
                play_buttons(ui, t, "Shuffle", out);
            }
        },
    );
    match &state.liked {
        Loadable::Ready(tracks) => table(ui, state, view, "liked", tracks, out),
        Loadable::Idle if state.needs_login() => note(ui, NEEDS_LOGIN),
        other => loading_or_error(ui, other, "liked songs"),
    }
}

pub fn show(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((open, page)) = state.playlist.as_ref().filter(|(open, _)| open == id) else {
        return loading_or_error::<()>(ui, &Loadable::Loading, "this playlist");
    };
    let Loadable::Ready(p) = page else {
        // While loading, show what the library already knows.
        let summary = state
            .library
            .ready()
            .and_then(|l| l.iter().find(|s| &s.id == open).cloned());
        let (name, thumbs) =
            summary.map_or((String::new(), Vec::new()), |s| (s.name, s.thumbnails));
        header(
            ui,
            Header {
                key: open,
                art: Art::Square(&thumbs),
                art_size: theme::ARTIST_AVATAR,
                caption: "Playlist",
                title: &name,
                detail: "",
                status: None,
            },
            |_| {},
        );
        return loading_or_error(ui, page, "this playlist");
    };
    let detail = memo(
        ui,
        egui::Id::unique("playlist-detail"),
        Arc::clone(p),
        Arc::ptr_eq,
        || Arc::<str>::from(playlist_detail(p)),
    );
    header(
        ui,
        Header {
            key: &p.summary.id,
            art: Art::Square(&p.summary.thumbnails),
            art_size: theme::ARTIST_AVATAR,
            caption: "Playlist",
            title: &p.summary.name,
            detail: &detail,
            status: None,
        },
        |ui| play_buttons(ui, &p.tracks, "Shuffle", out),
    );
    table(ui, state, view, &format!("pl-{id}"), &p.tracks, out);
}

/// "Owner · 120 songs · 7 hr 3 min".
fn playlist_detail(p: &PlaylistView) -> String {
    let counts = count_and_length(p.tracks.iter().map(|t| &**t));
    match &p.summary.owner {
        Some(owner) => format!("{owner} · {counts}"),
        None => counts,
    }
}

fn table(
    ui: &mut Ui,
    state: &State,
    view: &mut View,
    salt: &str,
    tracks: &Arc<Vec<Arc<Track>>>,
    out: &mut Out,
) {
    track_table::show(
        ui,
        state,
        view,
        track_table::Table {
            salt,
            tracks,
            rows: None,
            columns: track_table::OUTSIDE,
            scroll_to: None,
        },
        out,
    );
}
