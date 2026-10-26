# Rustle agent guide

Rustle is a native Rust YouTube Music desktop client, as lean as Spotifast.
The build plan is the Claude doc "Rustle: Build Plan"; this file holds the
rules from it that the code must follow.

## Targets

Measure against these at the end of every phase (`scripts/perf-check.sh`).

| Metric | Target |
| --- | --- |
| RAM while playing | under 150 MB |
| Cold start to usable UI | under 1 s |
| CPU while idle (window open, paused) | ~0% |
| CPU while playing | under 2% of one core |
| Release binary size | under 25 MB |
| Browser engine | none |

Platform: Nobara 44 (Fedora-based Linux, Wayland, KDE) only until it is
satisfactory. Keep code portable, but do not test or fix other platforms yet.

## Crates

| Crate | Owns | May depend on |
| --- | --- | --- |
| `model` | Shared domain types (`Track`, `Playlist`, `StreamSource`, ...) | serde |
| `ytm-core` | `MusicBackend` trait, `RustyPipeBackend`, `FixtureBackend` | model, rustypipe |
| `player` | HTTP Range fetch, symphonia demux, Opus/AAC decode, cpal output | model |
| `store` | SQLite cache of library, metadata, queue | model, rusqlite |
| `app` (binary `rustle`) | egui UI, wiring, keychain, media keys | all of the above |

- No rustypipe, rusqlite, symphonia or cpal type crosses a crate boundary.
  Public APIs use `model` types.
- `ytm-core` is the only crate that talks to YouTube. When YouTube breaks
  something, the fix lands there and nowhere else.
- `model` changes go through the integrator. Do not widen `MusicBackend`,
  `Player` or `Store` public APIs without a UI feature that needs the method.

## Resource rules (hard)

- egui repaints on input or state change only. While playing, call
  `request_repaint_after(Duration::from_secs(1))` for the progress bar. Never
  repaint in a continuous loop.
- No network or disk work on the UI thread. The UI sends commands over a
  channel and reads state snapshots.
- Lists use `ScrollArea::show_rows`. A 5,000-track library must not build
  5,000 rows per frame.
- Thumbnails: fetch the smallest size that fits the slot, decode once, keep an
  LRU texture cache capped at about 200 images.
- Prefetch only the next track's stream URL, not its audio.
- Paginate library and playlist fetches. Store results in SQLite, render from
  SQLite.
- Audio-only stream formats always. Never fetch a video stream for a song.
  Preference: Premium Opus, then Opus itag 251, then AAC itag 140.
- tokio runtime: 2 worker threads max.
- `mimalloc` is the global allocator.
- No Electron, Tauri, webview, or browser engine in the main process.
  `rustypipe-botguard` runs as a separate helper binary.
- Exception: rustypipe's QuickJS interpreter (`rquickjs`) is allowed
  in-process. It only runs YouTube's URL-decoding function, has no file or
  network access, and costs about 1 to 2 MB. Do not add any other JS runtime.

## Dependencies

- Prefer crates already in the workspace. A new crate needs a one-line comment
  next to it in `Cargo.toml` saying why.
- Choose low-footprint crates; disable default features you do not use.
- `cargo deny check` must pass.

## Auth and secrets

- Login is a pasted `Cookie:` header or Netscape cookies.txt from
  music.youtube.com. Store it with the `keyring` crate, never in a plaintext
  file.
- Never log cookies, tokens, or authorization responses.
- On auth errors, show one "session expired, paste new cookies" banner. No
  silent retry loops.

## Testing

- Recorded fixtures: save real rustypipe JSON responses once under
  `crates/ytm-core/fixtures/`, and run parser and state tests against them
  offline. No live network in tests or CI.
- `rustle --demo` runs the UI on `FixtureBackend` with no account. Keep it
  working.
- Before you report done: `cargo build`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, and `cargo fmt --check` all pass for the crates you touched.

## Working in parallel

Several agents work on this repo at once, each in its own git worktree and
branch, each owning one directory. Edit only the directory your task names.
If you need a change in another crate or in `model`, stop and report it
instead of making it. Commit your work on your branch; do not merge, rebase
onto, or push other branches.
