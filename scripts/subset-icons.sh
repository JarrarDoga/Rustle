#!/usr/bin/env bash
# Subsets the Phosphor icon fonts to the icons listed in crates/app/src/icons.rs.
# Run it after adding an icon there. Needs uv (uvx runs fonttools).
# See scripts/README.md.
set -euo pipefail
export LC_ALL=C

cd "$(dirname "${BASH_SOURCE[0]}")/.."
FONTS=crates/app/assets/fonts

# U+E000,U+E001,... from the `"\u{E000}"` literals.
codes=$(grep -oE '\\u\{[0-9A-Fa-f]+\}' crates/app/src/icons.rs | tr -d '\\u{}' | sort -u | sed 's/^/U+/' | paste -sd, -)
if [[ -z $codes ]]; then
    echo "subset-icons.sh: no codepoints found in crates/app/src/icons.rs" >&2
    exit 1
fi

for name in Phosphor Phosphor-Fill; do
    uvx --from 'fonttools==4.68.0' pyftsubset "$FONTS/phosphor-full/$name.ttf" \
        --unicodes="$codes" --layout-features='' --name-IDs='*' --notdef-outline \
        --output-file="$FONTS/$name.ttf"
    echo "$FONTS/$name.ttf: $(stat -c %s "$FONTS/$name.ttf") bytes"
done
