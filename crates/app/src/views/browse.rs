//! The 232 px browse panel (D-Collection, D-Filter): every artist or album in
//! the main playlist with its song count. Clicking a row filters the table
//! by adding a chip to `view.collection.chips`; the collection draws the
//! chips and applies them.
//!
//! Counts are computed once per main-playlist change (see [`facets`]), never
//! per frame, and the list draws only the rows on screen.

use std::collections::HashMap;
use std::sync::Arc;

use egui::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, FontId, Galley, Id, Painter, Pos2, Rect, ScrollArea, Sense, TextFormat, Ui,
    UiBuilder, vec2,
};
use model::Track;

use crate::icons;
use crate::state::State;
use crate::theme;
use crate::views::{Action, BrowseTab, Chip, Out, Route, View};

/// Space between the caption and the tabs (D-Collection header `gap`).
const CAPTION_GAP: f32 = 6.0;
/// The active tab's underline.
const TAB_UNDERLINE: f32 = 2.0;
/// Space above the first row (D-Collection list `padding`).
const LIST_PAD: f32 = 6.0;
/// The playing marker before the count.
const MARKER: f32 = 12.0;

pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    // The header rule and the row fills span the whole panel, but the panel
    // frame in app.rs has an inner margin. The panel's clip rect is its
    // outer rect, so lay out in that. A child that the frame does not see:
    // growing the frame's content past its margin would shift the panel.
    let panel = ui.clip_rect();
    let mut child = ui.new_child(UiBuilder::new().max_rect(panel));
    panel_ui(&mut child, panel, state, view, out);
}

fn panel_ui(ui: &mut Ui, panel: Rect, state: &State, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let painter = ui.painter().clone();
    let left = panel.left() + theme::SPACE_16;
    let width = panel.width() - 2.0 * theme::SPACE_16;
    let header = Rect::from_min_size(panel.min, vec2(panel.width(), theme::PAGE_HEADER_H));
    let tabs_bottom = header.bottom() - theme::STROKE_W;
    let tabs_top = tabs_bottom - theme::FACET_ROW_H;

    let name = state
        .main_view()
        .map_or("Everything", |v| v.summary.name.as_str());
    let caption = galley(
        &painter,
        &format!("Browse {name}"),
        theme::caption(),
        p.text_3,
        width,
    );
    let caption_top = tabs_top - CAPTION_GAP - caption.size().y;
    painter.galley(Pos2::new(left, caption_top), caption, p.text_3);

    let main = match state.main_view() {
        Some(main) if !state.needs_login() => main,
        _ => return,
    };
    painter.hline(
        panel.x_range(),
        header.bottom() - theme::STROKE_W / 2.0,
        theme::hairline(),
    );

    // Tabs.
    let mut x = left;
    for (tab, label) in [
        (BrowseTab::Artists, "Artists"),
        (BrowseTab::Albums, "Albums"),
    ] {
        let active = view.browse_tab == tab;
        let font = if active {
            theme::row_strong()
        } else {
            theme::row()
        };
        let text = galley(&painter, label, font, p.text_1, f32::INFINITY);
        let rect = Rect::from_min_max(
            Pos2::new(x, tabs_top),
            Pos2::new(x + text.size().x, tabs_bottom),
        );
        x = rect.right() + theme::SPACE_16;
        let resp = ui.allocate_rect(rect, Sense::click());
        let color = if active || resp.hovered() {
            p.text_1
        } else {
            p.text_2
        };
        let text_y = rect.top() + (rect.height() - TAB_UNDERLINE - text.size().y) / 2.0;
        painter.galley(Pos2::new(rect.left(), text_y), text, color);
        if active {
            let line = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom() - TAB_UNDERLINE),
                rect.max,
            );
            painter.rect_filled(line, theme::RADIUS_REGION, p.text_1);
        }
        if resp.clicked() && !active {
            view.browse_tab = tab;
            ui.ctx().request_repaint();
        }
    }

    // The list: "All artists" first, then one row per artist.
    let facets = facets(ui.ctx(), &main.tracks);
    let tab = view.browse_tab;
    let (all, list) = match tab {
        BrowseTab::Artists => ("All artists", &facets.artists),
        BrowseTab::Albums => ("All albums", &facets.albums),
    };
    let selected: Vec<String> = selected(&view.collection.chips, tab)
        .into_iter()
        .map(str::to_owned)
        .collect();
    let playing: Vec<&str> = match (state.current_track(), tab) {
        (Some(t), BrowseTab::Artists) => t.artists.iter().map(|a| a.name.as_str()).collect(),
        (Some(t), BrowseTab::Albums) => t.album.iter().map(|a| a.name.as_str()).collect(),
        (None, _) => Vec::new(),
    };

    let list_rect = Rect::from_min_max(
        Pos2::new(panel.left(), header.bottom() + LIST_PAD),
        panel.max,
    );
    let mut clicked = None;
    ui.scope_builder(UiBuilder::new().max_rect(list_rect), |ui| {
        // show_rows adds item spacing to each row height.
        ui.spacing_mut().item_spacing.y = 0.0;
        ScrollArea::vertical()
            .id_salt(match tab {
                BrowseTab::Artists => "browse-artists",
                BrowseTab::Albums => "browse-albums",
            })
            .auto_shrink([false, false])
            .show_rows(ui, theme::FACET_ROW_H, list.len() + 1, |ui, range| {
                for i in range {
                    let (label, count) = match i {
                        0 => (all, list.len()),
                        _ => {
                            let (name, count) = &list[i - 1];
                            (name.as_str(), *count)
                        }
                    };
                    let row = Row {
                        label,
                        count,
                        selected: if i == 0 {
                            selected.is_empty()
                        } else {
                            selected.iter().any(|s| s == label)
                        },
                        playing: i > 0 && playing.contains(&label),
                    };
                    if facet_row(ui, &row) {
                        // Shift adds or removes this row only; it never
                        // selects a span of rows.
                        clicked = Some((i, ui.input(|inp| inp.modifiers.shift)));
                    }
                }
            });
    });

    if let Some((i, shift)) = clicked {
        let chips = &mut view.collection.chips;
        match i {
            0 => clear(chips, tab),
            _ => toggle(chips, chip(tab, list[i - 1].0.clone()), shift),
        }
        if view.route != Route::Collection {
            out.push(Action::Go(Route::Collection));
        }
        // The collection applies the chips at the start of the next frame.
        ui.ctx().request_repaint();
    }
}

struct Row<'a> {
    label: &'a str,
    count: usize,
    selected: bool,
    playing: bool,
}

/// One 28 px facet row (D-Tokens facet row: default, hover, selected).
/// Returns whether it was clicked.
fn facet_row(ui: &mut Ui, row: &Row<'_>) -> bool {
    let p = theme::pal();
    let (rect, resp) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::FACET_ROW_H),
        Sense::click(),
    );
    if !ui.is_rect_visible(rect) {
        return resp.clicked();
    }
    let painter = ui.painter();
    let hovered = resp.hovered();
    if row.selected {
        painter.rect_filled(rect, theme::RADIUS_REGION, p.surface_3);
    } else if hovered {
        painter.rect_filled(rect, theme::RADIUS_REGION, p.surface_2);
    }
    let y = rect.center().y;
    let right = rect.right() - theme::SPACE_16;

    let count = galley(
        painter,
        &row.count.to_string(),
        theme::caption(),
        p.text_3,
        f32::INFINITY,
    );
    let mut end = right - count.size().x;
    painter.galley(Pos2::new(end, y - count.size().y / 2.0), count, p.text_3);
    if row.playing {
        end -= theme::SPACE_8;
        painter.text(
            Pos2::new(end, y),
            Align2::RIGHT_CENTER,
            icons::SPEAKER_HIGH,
            theme::icon_fill(MARKER),
            theme::accent(),
        );
        end -= MARKER;
    }

    let (font, color) = if row.selected {
        (theme::row_strong(), p.text_1)
    } else if hovered {
        (theme::row(), p.text_1)
    } else {
        (theme::row(), p.text_2)
    };
    let left = rect.left() + theme::SPACE_16;
    let name = galley(painter, row.label, font, color, end - theme::SPACE_8 - left);
    painter.galley(Pos2::new(left, y - name.size().y / 2.0), name, color);
    resp.clicked()
}

/// One line of text, cut with an ellipsis at `max_width`.
fn galley(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Arc<Galley> {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_width.max(0.0));
    painter.layout_job(job)
}

// ------------------------------------------------------------------ counts

/// Artist and album counts for one main playlist.
struct Facets {
    /// The tracks these counts are for. Holding the `Arc` keeps its address
    /// from being reused, so pointer equality means "same playlist".
    tracks: Arc<Vec<Arc<Track>>>,
    artists: Vec<(String, usize)>,
    albums: Vec<(String, usize)>,
}

/// The counts for `tracks`, computed once and kept in egui's temp data
/// until the main playlist changes.
fn facets(ctx: &egui::Context, tracks: &Arc<Vec<Arc<Track>>>) -> Arc<Facets> {
    let id = Id::unique("browse-facets");
    if let Some(f) = ctx.data(|d| d.get_temp::<Arc<Facets>>(id))
        && Arc::ptr_eq(&f.tracks, tracks)
    {
        return f;
    }
    let f = Arc::new(Facets {
        tracks: Arc::clone(tracks),
        artists: count_artists(tracks),
        albums: count_albums(tracks),
    });
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&f)));
    f
}

/// Songs per artist. A track credited to several artists counts once for
/// each of them.
fn count_artists(tracks: &[Arc<Track>]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in tracks {
        for (i, a) in t.artists.iter().enumerate() {
            let repeat = t.artists[..i].iter().any(|b| b.name == a.name);
            if !a.name.is_empty() && !repeat {
                *counts.entry(&a.name).or_default() += 1;
            }
        }
    }
    sorted(counts)
}

/// Songs per album name (album chips match by name). Tracks without an
/// album are skipped.
fn count_albums(tracks: &[Arc<Track>]) -> Vec<(String, usize)> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for a in tracks.iter().filter_map(|t| t.album.as_ref()) {
        if !a.name.is_empty() {
            *counts.entry(&a.name).or_default() += 1;
        }
    }
    sorted(counts)
}

/// Most songs first, then by name.
fn sorted(counts: HashMap<&str, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = counts
        .into_iter()
        .map(|(name, n)| (name.to_owned(), n))
        .collect();
    v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

// ------------------------------------------------------------------- chips

fn kind(chip: &Chip) -> BrowseTab {
    match chip {
        Chip::Artist(_) => BrowseTab::Artists,
        Chip::Album(_) => BrowseTab::Albums,
    }
}

fn chip(tab: BrowseTab, name: String) -> Chip {
    match tab {
        BrowseTab::Artists => Chip::Artist(name),
        BrowseTab::Albums => Chip::Album(name),
    }
}

/// The names in the active chips of `tab`'s kind: the selected rows.
fn selected(chips: &[Chip], tab: BrowseTab) -> Vec<&str> {
    chips
        .iter()
        .filter(|c| kind(c) == tab)
        .map(|c| match c {
            Chip::Artist(n) | Chip::Album(n) => n.as_str(),
        })
        .collect()
}

/// A click on a row. With `shift`, adds `chip` or removes it if active,
/// keeping the other rows. Without, selects only this row; a click on the
/// only selected row clears it.
fn toggle(chips: &mut Vec<Chip>, chip: Chip, shift: bool) {
    let k = kind(&chip);
    let active = chips.iter().position(|c| *c == chip);
    if shift {
        match active {
            Some(i) => {
                chips.remove(i);
            }
            None => chips.push(chip),
        }
        return;
    }
    if let Some(i) = active
        && chips.iter().filter(|c| kind(c) == k).count() == 1
    {
        chips.remove(i);
        return;
    }
    let mut new = Some(chip);
    chips.retain_mut(|c| {
        if kind(c) != k {
            return true;
        }
        match new.take() {
            Some(n) => {
                *c = n;
                true
            }
            None => false,
        }
    });
    if let Some(n) = new {
        chips.push(n);
    }
}

/// A click on "All artists" / "All albums": removes that kind's chip.
fn clear(chips: &mut Vec<Chip>, tab: BrowseTab) {
    chips.retain(|c| kind(c) != tab);
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{AlbumRef, ArtistRef};

    fn track(id: &str, artists: &[&str], album: Option<&str>) -> Arc<Track> {
        Arc::new(Track {
            id: id.into(),
            title: id.into(),
            artists: artists
                .iter()
                .map(|n| ArtistRef {
                    id: None,
                    name: (*n).into(),
                })
                .collect(),
            album: album.map(|n| AlbumRef {
                id: format!("id-{n}"),
                name: n.into(),
            }),
            duration: None,
            thumbnails: Vec::new(),
        })
    }

    fn counts(v: &[(&str, usize)]) -> Vec<(String, usize)> {
        v.iter().map(|(n, c)| ((*n).to_owned(), *c)).collect()
    }

    #[test]
    fn featured_artists_count_for_each_artist() {
        let tracks = [
            track("1", &["A", "B"], None),
            track("2", &["A"], None),
            track("3", &["B", "C"], None),
            // Listed twice on one track: counts once.
            track("4", &["C", "C"], None),
        ];
        assert_eq!(
            count_artists(&tracks),
            counts(&[("A", 2), ("B", 2), ("C", 2)])
        );
    }

    #[test]
    fn ties_sort_by_name_after_count() {
        let tracks = [
            track("1", &["Zed"], None),
            track("2", &["Abe"], None),
            track("3", &["Mia"], None),
            track("4", &["Mia"], None),
        ];
        assert_eq!(
            count_artists(&tracks),
            counts(&[("Mia", 2), ("Abe", 1), ("Zed", 1)])
        );
    }

    #[test]
    fn albums_without_an_album_are_skipped() {
        let tracks = [
            track("1", &["A"], Some("Salt Year")),
            track("2", &["A"], None),
            track("3", &["B"], Some("Salt Year")),
            track("4", &["B"], Some("Halogen")),
        ];
        assert_eq!(
            count_albums(&tracks),
            counts(&[("Salt Year", 2), ("Halogen", 1)])
        );
    }

    #[test]
    fn toggle_adds_replaces_and_removes() {
        let artist = |n: &str| Chip::Artist(n.into());
        let album = |n: &str| Chip::Album(n.into());
        let mut chips = vec![album("Halogen")];

        toggle(&mut chips, artist("A"), false);
        assert_eq!(chips, [album("Halogen"), artist("A")]);
        assert_eq!(selected(&chips, BrowseTab::Artists), ["A"]);

        // Another artist replaces the first, in place.
        toggle(&mut chips, artist("B"), false);
        assert_eq!(chips, [album("Halogen"), artist("B")]);

        // Clicking the selected row again removes its chip.
        toggle(&mut chips, artist("B"), false);
        assert_eq!(chips, [album("Halogen")]);
        assert!(selected(&chips, BrowseTab::Artists).is_empty());
    }

    #[test]
    fn shift_click_adds_and_removes_one_artist() {
        let artist = |n: &str| Chip::Artist(n.into());
        let album = |n: &str| Chip::Album(n.into());
        let mut chips = vec![album("Halogen")];
        toggle(&mut chips, artist("A"), false);
        toggle(&mut chips, artist("C"), true);
        toggle(&mut chips, artist("B"), true);
        // Only the rows clicked, not the ones between them.
        assert_eq!(selected(&chips, BrowseTab::Artists), ["A", "C", "B"]);
        // Shift on a selected row removes just that one.
        toggle(&mut chips, artist("C"), true);
        assert_eq!(selected(&chips, BrowseTab::Artists), ["A", "B"]);
        // A plain click on a selected row, with others selected, keeps only it.
        toggle(&mut chips, artist("B"), false);
        assert_eq!(chips, [album("Halogen"), artist("B")]);
    }

    #[test]
    fn all_row_clears_only_its_kind() {
        let mut chips = vec![Chip::Artist("A".into()), Chip::Album("Halogen".into())];
        clear(&mut chips, BrowseTab::Artists);
        assert_eq!(chips, [Chip::Album("Halogen".into())]);
        clear(&mut chips, BrowseTab::Artists);
        assert_eq!(chips, [Chip::Album("Halogen".into())]);
    }
}
