# Rustle batch plan

The work after Phase 0 runs in batches of parallel workers. Batch 1 (5 workers)
built the scaffold, store, tooling, ytm-core, player, and the app shell.

## How a batch runs

1. **Integrator step (serial, one agent).** Widen the shared interfaces the
   batch needs: `model` types, `MusicBackend`, `Command` and state variants.
   Create every new file a worker will own, as a stub with `todo!()` or a
   no-op hook, so two workers never create the same file. Commit to `main`.
2. **Workers (parallel).** One git worktree and branch per worker under
   `~/Coding/Rustle-wt/<worker>`. Each worker edits only the paths in its
   "Owns" column. A worker that needs a change elsewhere stops and reports it.
3. **Merge (serial).** Merge each branch as it finishes. Regenerate a
   conflicting `Cargo.lock` with `git checkout --ours Cargo.lock && cargo
   metadata`. Run `scripts/check.sh > log; echo $?` and commit only on exit 0.
   Run `scripts/perf-check.sh` after any merge that touches the app or player.

Worker defaults: Claude Opus 5.5, high effort, fast mode off. Each brief names
the worktree path, the owned paths, the APIs it implements or consumes, and
its "done means" checks: build, `clippy -D warnings`, tests, and `fmt --check`
for the owned crates.

## Batch 2: real app (finishes Phase 1)

Goal: `rustle` without `--demo` logs in with your cookies, plays real tracks
through the real player, shows thumbnails, and answers media keys.

Integrator step:
- Split `crates/app/src/views.rs` into `views/` modules, with the login
  screen in `views/login.rs`.
- Create stub files: `app/src/auth.rs`, `app/src/thumbs.rs`,
  `app/src/media_keys.rs`.
- Add `Command::Login(String)` and `Command::Logout`, plus no-op hook calls
  for thumbnails and media keys in `app.rs`.

| Worker | Owns | Scope | Done means |
| --- | --- | --- | --- |
| playback | `crates/player/`, `crates/app/src/playback.rs` | Make `Player` `Send` (keep `cpal::Stream` on the engine thread) with a compile-time assert. Report stalls as `Buffering`. Subtract the AAC priming delay. Make `Drop` non-blocking. Implement the app's `Playback` trait for `Player`. | Real YouTube Opus and AAC play through the app's `Playback` trait in tests; all player tests pass |
| wiring | `crates/app/src/backend.rs`, `crates/app/src/main.rs`, `crates/app/src/app.rs` | Choose `RustyPipeBackend` without `--demo` and build it on the tokio runtime, never on the UI thread. Read the library through the store on open and write it back after fetches. Resolve `stream()`, then call `player.load`. Prefetch the next track's URL only. | `rustle` without `--demo` searches and plays a public track on Xvfb; demo mode unchanged |
| auth | `crates/app/src/views/login.rs`, `crates/app/src/auth.rs` | Paste a cookie header or cookies.txt, call `set_cookies`, and save to the keyring. Load from the keyring on start. Show the "session expired" banner on `BackendError::Auth` only, not `Network`. Add a log-out action. | Cookies round-trip through the keyring in a test; bad cookies show the error |
| thumbs | `crates/app/src/thumbs.rs`, `art()` in `crates/app/src/widgets.rs` | Pick the smallest thumbnail that fits the slot. Fetch and decode off the UI thread, once. Keep an LRU texture cache of about 200 images. Keep the letter tile as the placeholder. | A 5,000-row list scrolls without fetching off-screen art; RSS stays in budget |
| media-keys | `crates/app/src/media_keys.rs` | MPRIS through souvlaki: play/pause/next/previous/seek in, metadata and position out. | `playerctl` controls `rustle --demo` on Xvfb |
| opus | `crates/ytm-core/` | When web-client extraction fails, run `yt-dlp -f 251 -g` (with a timeout and a URL cache) and prefer that Opus stream over iOS AAC. Do not patch rustypipe from a git fork: `deny.toml` denies git sources. | `spike stream <id>` returns Opus 251 and its URL answers 206 |

## Batch 3: daily driver (Phase 2, cancel Spotify here)

Integrator step: widen `MusicBackend` with `like`, `unlike`,
`add_to_playlist`, `create_playlist`, `radio`, `home`, and `charts`, with
`FixtureBackend` implementations and `model` types for home and charts
sections. Create the view files below.

| Worker | Owns | Scope |
| --- | --- | --- |
| ytm-write | `crates/ytm-core/` | Implement like/unlike, add to playlist, create playlist, radio, home, charts in `RustyPipeBackend`, with recorded fixtures |
| radio | `crates/app/src/queue.rs`, `crates/app/src/backend.rs` | Autoplay from YTM radio when the queue runs out; near-gapless playback from the prefetched URL |
| library-edit | `crates/app/src/views/library.rs`, `crates/app/src/views/playlist.rs` | Like buttons, add-to-playlist menu, new playlist dialog, all optimistic |
| home | `crates/app/src/views/home.rs`, `crates/app/src/views/browse.rs` | Home feed, moods and genres, charts, new releases |
| offline | `crates/store/`, store calls in the backend | Library opens from SQLite with no network; staleness and refresh rules |
| spotify-import | new `crates/spotify-import/` | One-time import: export Spotify playlists through the Web API, match each track by YTM search, create the playlists |

## Batch 4: polish (Phase 3)

| Worker | Owns | Scope |
| --- | --- | --- |
| lyrics | `crates/app/src/views/lyrics.rs`, lyrics in `ytm-core` | Synced lyrics panel |
| control | new `crates/app/src/control.rs` | Keyboard shortcuts and a CLI control socket (`rustle play`, `rustle next`) |
| themes | `crates/app/src/theme.rs` | Theme choice, Discord rich presence |
| packaging | `packaging/` | RPM for Nobara |
| memory | crates the profile points at | Profile RSS on the real GPU and cut it, if Batch 2 measurements show a need |

## Decisions and user steps

1. **Decided 2026-10-26: keep rustypipe's QuickJS interpreter in-process.**
   AGENTS.md now bans browser engines and webviews, with an explicit
   exception for `rquickjs`. A helper process would have added an IPC
   protocol, a second binary, and more total RAM. No helper worker in Batch 3.
2. **Your step after Batch 2: paste cookies.** Library calls, private
   playlists, and Premium itag 774 cannot be tested until you log in through
   the new login screen.
3. **Your step after Batch 2: real-desktop RSS.** Run
   `scripts/perf-check.sh --no-headless` once on your desktop. The 129 MB
   measured so far ran under Xvfb with software rendering, so it is likely
   higher than on your GPU.
4. **Your step before Batch 3: Spotify Web API credentials** for the import
   worker (a client ID from developer.spotify.com).
