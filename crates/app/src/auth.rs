//! Saved login cookies in the OS keyring (the Secret Service on Linux, which
//! KWallet provides on KDE).
//!
//! Never write cookies to a plain file or a log. There is no file fallback:
//! without a keyring the user pastes cookies on every start.

use keyring::{Entry, Error};

const SERVICE: &str = "rustle";
const USER: &str = "youtube-music-cookies";

/// The saved cookies, if any. Called off the UI thread.
#[allow(dead_code)] // called by the batch 2 wiring worker
pub fn load_cookies() -> Option<String> {
    load(USER)
}

/// Save validated cookies. Called off the UI thread after `set_cookies` succeeds.
#[allow(dead_code)] // called by the batch 2 wiring worker
pub fn save_cookies(cookies: &str) -> Result<(), String> {
    save(USER, cookies)
}

/// Forget the saved cookies.
#[allow(dead_code)] // called by the batch 2 wiring worker
pub fn clear_cookies() {
    clear(USER);
}

/// Turn pasted text into what `MusicBackend::set_cookies` takes: a `Cookie`
/// header value or a Netscape cookies.txt body.
///
/// A cookies.txt body is only trimmed. A header loses a leading `Cookie:` and
/// its line breaks. Returns an empty string if nothing is left.
pub fn normalize_cookie_input(input: &str) -> String {
    let input = input.trim();
    // Same rule as ytm-core's `parse_cookie_input`: cookies.txt lines are
    // tab-separated with 7 fields; a header value never contains tabs.
    if input.starts_with('#') || input.lines().any(|l| l.split('\t').count() >= 7) {
        return input.to_owned();
    }
    let value = input
        .get(..7)
        .filter(|p| p.eq_ignore_ascii_case("cookie:"))
        .map_or(input, |_| &input[7..]);
    // A header copied across several lines: join the pieces with "; ".
    value
        .lines()
        .map(|l| l.trim().trim_end_matches(';').trim_end())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn entry(user: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, user).map_err(describe)
}

fn load(user: &str) -> Option<String> {
    let cookies = entry(user).ok()?.get_password().ok()?;
    (!cookies.is_empty()).then_some(cookies)
}

fn save(user: &str, cookies: &str) -> Result<(), String> {
    entry(user)?.set_password(cookies).map_err(describe)
}

fn clear(user: &str) {
    if let Ok(entry) = entry(user) {
        // `NoEntry` means there was nothing to forget; other errors leave
        // nothing else to try.
        let _ = entry.delete_credential();
    }
}

/// A message for the login screen. Never includes the secret: `BadEncoding`
/// carries the stored bytes, so it gets a fixed string.
fn describe(e: Error) -> String {
    match e {
        Error::PlatformFailure(err) | Error::NoStorageAccess(err) => format!(
            "Could not use the system keyring (Secret Service / KWallet), so the login \
             was not saved: {err}"
        ),
        Error::NoEntry => "No saved login in the system keyring.".to_owned(),
        Error::BadEncoding(_) => "The saved login in the system keyring is not text.".to_owned(),
        Error::TooLong(..) => "The cookies are too long for the system keyring.".to_owned(),
        Error::Invalid(..) | Error::Ambiguous(_) => {
            "The system keyring rejected the login entry.".to_owned()
        }
        _ => "The system keyring failed.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_header_is_kept() {
        assert_eq!(
            normalize_cookie_input("SAPISID=abc; __Secure-3PSID=def"),
            "SAPISID=abc; __Secure-3PSID=def"
        );
    }

    #[test]
    fn cookie_prefix_is_stripped() {
        for input in [
            "Cookie: SAPISID=abc; HSID=x",
            "cookie:SAPISID=abc; HSID=x",
            "COOKIE:   SAPISID=abc; HSID=x",
        ] {
            assert_eq!(normalize_cookie_input(input), "SAPISID=abc; HSID=x");
        }
    }

    #[test]
    fn header_whitespace_and_line_breaks() {
        assert_eq!(
            normalize_cookie_input("\r\n  Cookie: SAPISID=abc; HSID=x  \r\n"),
            "SAPISID=abc; HSID=x"
        );
        assert_eq!(
            normalize_cookie_input("SAPISID=abc;\r\nHSID=x\n\n SSID=y;\n"),
            "SAPISID=abc; HSID=x; SSID=y"
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(normalize_cookie_input(""), "");
        assert_eq!(normalize_cookie_input(" \n\t "), "");
        assert_eq!(normalize_cookie_input("Cookie:  "), "");
    }

    #[test]
    fn cookies_txt_is_only_trimmed() {
        let body = "# Netscape HTTP Cookie File\n\
                    .youtube.com\tTRUE\t/\tTRUE\t1790000000\tSAPISID\tabc\n\
                    .youtube.com\tTRUE\t/\tTRUE\t1790000000\tHSID\tx";
        assert_eq!(normalize_cookie_input(&format!("\n{body}\n\n")), body);
    }

    #[test]
    fn cookies_txt_without_header_line() {
        let body = ".youtube.com\tTRUE\t/\tTRUE\t1790000000\tSAPISID\tabc\n\
                    .youtube.com\tTRUE\t/\tTRUE\t1790000000\tHSID\tx";
        assert_eq!(normalize_cookie_input(&format!("  {body}\n")), body);
    }

    /// Deletes the test entry even if an assertion fails.
    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            clear(&self.0);
        }
    }

    /// Needs the session D-Bus and an unlocked Secret Service (KWallet).
    /// Run with `cargo test -p rustle -- --ignored keyring`.
    #[test]
    #[ignore = "needs the session D-Bus and a Secret Service"]
    fn keyring_round_trip() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let user = format!("test-round-trip-{}-{nanos}", std::process::id());
        let _cleanup = Cleanup(user.clone());

        assert_eq!(load(&user), None);
        save(&user, "SAPISID=test; HSID=test").unwrap();
        assert_eq!(load(&user).as_deref(), Some("SAPISID=test; HSID=test"));
        save(&user, "SAPISID=second").unwrap();
        assert_eq!(load(&user).as_deref(), Some("SAPISID=second"));
        clear(&user);
        assert_eq!(load(&user), None);
        // Clearing twice is harmless.
        clear(&user);
    }
}
