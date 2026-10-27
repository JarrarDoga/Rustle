//! Library: other playlists and liked songs. Rarely used in Direction D, so
//! it sits behind one rail icon. Also where the main playlist is changed.
//!
//! Integrator stub (the old library page). The `pages` worker owns this file.

use std::sync::Arc;

use egui::Ui;

use crate::state::{Loadable, State};
use crate::views::{Action, Out, Route, loading_or_error, open_playlist, rows};
use crate::widgets::{ArtKind, entity_row, title};

pub fn show(ui: &mut Ui, state: &State, out: &mut Out) {
    title(ui, "Your Library");
    ui.add_space(10.0);
    let liked = state.liked.ready().map_or(0, |l| l.len());
    let playlists = match &state.library {
        Loadable::Ready(p) => Arc::clone(p),
        other => {
            loading_or_error(ui, other, "your library");
            return;
        }
    };
    rows(ui, "library", 60.0, playlists.len() + 1, |ui, i| {
        if i == 0 {
            let sub = format!("Playlist · {liked} songs");
            if entity_row(
                ui,
                60.0,
                "liked",
                "Liked Songs",
                &sub,
                ArtKind::Liked,
                false,
            )
            .clicked()
            {
                out.push(Action::Go(Route::Liked));
            }
            return;
        }
        let p = &playlists[i - 1];
        let mut sub = format!("Playlist · {}", p.owner.as_deref().unwrap_or("You"));
        if let Some(n) = p.track_count {
            sub.push_str(&format!(" · {n} songs"));
        }
        if entity_row(
            ui,
            60.0,
            &p.id,
            &p.name,
            &sub,
            ArtKind::Square(&p.thumbnails),
            false,
        )
        .clicked()
        {
            open_playlist(out, &p.id);
        }
    });
}
