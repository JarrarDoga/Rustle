# scripts

## check.sh

The local equivalent of CI. Run it before you report work as done:

```sh
scripts/check.sh
```

It runs `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` and `cargo deny check`, in that order, and stops at the
first failure. It needs `cargo-deny` (`cargo install --locked cargo-deny`).
`.github/workflows/ci.yml` runs the same four commands as separate steps; change
both files together.

## perf-check.sh

Measures the release build against the targets table in `AGENTS.md`:

```sh
scripts/perf-check.sh              # build, then measure for 60 s, headless
scripts/perf-check.sh --secs 10    # shorter sampling window
scripts/perf-check.sh --no-build   # measure the existing target/release/rustle
scripts/perf-check.sh --no-headless  # open a real window on your desktop
```

It runs `rustle --demo` with `RUSTLE_PERF=1` and reports:

- release binary size (target: under 25 MiB);
- cold start: time from launch until the app prints `rustle: first frame` on
  stderr (target: under 1 s). If the line does not appear within
  `--start-timeout` seconds (default 10), the row reads N/A and the run continues;
- peak RSS: the highest `VmHWM` from `/proc/<pid>/status` (target: under 150 MiB);
- average CPU over `--secs` seconds after the first frame, from
  `/proc/<pid>/stat` (target for idle: under 1% of one core). `--demo` does not
  play audio, so the "CPU while playing" row is always N/A.

Headless is the default: the script re-runs itself under `xvfb-run -a` with
`WAYLAND_DISPLAY` unset, so no window opens on the desktop. Xvfb renders with
llvmpipe (software GL), so headless CPU and RSS are an upper bound for the
real GPU path. Use `--no-headless` for numbers on the real compositor.

### Large-library mode

```sh
scripts/perf-check.sh --large          # 1,535 tracks
scripts/perf-check.sh --large=5000     # another size
```

`--demo` has 33 unique tracks, so it does not show what a real library
costs. `--large` runs `rustle --demo` on a generated library instead
(`RUSTLE_DEMO_LARGE=<tracks>`, `FixtureBackend::large`), opened as the main
playlist, with no network:

- `crates/app/examples/demo_art.rs` draws one JPEG per track in the sizes
  YouTube Music lists for songs (60, 120, 226 and 544 px), each in its own
  hue. The script generates them once into `target/perf-art/` and reuses them.
- A Python keep-alive HTTP server on 127.0.0.1 serves them; the app gets its
  address in `RUSTLE_DEMO_ART`. `XDG_CACHE_HOME` points at a fresh temp
  directory, so every image is fetched and stored once, as on a first run,
  and your own thumbnail cache is not touched.
- After the settle time, `xdotool` scrolls the track table from top to
  bottom with the mouse wheel (one notch every 25 ms, about 33 s for 1,535
  tracks). The art color pass runs at the same time.
- The script then waits until the app reports a color for every track
  (`rustle: art colors N` on stderr), or until no new color came for 20 s,
  and only then opens the idle CPU window.

Extra rows: the scroll (notches and time), the colors computed against the
track count (fail if any are missing), and the files fetched. Peak RSS is
judged against the 150 MiB target; the Batch 4 goal for this mode is under
135 MiB. It needs `python3` and `xdotool`.

`RUSTLE_PERF_DEBUG=<dir>` (either mode) keeps `/proc/<pid>/smaps` and
`status` at each phase end (`first-frame`, `scrolled`, `colored`, `end`),
`smaps_rollup` at the highest RSS seen, and, in large mode, a screenshot
after the scroll (needs ImageMagick `import`).

Each run writes `perf-results/<timestamp>-<mode>.json` (gitignored). The script exits
1 if a target fails or the app exits before the window ends (the table shows
the exit code, the signal and the last stderr lines), and 2 on a usage or setup
error.
