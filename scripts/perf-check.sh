#!/usr/bin/env bash
# Measure the release build of `rustle --demo` against the AGENTS.md targets.
# Usage: scripts/perf-check.sh [--no-build] [--secs N] [--start-timeout N] [--headless|--no-headless]
#                              [--large[=TRACKS]]
# See scripts/README.md. Exit status: 0 = no target failed, 1 = a target failed
# or the app crashed, 2 = usage or setup error.
set -euo pipefail
export LC_ALL=C

usage() {
    cat <<'EOF'
Usage: scripts/perf-check.sh [options]

  --no-build          Skip `cargo build --release -p rustle`.
  --secs N            CPU/RSS sampling window after the first frame (default 60).
  --start-timeout N   Seconds to wait for "rustle: first frame" (default 10).
  --headless          Run under `xvfb-run -a` so no window opens (default).
  --no-headless       Open a real window on the current desktop.
  --large[=TRACKS]    Large-library mode: a generated library of TRACKS
                      (default 1535) with art from a local server, scrolled
                      top to bottom while the art color pass runs.
  -h, --help          Show this help.

Writes perf-results/<timestamp>.json and exits 1 if a target fails or the app crashes.
EOF
}

ORIG_ARGS=("$@")
BUILD=1
SECS=60
START_TIMEOUT=10
HEADLESS=1
LARGE=0 # tracks in the generated library; 0 = the recorded --demo fixture

while (($#)); do
    case $1 in
        --no-build) BUILD=0 ;;
        --secs) SECS=${2:?--secs needs a value}; shift ;;
        --secs=*) SECS=${1#*=} ;;
        --start-timeout) START_TIMEOUT=${2:?--start-timeout needs a value}; shift ;;
        --start-timeout=*) START_TIMEOUT=${1#*=} ;;
        --headless) HEADLESS=1 ;;
        --no-headless) HEADLESS=0 ;;
        --large) LARGE=1535 ;;
        --large=*) LARGE=${1#*=} ;;
        -h | --help) usage; exit 0 ;;
        *) echo "perf-check.sh: unknown option '$1'" >&2; usage >&2; exit 2 ;;
    esac
    shift
done

if ! [[ $LARGE =~ ^[0-9]+$ ]]; then
    echo "perf-check.sh: --large needs a track count" >&2
    exit 2
fi

for v in SECS START_TIMEOUT; do
    if ! [[ ${!v} =~ ^[0-9]+$ ]] || ((${!v} < 1)); then
        echo "perf-check.sh: ${v,,} must be a positive integer" >&2
        exit 2
    fi
done

TOOLS=(jq cargo)
((LARGE)) && TOOLS+=(python3 xdotool)
for tool in "${TOOLS[@]}"; do
    command -v "$tool" >/dev/null || { echo "perf-check.sh: '$tool' not found" >&2; exit 2; }
done

SELF=$(realpath "${BASH_SOURCE[0]}")
cd "$(dirname "$SELF")/.."
ROOT=$PWD

TARGET_DIR=$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)
BIN=$TARGET_DIR/release/rustle
# Bump when crates/app/examples/demo_art.rs changes what it draws.
ART_VERSION=1
ART_DIR=$TARGET_DIR/perf-art/v$ART_VERSION-$LARGE

# Build once, outside Xvfb.
if ((BUILD)) && [[ -z ${RUSTLE_PERF_XVFB:-} ]]; then
    echo "==> cargo build --release -p rustle"
    cargo build --release -p rustle
fi

# Large mode: generate the art once per track count; later runs reuse it.
if ((LARGE)) && [[ -z ${RUSTLE_PERF_XVFB:-} ]] && [[ $(cat "$ART_DIR/done" 2>/dev/null) != "$LARGE" ]]; then
    echo "==> generating art for $LARGE tracks in ${ART_DIR#"$ROOT"/}"
    cargo run --release -q -p rustle --example demo_art -- "$ART_DIR" "$LARGE"
fi

# Headless: re-run this script inside a private X server, so the PID we sample
# is rustle itself and not xvfb-run. 24-bit depth: GL contexts fail on 8-bit.
if ((HEADLESS)) && [[ -z ${RUSTLE_PERF_XVFB:-} ]]; then
    command -v xvfb-run >/dev/null || {
        echo "perf-check.sh: xvfb-run not found (install xorg-x11-server-Xvfb or use --no-headless)" >&2
        exit 2
    }
    RUSTLE_PERF_XVFB=1 exec xvfb-run -a -s "-screen 0 1280x800x24" "$SELF" "${ORIG_ARGS[@]}" --no-build
fi

[[ -x $BIN ]] || { echo "perf-check.sh: $BIN not found; run without --no-build" >&2; exit 2; }

# Targets from AGENTS.md.
MIB=$((1024 * 1024))
SIZE_MAX=$((25 * MIB))
START_MAX_MS=1000
RSS_MAX_KB=$((150 * 1024))
CPU_IDLE_MAX=1.0 # "~0%": pass under 1% of one core

# Large mode: wheel notches per xdotool call, and the pause between notches.
# egui scrolls 40 points per notch.
SCROLL_CHUNK=20
SCROLL_DELAY_MS=25
# Large mode: stop waiting for the color pass after this long with no new color.
COLORS_QUIET_S=20
COLORS_MAX_S=300

TMP=$(mktemp -d)
PID=
SERVER_PID=
cleanup() {
    if [[ -n $PID ]] && kill -0 "$PID" 2>/dev/null; then
        kill "$PID" 2>/dev/null || true
        sleep 0.5
        kill -9 "$PID" 2>/dev/null || true
    fi
    if [[ -n $SERVER_PID ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

now_us() { echo "${EPOCHREALTIME/./}"; }
alive() { kill -0 "$PID" 2>/dev/null && [[ $(proc_state) != Z ]]; }
# Fields after "(comm)": state is field 3, utime 14, stime 15.
proc_stat() { local s; s=$(<"/proc/$PID/stat") && echo "${s##*) }"; }
proc_state() { local f; f=$(proc_stat 2>/dev/null) || return 0; echo "${f%% *}"; }
cpu_ticks() { local -a f; read -ra f <<<"$(proc_stat)" && echo $((f[11] + f[12])); }
hwm_kb() { awk '/^VmHWM:/ {print $2}' "/proc/$PID/status" 2>/dev/null; }

PEAK_KB=0
sample_rss() {
    local kb
    kb=$(hwm_kb) || return 0
    if [[ -n $kb ]] && ((kb > PEAK_KB)); then
        PEAK_KB=$kb
        # Debug: the memory map breakdown at each new peak.
        if [[ -n $DEBUG_DIR ]]; then
            cat "/proc/$PID/smaps_rollup" >"$DEBUG_DIR/peak-smaps_rollup" 2>/dev/null || true
        fi
    fi
    return 0
}
# RUSTLE_PERF_DEBUG=<dir>: keep /proc/<pid>/smaps at each phase end, and a
# screenshot after the large-mode scroll.
DEBUG_DIR=${RUSTLE_PERF_DEBUG:-}
[[ -n $DEBUG_DIR ]] && mkdir -p "$DEBUG_DIR"
snapshot() {
    [[ -n $DEBUG_DIR ]] || return 0
    cat "/proc/$PID/smaps" >"$DEBUG_DIR/$1-smaps" 2>/dev/null || true
    cat "/proc/$PID/status" >"$DEBUG_DIR/$1-status" 2>/dev/null || true
}
# The newest "rustle: art colors N" count, or 0.
colors_now() {
    local n
    n=$(grep -o 'rustle: art colors [0-9]*' "$TMP/stderr.log" | tail -n 1)
    echo "${n##* }" | sed 's/^$/0/'
}

SIZE_BYTES=$(stat -c %s "$BIN")

LAUNCH_ENV=(RUSTLE_PERF=1)
((HEADLESS)) && LAUNCH_ENV=(-u WAYLAND_DISPLAY -u WAYLAND_SOCKET "${LAUNCH_ENV[@]}")
MODE=demo
if ((LARGE)); then
    MODE=large
    [[ $(cat "$ART_DIR/done" 2>/dev/null) == "$LARGE" ]] || {
        echo "perf-check.sh: no art in $ART_DIR; run without --no-build" >&2
        exit 2
    }
    # A keep-alive HTTP/1.1 server like the CDN's, in its own process so its
    # memory is not counted. It prints its port.
    python3 -I -c '
import sys, http.server as h
class H(h.SimpleHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def __init__(s, *a, **k): super().__init__(*a, directory=sys.argv[1], **k)
    def log_message(s, *a): pass
srv = h.ThreadingHTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
' "$ART_DIR" >"$TMP/port" &
    SERVER_PID=$!
    for _ in $(seq 50); do [[ -s $TMP/port ]] && break; sleep 0.1; done
    PORT=$(<"$TMP/port")
    [[ -n $PORT ]] || { echo "perf-check.sh: the art server did not start" >&2; exit 2; }
    # A fresh thumbnail cache: every image is fetched and stored once, as on
    # a first run, and the user's own cache is never touched.
    mkdir -p "$TMP/cache"
    LAUNCH_ENV+=(XDG_CACHE_HOME="$TMP/cache" RUSTLE_DEMO_LARGE="$LARGE"
        RUSTLE_DEMO_ART="http://127.0.0.1:$PORT")
fi

echo "==> launching $BIN --demo (mode=$MODE, headless=$HEADLESS, window=${SECS}s)"
T0=$(now_us)
# No core dump: a crash would otherwise spend about 1 s in systemd-coredump.
(ulimit -c 0; exec env "${LAUNCH_ENV[@]}" "$BIN" --demo >"$TMP/stdout.log" 2>"$TMP/stderr.log") &
PID=$!

# Phase 1: wait for the first frame, the process to die, or the timeout.
START_MS=
deadline=$((T0 + START_TIMEOUT * 1000000))
while alive; do
    sample_rss
    if grep -q 'rustle: first frame' "$TMP/stderr.log"; then
        START_MS=$((($(now_us) - T0) / 1000))
        break
    fi
    (($(now_us) >= deadline)) && break
    sleep 0.02
done
[[ -n $START_MS ]] && snapshot first-frame

# Phase 2: sample CPU and RSS over the window.
CPU_PCT=
WINDOW_S=
# Let post-first-frame startup work (fonts, fixtures) finish before the idle
# window opens, so it measures the settled app. RSS is still sampled.
SETTLE_S=2
settle_end=$(($(now_us) + SETTLE_S * 1000000))
while alive && (($(now_us) < settle_end)); do sample_rss; sleep 0.2; done

# Large mode: scroll the track table top to bottom with the mouse wheel, then
# wait for the art color pass (it started when the library loaded).
SCROLL_MS=
NOTCHES=0
COLORS=0
COLORS_MS=
PEAK_SCROLL_KB=
if ((LARGE)) && alive; then
    WID=$(timeout 10 xdotool search --sync --onlyvisible --name '^Rustle$' | head -n 1) || true
    if [[ -n $WID ]]; then
        eval "$(xdotool getwindowgeometry --shell "$WID")"
        # The middle of the window is inside the track table.
        xdotool mousemove --window "$WID" $((WIDTH / 2)) $((HEIGHT / 2))
        # 32 px rows, 40 px per notch, plus some to be sure it hits the end.
        want=$(((LARGE * 32 + 39) / 40 + 40))
        t=$(now_us)
        while alive && ((NOTCHES < want)); do
            xdotool click --repeat "$SCROLL_CHUNK" --delay "$SCROLL_DELAY_MS" 5 || break
            NOTCHES=$((NOTCHES + SCROLL_CHUNK))
            sample_rss
        done
        SCROLL_MS=$((($(now_us) - t) / 1000))
        PEAK_SCROLL_KB=$PEAK_KB
        snapshot scrolled
        if [[ -n $DEBUG_DIR ]] && command -v import >/dev/null; then
            sleep 0.5
            import -window root "$DEBUG_DIR/scrolled.png" 2>/dev/null || true
        fi
    else
        echo "perf-check.sh: no Rustle window found; not scrolling" >&2
    fi
    last=-1 quiet_end=0
    colors_end=$(($(now_us) + COLORS_MAX_S * 1000000))
    while alive && (($(now_us) < colors_end)); do
        sample_rss
        COLORS=$(colors_now)
        if ((COLORS >= LARGE)); then break; fi
        if ((COLORS != last)); then
            last=$COLORS
            quiet_end=$(($(now_us) + COLORS_QUIET_S * 1000000))
        elif (($(now_us) >= quiet_end)); then
            break
        fi
        sleep 0.2
    done
    COLORS_MS=$((($(now_us) - T0) / 1000))
    snapshot colored
fi

if alive; then
    clk=$(getconf CLK_TCK)
    c0=$(cpu_ticks) t0=$(now_us) c1=$c0 t1=$t0
    end=$((t0 + SECS * 1000000))
    while alive && (($(now_us) < end)); do
        sample_rss
        if c=$(cpu_ticks 2>/dev/null); then c1=$c t1=$(now_us); fi
        sleep 0.5
    done
    if alive; then
        sample_rss
        snapshot end
        c1=$(cpu_ticks) t1=$(now_us)
        CPU_PCT=$(awk -v d="$((c1 - c0))" -v hz="$clk" -v us="$((t1 - t0))" \
            'BEGIN { printf "%.2f", d / hz / (us / 1e6) * 100 }')
        WINDOW_S=$(awk -v us="$((t1 - t0))" 'BEGIN { printf "%.1f", us / 1e6 }')
    fi
fi

# Collect the exit status.
EXIT_CODE=
CRASHED=0
if alive; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
else
    rc=0
    wait "$PID" 2>/dev/null || rc=$?
    EXIT_CODE=$rc
    CRASHED=1
fi
RAN_MS=$((($(now_us) - T0) / 1000))
PID=

exit_desc() {
    local rc=$1
    if ((rc > 128)); then
        local sig
        sig=$(kill -l $((rc - 128)) 2>/dev/null || echo "?")
        echo "exit $rc, SIG$sig"
    else
        echo "exit $rc"
    fi
}

# Build result rows: name|value|target|status|note
ROWS=()
FAILED=0
row() {
    ROWS+=("$1|$2|$3|$4|$5")
    if [[ $4 == fail ]]; then FAILED=1; fi
}
judge() { if "$@"; then echo pass; else echo fail; fi; }

size_mib=$(awk -v b="$SIZE_BYTES" -v m="$MIB" 'BEGIN { printf "%.1f", b / m }')
row "binary size" "$size_mib MiB" "< 25 MiB" "$(judge test "$SIZE_BYTES" -lt "$SIZE_MAX")" ""

if [[ -n $START_MS ]]; then
    row "cold start" "$START_MS ms" "< $START_MAX_MS ms" "$(judge test "$START_MS" -lt "$START_MAX_MS")" "launch to 'rustle: first frame'"
elif ((CRASHED)); then
    row "cold start" "-" "< $START_MAX_MS ms" "n/a" "process exited before first frame"
else
    row "cold start" "-" "< $START_MAX_MS ms" "n/a" "no 'rustle: first frame' line within ${START_TIMEOUT}s"
fi

RSS_NOTE="VmHWM over the whole run"
((LARGE)) && RSS_NOTE="$RSS_NOTE; Batch 4 goal < 135 MiB"
if [[ -n $CPU_PCT ]]; then
    rss_mib=$(awk -v k="$PEAK_KB" 'BEGIN { printf "%.1f", k / 1024 }')
    row "peak RSS ($MODE)" "$rss_mib MiB" "< 150 MiB" "$(judge test "$PEAK_KB" -lt "$RSS_MAX_KB")" "$RSS_NOTE"
    row "CPU idle ($MODE, paused)" "$CPU_PCT %" "< $CPU_IDLE_MAX %" \
        "$(judge awk -v c="$CPU_PCT" -v m="$CPU_IDLE_MAX" 'BEGIN { exit !(c < m) }')" "average over ${WINDOW_S}s"
else
    if ((PEAK_KB)); then
        rss_mib=$(awk -v k="$PEAK_KB" 'BEGIN { printf "%.1f", k / 1024 }')
        row "peak RSS ($MODE)" "$rss_mib MiB" "< 150 MiB" "n/a" "sampled until exit after ${RAN_MS} ms"
    else
        row "peak RSS ($MODE)" "-" "< 150 MiB" "n/a" "process exited after ${RAN_MS} ms"
    fi
    row "CPU idle ($MODE, paused)" "-" "< $CPU_IDLE_MAX %" "n/a" "process exited after ${RAN_MS} ms"
fi
row "CPU while playing" "-" "< 2 %" "n/a" "not measured: --demo does not play audio"
if ((LARGE)); then
    if [[ -n $SCROLL_MS ]]; then
        row "scroll" "$NOTCHES notches" "top to bottom" "info" "${SCROLL_MS} ms, ${SCROLL_DELAY_MS} ms between notches"
    else
        row "scroll" "-" "top to bottom" "fail" "no Rustle window found"
    fi
    row "art colors" "$COLORS / $LARGE" "all tracks" "$(judge test "$COLORS" -ge "$LARGE")" \
        "done ${COLORS_MS:-?} ms after launch"
    ART_FETCHED=$(find "$TMP/cache" -type f 2>/dev/null | wc -l)
    row "art fetched" "$ART_FETCHED files" "-" "info" "from the local server into a fresh cache"
fi

if ((CRASHED)); then
    row "process" "crashed" "runs for ${SECS}s" "fail" "$(exit_desc "$EXIT_CODE") after ${RAN_MS} ms"
else
    row "process" "ran" "runs for ${SECS}s" "pass" ""
fi

# Print the table.
echo
printf '%-24s %-12s %-14s %-6s %s\n' METRIC VALUE TARGET STATUS NOTE
printf '%-24s %-12s %-14s %-6s %s\n' ------ ----- ------ ------ ----
for r in "${ROWS[@]}"; do
    IFS='|' read -r n v t s note <<<"$r"
    printf '%-24s %-12s %-14s %-6s %s\n' "$n" "$v" "$t" "${s^^}" "$note"
done

if ((CRASHED)) && [[ -s $TMP/stderr.log ]]; then
    echo
    echo "last stderr lines:"
    tail -n 5 "$TMP/stderr.log" | sed 's/^/  /'
fi

# Write JSON.
mkdir -p "$ROOT/perf-results"
STAMP=$(date +%Y%m%dT%H%M%S)
OUT=$ROOT/perf-results/$STAMP-$MODE.json
printf '%s\n' "${ROWS[@]}" | jq -R -s \
    --arg timestamp "$(date -Iseconds)" \
    --arg commit "$(git -C "$ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)" \
    --arg binary "$BIN" \
    --arg mode "$MODE" \
    --argjson headless "$HEADLESS" \
    --argjson large_tracks "$LARGE" \
    --argjson scroll_ms "${SCROLL_MS:-null}" \
    --argjson scroll_notches "$NOTCHES" \
    --argjson scroll_delay_ms "$SCROLL_DELAY_MS" \
    --argjson colors "$COLORS" \
    --argjson colors_ms "${COLORS_MS:-null}" \
    --argjson peak_rss_scroll_kb "${PEAK_SCROLL_KB:-null}" \
    --argjson secs "$SECS" \
    --argjson size_bytes "$SIZE_BYTES" \
    --argjson start_ms "${START_MS:-null}" \
    --argjson peak_rss_kb "$( ((PEAK_KB)) && echo "$PEAK_KB" || echo null)" \
    --argjson cpu_idle_pct "${CPU_PCT:-null}" \
    --argjson exit_code "${EXIT_CODE:-null}" \
    --argjson crashed "$CRASHED" \
    --argjson failed "$FAILED" \
    --arg stderr_tail "$(tail -n 5 "$TMP/stderr.log")" \
    '{
        timestamp: $timestamp, commit: $commit, binary: $binary, mode: $mode,
        headless: ($headless == 1), window_secs: $secs,
        status: (if $crashed == 1 then "crashed" elif $failed == 1 then "fail" else "pass" end),
        exit_code: $exit_code,
        raw: { binary_size_bytes: $size_bytes, cold_start_ms: $start_ms,
               peak_rss_kb: $peak_rss_kb, cpu_idle_pct: $cpu_idle_pct },
        large: (if $large_tracks == 0 then null else {
            tracks: $large_tracks, scroll_ms: $scroll_ms, scroll_notches: $scroll_notches,
            scroll_delay_ms: $scroll_delay_ms, peak_rss_after_scroll_kb: $peak_rss_scroll_kb,
            colors: $colors, colors_done_ms: $colors_ms } end),
        metrics: [ split("\n")[] | select(length > 0) | split("|")
                   | { name: .[0], value: .[1], target: .[2], status: .[3], note: .[4] } ],
        stderr_tail: $stderr_tail
    }' >"$OUT"

echo
echo "results: ${OUT#"$ROOT"/}"
if ((FAILED)); then
    echo "perf-check: FAIL"
    exit 1
fi
echo "perf-check: PASS"
