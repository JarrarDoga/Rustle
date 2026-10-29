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

## Batch 3: daily driver plumbing (Phase 2, cancel Spotify here)

**Merged 2026-10-26** (`main` at `e5fde1c`). `scripts/check.sh` passes;
perf-check: binary 20.8 MiB, peak RSS 140.6 MiB (demo; the art color pass
measured +0.6 to 1.0 MB on its own), idle CPU 0%, cold start 101 ms.
Accepted as done by the user on 2026-10-26; the live checks below move to
one debugging session after the UI batch and Batch 4. Still open:
- Live checks (BATCH3.md section 7) wait for the Liked-songs migration job to
  print `done:`. The ytm-write worker also got HTTP 403 on all live reads on
  2026-10-26. The like, edit_playlist, owned-browse and radio fixtures are
  hand-built from ytmusicapi; re-record them with `spike record-owned` and
  `spike record-radio` once reads work.
- The UI must show `state::NEEDS_LOGIN` when `State::needs_login()` is true
  (guests still see "Loading…").
- Not persisted across restarts: the autoplay setting, the queue's "Playing
  from" label, and radio entry marks (`save_queue`/`load_queue`).
- The color pass now uses 2 fetch workers (Batch 4 memory pass: about 16 s
  for 1,535 generated thumbnails from a local server; real CDN not measured). Art with no hue
  gets a gray accent at L 0.80.

Revised 2026-10-26 for Direction D. Batch 3 builds the data and behavior the
D screens need and adds no new screens: the UI batch after it draws them.
Full brief for the team lead: `docs/handoff/BATCH3.md`.

Integrator step: widen `MusicBackend` with `like`, `unlike`,
`add_to_playlist`, `remove_from_playlist`, `radio` and `clear_cookies`, with
`FixtureBackend` implementations; add the playlist entry id that removal
needs to `model`; add the new `Command`, `Effect` and `Event` variants;
widen the `Store` API with stubs.

| Worker | Owns | Scope |
| --- | --- | --- |
| ytm-write | `crates/ytm-core/` | Like, unlike, add to and remove from a playlist, radio, clear cookies; smaller thumbnail URLs; recorded fixtures |
| queue | `crates/app/src/queue.rs` | Scope shuffle and play in order, "playing from" label, autoplay radio when the queue runs out |
| store | `crates/store/` | Main playlist setting, play history, last played, not heard in 3 months, art color cache, artist and album counts, staleness |
| art | `crates/app/src/thumbs.rs`, new `crates/app/src/art_color.rs` | Dominant art color to OKLCH accent, computed once off the UI thread and cached; thumbnail disk cache eviction |
| wiring | `crates/app/src/backend.rs`, `crates/app/src/state.rs` | Optimistic like and add/remove with rollback, record plays, offline start, guest "needs login", logout, buffering debounce |

Cut: `home` (Direction D has no Home screen). Deferred: `spotify-import`
(needs a Spotify client ID; the user's own playlist is already migrated by
the scripts in `~/Coding/spotify-ytm-migration/`).

## Batch 4: polish (Phase 3)

**Merged 2026-10-27** (`main` at `7448932`). `scripts/check.sh` passes.
perf-check: demo peak RSS 117.5 MiB, large mode (`--large`, 1,535 tracks)
137.9 MiB (goal 135 missed; about 100 MiB of it is llvmpipe under Xvfb),
binary 21.3 MiB, idle CPU 0%, cold start 76 ms (demo) / 123 ms (large).
RPM: `packaging/build-rpm.sh` builds `target/generate-rpm/rustle-0.1.0-1.x86_64.rpm`
(21 MB, 5 files); not installed. The art color pass now runs 2 fetch
workers. Still open, all for the live debugging session:
- Real-desktop RSS on the NVIDIA GPU: `scripts/perf-check.sh --no-headless`
  and `--large` is headless-only.
- `rustle next` while the window is minimized on Wayland (socket commands
  are handled when a frame runs); `raise` focus under KWin.
- Escape closing the settings popover (not confirmed under Xvfb).
- Live lyrics through the app (verified only through the spike).
- No `LICENSE` file: the RPM declares GPL-3.0-or-later without the text.

Runs after the UI batch. Revised 2026-10-27. Full brief for the team lead:
`docs/handoff/BATCH4.md`. User decisions (2026-10-27): no Discord presence;
theme choice stays, as dark and medium presets only (no light palettes),
built by a `themes` worker in the presence slot.

Integrator step (done 2026-10-27): `model::Lyrics`, `MusicBackend::lyrics`
(demo: synced placeholder lines on the first Everything track);
`State::settings` (`Settings { autoplay, accent_from_art, theme }`, saved
through `settings.rs`), `Command::SetSetting`, `Command::ShowLyrics(bool)`
with `state.lyrics_open` (the reducer fetches on track change while open);
queue label and radio marks ride on `Event::CachedQueue`; `control.rs`
(`Control` like `MediaKeys`: socket commands go through the UI thread),
`run_cli` in `main.rs` before tokio; lyrics button in the player bar,
settings button and popover from the rail, `views/theme_picker.rs`;
`theme::set_palette_by_name`, `theme::body()` and `small()`.

| Worker | Owns | Scope |
| --- | --- | --- |
| lyrics | lyrics in `ytm-core`, `views/lyrics.rs` | Synced lyrics (timed via the Android Music client, plain fallback) in a drawer |
| control | `crates/app/src/control.rs` | Keyboard shortcuts, control socket, `rustle play/next/status` CLI, single instance |
| settings | `crates/store/`, `settings.rs`, `views/settings.rs`, `queue.rs` | Saved settings (autoplay, accent from art, theme); queue label and radio marks survive restart |
| themes | `theme.rs` palettes, `views/theme_picker.rs` | Well-known dark and medium palettes (no light) mapped onto the D neutrals; album art stays the only hue source |
| packaging | `packaging/`, RPM metadata in `crates/app/Cargo.toml` | RPM for Nobara with desktop file, icon and the botguard helper |
| memory (serial, last) | any crate | Large-library perf mode; cut peak RSS under 135 MiB |

## UI batch: Direction D ("One playlist")

**Merged 2026-10-27** (`main` at `4f1ea94` plus fixes). `scripts/check.sh`
passes; perf-check: binary 22.0 MiB, peak RSS 136.9 MiB, idle CPU 0%, cold
start 102 ms. Screenshots of Collection (filter, playing) and the queue
drawer match D-Collection, D-Filter and D-Queue. Still open:
- `--demo` has 33 unique tracks, so the spine is about 13 px tall; the
  Batch 4 memory worker adds the 1,535-track fixture.
- The loaded-audio band in the buffering seek bar (D-States) needs a
  buffered position in `Playback`; not built.
- Album and artist pages are verified live only for anonymous reads
  (Daft Punk); artist images are wide headers, center-cropped.
- theme.rs has no 13/400 or 11/400 text style; login, first run and banners
  use private helpers.
- Not checked live with the real account: everything in this batch.

Runs before Batch 4 (decided 2026-10-27). The design is done:
`docs/design/README.md` (brief and rules) and the 12 D boards in
`docs/design/png/`. Every worker reads the README and D-Tokens first, and
compares a screenshot of its screen against its board before it reports
done.

Integrator step (done 2026-10-27):
- `theme.rs`: D tokens behind a `Palette` (`theme::pal()`), so Batch 4 can
  add presets; sizes, spacing, radii and the 5 Inter text styles as
  constants; the accent from the playing art (`theme::accent()`,
  `theme::tint()`), set by `app.rs` each frame.
- Fonts: Inter 400/500/600 (OFL) and Phosphor regular and fill (MIT)
  embedded from `crates/app/assets/fonts/`; glyph constants in `icons.rs`
  (egui-phosphor pins egui 0.36, so it is not a dependency). eframe's and
  egui's bundled fonts are off. Measured: peak RSS 140.2 to 140.3 MiB,
  binary 20.8 to 21.5 MiB.
- `model::Album`, `model::Artist`; `MusicBackend::album` / `artist`
  (`FixtureBackend` real, `RustyPipeBackend` "not implemented").
- `State`: `album`, `artist`, `last_played`; `OpenAlbum` / `OpenArtist`
  commands; play history read at startup and updated per recorded play.
- `views::View` holds the UI-only state; `CollectionView` is the handoff
  between collection, browse, spine and player (filter, chips, range,
  `visible`, `scope`, `on_screen`, `scroll_to`).
- Stub files for every owned path below, seeded with the old views so
  `rustle --demo` keeps working, and the D shell in `app.rs`.
- Demo: `rustle --demo` opens the "Everything" demo playlist;
  `rustle --demo --first-run` shows the main-playlist picker.

| Worker | Owns | Scope | Board |
| --- | --- | --- | --- |
| track-table | `widgets/track_table.rs` | The one table every song list uses: `show_rows`, columns per screen (Last played or In Everything), row states default / hover / selected / playing, like on hover, scroll-to-row, loading skeleton | D-Tokens track rows |
| collection | `views/collection.rs` | Page header with scope-aware Shuffle / Play in order, filter field with removable chips, "Not heard in 3 months", sort menu, local filtering (`update`) | D-Collection, D-Filter |
| spine | `widgets/spine.rs` | 20 px strip, 4 px segment per ~10 songs colored by art, viewport box, playing line, filter ticks, click to jump, drag to select a range with resize handles | D-Range |
| browse | `views/browse.rs`, `views/rail.rs` | Icon rail; Artists / Albums panel with counts, playing marker, click to filter | D-Collection |
| player | `views/player_bar.rs`, `views/queue.rs` | Player bar with 14% accent tint, Locate and "#412", "Playing from" scope chip, seek and volume; queue drawer with autoplay toggle | D-Queue, D-States buffering |
| pages | `views/search.rs`, `views/album.rs`, `views/artist.rs`, `views/library.rs`, `views/playlist.rs` | Search, Album, Artist pages on the shared table with the In Everything column and Add buttons; Library and other playlists restyled | D-Search, D-Album, D-Artist |
| onboarding | `views/login.rs`, `views/first_run.rs`, `views/banners.rs` | Sign in and its error, pick-your-main-playlist, offline and session-expired banners, the guest bar | D-Login, D-LoginError, D-Empty, D-States |
| ytm-pages | `crates/ytm-core/` | `RustyPipeBackend::album` and `artist` with recorded fixtures | none |

The data these screens need (play history, art colors, counts, scope
shuffle, add/remove) comes from Batch 3, so the UI batch runs after it.

## Decisions and user steps

1. **Decided 2026-10-26: keep rustypipe's QuickJS interpreter in-process.**
   AGENTS.md now bans browser engines and webviews, with an explicit
   exception for `rquickjs`. A helper process would have added an IPC
   protocol, a second binary, and more total RAM. No helper worker in Batch 3.
2. **Done 2026-10-26: cookies pasted.** Rustle is logged in through a
   dedicated browser session (keyring service `rustle`, user
   `youtube-music-cookies`) and stayed logged in across restarts.
3. **Your step after Batch 2: real-desktop RSS.** Run
   `scripts/perf-check.sh --no-headless` once on your desktop. The 129 MB
   measured so far ran under Xvfb with software rendering, so it is likely
   higher than on your GPU.
4. **Decided 2026-10-26: Spotify import deferred.** The user's one playlist
   was migrated by scripts in `~/Coding/spotify-ytm-migration/`, so Batch 3
   does not need a Spotify client ID.
5. **Decided 2026-10-26: Direction D UI.** No Home screen; the main playlist
   is the app. See `docs/design/README.md`.
6. **Decided 2026-10-27: UI batch before Batch 4; no Discord presence;
   themes as dark and medium presets only.** A preset swaps the D neutrals;
   the accent still comes from album art.
7. **Decided 2026-10-28: visionOS is the only stream path.** The rustypipe
   web + botguard, yt-dlp and iOS fallbacks were removed in #12 after the
   visionOS client (ed1e37a) took click-to-audio from about 6 s to about
   0.2 s. `docs/design/stream-fallbacks.md` records what they were, their
   timings and how to restore one. The RPM no longer needs `yt-dlp` or the
   botguard helper. Same day, the QA batch (#2 to #9, #11) merged: loading
   clock, system font fallback, toolbar at the minimum size, CLI flags,
   serialised likes, transient selection, single-click play, swipe to
   queue, album re-rank.
