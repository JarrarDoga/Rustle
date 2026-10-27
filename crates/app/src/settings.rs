//! Saved settings: [`State::settings`](crate::state::Settings) to and from
//! the store, one key per field, plain text values.
//!
//! Integrator stub. The `settings` worker owns this file.

use store::Store;

use crate::state::Settings;

/// Reads the settings; missing or unreadable keys keep their defaults.
pub fn load(store: &Store) -> Settings {
    // b4-settings
    let _ = store;
    Settings::default()
}

/// Writes every field.
pub fn save(store: &Store, settings: &Settings) -> store::Result<()> {
    // b4-settings
    let _ = (store, settings);
    Ok(())
}
