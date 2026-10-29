//! Saved settings: [`State::settings`](crate::state::Settings) to and from
//! the store, one key per field, plain text values: `"true"` / `"false"`
//! for switches, decimal numbers, the palette name for the theme, one
//! line per custom theme, and `key.<action>` = `Ctrl+ArrowRight` for the
//! shortcuts. A missing or unreadable value keeps its default.

use std::str::FromStr;

use store::Store;

use crate::shortcuts::{Action, Binding};
use crate::state::{CustomTheme, Settings};
use crate::theme;

const AUTOPLAY: &str = "autoplay";
const ACCENT_FROM_ART: &str = "accent_from_art";
const THEME: &str = "theme";
const CUSTOM_THEMES: &str = "custom_themes";
const PLAYER_TINT: &str = "player_tint";
const TEXT_SIZE: &str = "text_size";
const ROW_HEIGHT: &str = "row_height";
const UI_SCALE: &str = "ui_scale";
const ROW_ART: &str = "row_art";
const ALBUM_COLUMN: &str = "album_column";
const SPINE: &str = "spine";
const ANIMATIONS: &str = "animations";
const VOLUME_STEP: &str = "volume_step";
const LYRICS_SIZE: &str = "lyrics_size";
const LYRICS_CENTER: &str = "lyrics_center";
const LYRICS_ACCENT: &str = "lyrics_accent";
const NOT_HEARD_MONTHS: &str = "not_heard_months";
const REFRESH_ON_FOCUS: &str = "refresh_on_focus";

/// The allowed values of the settings that are a choice, not a range.
pub const ROW_HEIGHTS: [u8; 3] = [26, 32, 40];
pub const VOLUME_STEPS: [u8; 3] = [2, 5, 10];
pub const NOT_HEARD: [u8; 4] = [1, 3, 6, 12];
pub const TEXT_SIZES: std::ops::RangeInclusive<u8> = 10..=18;
pub const UI_SCALES: std::ops::RangeInclusive<u8> = 80..=130;
pub const PLAYER_TINTS: std::ops::RangeInclusive<u8> = 0..=24;
pub const LYRICS_SIZES: std::ops::RangeInclusive<u8> = 12..=28;

/// Reads the settings; missing or unreadable keys keep their defaults.
pub fn load(store: &Store) -> Settings {
    let mut s = Settings::default();
    let flag = |key: &str, v: &mut bool| {
        if let Some(on) = read(store, key).as_deref().and_then(parse_bool) {
            *v = on;
        }
    };
    flag(AUTOPLAY, &mut s.autoplay);
    flag(ACCENT_FROM_ART, &mut s.accent_from_art);
    flag(ROW_ART, &mut s.row_art);
    flag(ALBUM_COLUMN, &mut s.album_column);
    flag(SPINE, &mut s.spine);
    flag(ANIMATIONS, &mut s.animations);
    flag(LYRICS_CENTER, &mut s.lyrics_center);
    flag(LYRICS_ACCENT, &mut s.lyrics_accent);
    flag(REFRESH_ON_FOCUS, &mut s.refresh_on_focus);
    let number = |key: &str, ok: &dyn Fn(u8) -> bool, v: &mut u8| {
        if let Some(n) = read(store, key)
            .and_then(|t| u8::from_str(&t).ok())
            .filter(|&n| ok(n))
        {
            *v = n;
        }
    };
    number(
        PLAYER_TINT,
        &|n| PLAYER_TINTS.contains(&n),
        &mut s.player_tint,
    );
    number(TEXT_SIZE, &|n| TEXT_SIZES.contains(&n), &mut s.text_size);
    number(ROW_HEIGHT, &|n| ROW_HEIGHTS.contains(&n), &mut s.row_height);
    number(UI_SCALE, &|n| UI_SCALES.contains(&n), &mut s.ui_scale);
    number(
        VOLUME_STEP,
        &|n| VOLUME_STEPS.contains(&n),
        &mut s.volume_step,
    );
    number(
        LYRICS_SIZE,
        &|n| LYRICS_SIZES.contains(&n),
        &mut s.lyrics_size,
    );
    number(
        NOT_HEARD_MONTHS,
        &|n| NOT_HEARD.contains(&n),
        &mut s.not_heard_months,
    );
    if let Some(text) = read(store, CUSTOM_THEMES) {
        s.custom_themes = parse_custom_themes(&text);
    }
    for action in Action::ALL {
        if let Some(b) = read(store, &key_name(action))
            .as_deref()
            .and_then(Binding::parse)
        {
            s.keys.set(action, b);
        }
    }
    // Only a palette that exists, so the gallery always shows a selection.
    if let Some(name) = read(store, THEME)
        .filter(|n| theme::preset(n).is_some() || s.custom_themes.iter().any(|t| &t.name == n))
    {
        s.theme = name;
    }
    s
}

/// Writes every field.
pub fn save(store: &Store, settings: &Settings) -> store::Result<()> {
    let s = settings;
    for (key, on) in [
        (AUTOPLAY, s.autoplay),
        (ACCENT_FROM_ART, s.accent_from_art),
        (ROW_ART, s.row_art),
        (ALBUM_COLUMN, s.album_column),
        (SPINE, s.spine),
        (ANIMATIONS, s.animations),
        (LYRICS_CENTER, s.lyrics_center),
        (LYRICS_ACCENT, s.lyrics_accent),
        (REFRESH_ON_FOCUS, s.refresh_on_focus),
    ] {
        store.set_setting(key, bool_text(on))?;
    }
    for (key, n) in [
        (PLAYER_TINT, s.player_tint),
        (TEXT_SIZE, s.text_size),
        (ROW_HEIGHT, s.row_height),
        (UI_SCALE, s.ui_scale),
        (VOLUME_STEP, s.volume_step),
        (LYRICS_SIZE, s.lyrics_size),
        (NOT_HEARD_MONTHS, s.not_heard_months),
    ] {
        store.set_setting(key, &n.to_string())?;
    }
    store.set_setting(CUSTOM_THEMES, &custom_themes_text(&s.custom_themes))?;
    for action in Action::ALL {
        store.set_setting(&key_name(action), &s.keys.get(action).text())?;
    }
    store.set_setting(THEME, &s.theme)
}

fn key_name(action: Action) -> String {
    format!("key.{}", action.name())
}

/// One line per theme: its name, a tab, then 11 `#RRGGBB` colors.
fn custom_themes_text(themes: &[CustomTheme]) -> String {
    let mut out = String::new();
    for t in themes {
        out.push_str(&clean_name(&t.name));
        out.push('\t');
        let colors: Vec<String> = t.colors.iter().map(|&c| hex_text(c)).collect();
        out.push_str(&colors.join(" "));
        out.push('\n');
    }
    out
}

fn parse_custom_themes(text: &str) -> Vec<CustomTheme> {
    text.lines()
        .filter_map(|line| {
            let (name, colors) = line.split_once('\t')?;
            let colors: Vec<[u8; 3]> = colors.split(' ').map(parse_hex).collect::<Option<_>>()?;
            Some(CustomTheme {
                name: name.to_owned(),
                colors: colors.try_into().ok()?,
            })
        })
        .filter(|t| !t.name.is_empty())
        .collect()
}

/// A theme name as the store keeps it: one line, no tabs, trimmed.
pub fn clean_name(name: &str) -> String {
    name.replace(['\t', '\n', '\r'], " ").trim().to_owned()
}

/// `#RRGGBB`.
pub fn hex_text([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

/// `#RRGGBB` or `RRGGBB`, any case.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let n = u32::from_str_radix(t, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// A theme file for the builder's Import / export tab:
/// `name = "..."` then one `role = "#RRGGBB"` line per color.
pub fn theme_file(name: &str, colors: &[[u8; 3]; 11]) -> String {
    let mut out = format!(
        "# Rustle theme\nname = \"{}\"\n\n",
        clean_name(name).replace('"', "'")
    );
    for ((role, hint), &c) in theme::ROLES.iter().zip(colors) {
        out.push_str(&format!("{role:<16}= \"{}\"  # {hint}\n", hex_text(c)));
    }
    out
}

/// Reads [`theme_file`] back: the name (if any) and all 11 colors, or how
/// many colors it found.
pub fn parse_theme_file(text: &str) -> Result<(Option<String>, [[u8; 3]; 11]), usize> {
    let mut name = None;
    let mut colors: [Option<[u8; 3]>; 11] = [None; 11];
    for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        // A quoted value runs to its closing quote; a bare one is one word.
        let value = match value.strip_prefix('"') {
            Some(rest) => rest.split('"').next().unwrap_or(rest),
            None => value.split_whitespace().next().unwrap_or(""),
        };
        if key == "name" {
            name = Some(clean_name(value)).filter(|n| !n.is_empty());
        } else if let Some(i) = theme::ROLES.iter().position(|(r, _)| *r == key) {
            colors[i] = parse_hex(value);
        }
    }
    let found = colors.iter().flatten().count();
    let all: Option<Vec<[u8; 3]>> = colors.into_iter().collect();
    match all.and_then(|v| v.try_into().ok()) {
        Some(c) => Ok((name, c)),
        None => Err(found),
    }
}

/// One stored value; a store error reads as missing.
fn read(store: &Store, key: &str) -> Option<String> {
    store.setting(key).unwrap_or_else(|e| {
        eprintln!("rustle: read setting {key}: {e}");
        None
    })
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn bool_text(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use model::Track;

    use super::*;
    use crate::queue::tests::track;
    use crate::state::{Event, State, on_event};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "rustle-settings-test-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn db(&self) -> PathBuf {
            self.0.join("cache.sqlite")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_database_gives_defaults() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(load(&s), Settings::default());
    }

    #[test]
    fn round_trip_survives_reopen() {
        let dir = TempDir::new("round-trip");
        let mut want = Settings {
            autoplay: false,
            accent_from_art: false,
            theme: theme::PALETTES[theme::PALETTES.len() - 1].name.to_owned(),
            custom_themes: vec![CustomTheme {
                name: "Moss".to_owned(),
                colors: theme::colors(&theme::GRAPHITE).map(|c| [c.r(), c.g(), c.b()]),
            }],
            player_tint: 20,
            text_size: 16,
            row_height: 40,
            ui_scale: 115,
            row_art: false,
            album_column: false,
            spine: false,
            animations: false,
            volume_step: 10,
            lyrics_size: 22,
            lyrics_center: true,
            lyrics_accent: true,
            not_heard_months: 12,
            refresh_on_focus: false,
            ..Settings::default()
        };
        want.keys
            .set(Action::Lyrics, Binding::parse("Ctrl+Shift+K").unwrap());
        save(&Store::open(&dir.db()).unwrap(), &want).unwrap();
        assert_eq!(load(&Store::open(&dir.db()).unwrap()), want);

        // A custom theme can be the theme.
        want.theme = "Moss".to_owned();
        save(&Store::open(&dir.db()).unwrap(), &want).unwrap();
        assert_eq!(load(&Store::open(&dir.db()).unwrap()).theme, "Moss");

        // Saving again replaces the values.
        let s = Store::open(&dir.db()).unwrap();
        save(&s, &Settings::default()).unwrap();
        assert_eq!(load(&s), Settings::default());
        assert_eq!(s.setting(AUTOPLAY).unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn bad_values_keep_defaults() {
        let s = Store::open_in_memory().unwrap();
        s.set_setting(AUTOPLAY, "yes").unwrap();
        s.set_setting(ACCENT_FROM_ART, "").unwrap();
        s.set_setting(THEME, "No Such Palette").unwrap();
        assert_eq!(load(&s), Settings::default());

        // One bad key does not reset the good ones.
        s.set_setting(AUTOPLAY, "false").unwrap();
        s.set_setting(ACCENT_FROM_ART, "TRUE").unwrap();
        let got = load(&s);
        assert!(!got.autoplay);
        assert_eq!(got.accent_from_art, Settings::default().accent_from_art);
        assert_eq!(got.theme, Settings::default().theme);

        // Out-of-range numbers and broken themes are dropped too.
        s.set_setting(ROW_HEIGHT, "33").unwrap();
        s.set_setting(TEXT_SIZE, "200").unwrap();
        s.set_setting(CUSTOM_THEMES, "Bad\t#123456 #zzzzzz\nShort\t#111111\n")
            .unwrap();
        s.set_setting("key.next", "Ctrl+Nope").unwrap();
        let got = load(&s);
        assert_eq!(got.row_height, 32);
        assert_eq!(got.text_size, 13);
        assert!(got.custom_themes.is_empty());
        assert_eq!(got.keys, Settings::default().keys);
    }

    #[test]
    fn theme_file_round_trips() {
        let colors = theme::colors(&theme::NORD).map(|c| [c.r(), c.g(), c.b()]);
        let text = theme_file("Night\tOwl", &colors);
        assert!(text.contains("name = \"Night Owl\""));
        assert!(text.contains("text_1          = \"#ECEFF4\""));
        assert_eq!(
            parse_theme_file(&text),
            Ok((Some("Night Owl".into()), colors))
        );
        // Hand-written: any order, lower case, no quotes, no name.
        let mut hand: String = theme::ROLES
            .iter()
            .zip(colors)
            .rev()
            .map(|((r, _), c)| format!("{r}={}\n", hex_text(c).to_lowercase()))
            .collect();
        assert_eq!(parse_theme_file(&hand), Ok((None, colors)));
        hand = hand.replace("bg=", "background=");
        assert_eq!(parse_theme_file(&hand), Err(10));
    }

    /// The restart path for the queue: what the backend saves
    /// (`save_queue` then `save_queue_extras`), read back as `read_cache`
    /// does and fed to the reducer, restores the label, the radio marks
    /// and the tracks queued to play next.
    #[test]
    fn queue_label_and_radio_survive_restart() {
        let dir = TempDir::new("queue");
        let now = Instant::now();
        {
            let mut state = State::default();
            state.queue.set_autoplay(true);
            state.queue.play_scope(
                &[track("a"), track("b")],
                "Everything \u{b7} 2".into(),
                false,
                1,
            );
            state.queue.extend_radio(vec![track("r1"), track("r2")]);
            state.queue.add(track("x"));
            state.queue.add(track("y"));
            let entries = state.queue.entries();
            let tracks: Vec<Track> = entries.iter().map(|e| Track::clone(&e.track)).collect();
            let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
            let radio: Vec<bool> = entries.iter().map(|e| e.radio).collect();
            let queued: Vec<bool> = entries.iter().map(|e| e.queued).collect();

            let store = Store::open(&dir.db()).unwrap();
            store.upsert_tracks(&tracks).unwrap();
            store.save_queue(&ids, state.queue.current_index()).unwrap();
            store
                .save_queue_extras(state.queue.playing_from(), &radio, &queued)
                .unwrap();
            save(
                &store,
                &Settings {
                    autoplay: false,
                    ..Settings::default()
                },
            )
            .unwrap();
        }

        let store = Store::open(&dir.db()).unwrap();
        let mut state = State::default();
        on_event(&mut state, Event::CachedSettings(load(&store)), now);
        let (tracks, current) = store.load_queue().unwrap();
        let (label, radio, queued) = store.queue_extras().unwrap();
        on_event(
            &mut state,
            Event::CachedQueue {
                tracks,
                current,
                label,
                radio,
                queued,
            },
            now,
        );
        assert!(!state.settings.autoplay);
        assert!(!state.queue.autoplay());
        assert_eq!(state.queue.playing_from(), Some("Everything \u{b7} 2"));
        let marks: Vec<(&str, bool, bool)> = state
            .queue
            .entries()
            .iter()
            .map(|e| (e.track.id.as_str(), e.radio, e.queued))
            .collect();
        assert_eq!(
            marks,
            [
                ("a", false, false),
                ("x", false, true),
                ("y", false, true),
                ("b", false, false),
                ("r1", true, false),
                ("r2", true, false)
            ]
        );
        assert_eq!(state.queue.upcoming_position("y"), Some(2));
        assert_eq!(state.queue.upcoming_position("b"), None);
        assert_eq!(
            state.queue.current().map(|e| e.track.id.as_str()),
            Some("a")
        );
    }
}
