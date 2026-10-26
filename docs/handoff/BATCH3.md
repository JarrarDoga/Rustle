# Batch 3 handoff: team lead brief

You are the team lead for Rustle's Batch 3. You do the integrator step
yourself, spawn 5 parallel workers, merge their branches one at a time, and
report to the user. Written 2026-10-26 by the lead of Batches 1 and 2.

## 1. Read these first, in order

1. `~/.claude/CLAUDE.md` and `~/AGENTS.md`: machine rules (sudo through
   swoq askpass, rootless installs, agentdesk rules, short replies).
2. `~/Coding/Rustle/AGENTS.md`: the code rules. Hard requirements.
3. `~/Coding/Rustle/docs/PLAN.md`: batch process, the Batch 3 table, and the
   UI batch that comes after you.
4. `~/Coding/Rustle/docs/design/README.md`: Direction D. Batch 3 builds the
   data this design needs. Look at `docs/design/png/D-Collection.png`,
   `D-Filter.png`, `D-Range.png` and `D-Queue.png` to see what the data
   feeds.
5. This file.

## 2. Goal and scope

Batch 3 makes Rustle a daily driver under the hood: writes to YouTube Music
(like, add to and remove from the main playlist), scope shuffle and autoplay
radio, play history, album-art accent colors, and a library that opens with
no network.

Batch 3 adds **no new screens**. The UI batch after it draws Direction D on
top of this work. Only change existing views when a test or a check needs a
hook; keep those changes minimal, because the UI batch replaces those views.

Done means:

- All 5 branches merged into `main`, and `scripts/check.sh` exits 0 on the
  result.
- `scripts/perf-check.sh` passes: binary under 25 MB, RSS under 150 MB, idle
  CPU about 0%, cold start under 1 s.
- Live checks in section 7 pass against the user's account.
- `docs/PLAN.md` marks Batch 3 merged, and the user's build-plan doc is
  updated (section 9).

## 3. Current state (main at the commit that added this file)

- Workspace crates: `model`, `ytm-core`, `player`, `store`, `app` (binary
  `rustle`). `scripts/check.sh` passed on `fef740d`.
- Working today: cookie login stored in the keyring, library, playlists,
  liked songs, search, real playback (Opus via yt-dlp fallback, AAC 140
  last), thumbnails with an LRU, MPRIS media keys, queue with shuffle and
  repeat, SQLite cache at `~/.config/rustle/library.db`.
- Last perf-check (Batch 2, Xvfb): binary 20.4 MiB of 25, peak RSS
  142.7 MiB of 150, idle CPU 0%, cold start 76 ms. RSS headroom is about
  7 MiB. Keep new caches small; measure after every merge that touches the
  app.
- Known follow-ups from Batch 2, all assigned below: `clear_cookies` for
  logout; guests see "Loading…" instead of "needs login" for liked songs;
  the Buffering spinner flickers on seek; ytm-core picks thumbnails larger
  than needed; the thumbnail disk cache has no eviction.
- No worktrees or branches remain from earlier batches. `main` is the only
  branch.

### Facts about the code you will need

- `MusicBackend` (`crates/ytm-core/src/lib.rs`) uses RPITIT:
  `fn x(&self, ..) -> impl Future<Output = Result<T>> + Send`. Errors are
  `BackendError { Auth, NotFound, Network, Extraction }`.
- rustypipe 0.11.4 source:
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/rustypipe-0.11.4`.
  It has `music_radio`, `music_radio_track` and `music_radio_playlist`
  (`src/client/music_details.rs`), but **no** like, add-to-playlist or
  remove-from-playlist calls. It has `RustyPipeQuery::raw(ctype, endpoint,
  body)` (`src/client/mod.rs:2609`), which sends an Innertube request with
  rustypipe's context. Check that an authenticated query sends the cookie
  and SAPISIDHASH headers before building on it. Reference implementation
  for the endpoints and bodies: ytmusicapi (`like/like`,
  `like/removelike`, `browse/edit_playlist` with `ACTION_ADD_VIDEO` and
  `ACTION_REMOVE_VIDEO`). It is installed in the migration tool's uv env
  (`uv run --with ytmusicapi python`).
- Removing a song from a YouTube Music playlist needs that entry's
  `setVideoId`, which `model::Playlist` does not carry yet. Integrator step
  item 1 adds it.
- Playlist fetches follow pagination up to `MAX_ITEMS = 5_000`
  (`rustypipe_backend.rs:21`). The user's main playlist has about 1,535
  songs.
- Stream resolution: rustypipe's deobfuscation fails on current YouTube, so
  `ytm-core/src/ytdlp.rs` runs `yt-dlp -f 774/251` and falls back to iOS AAC
  140. `rustypipe-botguard` (at `~/.local/bin`) supplies PO tokens.
- The app: UI thread sends `Command`s; `backend.rs` runs on a 2-worker tokio
  runtime and turns `Effect`s into `Event`s; `state.rs` holds the reducer
  (`Command`/`Effect`/`Event` enums, `State`, `Session`). The queue model is
  `crates/app/src/queue.rs`.
- `Store` (`crates/store/src/lib.rs`) is rusqlite, WAL, migrations through
  `PRAGMA user_version`. It lives on the backend task, not the UI thread.

## 4. Live account rules (read before any live call)

- The user's YouTube Music account is in use for a playlist migration. A
  background job is liking ~1,511 songs in a fixed order so "Liked music"
  matches their Spotify order:
  `~/Coding/spotify-ytm-migration/` (`run_likes.sh` restarts `like.py`
  after YouTube throttling). Check progress with:
  `python3 -c "import json;print(len(json.load(open('$HOME/Coding/spotify-ytm-migration/like_progress.json'))['done']))"`
  and `tail -n 3 ~/Coding/spotify-ytm-migration/like.log`.
- **No live like or unlike calls until that job prints `done:` in
  `like.log`.** A like during the run inserts a song into the middle of the
  user's ordered Liked list. YouTube is also throttling the account; extra
  traffic slows the job.
- After the job finishes, live write tests follow these rules:
  - Like and unlike only a song that is **not** already liked. Never
    unlike and re-like an already-liked song: that moves it to the top and
    breaks the order.
  - Add and remove only on a scratch playlist the test creates (name it
    `Rustle test`), never on `LikedSongsMigration` or any other user
    playlist. Delete the scratch playlist after the test.
- Live reads are fine at a low rate. Tests use recorded fixtures only
  (`crates/ytm-core/fixtures/`); no network in `cargo test`.
- Never print, log or commit cookie values. Cookies live in the keyring
  (service `rustle`, user `youtube-music-cookies`) for the app. For the
  ytm-core `spike` example, `RUSTLE_COOKIES_FILE` takes a Netscape
  cookies.txt; `~/Coding/spotify-ytm-migration/refresh_cookies.sh` writes
  one from the user's Opera to `$XDG_RUNTIME_DIR/rustle/ytm-cookies.txt`
  (mode 0600). Those Opera cookies rotate after about 20 minutes, so
  refresh before each live session.
- If the account shows `Session expired`, stop and tell the user. Do not
  loop retries (AGENTS.md).

## 5. Integrator step (you, serial, before spawning)

Commit this to `main` as one commit, with `scripts/check.sh` passing. Every
new method gets a `FixtureBackend` or stub implementation, so each worker's
branch compiles on its own.

1. `model`: carry each playlist entry's `setVideoId`. Recommended: add
   `#[serde(default)] pub entry_ids: Vec<Option<String>>` to `Playlist`,
   parallel to `tracks` (same length when present). It breaks the least
   code. Changing `tracks` to a `Vec<PlaylistEntry>` is cleaner but touches
   every view.
2. `MusicBackend`: add `like(track_id)`, `unlike(track_id)`,
   `add_to_playlist(playlist_id, &[track_id]) -> Vec<entry id>`,
   `remove_from_playlist(playlist_id, &[(track_id, entry_id)])`,
   `radio(track_id) -> Vec<Track>`, and `clear_cookies()`. Implement them
   in `FixtureBackend` (in-memory, deterministic). Leave
   `RustyPipeBackend` versions as `Err(BackendError::Extraction("not
   implemented"))`; the ytm-write worker fills them in.
3. `Store` public API stubs (bodies `todo!()` are fine only if no existing
   test calls them; prefer empty returns):
   `set_main_playlist(id)`, `main_playlist()`,
   `record_play(track_id, at, played_ms)`, `last_played(&[ids]) -> map`,
   `not_heard_since(cutoff) -> Vec<TrackId>`,
   `set_art_color(track_id, [u8; 3])`, `art_colors(&[ids]) -> map`,
   `artist_counts(playlist_id)`, `album_counts(playlist_id)`,
   `fetched_at(key)` / `set_fetched_at(key)` for staleness.
4. `state.rs`: add `Scope` (`All`, `Filter { label, ids }`, `Range { from,
   to }`) and the commands the UI batch will send: `Like(id)`, `Unlike(id)`,
   `AddToMain(ids)`, `RemoveFromMain(ids)`, `SetMainPlaylist(id)`,
   `PlayScope { scope, shuffle }`, `SetAutoplay(bool)`. Add matching
   `Effect` and `Event` variants. Leave their reducer arms as no-ops with a
   `// b3-wiring` marker.
5. `queue.rs`: add `Queue::play_scope(tracks, label, shuffle, seed)`,
   `playing_from() -> Option<&str>`, `set_autoplay(bool)`,
   `needs_radio() -> bool` and `extend_radio(tracks)` as stubs.
6. Create the empty module `crates/app/src/art_color.rs` and register it in
   `main.rs`.
7. Run `scripts/check.sh > /tmp/b3-int.log 2>&1; echo $?`. Commit only on
   exit 0. Message ends with
   `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## 6. Workers

### Spawning

One git worktree and branch per worker, created from the integrator commit:

```
git -C ~/Coding/Rustle worktree add ~/Coding/Rustle-wt/b3-<name> -b b3-<name> main
```

Spawn each with the T3 `delegate_task` tool, all 5 in one turn:

```json
{
  "title": "Rustle B3: <name>",
  "clientRequestId": "rustle-b3-<name>",
  "role": "implementation",
  "mode": "async",
  "target": {
    "providerInstanceId": "claudeAgent",
    "model": "claude-opus-5-5",
    "options": { "effort": "high", "fastMode": false }
  },
  "task": "<brief: template below plus the worker's section>"
}
```

- Model and effort: Claude Opus 5.5, high effort, fast mode off, for every
  worker. The user chose this; do not change it without asking.
- Keep each returned `taskId`. Check progress with `task_status`. To steer a
  running worker (new fact, scope change), use `t3_thread_send` on its child
  thread.
- The user prefers 5 or more parallel workers and is not limited by usage.

### Brief template (copy into every task, then add the worker section)

```
You are one of 5 parallel workers on Rustle Batch 3, a native Rust YouTube
Music desktop client (egui + rustypipe) on Nobara 44 Linux.

Where you work
- Worktree: /home/caprisonne/Coding/Rustle-wt/b3-<name> (branch b3-<name>,
  based on main at <integrator sha>). Work only there; use absolute paths.
- You may edit only: <owned paths>. Adding deps to your crate's Cargo.toml
  is fine (one-line comment saying why, per AGENTS.md); Cargo.lock will
  change.
- Read AGENTS.md, docs/PLAN.md (Batch 3), docs/design/README.md and
  docs/handoff/BATCH3.md section 4 (live account rules) first. They are
  hard requirements.
- Do not change model, MusicBackend, Store or Command/Effect/Event
  signatures. If you need a change outside your paths, stop and describe it
  in your final report.
- Commit on your branch (message ends with
  "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"). Do not merge,
  rebase, push, or touch other branches or worktrees. Four other workers
  build at the same time; do not pass -j.
- Never print or log cookie values. No live network in tests.
- When you run scripts/check.sh, redirect to a file and read the exit code:
  `scripts/check.sh > /tmp/b3-<name>.log 2>&1; echo $?`. Do not pipe it
  into tail (that hides the exit code).

Done means
cargo build, cargo clippy --all-targets -- -D warnings, cargo test and
cargo fmt --check pass for the crates you touched, and the work is
committed. Final report (short): what works, test results, what was
verified live and what was not, commit hash, any change you need outside
your paths.
```

### ytm-write — `crates/ytm-core/`

- Implement `like`, `unlike`, `add_to_playlist`, `remove_from_playlist`,
  `clear_cookies` and `radio` in `RustyPipeBackend`. Radio uses
  `music_radio_track`. Writes go through `RustyPipeQuery::raw` with the
  authenticated client if it sends auth headers; otherwise a minimal
  Innertube POST in a new `innertube.rs` reusing rustypipe's HTTP stack and
  cookie jar. Read ytmusicapi for bodies and SAPISIDHASH.
- Fill `Playlist::entry_ids` from the playlist response (`setVideoId`).
- Map responses: a missing or failed `status` is `Extraction`; 401/403 or a
  logged-out response is `Auth`; rate limiting (empty body, 429) is
  `Network` so the app does not show "session expired".
- Thumbnails: emit the smallest URLs that fit the 24 / 32 / 40 / 112 px
  slots (D-Tokens sizes), for example `=w60-h60` variants, in addition to
  what exists.
- Record fixtures for every new response shape. Extend the `spike` example
  with `like`, `unlike`, `radio`, `add`, `remove` subcommands.
- Live verification only after the migration job is done (section 4): like
  and unlike a song that is not liked; create `Rustle test`, add 2 songs,
  remove 1, delete the playlist; radio for one track returns 20+ tracks.
  Until then, report writes as "not verified live".

### queue — `crates/app/src/queue.rs`

- `play_scope`: build the queue from the scope's tracks, shuffled (seeded,
  testable) or in order, and remember the scope label for "Playing from"
  (D-Filter shows `Saint Juniper · 41`, D-Range shows `#380 to #520 · 141`,
  whole playlist shows `Everything · 1,535`).
- Autoplay: when the queue runs out and autoplay is on, `needs_radio()` is
  true once; `extend_radio(tracks)` appends radio tracks after the current
  entry, marked as "not in Everything" (D-Queue shows them in a separate
  "Autoplay radio" section).
- Keep existing behavior and tests: shuffle, repeat, play next, move,
  remove, back.
- Tests: scope shuffle covers every track once, order mode keeps playlist
  order, label text, autoplay fires exactly once per run-out, repeat-all
  disables autoplay.

### store — `crates/store/`

- New migration (next `user_version`) for: settings (main playlist id),
  `plays` (track_id, played_at, played_ms), `art_colors` (track_id, r, g,
  b), `fetched_at` (key, unix time). Indexes for last-played lookups.
- `record_play` counts a play when the app calls it; the wiring worker
  decides the threshold. `last_played` returns the latest `played_at` per
  id. `not_heard_since(cutoff)` returns main-playlist tracks never played
  or last played before the cutoff (D-Collection's "Not heard in 3 months
  412").
- `artist_counts` and `album_counts` for a playlist, sorted by count
  descending then name (D-Collection browse panel: `Saint Juniper 41`).
  Count a featured artist under each listed artist.
- Staleness: `fetched_at` per key ("library", "playlist:<id>", "liked").
  Document the refresh rule in a comment: show cache at once, refresh in
  the background if older than 10 minutes.
- Performance: every query on a 5,000-track playlist with 50,000 plays
  under 20 ms in release; add a timing test like the existing 5,000-track
  one.

### art — `crates/app/src/thumbs.rs`, `crates/app/src/art_color.rs`

- `art_color.rs`: dominant color of a decoded thumbnail (small histogram or
  k-means on a 16×16 downsample; ignore near-black and near-white unless
  the art is only that), then the accent: convert to OKLCH, set L to 0.80,
  clamp chroma to 0.09–0.11, keep hue, convert back to sRGB. Also the 14%
  player tint over `bg #111214`. Values to check against the D-Tokens
  board: art in that board maps to `#71CFDA`, `#8DC4F4`, `#FA9FB4`,
  `#E0B767`; tints `#1E2C30`, `#222B33`, `#32262A`, `#2E2920`.
- Compute once per track, off the UI thread, when the thumbnail is first
  decoded; hand the result to the backend to store (`set_art_color`). The
  spine needs a color for every playlist track, so add a low-priority
  background pass that fetches the smallest thumbnail for tracks without a
  color, at most 4 at a time, without keeping the images.
- Thumbnail disk cache: cap it (about 100 MB), evict least recently used.
- RSS: the color pass must not grow peak RSS by more than 3 MB. Measure with
  `scripts/perf-check.sh`.
- Tests: the OKLCH round trip, the 4 D-Tokens examples within ±2 per
  channel, eviction order.

### wiring — `crates/app/src/backend.rs`, `crates/app/src/state.rs`

- Reducer arms and effects for the new commands. Like, unlike, add and
  remove are optimistic: update `State` at once, send the effect, roll back
  and show one banner on failure.
- Record a play when a track has played 30 s or half its length, whichever
  comes first; one record per play.
- Offline start: render library, main playlist and liked songs from the
  store before any network result; then refresh per the store's staleness
  rule. No network must not show "session expired".
- Guest (logged out): liked songs and library show "Sign in to see your
  library" state, not "Loading…".
- Logout: call `clear_cookies`, delete the keyring entry (through
  `auth.rs` functions; if a change in `auth.rs` is needed, report it), clear
  cached user data from `State`.
- Buffering: show `Buffering` only after 250 ms of continuous buffering, so
  seeks within buffered audio do not flicker.
- Autoplay: when `Queue::needs_radio()` is true, fetch `radio` for the last
  track and call `extend_radio`.
- Main playlist: `SetMainPlaylist` stores the id; on start, open it.
- Tests: reducer tests on `FixtureBackend` for each command, including
  rollback on a failed like and offline start from a pre-filled store.
  Offline check by hand: run with `HTTPS_PROXY=http://127.0.0.1:9` (reqwest
  reads proxy variables unless the client disables them; confirm requests
  fail) and confirm the library still renders.

## 7. Merge and verify (you, serial)

Merge in this order, each as soon as its worker finishes: store, ytm-write,
queue, art, wiring (wiring consumes the others).

For each branch:

1. `git -C ~/Coding/Rustle merge --no-ff b3-<name>`
2. If `Cargo.lock` conflicts: `git checkout --ours Cargo.lock && cargo
   metadata -q --format-version 1 >/dev/null`, then `git add Cargo.lock`.
   If a crate's `Cargo.toml` dependency list conflicts, keep both sides.
3. `scripts/check.sh > /tmp/b3-merge.log 2>&1; echo $?`. Commit the merge
   only on exit 0. Never pipe check.sh into `tail`: an earlier merge went in
   red that way.
4. After the art and wiring merges: `scripts/perf-check.sh` (unsets
   `WAYLAND_DISPLAY` itself).
5. Remove the worktree and branch after the merge.

After all merges, live checks (after the migration job is done):

- `rustle` logged in: like and unlike a non-liked song; add to and remove
  from the `Rustle test` scratch playlist; play a scope with shuffle and
  confirm the label; let the queue run out with autoplay on and confirm
  radio tracks append.
- Offline start with the proxy trick shows the cached library.
- Build release: `cargo build --release` (`~/.local/bin/rustle` is a symlink
  to `target/release/rustle`).

## 8. Running and screenshotting the app

The user is usually remote. Show results as screenshots, not as windows on
their desktop. Never start apps on the user's real desktop and never send
input to it.

```
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET \
  xvfb-run -a -s '-screen 0 1280x800x24' \
  bash -c 'target/release/rustle & sleep 8; import -window root /tmp/rustle-shot.png; kill $!'
```

Use `rustle --demo` for fixture data with no account. Embed the PNG in
your reply with its absolute path.

## 9. Reporting to the user

- Style: short, first line is the result or the next action, numbered
  steps, concrete times, no recap (the user's `AGENTS.md` and ADHD rules).
  Use they/them for the user.
- Restate where the batch is every time: "integrator done, 3 of 5 workers
  merged, next: art".
- Keep the user's build-plan doc current:
  https://claude.ai/artifact/SkUkjyTdA6GUfCZkESZvnj (a Claude doc; load
  the docs guide before editing, then update the Batch 3 rows as workers
  start and merge).
- When Batch 3 is merged, tell the user the UI batch (`docs/PLAN.md`, 7
  workers) is next and wait for their go.
- If the migration job finishes during your batch: run
  `uv run -q --with ytmusicapi python verify.py` in
  `~/Coding/spotify-ytm-migration/`, report the result, and point the user
  to `report.md` (26 low-confidence and 2 unmatched tracks to check by
  hand). Two songs, "IG" and "2020", were liked before the migration and
  sit outside the Spotify order; offer to unlike and re-like them at the
  right moment if the user wants them in place.

## 10. Known traps

- `pkill -f <pattern>` matches your own bash command line when the pattern
  appears in it (exit 144). Use `pgrep` with a pattern that is not in the
  command, or kill by PID.
- yt-dlp tests share a fake binary; `FAKE_LOCK` in `ytdlp.rs` serializes
  them (ETXTBSY otherwise). Keep new tests that use it under the lock.
- Headless runs need `WAYLAND_DISPLAY` unset, or eframe tries Wayland.
- tokio runtime must keep `enable_all()` (rustypipe needs IO and timers)
  and 2 workers.
- `deny.toml` denies git sources: no rustypipe git forks.
- `StreamSource` has an `f32` field, so it cannot derive `Eq`.
- `perf-check.sh` waits 2 s before sampling idle CPU; do not shorten it.
- YouTube throttling shows as empty JSON bodies (`JSONDecodeError` in
  Python, a serde error in Rust). Treat it as `Network` with backoff, never
  as `Auth`.
