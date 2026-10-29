//! Search (D-Search): results on the shared table with the "In
//! Everything" column.

use std::sync::Arc;

use egui::{
    Align2, CornerRadius, CursorIcon, Frame, Key, Margin, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Ui, vec2,
};

use crate::state::{Command, Loadable, SearchView, State};
use crate::views::album::{
    Art, LIST_INSET, ListRow, bleed, list_row, main_name, main_tracks, memo, note, rule, same,
};
use crate::views::artist::{card_subtitle, in_main_label, songs_in_main};
use crate::views::library::playlist_sub;
use crate::views::{Out, SearchTab, View, cmd, open_album, open_artist, open_playlist, rows};
use crate::widgets::track_table;
use crate::{icons, theme};

/// D-Search's field width. Not a D-Tokens size.
const FIELD_W: f32 = 560.0;
const TAB_H: f32 = 44.0;

const TABS: [(SearchTab, &str); 4] = [
    (SearchTab::Songs, "Songs"),
    (SearchTab::Albums, "Albums"),
    (SearchTab::Artists, "Artists"),
    (SearchTab::Playlists, "Playlists"),
];

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    field(ui, view, out);
    rule(ui);

    let data = state.search.ready().map(Arc::clone);
    tabs(ui, view, data.as_deref());
    rule(ui);

    let data = match &state.search {
        Loadable::Idle => {
            return note(
                ui,
                "Search YouTube Music for songs, albums, artists and playlists, then press Enter.",
            );
        }
        Loadable::Loading => return note(ui, "Searching…"),
        Loadable::Failed(e) => return note(ui, &format!("Search failed: {e}")),
        Loadable::Ready(v) => Arc::clone(v),
    };
    let tab = view.search_tab;
    if count(&data, tab) == 0 {
        let what = TABS
            .iter()
            .find(|(t, _)| *t == tab)
            .map_or("results", |(_, l)| l)
            .to_lowercase();
        return note(ui, &format!("No {what} for \"{}\".", state.search_query));
    }
    let main = main_tracks(state);
    let main_name = main.is_some().then(|| main_name(state));
    let counts = memo(
        ui,
        egui::Id::unique("search-counts"),
        (Arc::clone(&data), main.clone()),
        |a, b| Arc::ptr_eq(&a.0, &b.0) && same(&a.1, &b.1),
        || Arc::new(in_main_counts(&data, main.as_deref().map(|m| &m[..]))),
    );
    let r = &data.results;
    match tab {
        SearchTab::Songs => {
            ui.add_space(theme::SPACE_4);
            bleed(ui, 0.0, 0.0, |ui| {
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
                )
            });
        }
        SearchTab::Albums => {
            ui.add_space(theme::SPACE_8);
            bleed(ui, LIST_INSET.0, LIST_INSET.1, |ui| {
                rows(
                    ui,
                    "search-albums",
                    theme::QUEUE_ROW_H,
                    r.albums.len(),
                    |ui, i| {
                        let a = &r.albums[i];
                        let by: Vec<&str> = a.artists.iter().map(|x| x.name.as_str()).collect();
                        let mut sub = format!("{} · {}", type_label(a.album_type), by.join(", "));
                        let tail = card_subtitle(a.year, counts.albums[i], main_name);
                        if !tail.is_empty() {
                            sub = format!("{sub} · {tail}");
                        }
                        let row = ListRow {
                            key: &a.id,
                            art: Art::Square(&a.thumbnails),
                            title: &a.name,
                            sub: &sub,
                            chip: None,
                            reserve: 0.0,
                        };
                        if list_row(ui, row).clicked() {
                            open_album(out, &a.id);
                        }
                    },
                )
            });
        }
        SearchTab::Artists => {
            ui.add_space(theme::SPACE_8);
            bleed(ui, LIST_INSET.0, LIST_INSET.1, |ui| {
                rows(
                    ui,
                    "search-artists",
                    theme::QUEUE_ROW_H,
                    r.artists.len(),
                    |ui, i| {
                        let a = &r.artists[i];
                        let sub = match main_name {
                            Some(m) if counts.artists[i] > 0 => {
                                format!("Artist · {}", in_main_label(counts.artists[i], m))
                            }
                            _ => "Artist".to_owned(),
                        };
                        let row = ListRow {
                            key: &a.id,
                            art: Art::Round(&a.thumbnails),
                            title: &a.name,
                            sub: &sub,
                            chip: None,
                            reserve: 0.0,
                        };
                        if list_row(ui, row).clicked() {
                            open_artist(out, &a.id);
                        }
                    },
                )
            });
        }
        SearchTab::Playlists => {
            ui.add_space(theme::SPACE_8);
            bleed(ui, LIST_INSET.0, LIST_INSET.1, |ui| {
                rows(
                    ui,
                    "search-playlists",
                    theme::QUEUE_ROW_H,
                    r.playlists.len(),
                    |ui, i| {
                        let pl = &r.playlists[i];
                        let sub = playlist_sub(
                            Some(pl.owner.as_deref().unwrap_or("YouTube Music")),
                            pl.track_count,
                        );
                        let row = ListRow {
                            key: &pl.id,
                            art: Art::Square(&pl.thumbnails),
                            title: &pl.name,
                            sub: &sub,
                            chip: None,
                            reserve: 0.0,
                        };
                        if list_row(ui, row).clicked() {
                            open_playlist(out, &pl.id);
                        }
                    },
                )
            });
        }
    }
}

/// The search field in the 64 px page header: magnifier, text, clear
/// button, and the 2 px accent focus ring.
fn field(ui: &mut Ui, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let (row, _) = ui.allocate_exact_size(
        vec2(
            ui.available_width(),
            theme::PAGE_HEADER_H - 2.0 * theme::SPACE_16,
        ),
        Sense::hover(),
    );
    let rect = Rect::from_min_size(
        Pos2::new(row.left(), row.center().y - theme::FIELD_H / 2.0),
        vec2(row.width().min(FIELD_W), theme::FIELD_H),
    );
    let radius = CornerRadius::same(theme::RADIUS_CONTROL);
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(theme::STROKE_W, p.stroke_2),
        StrokeKind::Inside,
    );
    let icon = 16.0;
    ui.painter().text(
        Pos2::new(rect.left() + theme::SPACE_12 + icon / 2.0, rect.center().y),
        Align2::CENTER_CENTER,
        icons::MAGNIFYING_GLASS,
        theme::icon(icon),
        p.text_2,
    );
    let clear = Rect::from_center_size(
        Pos2::new(rect.right() - 6.0 - 12.0, rect.center().y),
        vec2(24.0, 24.0),
    );
    let text_rect = Rect::from_min_max(
        Pos2::new(
            rect.left() + theme::SPACE_12 + icon + 10.0,
            rect.center().y - 9.0,
        ),
        Pos2::new(clear.left() - theme::SPACE_4, rect.center().y + 9.0),
    );
    // A click anywhere in the box focuses the text; the text edit is on top.
    let field_click = ui.interact(rect, ui.scope_id().with("search-box"), Sense::click());
    let resp = ui.put(
        text_rect,
        egui::TextEdit::singleline(&mut view.search_text)
            .frame(Frame::NONE)
            .margin(Margin::ZERO)
            .hint_text(RichText::new("Search YouTube Music").color(p.text_3))
            .font(theme::row())
            .text_color(p.text_1)
            .desired_width(text_rect.width()),
    );
    if view.focus_search || field_click.clicked() {
        resp.request_focus();
        view.focus_search = false;
    }
    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        cmd(out, Command::Search(view.search_text.clone()));
    }
    if !view.search_text.is_empty() {
        let c = ui
            .interact(clear, ui.scope_id().with("search-clear"), Sense::click())
            .on_hover_text("Clear search")
            .on_hover_cursor(CursorIcon::PointingHand);
        if c.hovered() {
            ui.painter().rect_filled(clear, radius, p.surface_2);
        }
        ui.painter().text(
            clear.center(),
            Align2::CENTER_CENTER,
            icons::X,
            theme::icon(14.0),
            if c.hovered() { p.text_1 } else { p.text_2 },
        );
        if c.clicked() {
            view.search_text.clear();
            cmd(out, Command::Search(String::new()));
            resp.request_focus();
        }
    }
    if resp.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(2.0),
            CornerRadius::same(theme::RADIUS_CONTROL + 2),
            Stroke::new(theme::FOCUS_W, theme::accent()),
            StrokeKind::Outside,
        );
    }
    // `put` moved the cursor up to the text; the page margin above plus
    // this space below center the field in the 64 px header.
    ui.advance_cursor_after_rect(row);
    ui.add_space(theme::SPACE_16);
}

/// Songs / Albums / Artists / Playlists with result counts once results
/// are in.
fn tabs(ui: &mut Ui, view: &mut View, data: Option<&SearchView>) {
    let p = theme::pal();
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), TAB_H), Sense::hover());
    let mut x = row.left();
    for (tab, label) in TABS {
        let on = view.search_tab == tab;
        let font = if on {
            theme::row_strong()
        } else {
            theme::row()
        };
        let name = ui
            .painter()
            .layout_no_wrap(label.to_owned(), font, p.text_1);
        let n = data.map(|d| {
            ui.painter()
                .layout_no_wrap(count(d, tab).to_string(), theme::caption(), p.text_3)
        });
        let w = name.size().x + n.as_ref().map_or(0.0, |g| 6.0 + g.size().x);
        let rect = Rect::from_min_size(Pos2::new(x, row.top()), vec2(w, TAB_H));
        let resp = ui
            .interact(
                rect,
                ui.scope_id().with(("search-tab", label)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            view.search_tab = tab;
        }
        let color = if on || resp.hovered() {
            p.text_1
        } else {
            p.text_2
        };
        let cy = rect.center().y;
        let name_w = name.size().x;
        ui.painter()
            .galley(Pos2::new(x, cy - name.size().y / 2.0), name, color);
        if let Some(g) = n {
            // Align the smaller count on the label's baseline.
            let y = cy + 6.5 - g.size().y + 2.0;
            ui.painter()
                .galley(Pos2::new(x + name_w + 6.0, y), g, p.text_3);
        }
        if on {
            ui.painter().rect_filled(
                Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - 2.0), rect.max),
                CornerRadius::ZERO,
                p.text_1,
            );
        }
        x += w + theme::SPACE_20;
    }
}

/// "Album", "EP" or "Single" for an album row; unknown types read "Album".
fn type_label(t: model::AlbumType) -> &'static str {
    match t {
        model::AlbumType::Ep => "EP",
        model::AlbumType::Single => "Single",
        model::AlbumType::Album | model::AlbumType::Unknown => "Album",
    }
}

fn count(d: &SearchView, tab: SearchTab) -> usize {
    match tab {
        SearchTab::Songs => d.tracks.len(),
        SearchTab::Albums => d.results.albums.len(),
        SearchTab::Artists => d.results.artists.len(),
        SearchTab::Playlists => d.results.playlists.len(),
    }
}

/// How many main-playlist songs each album and artist result has.
struct Counts {
    albums: Vec<usize>,
    artists: Vec<usize>,
}

fn in_main_counts(d: &SearchView, main: Option<&[Arc<model::Track>]>) -> Counts {
    let main = main.unwrap_or_default();
    Counts {
        albums: d
            .results
            .albums
            .iter()
            .map(|a| {
                main.iter()
                    .filter(|t| t.album.as_ref().is_some_and(|b| b.id == a.id))
                    .count()
            })
            .collect(),
        artists: d
            .results
            .artists
            .iter()
            .map(|a| songs_in_main(main, &a.id, &a.name))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::AlbumType;

    #[test]
    fn album_type_labels() {
        assert_eq!(type_label(AlbumType::Album), "Album");
        assert_eq!(type_label(AlbumType::Ep), "EP");
        assert_eq!(type_label(AlbumType::Single), "Single");
        assert_eq!(type_label(AlbumType::Unknown), "Album");
    }
}
