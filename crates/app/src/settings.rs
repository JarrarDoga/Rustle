//! Saved settings: [`State::settings`](crate::state::Settings) to and from
//! the store, one key per field, plain text values: `"true"` / `"false"`
//! for switches and the palette name for the theme.

use store::Store;

use crate::state::Settings;
use crate::theme;

const AUTOPLAY: &str = "autoplay";
const ACCENT_FROM_ART: &str = "accent_from_art";
const THEME: &str = "theme";

/// Reads the settings; missing or unreadable keys keep their defaults.
pub fn load(store: &Store) -> Settings {
    let mut s = Settings::default();
    if let Some(on) = read(store, AUTOPLAY).as_deref().and_then(parse_bool) {
        s.autoplay = on;
    }
    if let Some(on) = read(store, ACCENT_FROM_ART).as_deref().and_then(parse_bool) {
        s.accent_from_art = on;
    }
    // Only a palette that exists, so the picker always shows a selection.
    if let Some(name) = read(store, THEME).filter(|n| theme::PALETTES.iter().any(|p| p.name == n)) {
        s.theme = name;
    }
    s
}

/// Writes every field.
pub fn save(store: &Store, settings: &Settings) -> store::Result<()> {
    store.set_setting(AUTOPLAY, bool_text(settings.autoplay))?;
    store.set_setting(ACCENT_FROM_ART, bool_text(settings.accent_from_art))?;
    store.set_setting(THEME, &settings.theme)
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
        let want = Settings {
            autoplay: false,
            accent_from_art: false,
            theme: theme::PALETTES[theme::PALETTES.len() - 1].name.to_owned(),
        };
        save(&Store::open(&dir.db()).unwrap(), &want).unwrap();
        assert_eq!(load(&Store::open(&dir.db()).unwrap()), want);

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
