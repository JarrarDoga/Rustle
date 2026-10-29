#!/usr/bin/env bash
# Builds the Rustle RPM into target/generate-rpm/ and prints its path.
# Needs cargo-generate-rpm (cargo install --locked cargo-generate-rpm). Does
# not need root.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! cargo generate-rpm --version >/dev/null 2>&1; then
    echo "build-rpm.sh: cargo-generate-rpm is not installed. Run: cargo install --locked cargo-generate-rpm" >&2
    exit 1
fi

cargo build --release -p rustle

# Marks the start, so an RPM left over from an earlier run is not reported.
stamp=$(mktemp)
trap 'rm -f "$stamp"' EXIT

out=$(cargo generate-rpm -p crates/app)
rpm=$(find target/generate-rpm -maxdepth 1 -name 'rustle-*.rpm' -newer "$stamp" -print -quit)
if [[ -z "$rpm" ]]; then
    echo "build-rpm.sh: cargo generate-rpm wrote no RPM. Its output:" >&2
    echo "$out" >&2
    exit 1
fi
realpath "$rpm"
