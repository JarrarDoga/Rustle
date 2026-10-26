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

Each run writes `perf-results/<timestamp>.json` (gitignored). The script exits
1 if a target fails or the app exits before the window ends (the table shows
the exit code, the signal and the last stderr lines), and 2 on a usage or setup
error.
