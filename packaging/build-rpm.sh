#!/usr/bin/env bash
# Builds the Rustle RPM into target/generate-rpm/ and prints its path.
# Needs cargo-generate-rpm (cargo install --locked cargo-generate-rpm) and
# rustypipe-botguard in ~/.local/bin. Does not need root.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

if ! cargo generate-rpm --version >/dev/null 2>&1; then
    echo "build-rpm.sh: cargo-generate-rpm is not installed. Run: cargo install --locked cargo-generate-rpm" >&2
    exit 1
fi

botguard="${RUSTYPIPE_BOTGUARD:-$HOME/.local/bin/rustypipe-botguard}"
if [[ ! -f "$botguard" ]]; then
    echo "build-rpm.sh: rustypipe-botguard not found at $botguard." >&2
    echo "Install it with: cargo install --locked rustypipe-botguard, then link it into ~/.local/bin," >&2
    echo "or set RUSTYPIPE_BOTGUARD to its path." >&2
    exit 1
fi
version=$("$botguard" --version | awk 'NR == 1 { print $2 }')
license=$(find "${CARGO_HOME:-$HOME/.cargo}/registry/src" -maxdepth 3 -path "*/rustypipe-botguard-$version/LICENSE" -print -quit 2>/dev/null || true)
if [[ -z "$license" ]]; then
    echo "build-rpm.sh: the LICENSE of rustypipe-botguard $version is not in the cargo registry." >&2
    echo "Run: cargo install --locked rustypipe-botguard@$version (it downloads the source)." >&2
    exit 1
fi

cargo build --release -p rustle

# The RPM manifest (crates/app/Cargo.toml) takes both from target/release/.
cp -L "$botguard" target/release/rustypipe-botguard
cp "$license" target/release/LICENSE.rustypipe-botguard

out=$(cargo generate-rpm -p crates/app)
rpm=$(find target/generate-rpm -maxdepth 1 -name 'rustle-*.rpm' -newer target/release/LICENSE.rustypipe-botguard -print -quit)
if [[ -z "$rpm" ]]; then
    echo "build-rpm.sh: cargo generate-rpm wrote no RPM. Its output:" >&2
    echo "$out" >&2
    exit 1
fi
realpath "$rpm"
