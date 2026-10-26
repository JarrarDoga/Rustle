#!/usr/bin/env bash
# Local equivalent of CI (.github/workflows/ci.yml). Stops at the first failure.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! cargo deny --version >/dev/null 2>&1; then
    echo "check.sh: cargo-deny is not installed. Run: cargo install --locked cargo-deny" >&2
    exit 1
fi

step() {
    local name=$1
    shift
    echo "==> $name: $*"
    if ! "$@"; then
        echo >&2
        echo "check.sh: FAILED at '$name' ($*)" >&2
        exit 1
    fi
}

step fmt cargo fmt --all --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step test cargo test --workspace
step deny cargo deny check --hide-inclusion-graph

echo
echo "check.sh: all checks passed"
