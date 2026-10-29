# Login cookies: use Firefox, not Chrome

Rustle signs in with a `Cookie` header copied from music.youtube.com and keeps it in the
system keyring (`crates/app/src/auth.rs`, KWallet on KDE). This page records why the cookie
has to come from Firefox, what went wrong with Chrome, and how to get a cookie that lasts.

## What the cookie is for

Only the account features go out signed in (`crates/ytm-core/src/rustypipe_backend.rs`):

| Signed in (cookie) | Anonymous (no cookie) |
| --- | --- |
| Library playlists, Liked Music, playlist contents | Search |
| Likes, unlikes, playlist adds and removes (`innertube.rs`) | Album and artist pages |
| Autoplay radio | Lyrics |
| | Streams: the visionOS client sends no cookie, only a visitor id (`visionos.rs`) |

So a dead cookie never stops playback. It shows the "session expired" banner, the library
still renders from the SQLite cache, and changes and refreshes stop until a new cookie is
pasted.

## What happened with Chrome (2026-10-28)

A cookie copied from a Chrome incognito window, closed without signing out (the method the
README described), was saved at 9:30 AM. By about 2:45 PM YouTube answered it with
`LOGGED_IN: false` and told the client to delete `SID`, `__Secure-1PSID` and the other login
cookies. Google had ended the session on its side after about 5 hours. Nothing in Rustle
caused it: the keyring still held the cookie and Rustle read it fine.

The likely cause, not a proven one: Chrome's Device Bound Session Credentials (DBSC), which
Google turned on for personal accounts in 2026 (Chrome 146). DBSC ties each Google login to
a key that never leaves the machine, so a copied cookie works only briefly and cannot be
renewed without that key. Two things fit:

- The Chrome cookie held `__Secure-1PSIDRTS` and `__Secure-3PSIDRTS`, which look like part of
  that system. The Firefox cookie that replaced it has neither.
- The session died after hours, the short lifetime DBSC gives a cookie without its key.

What is not confirmed: that those two cookies belong to DBSC, and that DBSC is on for Chrome
on Linux (reports at the time only confirmed Windows and Mac).

Sources: [BleepingComputer](https://www.bleepingcomputer.com/news/security/google-chrome-adds-session-cookie-theft-protection-for-all-users/),
[The Hacker News](https://thehackernews.com/2026/04/google-rolls-out-dbsc-in-chrome-146-to.html),
[Help Net Security](https://www.helpnetsecurity.com/2026/04/10/google-chrome-device-bound-session-credentials/),
[yt-dlp #8227](https://github.com/yt-dlp/yt-dlp/issues/8227).

## The fix: a Firefox cookie

The same day, the cookie was taken from Firefox instead, with no Rustle code change:

1. Firefox (`flatpak install --user -y flathub org.mozilla.firefox`) in a throwaway profile
   or a private window, signed in to music.youtube.com.
2. YouTube answered the new cookie with `LOGGED_IN: true` before it was saved.
3. It replaced the dead cookie in the keyring entry Rustle reads (`rustle` /
   `youtube-music-cookies`).
4. Firefox was closed **without signing out** and the throwaway profile deleted, so no
   browser keeps using that session. A browser that stays signed in renews the session's
   cookies, and the copy in Rustle is left behind with the old ones.

It has held since 2026-10-28 (still signed in on 2026-10-29).

## Getting a new cookie

1. Open a Firefox private window and sign in to music.youtube.com.
2. Press F12, open Network, click any music.youtube.com request, and copy its `Cookie`
   request header.
3. Close the private window **without signing out**, and paste the header into Rustle.

Do not sign out of that session later from another device or Google's security page: that
ends the same login Rustle uses. If a Firefox cookie also dies within a day or two, cookies
no longer work for the account, and Rustle needs a different sign-in method (a real code
change in `ytm-core` and `auth.rs`).
