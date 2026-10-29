//! The settings page (`Route::Settings`), opened from the rail's settings
//! button or Ctrl+,: a section list on the left with a search field, the
//! section's settings on the right.
//!
//! Every control edits a copy of `state.settings`; a changed copy goes out
//! as `Command::SetSettings`, so the whole app restyles on the same frame.
//! A slider being dragged sends `save: false` and saves when released.
//!
//! The theme builder edits a draft palette held in the view ([`Builder`]).
//! While the draft is on, `app.rs` shows it instead of the saved theme;
//! Save stores it as a custom theme, allowed only when it passes the same
//! contrast checks as the presets (`theme::checks`).
//!
//! D look: rows are the D two-line row (13/400 text-1 label, 11/400 text-3
//! hint, 44 px) in hairline boxes; switches are 28 x 16; controls are 28 px
//! with radius 4.

use std::ops::RangeInclusive;

use egui::{
    Align, Align2, Color32, ComboBox, CornerRadius, CursorIcon, FontId, Frame, Id, Layout, Margin,
    Pos2, Rect, Response, Role, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder,
    WidgetInfo, pos2, vec2,
};

use crate::icons;
use crate::settings as saved;
use crate::shortcuts::{Action as Shortcut, Binding};
use crate::state::{Command, CustomTheme, Loadable, Settings, State};
use crate::theme::{self, Palette, Recipe, Tone};
use crate::views::{Out, View, cmd};
use crate::widgets::{seed, text_line};

const NAV_W: f32 = 184.0;
const PAD_X: f32 = theme::SPACE_12;
const ROW_MIN_H: f32 = 44.0;
const CONTROL_H: f32 = 28.0;
const SWITCH_W: f32 = 28.0;
const SWITCH_H: f32 = 16.0;
const KNOB_R: f32 = 6.0;
const SLIDER_W: f32 = 150.0;
const VALUE_W: f32 = 56.0;
const CARD_MIN_W: f32 = 92.0;
const CARD_ART_H: f32 = 42.0;
/// Width of the label column of the builder's Quick sliders.
const QUICK_LABEL_W: f32 = 104.0;

// -------------------------------------------------------------------- state

/// The page's own state, in [`View`].
#[derive(Debug, Clone, Default)]
pub struct SettingsView {
    pub section: Section,
    pub search: String,
    pub builder: Builder,
    /// The shortcut waiting for its new key.
    pub listening: Option<Shortcut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Section {
    #[default]
    Appearance,
    Text,
    Playback,
    Lyrics,
    Library,
    Shortcuts,
    Account,
}

impl Section {
    const ALL: [Self; 7] = [
        Self::Appearance,
        Self::Text,
        Self::Playback,
        Self::Lyrics,
        Self::Library,
        Self::Shortcuts,
        Self::Account,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Text => "Text & layout",
            Self::Playback => "Playback",
            Self::Lyrics => "Lyrics",
            Self::Library => "Library",
            Self::Shortcuts => "Shortcuts",
            Self::Account => "Account & about",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Appearance => "Theme, accent and the custom theme builder",
            Self::Text => "Text size, density and what song lists show",
            Self::Playback => "What plays next and how the keys step",
            Self::Lyrics => "The synced lyrics drawer",
            Self::Library => "The main playlist and how it refreshes",
            Self::Shortcuts => "Click a key to change it, then press the new one",
            Self::Account => "Sign-in and build details",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Appearance => icons::PALETTE,
            Self::Text => icons::TEXT_AA,
            Self::Playback => icons::SPEAKER_HIGH,
            Self::Lyrics => icons::MICROPHONE_STAGE,
            Self::Library => icons::BOOKS,
            Self::Shortcuts => icons::KEYBOARD,
            Self::Account => icons::USER_CIRCLE,
        }
    }

    /// Puts this section's settings back to their defaults. Saved custom
    /// themes stay.
    fn reset(self, s: &mut Settings) {
        let d = Settings::default();
        match self {
            Self::Appearance => {
                s.theme = d.theme;
                s.accent_from_art = d.accent_from_art;
                s.player_tint = d.player_tint;
            }
            Self::Text => {
                s.text_size = d.text_size;
                s.row_height = d.row_height;
                s.ui_scale = d.ui_scale;
                s.row_art = d.row_art;
                s.album_column = d.album_column;
                s.spine = d.spine;
                s.animations = d.animations;
            }
            Self::Playback => {
                s.autoplay = d.autoplay;
                s.volume_step = d.volume_step;
            }
            Self::Lyrics => {
                s.lyrics_size = d.lyrics_size;
                s.lyrics_center = d.lyrics_center;
                s.lyrics_accent = d.lyrics_accent;
            }
            Self::Library => {
                s.not_heard_months = d.not_heard_months;
                s.refresh_on_focus = d.refresh_on_focus;
            }
            Self::Shortcuts => s.keys = d.keys,
            Self::Account => {}
        }
    }

    fn changed(self, s: &Settings) -> bool {
        let mut reset = s.clone();
        self.reset(&mut reset);
        reset != *s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Quick,
    Colors,
    File,
}

/// The theme builder's draft.
#[derive(Debug, Clone)]
pub struct Builder {
    pub tab: Tab,
    pub recipe: Recipe,
    pub colors: [[u8; 3]; 11],
    /// The "Every color" hex fields, kept apart so a half-typed value
    /// stays on screen.
    pub hex: [String; 11],
    pub name: String,
    /// The draft is the theme on screen.
    pub on: bool,
    /// The Import / export text.
    pub file: String,
    /// The user typed in `file`: stop regenerating it.
    pub file_edited: bool,
    pub message: String,
}

impl Default for Builder {
    fn default() -> Self {
        let recipe = Recipe::default();
        let colors = theme::colors(&recipe.palette()).map(rgb);
        Self {
            tab: Tab::Quick,
            recipe,
            colors,
            hex: colors.map(saved::hex_text),
            name: "My theme".to_owned(),
            on: false,
            file: String::new(),
            file_edited: false,
            message: String::new(),
        }
    }
}

impl Builder {
    pub fn palette(&self) -> Palette {
        theme::from_colors(self.colors.map(color))
    }

    fn set_colors(&mut self, colors: [[u8; 3]; 11]) {
        self.colors = colors;
        self.hex = colors.map(saved::hex_text);
        self.file_edited = false;
        self.on = true;
    }

    fn load(&mut self, p: &Palette) {
        self.recipe = self.recipe.starting_from(p);
        self.set_colors(theme::colors(p).map(rgb));
    }
}

fn rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

fn color([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The saved theme's palette: a preset, a custom theme, else Graphite.
pub fn saved_palette(s: &Settings) -> Palette {
    if let Some(p) = theme::preset(&s.theme) {
        return *p;
    }
    s.custom_theme()
        .map_or(theme::GRAPHITE, |t| theme::from_colors(t.colors.map(color)))
}

/// The size settings as `theme` reads them.
pub fn layout(s: &Settings) -> theme::Layout {
    theme::Layout {
        text_size: f32::from(s.text_size),
        row_h: f32::from(s.row_height),
        row_art: s.row_art,
        album_column: s.album_column,
        tint: f64::from(s.player_tint) / 100.0,
    }
}

// ---------------------------------------------------------------------- page

/// One frame's walk through the settings: the search, the edited copy,
/// and the headings still to draw.
struct Page {
    query: String,
    section: &'static str,
    section_drawn: bool,
    group: &'static str,
    group_drawn: bool,
    group_rect: Option<Rect>,
    draft: Settings,
    /// A slider is held: send without saving.
    dragging: bool,
    /// A slider was let go or a choice clicked: save even if unchanged.
    release: bool,
    shown: bool,
}

impl Page {
    fn searching(&self) -> bool {
        !self.query.is_empty()
    }

    /// Whether a row with these words shows under the search.
    fn shows(&self, words: &[&str]) -> bool {
        if !self.searching() {
            return true;
        }
        [self.section, self.group]
            .iter()
            .chain(words)
            .any(|w| w.to_lowercase().contains(&self.query))
    }

    /// Draws the headings a row needs before it, once.
    fn begin(&mut self, ui: &mut Ui) {
        let p = theme::pal();
        if self.searching() && !self.section_drawn {
            self.section_drawn = true;
            if self.shown {
                ui.add_space(theme::SPACE_12);
            }
            heading(ui, self.section, theme::page_title(), p.text_2);
        }
        if !self.group_drawn && !self.group.is_empty() {
            self.group_drawn = true;
            heading(ui, self.group, theme::section(), p.text_1);
        }
        self.shown = true;
    }

    /// Adds `rect` to the current group's box.
    fn record(&mut self, rect: Rect) {
        self.group_rect = Some(self.group_rect.map_or(rect, |g| g.union(rect)));
    }
}

fn heading(ui: &mut Ui, text: &str, font: FontId, color: Color32) {
    ui.add_space(theme::SPACE_8);
    let h = font.size + theme::SPACE_8;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    ui.painter()
        .text(rect.left_top(), Align2::LEFT_TOP, text, font, color);
}

/// Draws the page into the content region.
pub fn show(ui: &mut Ui, state: &State, view: &mut View, out: &mut Out) {
    let p = theme::pal();
    let page_view = &mut view.settings;
    let mut page = Page {
        query: page_view.search.trim().to_lowercase(),
        section: "",
        section_drawn: false,
        group: "",
        group_drawn: false,
        group_rect: None,
        draft: state.settings.clone(),
        dragging: false,
        release: false,
        shown: false,
    };
    capture_shortcut(ui, page_view, &mut page);

    let rect = ui.max_rect();
    let (nav, body) = rect.split_left_right_at_x(rect.left() + NAV_W);
    ui.painter().vline(
        nav.right() - theme::SPACE_8 - 0.5,
        rect.y_range(),
        theme::hairline(),
    );
    let nav = nav
        .shrink2(vec2(0.0, 0.0))
        .with_max_x(nav.right() - theme::SPACE_16);
    ui.scope_builder(UiBuilder::new().max_rect(nav), |ui| {
        nav_list(ui, page_view, &page.draft);
    });
    let body = body.with_min_x(body.left() + theme::SPACE_8);
    ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
        header(ui, page_view, &mut page);
        // One scroll position per section, and one for search results.
        let scroll_id = if page.searching() {
            None
        } else {
            Some(page_view.section)
        };
        ScrollArea::vertical()
            .id_salt(("settings-body", scroll_id))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - theme::SPACE_8);
                ui.spacing_mut().item_spacing.y = 0.0;
                let sections: Vec<Section> = if page.searching() {
                    Section::ALL.to_vec()
                } else {
                    vec![page_view.section]
                };
                for section in sections {
                    page.section = section.title();
                    page.section_drawn = false;
                    draw_section(ui, section, state, page_view, &mut page, out);
                }
                if page.searching() && !page.shown {
                    ui.add_space(theme::SPACE_24);
                    ui.label(
                        egui::RichText::new("No settings match.")
                            .font(theme::body())
                            .color(p.text_3),
                    );
                }
                ui.add_space(theme::SPACE_24);
            });
    });

    if page.draft != state.settings || page.release {
        cmd(
            out,
            Command::SetSettings {
                settings: Box::new(page.draft),
                save: !page.dragging,
            },
        );
    }
}

/// While a shortcut waits for its key, the next key press becomes it;
/// Escape cancels. Every key event this frame is consumed.
fn capture_shortcut(ui: &mut Ui, view: &mut SettingsView, page: &mut Page) {
    let Some(action) = view.listening else {
        return;
    };
    let pressed = ui.input_mut(|i| {
        let mut first = None;
        i.events.retain(|e| match e {
            egui::Event::Key {
                key,
                pressed,
                modifiers,
                ..
            } => {
                if *pressed && first.is_none() && !crate::shortcuts::is_modifier(*key) {
                    first = Some((*key, *modifiers));
                }
                false
            }
            _ => true,
        });
        first
    });
    match pressed {
        Some((egui::Key::Escape, _)) => view.listening = None,
        Some((key, m)) => {
            if let Some(b) = Binding::from_press(key, m) {
                page.draft.keys.set(action, b);
                page.release = true;
            }
            view.listening = None;
        }
        None => {}
    }
}

fn nav_list(ui: &mut Ui, view: &mut SettingsView, settings: &Settings) {
    let p = theme::pal();
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add(
        TextEdit::singleline(&mut view.search)
            .hint_text("Search settings")
            .font(theme::secondary())
            .desired_width(ui.available_width())
            .margin(Margin::symmetric(8, 7)),
    );
    ui.add_space(theme::SPACE_12);
    let (cap, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
    ui.painter().text(
        pos2(cap.left() + theme::SPACE_8, cap.center().y),
        Align2::LEFT_CENTER,
        "Settings",
        theme::caption(),
        p.text_3,
    );
    let row_h = (theme::row().size + 15.0).max(30.0);
    for section in Section::ALL {
        let (rect, resp) =
            ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
        let on = view.section == section && view.search.trim().is_empty();
        let fill = if on {
            Some(p.surface_3)
        } else if resp.hovered() {
            Some(p.surface_2)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, theme::RADIUS_ROW, fill);
        }
        let fg = if on || resp.hovered() {
            p.text_1
        } else {
            p.text_2
        };
        let x = rect.left() + theme::SPACE_8;
        ui.painter().text(
            pos2(x + 8.0, rect.center().y),
            Align2::CENTER_CENTER,
            section.icon(),
            theme::icon(16.0),
            fg,
        );
        text_line(
            ui.painter(),
            pos2(x + 26.0, rect.center().y),
            section.title(),
            theme::row(),
            fg,
            rect.right() - x - 40.0,
        );
        if section.changed(settings) {
            ui.painter().circle_filled(
                pos2(rect.right() - theme::SPACE_12, rect.center().y),
                3.0,
                theme::accent(),
            );
        }
        let resp = resp.on_hover_cursor(CursorIcon::PointingHand);
        if resp.clicked() {
            view.section = section;
            view.search.clear();
        }
    }
}

fn header(ui: &mut Ui, view: &SettingsView, page: &mut Page) {
    let p = theme::pal();
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width(), theme::PAGE_HEADER_H),
        Sense::hover(),
    );
    let (title, sub) = if page.searching() {
        (
            "Search".to_owned(),
            format!("Settings matching \u{201c}{}\u{201d}", view.search.trim()),
        )
    } else {
        (
            view.section.title().to_owned(),
            view.section.subtitle().to_owned(),
        )
    };
    let title_size = theme::page_title().size;
    ui.painter().text(
        pos2(rect.left(), rect.center().y - 3.0),
        Align2::LEFT_BOTTOM,
        title,
        theme::page_title(),
        p.text_1,
    );
    ui.painter().text(
        pos2(rect.left(), rect.center().y + 1.0),
        Align2::LEFT_TOP,
        sub,
        theme::secondary(),
        p.text_3,
    );
    let _ = title_size;
    if !page.searching() && view.section.changed(&page.draft) {
        let label = "Reset section";
        let w = button_width(ui, label, Some(icons::ARROW_COUNTER_CLOCKWISE));
        let b = Rect::from_min_size(
            pos2(
                rect.right() - w - theme::SPACE_8,
                rect.center().y - CONTROL_H / 2.0,
            ),
            vec2(w, CONTROL_H),
        );
        let resp = button(
            ui,
            b,
            Id::unique("settings-reset"),
            label,
            Some(icons::ARROW_COUNTER_CLOCKWISE),
            Kind::Plain,
            true,
        );
        if resp.clicked() {
            view.section.reset(&mut page.draft);
            page.release = true;
        }
    }
    ui.painter()
        .hline(rect.x_range(), rect.bottom() - 0.5, theme::hairline());
    ui.add_space(theme::SPACE_8);
}

fn draw_section(
    ui: &mut Ui,
    section: Section,
    state: &State,
    view: &mut SettingsView,
    page: &mut Page,
    out: &mut Out,
) {
    match section {
        Section::Appearance => {
            group(ui, page, "Theme", |ui, page| {
                gallery(ui, page, &mut view.builder)
            });
            group(ui, page, "Custom theme builder", |ui, page| {
                builder(ui, page, &mut view.builder);
            });
            group(ui, page, "Accent", |ui, page| {
                let mut on = page.draft.accent_from_art;
                switch(
                    ui,
                    page,
                    "Accent from album art",
                    "Off: the theme's own accent",
                    &mut on,
                );
                page.draft.accent_from_art = on;
                let mut tint = page.draft.player_tint;
                slider_u8(
                    ui,
                    page,
                    "Player bar tint",
                    "How much of the accent colors the player bar",
                    &mut tint,
                    saved::PLAYER_TINTS,
                    1,
                    |v| format!("{v}%"),
                );
                page.draft.player_tint = tint;
            });
            group(ui, page, "Preview", |ui, page| preview(ui, page, state));
        }
        Section::Text => {
            group(ui, page, "Text", |ui, page| {
                let mut size = page.draft.text_size;
                slider_u8(
                    ui,
                    page,
                    "Text size",
                    "Every text style scales from the 13 px body size",
                    &mut size,
                    saved::TEXT_SIZES,
                    1,
                    |v| format!("{v} px"),
                );
                page.draft.text_size = size;
            });
            group(ui, page, "Size", |ui, page| {
                let mut h = page.draft.row_height;
                choice(
                    ui,
                    page,
                    "Row density",
                    "Height of the rows in song lists",
                    &mut h,
                    &[(26, "Compact"), (32, "Default"), (40, "Roomy")],
                );
                page.draft.row_height = h;
                let mut scale = page.draft.ui_scale;
                slider_u8(
                    ui,
                    page,
                    "Interface scale",
                    "Everything, art and icons included",
                    &mut scale,
                    saved::UI_SCALES,
                    5,
                    |v| format!("{v}%"),
                );
                page.draft.ui_scale = scale;
            });
            group(ui, page, "Song lists", |ui, page| {
                let d = &mut page.draft.clone();
                switch(
                    ui,
                    page,
                    "Album art in rows",
                    "The small cover next to each title",
                    &mut d.row_art,
                );
                switch(
                    ui,
                    page,
                    "Album column",
                    "In the main playlist, search and playlists",
                    &mut d.album_column,
                );
                switch(
                    ui,
                    page,
                    "The spine",
                    "The art-colored strip along the main playlist",
                    &mut d.spine,
                );
                switch(
                    ui,
                    page,
                    "Animations",
                    "Fades and slides; off makes every change instant",
                    &mut d.animations,
                );
                let s = &mut page.draft;
                (s.row_art, s.album_column, s.spine, s.animations) =
                    (d.row_art, d.album_column, d.spine, d.animations);
            });
            group(ui, page, "Preview", |ui, page| preview(ui, page, state));
        }
        Section::Playback => {
            group(ui, page, "Queue", |ui, page| {
                let mut on = page.draft.autoplay;
                switch(
                    ui,
                    page,
                    "Autoplay",
                    "Plays radio when the queue runs out",
                    &mut on,
                );
                page.draft.autoplay = on;
                let mut step = page.draft.volume_step;
                choice(
                    ui,
                    page,
                    "Volume step",
                    "For the volume up and down keys",
                    &mut step,
                    &[(2, "2%"), (5, "5%"), (10, "10%")],
                );
                page.draft.volume_step = step;
            });
        }
        Section::Lyrics => {
            group(ui, page, "Lyrics", |ui, page| {
                let mut size = page.draft.lyrics_size;
                slider_u8(
                    ui,
                    page,
                    "Lyrics size",
                    "Text size in the lyrics drawer",
                    &mut size,
                    saved::LYRICS_SIZES,
                    1,
                    |v| format!("{v} px"),
                );
                page.draft.lyrics_size = size;
                let mut center = page.draft.lyrics_center;
                choice(
                    ui,
                    page,
                    "Alignment",
                    "",
                    &mut center,
                    &[(false, "Left"), (true, "Center")],
                );
                page.draft.lyrics_center = center;
                let mut accent = page.draft.lyrics_accent;
                switch(
                    ui,
                    page,
                    "Current line in the accent",
                    "Off: the current line is bold primary text",
                    &mut accent,
                );
                page.draft.lyrics_accent = accent;
            });
            group(ui, page, "Preview", lyrics_preview);
        }
        Section::Library => {
            group(ui, page, "Main playlist", |ui, page| {
                main_playlist(ui, page, state, out);
                let mut months = page.draft.not_heard_months;
                choice(
                    ui,
                    page,
                    "\u{201c}Not heard in\u{201d} filter",
                    "The quick filter next to the filter field",
                    &mut months,
                    &[(1, "1 mo"), (3, "3 mo"), (6, "6 mo"), (12, "1 yr")],
                );
                page.draft.not_heard_months = months;
                let mut on = page.draft.refresh_on_focus;
                switch(
                    ui,
                    page,
                    "Refresh when the window regains focus",
                    "Picks up changes made on your phone; at most every 10 minutes",
                    &mut on,
                );
                page.draft.refresh_on_focus = on;
            });
        }
        Section::Shortcuts => {
            group(ui, page, "Keyboard", |ui, page| shortcuts(ui, page, view));
        }
        Section::Account => {
            group(ui, page, "Account", |ui, page| {
                let signed_in = state.session == crate::state::Session::LoggedIn;
                let hint = if signed_in {
                    "Your cookies stay in the system keyring, never in a file"
                } else {
                    "Browsing without an account"
                };
                if let Some((_, c)) = row(ui, page, "YouTube Music", hint, 96.0) {
                    let r = Rect::from_min_size(
                        pos2(c.right() - 96.0, c.center().y - CONTROL_H / 2.0),
                        vec2(96.0, CONTROL_H),
                    );
                    let label = if signed_in { "Sign out" } else { "Sign in" };
                    if button(
                        ui,
                        r,
                        Id::unique("sign-out"),
                        label,
                        Some(icons::SIGN_OUT),
                        Kind::Danger,
                        true,
                    )
                    .clicked()
                    {
                        cmd(out, Command::Logout);
                    }
                }
            });
            group(ui, page, "About", |ui, page| {
                let version = concat!("Rustle ", env!("CARGO_PKG_VERSION"));
                row(
                    ui,
                    page,
                    version,
                    "egui 0.37 on OpenGL, no browser engine; GPL-3.0-or-later",
                    0.0,
                );
            });
        }
    }
}

/// A group: its heading and a hairline box around its rows, drawn only if
/// a row in it shows.
fn group(ui: &mut Ui, page: &mut Page, title: &'static str, f: impl FnOnce(&mut Ui, &mut Page)) {
    page.group = title;
    page.group_drawn = false;
    page.group_rect = None;
    f(ui, page);
    if let Some(r) = page.group_rect.take() {
        ui.painter().rect_stroke(
            r,
            theme::RADIUS_CONTROL,
            Stroke::new(theme::STROKE_W, theme::pal().stroke),
            StrokeKind::Inside,
        );
    }
    if page.group_drawn {
        ui.add_space(theme::SPACE_16);
    }
    page.group = "";
}

// ---------------------------------------------------------------------- rows

fn row_height() -> f32 {
    (theme::body().size + theme::small().size + 20.0).max(ROW_MIN_H)
}

/// One setting row: label and hint on the left, `control_w` px on the right
/// for the control. Returns the row's click response and the control's
/// rect, or `None` when the search hides the row.
fn row(
    ui: &mut Ui,
    page: &mut Page,
    label: &str,
    hint: &str,
    control_w: f32,
) -> Option<(Response, Rect)> {
    if !page.shows(&[label, hint]) {
        return None;
    }
    page.begin(ui);
    let p = theme::pal();
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_height()), Sense::click());
    if page.group_rect.is_some() {
        ui.painter()
            .hline(rect.x_range(), rect.top() + 0.5, theme::hairline());
    }
    page.record(rect);
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect.shrink(1.0), CornerRadius::ZERO, p.surface_2);
    }
    let control = Rect::from_min_max(
        pos2(rect.right() - PAD_X - control_w, rect.top()),
        pos2(rect.right() - PAD_X, rect.bottom()),
    );
    let left = rect.left() + PAD_X;
    let width = control.left() - left - PAD_X;
    let (b, s) = (theme::body().size, theme::small().size);
    if hint.is_empty() {
        text_line(
            ui.painter(),
            pos2(left, rect.center().y),
            label,
            theme::body(),
            p.text_1,
            width,
        );
    } else {
        let top = rect.center().y - (b + 4.0 + s) / 2.0;
        text_line(
            ui.painter(),
            pos2(left, top + b / 2.0),
            label,
            theme::body(),
            p.text_1,
            width,
        );
        text_line(
            ui.painter(),
            pos2(left, top + b + 4.0 + s / 2.0),
            hint,
            theme::small(),
            p.text_3,
            width,
        );
    }
    Some((resp, control))
}

fn switch(ui: &mut Ui, page: &mut Page, label: &str, hint: &str, on: &mut bool) {
    let Some((resp, c)) = row(ui, page, label, hint, SWITCH_W) else {
        return;
    };
    resp.widget_info(|| WidgetInfo::selected(Role::Switch, true, *on, label));
    let track = Rect::from_center_size(
        pos2(c.right() - SWITCH_W / 2.0, c.center().y),
        vec2(SWITCH_W, SWITCH_H),
    );
    paint_switch(ui, track, *on);
    if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
        *on = !*on;
        page.release = true;
    }
}

/// The 28 x 16 switch: accent track with an on-accent knob on the right
/// when on; a surface-3 track with a text-2 knob on the left when off.
fn paint_switch(ui: &Ui, rect: Rect, on: bool) {
    let p = theme::pal();
    let (track, knob, knob_x) = if on {
        (theme::accent(), p.on_accent, rect.right() - SWITCH_H / 2.0)
    } else {
        (p.surface_3, p.text_2, rect.left() + SWITCH_H / 2.0)
    };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((SWITCH_H / 2.0) as u8), track);
    painter.circle_filled(Pos2::new(knob_x, rect.center().y), KNOB_R, knob);
}

#[allow(clippy::too_many_arguments)]
fn slider_u8(
    ui: &mut Ui,
    page: &mut Page,
    label: &str,
    hint: &str,
    value: &mut u8,
    range: RangeInclusive<u8>,
    step: u8,
    fmt: impl Fn(u8) -> String,
) {
    let Some((_, c)) = row(ui, page, label, hint, SLIDER_W + VALUE_W) else {
        return;
    };
    let p = theme::pal();
    ui.painter().text(
        pos2(c.left() + VALUE_W - theme::SPACE_12, c.center().y),
        Align2::RIGHT_CENTER,
        fmt(*value),
        theme::secondary(),
        p.text_2,
    );
    let track = Rect::from_min_size(
        pos2(c.right() - SLIDER_W, c.center().y - 8.0),
        vec2(SLIDER_W, 16.0),
    );
    let mut v = f32::from(*value);
    let range = f32::from(*range.start())..=f32::from(*range.end());
    if slider(
        ui,
        page,
        track,
        Id::unique(("settings-slider", label)),
        &mut v,
        range,
        f32::from(step),
        None,
    ) {
        *value = v as u8;
    }
}

/// A slider across `rect`. `rainbow` paints a hue track instead of the
/// accent fill. Returns whether the value changed.
#[allow(clippy::too_many_arguments)]
fn slider(
    ui: &mut Ui,
    page: &mut Page,
    rect: Rect,
    id: Id,
    value: &mut f32,
    range: RangeInclusive<f32>,
    step: f32,
    rainbow: Option<&dyn Fn(f32) -> Color32>,
) -> bool {
    let p = theme::pal();
    let (lo, hi) = (*range.start(), *range.end());
    let resp = ui
        .interact(rect.expand2(vec2(4.0, 6.0)), id, Sense::click_and_drag())
        .on_hover_cursor(CursorIcon::PointingHand);
    let mut changed = false;
    if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let t = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let v = ((lo + t * (hi - lo)) / step).round() * step;
        let v = v.clamp(lo, hi);
        if (v - *value).abs() > f32::EPSILON {
            *value = v;
            changed = true;
        }
    }
    if resp.dragged() {
        page.dragging = true;
    }
    if resp.drag_stopped() || resp.clicked() {
        page.release = true;
    }
    let t = if hi > lo {
        (*value - lo) / (hi - lo)
    } else {
        0.0
    };
    let knob_x = rect.left() + t.clamp(0.0, 1.0) * rect.width();
    let cy = rect.center().y;
    let painter = ui.painter();
    if let Some(hue) = rainbow {
        let n = 36;
        let w = rect.width() / n as f32;
        for i in 0..n {
            let seg = Rect::from_min_size(
                pos2(rect.left() + i as f32 * w, cy - 3.0),
                vec2(w + 0.5, 6.0),
            );
            painter.rect_filled(seg, CornerRadius::ZERO, hue((i as f32 + 0.5) / n as f32));
        }
    } else {
        let track = |x0: f32, x1: f32, c: Color32| {
            painter.rect_filled(
                Rect::from_min_max(
                    pos2(x0, cy - theme::SLIDER_TRACK_H / 2.0),
                    pos2(x1, cy + theme::SLIDER_TRACK_H / 2.0),
                ),
                CornerRadius::same(2),
                c,
            );
        };
        track(rect.left(), rect.right(), p.surface_3);
        track(rect.left(), knob_x, theme::accent());
    }
    let r = if resp.hovered() || resp.dragged() {
        KNOB_R + 1.0
    } else {
        KNOB_R
    };
    painter.circle_filled(pos2(knob_x, cy), r, p.text_1);
    if rainbow.is_some() {
        painter.circle_stroke(pos2(knob_x, cy), r, Stroke::new(2.0, p.bg));
    }
    changed
}

/// A segmented control.
fn choice<T: Copy + PartialEq>(
    ui: &mut Ui,
    page: &mut Page,
    label: &str,
    hint: &str,
    value: &mut T,
    options: &[(T, &str)],
) {
    let galleys: Vec<_> = options
        .iter()
        .map(|(_, text)| {
            ui.painter()
                .layout_no_wrap((*text).to_owned(), theme::row(), Color32::PLACEHOLDER)
        })
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 20.0).collect();
    let total: f32 = widths.iter().sum();
    let Some((_, c)) = row(ui, page, label, hint, total) else {
        return;
    };
    let p = theme::pal();
    let outer = Rect::from_min_size(
        pos2(c.right() - total, c.center().y - 13.0),
        vec2(total, 26.0),
    );
    let mut x = outer.left();
    for (i, ((opt, _), galley)) in options.iter().zip(galleys).enumerate() {
        let seg = Rect::from_min_size(pos2(x, outer.top()), vec2(widths[i], outer.height()));
        x += widths[i];
        let resp = ui
            .interact(
                seg,
                Id::unique(("settings-choice", label, i)),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand);
        let on = *opt == *value;
        let fill = if on {
            Some(p.surface_3)
        } else if resp.hovered() {
            Some(p.surface_2)
        } else {
            None
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(seg, CornerRadius::ZERO, fill);
        }
        if i > 0 {
            ui.painter().vline(
                seg.left(),
                seg.y_range(),
                Stroke::new(theme::STROKE_W, p.stroke_2),
            );
        }
        let fg = if on { p.text_1 } else { p.text_2 };
        ui.painter()
            .galley(seg.center() - galley.size() / 2.0, galley, fg);
        if resp.clicked() {
            *value = *opt;
            page.release = true;
        }
    }
    ui.painter().rect_stroke(
        outer,
        theme::RADIUS_CONTROL,
        Stroke::new(theme::STROKE_W, p.stroke_2),
        StrokeKind::Inside,
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    Primary,
    Danger,
}

fn button_width(ui: &Ui, text: &str, icon: Option<&str>) -> f32 {
    let g = ui
        .painter()
        .layout_no_wrap(text.to_owned(), theme::row(), Color32::PLACEHOLDER);
    g.size().x + 20.0 + if icon.is_some() { 20.0 } else { 0.0 }
}

fn button(
    ui: &mut Ui,
    rect: Rect,
    id: Id,
    text: &str,
    icon: Option<&str>,
    kind: Kind,
    enabled: bool,
) -> Response {
    let p = theme::pal();
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(rect, id, sense);
    let resp = if enabled {
        resp.on_hover_cursor(CursorIcon::PointingHand)
    } else {
        resp
    };
    let hovered = enabled && resp.hovered();
    let (fill, stroke, fg) = match kind {
        Kind::Primary => (
            Some(if hovered {
                theme::accent_hover()
            } else {
                theme::accent()
            }),
            None,
            p.on_accent,
        ),
        Kind::Plain => (hovered.then_some(p.surface_2), Some(p.stroke_2), p.text_1),
        Kind::Danger => (hovered.then_some(p.surface_2), Some(p.stroke_2), p.danger),
    };
    let fg = if enabled { fg } else { fg.gamma_multiply(0.45) };
    let fill = fill.map(|f| if enabled { f } else { f.gamma_multiply(0.45) });
    let painter = ui.painter();
    if let Some(fill) = fill {
        painter.rect_filled(rect, theme::RADIUS_CONTROL, fill);
    }
    if let Some(stroke) = stroke {
        painter.rect_stroke(
            rect,
            theme::RADIUS_CONTROL,
            Stroke::new(theme::STROKE_W, stroke),
            StrokeKind::Inside,
        );
    }
    let mut x = rect.left() + 10.0;
    if let Some(icon) = icon {
        painter.text(
            pos2(x + 7.0, rect.center().y),
            Align2::CENTER_CENTER,
            icon,
            theme::icon(14.0),
            fg,
        );
        x += 20.0;
    }
    painter.text(
        pos2(x, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        theme::row(),
        fg,
    );
    resp
}

/// A run of buttons from the left of the current line, `CONTROL_H` tall.
/// Returns which one was clicked.
fn button_bar(ui: &mut Ui, id: &str, items: &[(&str, Option<&str>, Kind, bool)]) -> Option<usize> {
    let widths: Vec<f32> = items
        .iter()
        .map(|(t, i, _, _)| button_width(ui, t, *i))
        .collect();
    let total = widths.iter().sum::<f32>() + theme::SPACE_8 * items.len().saturating_sub(1) as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(total, CONTROL_H), Sense::hover());
    let mut x = rect.left();
    let mut clicked = None;
    for (i, ((text, icon, kind, enabled), w)) in items.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(w, CONTROL_H));
        x += w + theme::SPACE_8;
        if button(ui, r, Id::unique((id, i)), text, *icon, *kind, *enabled).clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

// ------------------------------------------------------------------ gallery

#[derive(Clone)]
enum Pick {
    Saved(String),
    Draft,
}

fn gallery(ui: &mut Ui, page: &mut Page, b: &mut Builder) {
    let mut words = vec!["theme", "palette", "preset", "dark", "medium"];
    words.extend(theme::PALETTES.iter().map(|p| p.name));
    if !page.shows(&words) {
        return;
    }
    page.begin(ui);
    let p = theme::pal();
    let start = ui.cursor().min;
    let width = ui.available_width();
    let inner = width - 2.0 * PAD_X;
    let cards = |ui: &mut Ui,
                 caption: &str,
                 items: Vec<(Palette, String, Pick)>,
                 page: &mut Page,
                 b: &mut Builder| {
        if items.is_empty() {
            return;
        }
        ui.add_space(theme::SPACE_8);
        let (cap, _) = ui.allocate_exact_size(vec2(width, 18.0), Sense::hover());
        ui.painter().text(
            pos2(cap.left() + PAD_X, cap.center().y),
            Align2::LEFT_CENTER,
            caption,
            theme::caption(),
            p.text_3,
        );
        let gap = theme::SPACE_8;
        let cols = (((inner + gap) / (CARD_MIN_W + gap)).floor() as usize).max(1);
        let card_w = (inner - gap * (cols - 1) as f32) / cols as f32;
        let card_h = CARD_ART_H + 10.0 + theme::small().size + 6.0;
        for chunk in items.chunks(cols) {
            let (line, _) = ui.allocate_exact_size(vec2(width, card_h + gap / 2.0), Sense::hover());
            for (i, (palette, name, pick)) in chunk.iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(line.left() + PAD_X + i as f32 * (card_w + gap), line.top()),
                    vec2(card_w, card_h),
                );
                let selected = match pick {
                    Pick::Draft => b.on,
                    Pick::Saved(n) => !b.on && page.draft.theme == *n,
                };
                let resp = card(ui, r, palette, name, selected, pick);
                if resp.clicked() {
                    match pick {
                        Pick::Draft => b.on = true,
                        Pick::Saved(n) => {
                            b.on = false;
                            page.draft.theme.clone_from(n);
                            page.release = true;
                        }
                    }
                }
                if let Pick::Saved(n) = pick
                    && theme::preset(n).is_none()
                {
                    resp.context_menu(|ui| {
                        if ui.button("Edit in the builder").clicked() {
                            b.load(palette);
                            b.name.clone_from(n);
                        }
                        if ui.button("Delete").clicked() {
                            page.draft.custom_themes.retain(|t| t.name != *n);
                            if page.draft.theme == *n {
                                page.draft.theme = Settings::default().theme;
                            }
                            page.release = true;
                        }
                    });
                }
            }
        }
    };
    let by_tone = |tone| {
        theme::PALETTES
            .iter()
            .filter(|p| p.tone == tone)
            .map(|p| (*p, p.name.to_owned(), Pick::Saved(p.name.to_owned())))
            .collect::<Vec<_>>()
    };
    ui.add_space(theme::SPACE_4);
    cards(ui, "Dark", by_tone(Tone::Dark), page, b);
    cards(ui, "Medium", by_tone(Tone::Medium), page, b);
    let mut yours: Vec<(Palette, String, Pick)> = page
        .draft
        .custom_themes
        .iter()
        .map(|t| {
            (
                theme::from_colors(t.colors.map(color)),
                t.name.clone(),
                Pick::Saved(t.name.clone()),
            )
        })
        .collect();
    yours.push((b.palette(), format!("Editing: {}", b.name), Pick::Draft));
    cards(ui, "Yours (right-click to edit or delete)", yours, page, b);
    ui.add_space(theme::SPACE_8);
    let rect = Rect::from_min_max(start, pos2(start.x + width, ui.cursor().min.y));
    page.record(rect);
}

fn card(
    ui: &mut Ui,
    r: Rect,
    palette: &Palette,
    name: &str,
    selected: bool,
    pick: &Pick,
) -> Response {
    let p = theme::pal();
    let id = match pick {
        Pick::Draft => Id::unique("theme-card-draft"),
        Pick::Saved(n) => Id::unique(("theme-card", n)),
    };
    let resp = ui
        .interact(r, id, Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(name);
    let painter = ui.painter();
    if selected || resp.hovered() {
        painter.rect_filled(r, theme::RADIUS_CONTROL, p.surface_2);
    }
    if selected {
        painter.rect_stroke(
            r,
            theme::RADIUS_CONTROL,
            Stroke::new(theme::STROKE_W, theme::accent()),
            StrokeKind::Inside,
        );
    }
    let art = Rect::from_min_size(r.min + vec2(5.0, 5.0), vec2(r.width() - 10.0, CARD_ART_H));
    let c = palette;
    painter.rect_filled(art, 3, c.bg);
    painter.rect_stroke(
        art,
        3,
        Stroke::new(1.0, Color32::from_white_alpha(14)),
        StrokeKind::Inside,
    );
    painter.vline(art.left() + 10.0, art.y_range(), Stroke::new(1.0, c.stroke));
    let x = art.left() + 16.0;
    let w = art.right() - 6.0 - x;
    let bar = |y: f32, frac: f32, h: f32, col: Color32| {
        painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w * frac, h)), 2, col);
    };
    bar(art.top() + 6.0, 0.6, 4.0, c.text_1);
    painter.rect_filled(
        Rect::from_min_size(pos2(x - 2.0, art.top() + 13.0), vec2(w + 2.0, 8.0)),
        2,
        c.surface_3,
    );
    bar(art.top() + 15.5, 0.4, 3.0, c.fallback_accent);
    bar(art.top() + 25.0, 0.8, 4.0, c.text_2);
    bar(art.top() + 33.0, 0.45, 4.0, c.text_3);
    let fg = if selected { p.text_1 } else { p.text_2 };
    let mut tx = r.left() + 6.0;
    let ty = art.bottom() + 6.0 + theme::small().size / 2.0;
    if selected {
        painter.text(
            pos2(tx + 5.0, ty),
            Align2::CENTER_CENTER,
            icons::CHECK,
            theme::icon(11.0),
            fg,
        );
        tx += 13.0;
    }
    text_line(
        painter,
        pos2(tx, ty),
        name,
        theme::caption(),
        fg,
        r.right() - tx - 4.0,
    );
    resp
}

// ------------------------------------------------------------------ builder

fn builder(ui: &mut Ui, page: &mut Page, b: &mut Builder) {
    if !page.shows(&[
        "custom theme builder",
        "hue",
        "tint",
        "contrast",
        "darkness",
        "colors",
        "import",
        "export",
        "file",
        "contrast checks",
    ]) {
        return;
    }
    page.begin(ui);
    let p = theme::pal();
    let resp = Frame::NONE
        .inner_margin(Margin::same(PAD_X as i8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(theme::SPACE_8, theme::SPACE_8);
            builder_head(ui, page, b);
            builder_tabs(ui, b);
            match b.tab {
                Tab::Quick => quick(ui, page, b),
                Tab::Colors => every_color(ui, b),
                Tab::File => file(ui, b),
            }
            let shown = if b.on { b.palette() } else { theme::pal() };
            let name = if b.on {
                b.name.clone()
            } else {
                page.draft.theme.clone()
            };
            verdict(ui, &shown, &name, f64::from(page.draft.player_tint) / 100.0);
        });
    let _ = p;
    page.record(resp.response.rect);
}

fn builder_head(ui: &mut Ui, page: &mut Page, b: &mut Builder) {
    let passes = theme::checks(&b.palette(), f64::from(page.draft.player_tint) / 100.0)
        .iter()
        .all(theme::Check::ok);
    ui.horizontal(|ui| {
        ui.add(
            TextEdit::singleline(&mut b.name)
                .hint_text("Theme name")
                .font(theme::secondary())
                .desired_width((ui.available_width() - 330.0).max(120.0))
                .margin(Margin::symmetric(8, 7)),
        );
        let mut start: Option<Palette> = None;
        ComboBox::from_id_salt("builder-start")
            .selected_text(egui::RichText::new("Start from\u{2026}").font(theme::secondary()))
            .width(120.0)
            .show_ui(ui, |ui| {
                for preset in theme::PALETTES {
                    if ui.selectable_label(false, preset.name).clicked() {
                        start = Some(*preset);
                    }
                }
                for t in &page.draft.custom_themes {
                    if ui.selectable_label(false, &t.name).clicked() {
                        start = Some(theme::from_colors(t.colors.map(color)));
                    }
                }
            });
        if let Some(palette) = start {
            b.load(&palette);
        }
        let clicked = button_bar(
            ui,
            "builder-head",
            &[
                ("Surprise me", Some(icons::SHUFFLE), Kind::Plain, true),
                ("Save", Some(icons::FLOPPY_DISK), Kind::Primary, passes),
            ],
        );
        match clicked {
            Some(0) => surprise(b, f64::from(page.draft.player_tint) / 100.0),
            Some(1) => save_draft(page, b),
            _ => {}
        }
    });
    if !passes {
        ui.label(
            egui::RichText::new("Save unlocks once every check below passes.")
                .font(theme::small())
                .color(theme::pal().text_3),
        );
    }
}

fn save_draft(page: &mut Page, b: &mut Builder) {
    let mut name = saved::clean_name(&b.name);
    if name.is_empty() {
        name = "My theme".to_owned();
    }
    if theme::preset(&name).is_some() {
        name.push_str(" (custom)");
    }
    b.name.clone_from(&name);
    let theme = CustomTheme {
        name: name.clone(),
        colors: b.colors,
    };
    match page.draft.custom_themes.iter_mut().find(|t| t.name == name) {
        Some(t) => *t = theme,
        None => page.draft.custom_themes.push(theme),
    }
    page.draft.theme = name;
    b.on = false;
    page.release = true;
}

/// A random recipe that passes every check, from up to 60 tries.
fn surprise(b: &mut Builder, tint: f64) {
    let mut x = seed() | 1;
    let mut next = |lo: f32, hi: f32| {
        // xorshift64
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        lo + (x % 10_000) as f32 / 10_000.0 * (hi - lo)
    };
    for _ in 0..60 {
        let r = Recipe {
            hue: next(0.0, 360.0),
            tint: next(0.0, 0.04),
            darkness: next(0.13, 0.30),
            contrast: next(0.5, 1.0),
            steps: next(0.025, 0.07),
            accent_hue: next(0.0, 360.0),
        };
        let palette = r.palette();
        if theme::checks(&palette, tint).iter().all(theme::Check::ok) {
            b.recipe = r;
            b.set_colors(theme::colors(&palette).map(rgb));
            return;
        }
    }
}

fn builder_tabs(ui: &mut Ui, b: &mut Builder) {
    let p = theme::pal();
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
    ui.painter()
        .hline(line.x_range(), line.bottom() - 0.5, theme::hairline());
    let mut x = line.left();
    for (tab, label) in [
        (Tab::Quick, "Quick"),
        (Tab::Colors, "Every color"),
        (Tab::File, "Import / export"),
    ] {
        let g = ui
            .painter()
            .layout_no_wrap(label.to_owned(), theme::row(), Color32::PLACEHOLDER);
        let r = Rect::from_min_size(pos2(x, line.top()), vec2(g.size().x + 20.0, line.height()));
        x = r.right() + 2.0;
        let resp = ui
            .interact(r, Id::unique(("builder-tab", label)), Sense::click())
            .on_hover_cursor(CursorIcon::PointingHand);
        let on = b.tab == tab;
        let fg = if on || resp.hovered() {
            p.text_1
        } else {
            p.text_2
        };
        ui.painter().galley(
            pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0),
            g,
            fg,
        );
        if on {
            ui.painter().rect_filled(
                Rect::from_min_max(pos2(r.left(), r.bottom() - 2.0), r.max),
                CornerRadius::ZERO,
                theme::accent(),
            );
        }
        if resp.clicked() {
            b.tab = tab;
        }
    }
}

/// One Quick slider: label, value, range, step, hue track, value text.
type QuickRow<'a> = (&'a str, &'a mut f32, RangeInclusive<f32>, f32, bool, String);

fn quick(ui: &mut Ui, page: &mut Page, b: &mut Builder) {
    let hue = |t: f32| {
        color(crate::art_color::from_oklch(
            0.72,
            0.13,
            f64::from(t) * 360.0,
        ))
    };
    let mut r = b.recipe;
    let mut changed = false;
    {
        let rows: [QuickRow<'_>; 6] = [
            (
                "Hue",
                &mut r.hue,
                0.0..=360.0,
                1.0,
                true,
                format!("{:.0}\u{b0}", b.recipe.hue),
            ),
            (
                "Tint",
                &mut r.tint,
                0.0..=0.05,
                0.001,
                false,
                format!("{:.3}", b.recipe.tint),
            ),
            (
                "Darkness",
                &mut r.darkness,
                0.12..=0.34,
                0.005,
                false,
                format!("L {:.2}", b.recipe.darkness),
            ),
            (
                "Contrast",
                &mut r.contrast,
                0.0..=1.0,
                0.01,
                false,
                format!("{:.0}%", b.recipe.contrast * 100.0),
            ),
            (
                "Surface steps",
                &mut r.steps,
                0.02..=0.08,
                0.001,
                false,
                format!("{:.3}", b.recipe.steps),
            ),
            (
                "Accent hue",
                &mut r.accent_hue,
                0.0..=360.0,
                1.0,
                true,
                format!("{:.0}\u{b0}", b.recipe.accent_hue),
            ),
        ];
        for (label, value, range, step, rainbow, shown) in rows {
            let (line, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
            let p = theme::pal();
            ui.painter().text(
                pos2(line.left(), line.center().y),
                Align2::LEFT_CENTER,
                label,
                theme::body(),
                p.text_1,
            );
            ui.painter().text(
                pos2(line.right(), line.center().y),
                Align2::RIGHT_CENTER,
                shown,
                theme::secondary(),
                p.text_2,
            );
            let track = Rect::from_min_max(
                pos2(line.left() + QUICK_LABEL_W, line.center().y - 8.0),
                pos2(line.right() - VALUE_W, line.center().y + 8.0),
            );
            let paint: Option<&dyn Fn(f32) -> Color32> = if rainbow { Some(&hue) } else { None };
            changed |= slider(
                ui,
                page,
                track,
                Id::unique(("quick", label)),
                value,
                range,
                step,
                paint,
            );
        }
    }
    // The Quick sliders never touch the saved settings.
    page.dragging = false;
    page.release = false;
    if changed {
        b.recipe = r;
        b.set_colors(theme::colors(&r.palette()).map(rgb));
    }
}

fn every_color(ui: &mut Ui, b: &mut Builder) {
    let p = theme::pal();
    let mut changed = None;
    ui.columns(2, |cols| {
        for (i, (role, hint)) in theme::ROLES.iter().enumerate() {
            let ui = &mut cols[i % 2];
            ui.horizontal(|ui| {
                ui.set_min_height(34.0);
                let mut c = b.colors[i];
                if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                    changed = Some((i, c));
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new(*role)
                            .font(theme::row())
                            .color(p.text_1),
                    );
                    ui.label(
                        egui::RichText::new(*hint)
                            .font(theme::small())
                            .color(p.text_3),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let resp = ui.add(
                        TextEdit::singleline(&mut b.hex[i])
                            .font(theme::secondary())
                            .desired_width(70.0)
                            .margin(Margin::symmetric(6, 4)),
                    );
                    if resp.changed()
                        && let Some(c) = saved::parse_hex(&b.hex[i])
                    {
                        changed = Some((i, c));
                    }
                });
            });
        }
    });
    if let Some((i, c)) = changed {
        let mut colors = b.colors;
        colors[i] = c;
        let typed = b.hex[i].clone();
        b.set_colors(colors);
        // Keep what was typed (`aabbcc` stays lower case until it loses focus).
        if saved::parse_hex(&typed) == Some(c) {
            b.hex[i] = typed;
        }
    }
}

fn file(ui: &mut Ui, b: &mut Builder) {
    let p = theme::pal();
    if !b.file_edited {
        b.file = saved::theme_file(&b.name, &b.colors);
    }
    ui.label(
        egui::RichText::new(
            "A theme as text. Copy it to share it; paste one here and press Import.",
        )
        .font(theme::small())
        .color(p.text_3),
    );
    let resp = ui.add(
        TextEdit::multiline(&mut b.file)
            .font(theme::secondary())
            .desired_width(ui.available_width())
            .desired_rows(13)
            .margin(Margin::same(8)),
    );
    if resp.changed() {
        b.file_edited = true;
    }
    ui.horizontal(|ui| {
        let clicked = button_bar(
            ui,
            "builder-file",
            &[
                ("Copy", Some(icons::COPY), Kind::Plain, true),
                ("Import", Some(icons::DOWNLOAD_SIMPLE), Kind::Plain, true),
            ],
        );
        match clicked {
            Some(0) => {
                ui.ctx().copy_text(b.file.clone());
                b.message = "Copied".to_owned();
            }
            Some(1) => match saved::parse_theme_file(&b.file) {
                Ok((name, colors)) => {
                    if let Some(name) = name {
                        b.name = name;
                    }
                    b.load(&theme::from_colors(colors.map(color)));
                    b.message = "Imported".to_owned();
                }
                Err(n) => b.message = format!("Found {n} of 11 colors"),
            },
            _ => {}
        }
        if !b.message.is_empty() {
            ui.label(
                egui::RichText::new(&b.message)
                    .font(theme::small())
                    .color(p.text_3),
            );
        }
    });
}

/// The checks for the palette on screen, as the presets' tests run them.
fn verdict(ui: &mut Ui, palette: &Palette, name: &str, tint: f64) {
    let p = theme::pal();
    let checks = theme::checks(palette, tint);
    let bad = checks.iter().filter(|c| !c.ok()).count();
    let (icon, text, fg) = if bad == 0 {
        (
            icons::SEAL_CHECK,
            format!(
                "\u{201c}{name}\u{201d} passes all {} contrast checks, the same ones every preset is tested against.",
                checks.len()
            ),
            p.text_2,
        )
    } else {
        (
            icons::WARNING_CIRCLE,
            format!(
                "{bad} check{} fail for \u{201c}{name}\u{201d}. Text would be hard to read somewhere in the app.",
                if bad == 1 { "" } else { "s" }
            ),
            p.danger,
        )
    };
    Frame::NONE
        .fill(p.surface_2)
        .corner_radius(theme::RADIUS_CONTROL)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(icon).font(theme::icon(14.0)).color(fg));
                ui.label(egui::RichText::new(text).font(theme::secondary()).color(fg));
            });
        });
    ui.columns(2, |cols| {
        for (i, c) in checks.iter().enumerate() {
            let ui = &mut cols[i % 2];
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::hover());
            let fg = if c.ok() { p.text_2 } else { p.danger };
            let mark = if c.ok() { icons::CHECK } else { icons::X };
            ui.painter().text(
                pos2(r.left() + 6.0, r.center().y),
                Align2::CENTER_CENTER,
                mark,
                theme::icon(11.0),
                if c.ok() { p.text_3 } else { p.danger },
            );
            text_line(
                ui.painter(),
                pos2(r.left() + 16.0, r.center().y),
                c.what,
                theme::small(),
                fg,
                r.width() - 90.0,
            );
            let value = if c.what.starts_with("bg is") {
                if c.ok() {
                    "yes".to_owned()
                } else {
                    "too light".to_owned()
                }
            } else if c.ratio {
                format!("{:.1} \u{2265} {}", c.got, c.min)
            } else {
                format!("{:.3} \u{2265} {}", c.got, c.min)
            };
            ui.painter().text(
                pos2(r.right(), r.center().y),
                Align2::RIGHT_CENTER,
                value,
                theme::small(),
                fg,
            );
        }
    });
}

// ------------------------------------------------------------------ previews

/// A few rows of the main playlist in the current theme and sizes.
fn preview(ui: &mut Ui, page: &mut Page, state: &State) {
    if !page.shows(&["preview", "sample"]) {
        return;
    }
    page.begin(ui);
    let p = theme::pal();
    let rows: Vec<(String, String, String, Option<[u8; 3]>)> = state
        .main_view()
        .map(|v| {
            v.tracks
                .iter()
                .take(5)
                .map(|t| {
                    (
                        t.title.clone(),
                        crate::views::artists(t),
                        t.duration.map(crate::widgets::fmt_time).unwrap_or_default(),
                        state.art_colors.get(&t.id).copied(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let rows = if rows.is_empty() {
        vec![
            (
                "One More Time".to_owned(),
                "Daft Punk".to_owned(),
                "5:22".to_owned(),
                Some([0x2D, 0x63, 0xB8]),
            ),
            (
                "Teachers".to_owned(),
                "Daft Punk".to_owned(),
                "2:54".to_owned(),
                Some([0xB2, 0x2B, 0x2B]),
            ),
            (
                "Starboy".to_owned(),
                "The Weeknd".to_owned(),
                "4:34".to_owned(),
                Some([0x2F, 0x78, 0xC4]),
            ),
        ]
    } else {
        rows
    };
    let row_h = theme::row_h();
    let head_h = 52.0;
    let width = ui.available_width();
    let height = head_h + theme::TABLE_HEADER_H + row_h * rows.len() as f32 + theme::SPACE_8;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    page.record(rect);
    let painter = ui.painter_at(rect);
    let left = rect.left() + PAD_X;
    let right = rect.right() - PAD_X;
    // Header: cover, title, and a primary button in the accent.
    let cover = Rect::from_min_size(pos2(left, rect.top() + 8.0), vec2(36.0, 36.0));
    painter.rect_filled(
        cover,
        theme::RADIUS_ROW,
        Color32::from_rgb(0xB8, 0x8F, 0x4A),
    );
    painter.text(
        pos2(cover.right() + 10.0, cover.center().y - 1.0),
        Align2::LEFT_BOTTOM,
        state
            .main_view()
            .map_or("Everything", |v| v.summary.name.as_str()),
        theme::section(),
        p.text_1,
    );
    painter.text(
        pos2(cover.right() + 10.0, cover.center().y + 2.0),
        Align2::LEFT_TOP,
        "Playlist",
        theme::secondary(),
        p.text_2,
    );
    let bw = button_width(ui, "Shuffle all", Some(icons::SHUFFLE));
    let b = Rect::from_min_size(
        pos2(right - bw, cover.center().y - CONTROL_H / 2.0),
        vec2(bw, CONTROL_H),
    );
    button(
        ui,
        b,
        Id::unique("preview-shuffle"),
        "Shuffle all",
        Some(icons::SHUFFLE),
        Kind::Primary,
        true,
    );
    let painter = ui.painter_at(rect);
    let mut y = rect.top() + head_h;
    painter.hline(rect.x_range(), y + 0.5, theme::hairline());
    let col_artist = left + (right - left) * 0.55;
    painter.text(
        pos2(left + 18.0, y + theme::TABLE_HEADER_H / 2.0),
        Align2::CENTER_CENTER,
        "#",
        theme::caption(),
        p.text_3,
    );
    painter.text(
        pos2(left + 36.0, y + theme::TABLE_HEADER_H / 2.0),
        Align2::LEFT_CENTER,
        "Title",
        theme::caption(),
        p.text_3,
    );
    painter.text(
        pos2(col_artist, y + theme::TABLE_HEADER_H / 2.0),
        Align2::LEFT_CENTER,
        "Artist",
        theme::caption(),
        p.text_3,
    );
    painter.text(
        pos2(right, y + theme::TABLE_HEADER_H / 2.0),
        Align2::RIGHT_CENTER,
        "Time",
        theme::caption(),
        p.text_3,
    );
    y += theme::TABLE_HEADER_H;
    let art = theme::row_art();
    let show_art = page.draft.row_art;
    for (i, (title, artist, time, art_color)) in rows.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 1.0, y), vec2(width - 2.0, row_h));
        let playing = i == 1;
        if i == 2 {
            painter.rect_filled(r, theme::RADIUS_ROW, p.surface_3);
        }
        let cy = r.center().y;
        let (font, fg) = if playing {
            (theme::row_strong(), theme::accent())
        } else {
            (theme::row(), p.text_1)
        };
        if playing {
            painter.text(
                pos2(left + 18.0, cy),
                Align2::CENTER_CENTER,
                icons::SPEAKER_HIGH,
                theme::icon(14.0),
                theme::accent(),
            );
        } else {
            painter.text(
                pos2(left + 18.0, cy),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                theme::secondary(),
                p.text_3,
            );
        }
        let mut x = left + 36.0;
        if show_art {
            let a = Rect::from_min_size(pos2(x, cy - art / 2.0), vec2(art, art));
            painter.rect_filled(a, theme::RADIUS_ROW, art_color.map_or(p.surface_3, color));
            x += art + 10.0;
        }
        text_line(
            &painter,
            pos2(x, cy),
            title,
            font,
            fg,
            col_artist - x - 12.0,
        );
        text_line(
            &painter,
            pos2(col_artist, cy),
            artist,
            theme::secondary(),
            p.text_2,
            right - col_artist - 60.0,
        );
        painter.text(
            pos2(right, cy),
            Align2::RIGHT_CENTER,
            time,
            theme::secondary(),
            p.text_2,
        );
        y += row_h;
    }
}

fn lyrics_preview(ui: &mut Ui, page: &mut Page) {
    if !page.shows(&["preview", "lyrics"]) {
        return;
    }
    page.begin(ui);
    let p = theme::pal();
    let size = f32::from(page.draft.lyrics_size);
    let lines = [
        "Somewhere in the night I hear it",
        "A melody I almost know",
        "Turning every light to silver",
        "Carry me wherever you go",
    ];
    let gap = 8.0;
    let height = lines.len() as f32 * (size * 1.3 + gap) + 2.0 * theme::SPACE_16;
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    page.record(rect);
    let painter = ui.painter_at(rect);
    let mut y = rect.top() + theme::SPACE_16;
    for (i, line) in lines.iter().enumerate() {
        let current = i == 2;
        let (font, fg) = if current {
            (
                FontId::new(size, egui::FontFamily::Name(theme::INTER_600.into())),
                if page.draft.lyrics_accent {
                    theme::accent()
                } else {
                    p.text_1
                },
            )
        } else {
            (FontId::new(size, egui::FontFamily::Proportional), p.text_3)
        };
        let (pos, align) = if page.draft.lyrics_center {
            (pos2(rect.center().x, y), Align2::CENTER_TOP)
        } else {
            (pos2(rect.left() + theme::SPACE_16, y), Align2::LEFT_TOP)
        };
        painter.text(pos, align, *line, font, fg);
        y += size * 1.3 + gap;
    }
}

// --------------------------------------------------------------- library

fn main_playlist(ui: &mut Ui, page: &mut Page, state: &State, out: &mut Out) {
    let Some((_, c)) = row(
        ui,
        page,
        "Main playlist",
        "The playlist Rustle opens into; Liked Music works too",
        220.0,
    ) else {
        return;
    };
    let current = state.main_view().map(|v| v.summary.name.clone());
    let playlists = match &state.library {
        Loadable::Ready(list) => Some(list),
        _ => None,
    };
    let r = Rect::from_min_size(pos2(c.left(), c.center().y - 14.0), vec2(c.width(), 28.0));
    ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
        ComboBox::from_id_salt("main-playlist")
            .selected_text(
                egui::RichText::new(current.unwrap_or_else(|| "None".to_owned()))
                    .font(theme::secondary()),
            )
            .width(c.width())
            .show_ui(ui, |ui| match playlists {
                Some(list) => {
                    let main_id = state.main.as_ref().map(|(id, _)| id.as_str());
                    for pl in list.iter() {
                        if ui
                            .selectable_label(main_id == Some(pl.id.as_str()), &pl.name)
                            .clicked()
                            && main_id != Some(pl.id.as_str())
                        {
                            cmd(out, Command::SetMainPlaylist(pl.id.clone()));
                        }
                    }
                }
                None => {
                    ui.label("Loading your playlists\u{2026}");
                }
            });
    });
}

// ------------------------------------------------------------- shortcuts

fn shortcuts(ui: &mut Ui, page: &mut Page, view: &mut SettingsView) {
    let p = theme::pal();
    for action in Shortcut::ALL {
        let binding = page.draft.keys.get(action);
        let default = action.default_binding();
        let hint = if binding == default {
            String::new()
        } else {
            format!("Default {}", default.label())
        };
        let listening = view.listening == Some(action);
        let text = if listening {
            "Press a key\u{2026}".to_owned()
        } else {
            binding.label()
        };
        let g = ui
            .painter()
            .layout_no_wrap(text, theme::secondary(), Color32::PLACEHOLDER);
        let w = g.size().x + 16.0;
        let Some((_, c)) = row(ui, page, action.label(), &hint, w.max(80.0)) else {
            continue;
        };
        let chip = Rect::from_min_size(pos2(c.right() - w, c.center().y - 12.0), vec2(w, 24.0));
        let resp = ui
            .interact(
                chip,
                Id::unique(("shortcut", action.name())),
                Sense::click(),
            )
            .on_hover_cursor(CursorIcon::PointingHand)
            .on_hover_text("Click, then press the new key. Escape cancels.");
        let border = if listening {
            theme::accent()
        } else {
            p.stroke_2
        };
        ui.painter()
            .rect_filled(chip, theme::RADIUS_CONTROL, p.surface_2);
        ui.painter().rect_stroke(
            chip,
            theme::RADIUS_CONTROL,
            Stroke::new(theme::STROKE_W, border),
            StrokeKind::Inside,
        );
        let fg = if listening { theme::accent() } else { p.text_1 };
        ui.painter().galley(chip.center() - g.size() / 2.0, g, fg);
        if resp.clicked() {
            view.listening = if listening { None } else { Some(action) };
        }
    }
}
