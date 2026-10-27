# Batch 4 handoff: team lead brief

You are the team lead for Rustle's Batch 4 (polish). You confirm two
decisions with the user, do the integrator step yourself, spawn 5 parallel
workers, merge them, then run one serial memory worker, and report to the
user. Written 2026-10-26 by the lead of Batch 3.

**Batch 4 runs after the UI batch (Direction D).** When this file was
written, the UI batch had not run. Paths below that the UI batch creates
(`views/player_bar.rs`, `views/rail.rs`, `views/queue.rs`, `theme.rs`
tokens, the shell layout in `app.rs`) are taken from `docs/PLAN.md`. Before
the integrator step, read `main` and correct any path or name that changed.

## 1. Read these first, in order

1. `~/.claude/CLAUDE.md` and `~/AGENTS.md`: machine rules (sudo through
   swoq askpass, rootless installs, agentdesk rules, short replies).
2. `~/Coding/Rustle/AGENTS.md`: the code rules. Hard requirements.
3. `~/Coding/Rustle/docs/PLAN.md`: batch process, the Batch 3 "Still open"
   list, the UI batch result, and the Batch 4 table.
4. `~/Coding/Rustle/docs/design/README.md` and `docs/design/png/D-Tokens.png`:
   every new piece of UI in this batch uses D tokens only.
5. `~/Coding/Rustle/docs/handoff/BATCH3.md`: sections 4 (live account
   rules), 8 (running and screenshotting) and 10 (known traps) still apply.
   This file does not repeat them.
6. This file.

## 2. Goal and scope

Batch 4 finishes Phase 3: synced lyrics, keyboard and command-line control,
saved settings, Discord presence, an RPM for Nobara, and a memory pass that
restores RSS headroom. It also closes the Batch 3 persistence gaps.

Not in scope: the Batch 3 live checks (like, playlist edits, radio,
re-recording the hand-built fixtures). The user decided on 2026-10-26 to do
those in one debugging session after all batches merge. Do not run them
here unless the user asks.

Done means:

- All 5 parallel branches and the memory branch merged into `main`, and
  `scripts/check.sh` exits 0 on the result.
- `scripts/perf-check.sh` passes, including the new large-library mode the
  memory worker adds (section 6, memory).
- The checks in section 8 pass.
- `docs/PLAN.md` marks Batch 4 merged, and the user's build-plan doc is
  updated (section 10).

## 3. Decisions to confirm with the user before the integrator step

Ask both in one `AskUserQuestion` call. Recommended answers first.

1. **Themes.** `docs/PLAN.md` lists "Theme choice" for Batch 4, but
   Direction D has one dark palette and takes its only color from album
   art. Options:
   - (Recommended) Drop theme choice. The `settings` worker builds a small
     settings popover instead (autoplay, art accent on or off, Discord
     presence on or off).
   - Add a light palette. That needs a light D-Tokens board first, which
     does not exist.
2. **Discord presence.** It needs a Discord application ID, which the user
   creates at <https://discord.com/developers/applications> (name it
   "Rustle"; the ID is public, not a secret). Options:
   - (Recommended) The user creates it now and pastes the ID; the
     `presence` worker compiles it in as a constant.
   - Skip presence; the `presence` worker is not spawned.

If the user picks "skip presence", run 4 parallel workers.

## 4. Current state

Batch 3 merged on 2026-10-26 (`main` at `29b7767` before the UI batch):

- Real like, unlike, add and remove, radio and logout in `ytm-core`
  (`innertube.rs` holds the raw Innertube calls). Not verified live.
- Store migration 2: `settings` (key, value), `plays`, `art_colors`,
  `fetched_at`, `playlist_tracks.entry_id`. `user_version` is 2.
- Queue: scope play, "Playing from" label, autoplay radio (`Entry::radio`).
  Autoplay is turned on at startup by `state.rs`.
- Perf after Batch 3 (Xvfb, `--demo`): binary 20.8 MiB of 25, peak RSS
  140.6 MiB of 150, idle CPU 0%, cold start 101 ms. The UI batch adds the
  Inter font and Phosphor icons, so expect RSS to be closer to the limit.
  Read the UI batch's last `perf-results/*.json` before you start.

Batch 3 gaps this batch closes (from `docs/PLAN.md`):

- The autoplay setting is not saved across restarts (`settings`).
- The saved queue loses the "Playing from" label and radio marks on restart
  (`settings`, through the integrator's store stubs).
- The color pass runs 1 fetch worker to fit the 3 MB budget (`memory` may
  raise it if it frees RSS).
- Guests seeing "Loading…" instead of "Sign in to see your library" belongs
  to the UI batch's onboarding worker. Check it landed; if not, add it to
  the `settings` brief.

### Facts about the code you will need

- `MusicBackend` (`crates/ytm-core/src/lib.rs`) uses RPITIT with
  `BackendError { Auth, NotFound, Network, Extraction }`. Throttling (empty
  body, 429) is `Network`, never `Auth`.
- Lyrics in rustypipe 0.11.4: `music_details(video_id)` returns
  `lyrics_id`; `music_lyrics(lyrics_id)` returns plain text only
  (`Lyrics { body, footer }`, no timing).
- Timed lyrics: ytmusicapi `get_lyrics(browseId, timestamps=True)` sends the
  same `browse` request as the `ANDROID_MUSIC` client (clientVersion
  `7.21.50`) and reads
  `contents.elementRenderer.newElement.type.componentType.model.timedLyricsModel.lyricsData.timedLyricsData[]`;
  each line has `lyricLine` and `cueRange.startTimeMilliseconds` /
  `endTimeMilliseconds`, and `sourceMessage` names the source. rustypipe's
  `ClientType` has no Android Music variant, so `RustyPipeQuery::raw` cannot
  send it; the lyrics worker needs its own minimal POST in `innertube.rs`.
  ytmusicapi source: `~/.cache/uv/archive-v0/*/ytmusicapi/mixins/browsing.py`
  and `models/lyrics.py`.
- Helper binaries are found by `find_bin` (`rustypipe_backend.rs`): `PATH`
  first, then `~/.local/bin`. `rustypipe-botguard` lives in `~/.local/bin`
  today; `yt-dlp` is on `PATH`.
- The app: the UI thread sends `Command`s; `backend.rs` runs on a 2-worker
  tokio runtime and turns `Effect`s into `Event`s; `state.rs` holds the
  reducer, which runs on both the UI thread and the backend, so it must stay
  deterministic. `main.rs` builds the runtime and starts eframe with app id
  `rustle`.
- MPRIS (`media_keys.rs`, souvlaki over libdbus) already handles media keys
  and KDE's media applet. Global hotkeys on Wayland need a portal; this
  batch does not add them. KDE custom shortcuts can call `rustle next`
  once the control socket exists.
- Crates: `discord-rich-presence` 1.1.0 (Discord IPC over the
  `$XDG_RUNTIME_DIR/discord-ipc-0` socket; a Flatpak Discord uses
  `$XDG_RUNTIME_DIR/app/com.discordapp.Discord/discord-ipc-0`).
  `cargo-generate-rpm` 0.21.1 builds an RPM from `[package.metadata.generate-rpm]`.

## 5. Integrator step (you, serial, before spawning)

No worker owns `state.rs`, `backend.rs`, `app.rs` or `main.rs` in this
batch. You write all of their changes here, as real plumbing that calls
stub functions in the workers' modules. Commit to `main` as one commit with
`scripts/check.sh` passing. Every new trait method gets a `FixtureBackend`
implementation so `rustle --demo` keeps working.

1. `model`: add
   `pub struct Lyrics { pub lines: Vec<LyricLine>, pub synced: bool, pub source: Option<String> }`
   and `pub struct LyricLine { pub start: Option<Duration>, pub text: String }`.
2. `MusicBackend`: add `lyrics(track_id) -> Result<Option<Lyrics>>`
   (`Ok(None)` when the song has no lyrics). `FixtureBackend` returns a
   synced fixture for one demo track and `None` for the rest.
   `RustyPipeBackend` returns `Extraction("not implemented")`.
3. `Store` stubs (`// b4-settings`): `setting(key) -> Option<String>`,
   `set_setting(key, value)`, and queue extras
   `save_queue_extras(label: Option<&str>, radio: &[bool])` /
   `queue_extras() -> (Option<String>, Vec<bool>)`. Prefer empty returns.
4. `queue.rs` stub: `Queue::restore_extras(label, radio_flags)`.
5. `state.rs`:
   - `Settings { autoplay: bool, accent_from_art: bool, discord_presence: bool }`
     with defaults `true, true, false`, held in `State`.
   - Commands: `ToggleLyrics`, `SetSetting(SettingChange)` (one enum
     variant per field).
   - Effects: `FetchLyrics(TrackId)`, `SaveSettings(Settings)`.
   - Events: `Lyrics { track_id, result }`, `CachedSettings(Settings)`.
   - `State.lyrics: Option<(TrackId, Loadable<Arc<Lyrics>>)>`, fetched when
     the panel is open and the current track changes.
   - Restore queue extras in the `CachedQueue` arm with
     `restore_extras`.
6. `backend.rs`: execute the new effects: `lyrics()` on the backend;
   settings through `settings::load` / `settings::save` (stubs in
   `settings.rs`); call `presence::update(now_playing)` on track change and
   pause/resume, never on a timer; start `control::serve(command_sender)`
   at startup; save queue extras next to the existing debounced queue save.
7. New stub files, each registered in `main.rs` or `views/mod.rs`:
   `crates/app/src/control.rs` (`serve`, `shortcuts(ctx) -> Vec<Command>`,
   `run_cli(args) -> Option<ExitCode>`), `crates/app/src/presence.rs`
   (`update`), `crates/app/src/settings.rs` (`load`, `save`),
   `crates/app/src/views/lyrics.rs` (`show`), `crates/app/src/views/settings.rs`
   (`show`).
8. `main.rs`: before building the runtime, `if let Some(code) =
   control::run_cli(&args) { return code }`, so `rustle next` never opens a
   window.
9. `app.rs`: each frame, send the commands from `control::shortcuts(ctx)`;
   add the lyrics drawer slot (opened by `ToggleLyrics`, mutually exclusive
   with the queue drawer) and a settings button at the bottom of the rail
   that opens `views::settings::show`.
10. Run `scripts/check.sh > /tmp/b4-int.log 2>&1; echo $?`. Commit only on
    exit 0. Message ends with
    `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

Write the worker briefs from the committed code, not from this section:
where your integrator commit differs from the signatures above, the commit
wins.

## 6. Workers

### Spawning

Same as Batch 3 (`BATCH3.md` section 6): one worktree per worker from the
integrator commit,

```
git -C ~/Coding/Rustle worktree add ~/Coding/Rustle-wt/b4-<name> -b b4-<name> main
```

and one T3 `delegate_task` call per worker, all parallel workers in one
turn: `providerInstanceId` `claudeAgent`, model `claude-opus-5-5`, options
`{"effort": "high", "fastMode": false}`, `mode` `async`,
`clientRequestId` `rustle-b4-<name>`. Confirm the model and option IDs with
`orchestrator_capabilities` first. Use the Batch 3 brief template
(`BATCH3.md` section 6) with `b4-` names, then add the worker section below.
Steer running workers with `t3_thread_send` on their child thread; merging
`main` into a worker branch is allowed when you tell them to.

### lyrics — `crates/ytm-core/` (lyrics only), `crates/app/src/views/lyrics.rs`

- `RustyPipeBackend::lyrics`: try timed lyrics first (the `ANDROID_MUSIC`
  browse in section 4, a new function in `innertube.rs`), then fall back
  to rustypipe's plain `music_lyrics`. `synced` is true only when every
  line has a start time. Return `Ok(None)` when the song has no
  `lyrics_id`. Map errors as in `innertube.rs`.
- Record fixtures with low-rate live reads (reads are allowed; refresh
  cookies first): one synced song, one plain-only song, one with no lyrics.
  Extend `spike` with `lyrics <id>`.
- `views/lyrics.rs`: a right-side drawer with the queue drawer's size,
  border and shadow (D-Queue). The current line uses text-1 at weight 600;
  others use text-3; the source line sits at the bottom in 11 px text-3.
  It scrolls to keep the current line in view with no animation. Clicking
  a synced line seeks to it. Plain lyrics show as static text.
- Repaint: while playing synced lyrics, call `request_repaint_after` with
  the time until the next line starts, never a fixed short interval. No
  repaint while paused.
- Tests: the timed parser, the plain fallback, the no-lyrics case, and
  current-line lookup at boundaries (binary search on start times).
- Owns only lyrics code in `ytm-core`; do not touch the write calls.

### control — `crates/app/src/control.rs`

- In-window shortcuts from `shortcuts(ctx)`: Space play/pause, Ctrl+Right
  and Ctrl+Left next and previous, Ctrl+Up and Ctrl+Down volume ±5%, Ctrl+F
  focus the filter field (expose a flag the UI reads), Ctrl+L locate the
  playing song, Ctrl+Q quit. Ignore keys while a text field has focus.
  Check each against the UI batch's existing shortcuts; report conflicts.
- Control socket: `$XDG_RUNTIME_DIR/rustle/control.sock`, mode 0600, a
  line protocol (`play`, `pause`, `toggle`, `next`, `previous`,
  `status` → one JSON line with title, artist, position, playing). `serve`
  runs on the tokio runtime and forwards commands to the backend's input
  channel. Remove a stale socket on start.
- CLI: `rustle play|pause|toggle|next|previous|status` connects, sends one
  line, prints the reply, and exits 0, or exits 1 with "Rustle is not
  running". It runs before eframe and tokio start. A plain `rustle` while
  one is already running sends `raise` and exits; the running app asks for
  focus with `ViewportCommand::Focus` (best effort on Wayland; report what
  KWin does).
- Tests: protocol parsing, the stale socket, CLI exit codes against a fake
  server in a temp dir.

### settings — `crates/store/`, `crates/app/src/settings.rs`, `crates/app/src/views/settings.rs`, `crates/app/src/queue.rs`

- `settings.rs`: load and save `Settings` through `Store::setting` /
  `set_setting` (one key per field, plain text values).
- Store: implement the integrator's stubs. Queue extras need a migration 3
  (`queue.radio` column, label in `settings` or a `queue_state` column).
  Existing databases at version 2 must upgrade cleanly; add the test.
- `Queue::restore_extras`: set the label and radio flags after a restore;
  ignore flags whose length does not match.
- `views/settings.rs`: a popover from the rail's settings button with three
  toggles (Autoplay, Accent from album art, Discord presence), D tokens
  only. With "Accent from album art" off, the accent falls back to text-1,
  as with nothing playing.
- Tests: settings round trip, defaults on a fresh database, queue extras
  round trip, the v2 to v3 migration.

### presence — `crates/app/src/presence.rs` (skip if the user chose to)

- Discord rich presence through `discord-rich-presence`: song title,
  artist, album art URL (the 226 px or nearest thumbnail), and a start
  timestamp so Discord shows elapsed time. Clear it on pause and stop.
- Runs on its own thread with a channel; `update` never blocks the caller.
  Connect only when the setting is on. If Discord is not running, retry at
  most every 60 s, and never log an error per retry. Try both socket paths
  in section 4.
- No polling: updates come only from `update` calls.
- RSS: the connected client must add under 1 MB. Measure it.
- Tests: the message it builds for a track, pause clears, the retry
  schedule with a fake clock.

### packaging — `packaging/`, `[package.metadata.generate-rpm]` in `crates/app/Cargo.toml`

- An RPM for Nobara built with `cargo generate-rpm` (install it with
  `cargo install cargo-generate-rpm`), plus `packaging/build-rpm.sh` that
  runs the release build and writes the RPM to `target/generate-rpm/`.
- Contents: `/usr/bin/rustle`; `/usr/share/applications/rustle.desktop`
  (`StartupWMClass=rustle`, matching the eframe app id);
  an icon in `/usr/share/icons/hicolor/scalable/apps/rustle.svg` (draw a
  simple one from D tokens); `rustypipe-botguard` at
  `/usr/libexec/rustle/rustypipe-botguard`.
- `find_bin` must also look in `/usr/libexec/rustle`. That is a one-line
  change in `ytm-core`; report it so you (the lead) add it during the
  merge, or allow the worker to make exactly that change.
- `Requires: yt-dlp`. Check the Nobara repos for the package name with
  `dnf info yt-dlp` inside the `agentbox` distrobox.
- Verify with `rpm -qlp` and `rpm -qp --requires`. Do not install it on
  the host; installing needs the user's OK (section 8).

### memory — any crate, serial, after the other workers merge

Spawn this worker only after the parallel branches are merged, so it can
edit any crate without conflicts.

- Add a large-library mode to `scripts/perf-check.sh`: a demo fixture with
  1,535 tracks and full thumbnails (generated, no network), opened as the
  main playlist, scrolled through once, with the art color pass running.
  `--demo` with 40 tracks does not show what the real app costs.
- Profile peak RSS under Xvfb with heaptrack (install it in the `agentbox`
  distrobox and export it). Attribute the RSS: fonts, textures, egui, the
  tokio and rustypipe heap, mimalloc retention, thread stacks.
- Cut it. Target: large-library peak RSS under 135 MiB, so the app keeps
  15 MiB of headroom. Report every change with its measured saving. If the
  color pass can then run 2 fetch workers inside its 3 MB budget, raise it.
- Do not run anything on the user's real desktop. The real-GPU number
  stays a user step (`scripts/perf-check.sh --no-headless`).

## 7. Merge order

Merge each parallel branch as soon as it finishes; they own separate files.
`settings` touches `store` and `queue.rs`; merge it before `memory` starts.
For each merge, follow `BATCH3.md` section 7 steps 1 to 3 and 5
(`--no-ff`, `Cargo.lock` from `--ours` plus `cargo metadata`,
`scripts/check.sh > /tmp/b4-merge.log 2>&1; echo $?`, commit only on exit
0, remove the worktree). Run `scripts/perf-check.sh` after `lyrics`,
`presence` and `memory`.

## 8. Checks after all merges

Headless, under Xvfb (`BATCH3.md` section 8); never on the user's desktop.

- `rustle --demo`: open the lyrics drawer on the synced demo track and
  screenshot it; confirm only one repaint per line change (count frames
  with `RUSTLE_PERF=1`).
- With the app running under Xvfb: `rustle status` prints JSON;
  `rustle next` changes the track; with no app running, `rustle next` exits
  1 with the message.
- Restart with a queue built by `PlayScope` and radio entries: the label
  and radio marks come back.
- `packaging/build-rpm.sh` builds; `rpm -qlp` lists the 4 files. Ask the
  user before installing it with
  `SUDO_ASKPASS=~/.local/bin/swoq-askpass sudo -A dnf install ./<rpm>`.
- Discord presence (if spawned): only if the user has Discord running;
  ask them to look at their profile. Do not start Discord yourself.
- `scripts/perf-check.sh` and the large-library mode both pass.

## 9. Live account rules

`BATCH3.md` section 4 still applies. Before any live call, check whether
the Liked-songs migration finished (`tail -n 3
~/Coding/spotify-ytm-migration/like.log` shows `done:`). Batch 4 needs only
reads (lyrics); it sends no likes or playlist edits. If the account shows
`Session expired`, stop and tell the user.

## 10. Reporting to the user

- Style: short, first line is the result or the next action, no recap
  (the user's `AGENTS.md` and ADHD rules). Use they/them for the user.
- Restate where the batch is each time: "integrator done, 3 of 5 merged,
  next: memory".
- Keep the build-plan doc current:
  https://claude.ai/artifact/SkUkjyTdA6GUfCZkESZvnj (a Claude doc; load
  the docs guide before editing). Add a Batch 4 table like the Batch 3 one
  under "Batch plan", with the `Status` dropdown (`Queued`, `Running`,
  `Merged`, `Blocked`).
- When Batch 4 is merged, all planned batches are done. Tell the user the
  next step is their debugging session, and list everything still
  unverified live: the Batch 3 live checks from `docs/PLAN.md`, the
  re-recorded fixtures, and anything from this batch.

## 11. Known traps (in addition to `BATCH3.md` section 10)

- No worker owns `state.rs`, `backend.rs`, `app.rs` or `main.rs` this
  batch. A worker that needs a change there stops and reports it; you make
  the change on `main` and tell the worker to merge `main`.
- The reducer runs twice per command. Lyrics fetches must be emitted
  deterministically (on track change while the drawer is open), not from
  the view.
- The control socket must not keep the app alive after the window closes,
  and the CLI path must not start tokio or eframe (cold start for
  `rustle next` should be under 50 ms).
- `cargo deny check` runs in `check.sh`; new crates must pass the license
  and source rules in `deny.toml`.
- RSS headroom is the scarcest resource. Every worker reports its RSS
  delta; the lead rejects a merge that pushes peak RSS over 150 MiB.
