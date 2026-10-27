//! Search (D-Search): results on the shared table with the "In
//! Everything" column.
//!
//! Integrator stub (the old search page). The `pages` worker owns this file.

use std::sync::Arc;

use egui::{Key, RichText, Ui, vec2};

use crate::state::{Command, Loadable, State};
use crate::theme;
use crate::views::{Out, SearchTab, View, cmd, open_album, open_artist, open_playlist, rows};
use crate::widgets::{ArtKind, entity_row, title, track_table};

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    title(ui, "Search");
    ui.add_space(8.0);
    let resp = ui.add(
        egui::TextEdit::singleline(&mut view.search_text)
            .hint_text("What do you want to listen to?")
            .desired_width(420.0)
            .margin(vec2(12.0, 8.0))
            .font(theme::row()),
    );
    if view.focus_search {
        resp.request_focus();
        view.focus_search = false;
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        cmd(out, Command::Search(view.search_text.clone()));
    }
    ui.add_space(10.0);

    let view_data = state.search.ready();
    let count = |tab: SearchTab| {
        view_data.map_or(0, |v| match tab {
            SearchTab::Songs => v.tracks.len(),
            SearchTab::Albums => v.results.albums.len(),
            SearchTab::Artists => v.results.artists.len(),
            SearchTab::Playlists => v.results.playlists.len(),
        })
    };
    ui.horizontal(|ui| {
        for (tab, label) in [
            (SearchTab::Songs, "Songs"),
            (SearchTab::Albums, "Albums"),
            (SearchTab::Artists, "Artists"),
            (SearchTab::Playlists, "Playlists"),
        ] {
            let text = if view_data.is_some() {
                format!("{label}  {}", count(tab))
            } else {
                label.to_owned()
            };
            ui.selectable_value(&mut view.search_tab, tab, text);
        }
    });
    ui.add_space(8.0);

    let data = match &state.search {
        Loadable::Idle => {
            ui.label(
                RichText::new("Search for songs, albums, artists and playlists, then press Enter.")
                    .color(theme::pal().text_3),
            );
            return;
        }
        Loadable::Loading => {
            ui.label(RichText::new("Searching…").color(theme::pal().text_3));
            return;
        }
        Loadable::Failed(e) => {
            ui.label(RichText::new(format!("Search failed: {e}")).color(theme::accent()));
            return;
        }
        Loadable::Ready(v) => Arc::clone(v),
    };
    if count(view.search_tab) == 0 {
        ui.label(
            RichText::new(format!("No results for \"{}\".", state.search_query))
                .color(theme::pal().text_3),
        );
        return;
    }
    let r = &data.results;
    match view.search_tab {
        SearchTab::Songs => {
            track_table::show(
                ui,
                state,
                view,
                track_table::Table {
                    salt: "search-songs",
                    tracks: &data.tracks,
                    rows: None,
                    columns: track_table::OUTSIDE,
                    scroll_to: None,
                },
                out,
            );
        }
        SearchTab::Albums => rows(ui, "search-albums", 60.0, r.albums.len(), |ui, i| {
            let a = &r.albums[i];
            let by: Vec<&str> = a.artists.iter().map(|x| x.name.as_str()).collect();
            let year = a.year.map(|y| format!(" · {y}")).unwrap_or_default();
            let sub = format!("Album · {}{year}", by.join(", "));
            if entity_row(
                ui,
                60.0,
                &a.id,
                &a.name,
                &sub,
                ArtKind::Square(&a.thumbnails),
                false,
            )
            .clicked()
            {
                open_album(out, &a.id);
            }
        }),
        SearchTab::Artists => rows(ui, "search-artists", 60.0, r.artists.len(), |ui, i| {
            let a = &r.artists[i];
            if entity_row(
                ui,
                60.0,
                &a.id,
                &a.name,
                "Artist",
                ArtKind::Round(&a.thumbnails),
                false,
            )
            .clicked()
            {
                open_artist(out, &a.id);
            }
        }),
        SearchTab::Playlists => rows(ui, "search-playlists", 60.0, r.playlists.len(), |ui, i| {
            let p = &r.playlists[i];
            let sub = format!(
                "Playlist · {}",
                p.owner.as_deref().unwrap_or("YouTube Music")
            );
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
        }),
    }
}
