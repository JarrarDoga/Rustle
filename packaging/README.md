# Packaging

An RPM for Nobara 44 (Fedora-based), built with
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm). The
manifest is `[package.metadata.generate-rpm]` in `crates/app/Cargo.toml`.

## Build

Needs, once:

- `cargo install --locked cargo-generate-rpm`
- `rustypipe-botguard` in `~/.local/bin` (`cargo install --locked
  rustypipe-botguard`, then link it there), or `RUSTYPIPE_BOTGUARD=/path`.
  The script also takes its `LICENSE` from the cargo registry source of the
  same version, which `cargo install` leaves there.

Then, from any directory, without root:

```sh
packaging/build-rpm.sh
```

It runs `cargo build --release -p rustle`, copies `rustypipe-botguard` and
its license into `target/release/`, runs `cargo generate-rpm -p crates/app`,
and prints the RPM path, `target/generate-rpm/rustle-<version>-1.x86_64.rpm`.
The release profile already strips the binaries.

## Contents

| Path | From |
| --- | --- |
| `/usr/bin/rustle` | the release binary |
| `/usr/share/applications/rustle.desktop` | `packaging/rustle.desktop` (`StartupWMClass=rustle`, the eframe app id) |
| `/usr/share/icons/hicolor/scalable/apps/rustle.svg` | `packaging/rustle.svg`: the three-leaf Rustle mark on a dark rounded tile (`packaging/logo/svg/rustle-icon.svg`) |
| `/usr/share/icons/hicolor/<N>x<N>/apps/rustle.png` | `packaging/icons/hicolor/`: PNGs at 16, 22, 24, 32, 48, 64, 128, 256 and 512 px; the small sizes use the hand-tuned small variant |
| `/usr/libexec/rustle/rustypipe-botguard` | `~/.local/bin/rustypipe-botguard`, found by `ytm-core`'s `find_bin` after PATH and `~/.local/bin` |
| `/usr/share/licenses/rustle/LICENSE.rustypipe-botguard` | its MIT license, which must ship with copies |

Requires: `yt-dlp` (Fedora/Nobara package of that name), the libraries that
auto-req finds (glibc, libasound, libdbus, and libssl/libcrypto for
botguard), and the Wayland, xkbcommon and EGL libraries that eframe loads
with dlopen.

`rustypipe-botguard` is MIT (Copyright 2025 ThetaDev). It statically links a
stripped-down Deno/V8 and bundles JSDOM and other JS packages under MIT,
BSD-2-Clause, ISC and Apache-2.0 (its `js/dependencies.txt`). Shipping only
its own MIT notice is enough for a personal RPM; a public release would need
those notices too.

## Install

After review, on the host:

```sh
SUDO_ASKPASS=~/.local/bin/swoq-askpass sudo -A dnf install ./target/generate-rpm/rustle-0.1.0-1.x86_64.rpm
```

Remove it with `sudo -A dnf remove rustle` the same way.
