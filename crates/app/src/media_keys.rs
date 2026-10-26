//! MPRIS media keys and now-playing info. Owned by the media-keys worker in
//! batch 2.

use crate::state::{Command, State};

pub struct MediaKeys {
    _ctx: egui::Context,
}

impl MediaKeys {
    /// `ctx` lets a key press wake the UI with `request_repaint`.
    pub fn new(ctx: egui::Context) -> Self {
        Self { _ctx: ctx }
    }

    /// Commands from media keys since the last call. Called once per frame.
    pub fn poll(&mut self) -> Vec<Command> {
        Vec::new()
    }

    /// Publish the current track and position. Called once per frame; must be
    /// cheap when nothing changed.
    pub fn sync(&mut self, _state: &State) {}
}
