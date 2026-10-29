//! Rebindable keyboard shortcuts: the actions, their default keys, and the
//! text the store keeps for a key ("Ctrl+ArrowRight").
//!
//! `control::shortcuts` reads the bindings from the settings each frame;
//! the settings page's Shortcuts section changes them.

use egui::{Key, Modifiers};

/// What a shortcut does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    TogglePlay,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    Filter,
    Locate,
    Lyrics,
    Queue,
    Settings,
    Quit,
}

impl Action {
    pub const ALL: [Self; 11] = [
        Self::TogglePlay,
        Self::Next,
        Self::Previous,
        Self::VolumeUp,
        Self::VolumeDown,
        Self::Filter,
        Self::Locate,
        Self::Lyrics,
        Self::Queue,
        Self::Settings,
        Self::Quit,
    ];

    /// Shown in the settings page.
    pub fn label(self) -> &'static str {
        match self {
            Self::TogglePlay => "Play / pause",
            Self::Next => "Next song",
            Self::Previous => "Previous song",
            Self::VolumeUp => "Volume up",
            Self::VolumeDown => "Volume down",
            Self::Filter => "Filter songs",
            Self::Locate => "Locate the playing song",
            Self::Lyrics => "Show or hide lyrics",
            Self::Queue => "Show or hide the queue",
            Self::Settings => "Open settings",
            Self::Quit => "Quit",
        }
    }

    /// The store key: `key.<name>`.
    pub fn name(self) -> &'static str {
        match self {
            Self::TogglePlay => "toggle_play",
            Self::Next => "next",
            Self::Previous => "previous",
            Self::VolumeUp => "volume_up",
            Self::VolumeDown => "volume_down",
            Self::Filter => "filter",
            Self::Locate => "locate",
            Self::Lyrics => "lyrics",
            Self::Queue => "queue",
            Self::Settings => "settings",
            Self::Quit => "quit",
        }
    }

    /// Held-key repeats count, so holding the key keeps stepping.
    pub fn repeats(self) -> bool {
        matches!(self, Self::VolumeUp | Self::VolumeDown)
    }

    pub fn default_binding(self) -> Binding {
        let ctrl = |key| Binding {
            ctrl: true,
            alt: false,
            shift: false,
            key,
        };
        match self {
            Self::TogglePlay => Binding {
                ctrl: false,
                alt: false,
                shift: false,
                key: Key::Space,
            },
            Self::Next => ctrl(Key::ArrowRight),
            Self::Previous => ctrl(Key::ArrowLeft),
            Self::VolumeUp => ctrl(Key::ArrowUp),
            Self::VolumeDown => ctrl(Key::ArrowDown),
            Self::Filter => ctrl(Key::F),
            Self::Locate => ctrl(Key::L),
            Self::Lyrics => ctrl(Key::Y),
            Self::Queue => ctrl(Key::U),
            Self::Settings => ctrl(Key::Comma),
            Self::Quit => ctrl(Key::Q),
        }
    }
}

/// A key with its modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: Key,
}

impl Binding {
    /// The modifiers to match: Ctrl as egui's portable `command`, as
    /// `Modifiers::COMMAND` is.
    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            alt: self.alt,
            ctrl: false,
            shift: self.shift,
            mac_cmd: false,
            command: self.ctrl,
        }
    }

    /// A pressed key: `None` for keys that cannot be bound (Escape cancels
    /// a rebind; Enter and Tab drive the table and the fields).
    pub fn from_press(key: Key, m: Modifiers) -> Option<Self> {
        if matches!(key, Key::Escape | Key::Enter | Key::Tab) || is_modifier(key) {
            return None;
        }
        Some(Self {
            ctrl: m.ctrl || m.command,
            alt: m.alt,
            shift: m.shift,
            key,
        })
    }

    /// The store's text: modifiers then egui's key name, `Ctrl+ArrowRight`.
    pub fn text(&self) -> String {
        self.join(self.key.name())
    }

    /// For the screen: `Ctrl+→`.
    pub fn label(&self) -> String {
        // Inter has these arrows; egui's own symbols are not in it.
        let key = match self.key {
            Key::ArrowLeft => "←",
            Key::ArrowRight => "→",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            k => k.symbol_or_name(),
        };
        self.join(key)
    }

    fn join(&self, key: &str) -> String {
        let mut out = String::new();
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.alt, "Alt+"),
            (self.shift, "Shift+"),
        ] {
            if on {
                out.push_str(name);
            }
        }
        out.push_str(key);
        out
    }

    /// Reads [`Binding::text`] back.
    pub fn parse(text: &str) -> Option<Self> {
        let mut b = Self {
            ctrl: false,
            alt: false,
            shift: false,
            key: Key::Space,
        };
        let mut parts = text.split('+').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                b.key = Key::from_name(part)?;
                return Some(b);
            }
            match part {
                "Ctrl" => b.ctrl = true,
                "Alt" => b.alt = true,
                "Shift" => b.shift = true,
                _ => return None,
            }
        }
        None
    }
}

/// Ctrl, Shift, Alt or Super on its own: a rebind waits for the key that
/// comes with it.
pub fn is_modifier(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

/// Every action's binding, in [`Action::ALL`] order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys([Binding; 11]);

impl Default for Keys {
    fn default() -> Self {
        Self(Action::ALL.map(Action::default_binding))
    }
}

impl Keys {
    fn index(action: Action) -> usize {
        Action::ALL
            .iter()
            .position(|&a| a == action)
            .unwrap_or_default()
    }

    pub fn get(&self, action: Action) -> Binding {
        self.0[Self::index(action)]
    }

    /// Binds `action` to `binding`. An action that had the same binding
    /// takes `action`'s old one, so no key does two things.
    pub fn set(&mut self, action: Action, binding: Binding) {
        let i = Self::index(action);
        let old = self.0[i];
        if let Some(j) = self.0.iter().position(|&b| b == binding) {
            self.0[j] = old;
        }
        self.0[i] = binding;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trips() {
        for a in Action::ALL {
            let b = a.default_binding();
            assert_eq!(Binding::parse(&b.text()), Some(b), "{}", b.text());
        }
        let b = Binding {
            ctrl: true,
            alt: true,
            shift: true,
            key: Key::Num5,
        };
        assert_eq!(b.text(), "Ctrl+Alt+Shift+5");
        assert_eq!(Binding::parse(&b.text()), Some(b));
        assert_eq!(Action::Next.default_binding().label(), "Ctrl+→");
    }

    #[test]
    fn bad_text_is_none() {
        for bad in ["", "Ctrl+", "Hyper+F", "Ctrl+NoSuchKey"] {
            assert_eq!(Binding::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn taking_a_used_key_swaps() {
        let mut keys = Keys::default();
        keys.set(Action::Lyrics, Action::Filter.default_binding());
        assert_eq!(keys.get(Action::Lyrics), Action::Filter.default_binding());
        assert_eq!(keys.get(Action::Filter), Action::Lyrics.default_binding());
    }

    #[test]
    fn escape_and_enter_cannot_be_bound() {
        assert!(Binding::from_press(Key::Escape, Modifiers::NONE).is_none());
        assert!(Binding::from_press(Key::Enter, Modifiers::CTRL).is_none());
        assert!(Binding::from_press(Key::ControlLeft, Modifiers::CTRL).is_none());
        assert!(Binding::from_press(Key::K, Modifiers::CTRL).is_some());
    }
}
