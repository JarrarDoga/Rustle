//! A playlist page other than the main one, and Liked songs, on the shared
//! table with the "In Everything" column.
//!
//! Integrator stub (the old playlist page). The `pages` worker owns this file.

use std::sync::Arc;
use std::time::Duration;

use egui::{Pos2, Rect, Sense, Ui, vec2};
use model::Track;

use crate::state::{Command, Loadable, State};
use crate::theme;
use crate::views::{Out, View, cmd, loading_or_error};
use crate::widgets::{
    ArtKind, art, fmt_time, icon_button, round_button, seed, text_line, track_table,
};

/// The header above a playlist: cover, name, details, play buttons.
fn page_header(
    ui: &mut Ui,
    key: &str,
    name: &str,
    detail: &str,
    kind: ArtKind<'_>,
    tracks: Option<&Arc<Vec<Arc<Track>>>>,
    out: &mut Out,
) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 168.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let art_rect = Rect::from_min_size(rect.min, vec2(160.0, 160.0));
    art(&painter, art_rect, key, name, kind);
    let x = art_rect.right() + 24.0;
    let w = rect.right() - x;
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 56.0),
        "PLAYLIST",
        theme::caption(),
        theme::pal().text_2,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 92.0),
        name,
        theme::page_title(),
        theme::pal().text_1,
        w,
    );
    text_line(
        &painter,
        Pos2::new(x, rect.top() + 132.0),
        detail,
        theme::secondary(),
        theme::pal().text_2,
        w,
    );
    let Some(tracks) = tracks.filter(|t| !t.is_empty()) else {
        return;
    };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 64.0), Sense::hover());
    let play_center = Pos2::new(row.left() + 28.0, row.center().y);
    if round_button(
        ui,
        play_center,
        26.0,
        "page-play",
        crate::icons::PLAY,
        theme::accent(),
    )
    .on_hover_text("Play")
    .clicked()
    {
        cmd(
            out,
            Command::PlayTracks {
                tracks: Arc::clone(tracks),
                start: 0,
                seed: seed(),
            },
        );
    }
    let shuffle = Rect::from_center_size(
        Pos2::new(row.left() + 84.0, row.center().y),
        vec2(36.0, 36.0),
    );
    if icon_button(
        ui,
        shuffle,
        "page-shuffle",
        crate::icons::SHUFFLE,
        22.0,
        false,
        "Shuffle play",
    )
    .clicked()
    {
        let s = seed();
        cmd(out, Command::SetShuffle { on: true, seed: s });
        cmd(
            out,
            Command::PlayTracks {
                tracks: Arc::clone(tracks),
                start: (s % tracks.len() as u64) as usize,
                seed: s,
            },
        );
    }
}

pub fn liked(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let tracks = state.liked.ready();
    let detail = format!("You · {} songs", tracks.map_or(0, |t| t.len()));
    page_header(
        ui,
        "liked",
        "Liked Songs",
        &detail,
        ArtKind::Liked,
        tracks,
        out,
    );
    match &state.liked {
        Loadable::Ready(tracks) => {
            table(ui, state, view, "liked", tracks, out);
        }
        other => loading_or_error(ui, other, "liked songs"),
    }
}

pub fn show(ui: &mut Ui, state: &State, view: &mut View, id: &str, out: &mut Out) {
    let Some((open, page)) = state.playlist.as_ref().filter(|(open, _)| open == id) else {
        loading_or_error::<()>(ui, &Loadable::Loading, "this playlist");
        return;
    };
    let Loadable::Ready(p) = page else {
        // While loading, show what the library already knows.
        let name = state
            .library
            .ready()
            .and_then(|l| l.iter().find(|s| &s.id == open).map(|s| s.name.clone()))
            .unwrap_or_default();
        page_header(ui, open, &name, "", ArtKind::Square(&[]), None, out);
        loading_or_error(ui, page, "this playlist");
        return;
    };
    let mut detail = p
        .summary
        .owner
        .clone()
        .unwrap_or_else(|| "YouTube Music".into());
    detail.push_str(&format!(" · {} songs", p.tracks.len()));
    let total: Duration = p.tracks.iter().filter_map(|t| t.duration).sum();
    if !total.is_zero() {
        detail.push_str(&format!(" · {}", fmt_time(total)));
    }
    page_header(
        ui,
        &p.summary.id,
        &p.summary.name,
        &detail,
        ArtKind::Square(&p.summary.thumbnails),
        Some(&p.tracks),
        out,
    );
    table(ui, state, view, &format!("pl-{id}"), &p.tracks, out);
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
