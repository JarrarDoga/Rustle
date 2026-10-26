//! Saved login cookies in the OS keyring. Owned by the auth worker in batch 2.
//!
//! Never write cookies to a plain file or a log.

/// The saved cookies, if any. Called off the UI thread.
#[allow(dead_code)]
pub fn load_cookies() -> Option<String> {
    None
}

/// Save validated cookies. Called off the UI thread after `set_cookies` succeeds.
#[allow(dead_code)]
pub fn save_cookies(_cookies: &str) -> Result<(), String> {
    Err("cookie storage is not implemented yet".to_owned())
}

/// Forget the saved cookies.
#[allow(dead_code)]
pub fn clear_cookies() {}
