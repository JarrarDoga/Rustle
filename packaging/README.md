# Packaging

An RPM for Nobara 44 (Fedora-based), built with
[cargo-generate-rpm](https://github.com/cat-in-136/cargo-generate-rpm). The
manifest is `[package.metadata.generate-rpm]` in `crates/app/Cargo.toml`.

## Build

Needs, once: `cargo install --locked cargo-generate-rpm`.

Then, from any directory, without root:

```sh
packaging/build-rpm.sh
```

It runs `cargo build --release -p rustle`, runs `cargo generate-rpm -p
crates/app`, and prints the RPM path,
`target/generate-rpm/rustle-<version>-1.x86_64.rpm`. The release profile
already strips the binary.

## Contents

| Path | From |
| --- | --- |
| `/usr/bin/rustle` | the release binary |
| `/usr/share/applications/rustle.desktop` | `packaging/rustle.desktop` (`StartupWMClass=rustle`, the eframe app id) |
| `/usr/share/icons/hicolor/scalable/apps/rustle.svg` | `packaging/rustle.svg`: the three-leaf Rustle mark on a dark rounded tile (`packaging/logo/svg/rustle-icon.svg`) |
| `/usr/share/icons/hicolor/<N>x<N>/apps/rustle.png` | `packaging/icons/hicolor/`: PNGs at 16, 22, 24, 32, 48, 64, 128, 256 and 512 px; the small sizes use the hand-tuned small variant |

Requires: the libraries that auto-req finds (glibc, libasound, libdbus),
and the Wayland, xkbcommon and EGL libraries that eframe loads with dlopen.

Until #12 the RPM also required `yt-dlp` and shipped the `rustypipe-botguard`
helper in `/usr/libexec/rustle`; `docs/design/stream-fallbacks.md` says what
they were for and how to put them back.

## Install

After review, on the host:

```sh
SUDO_ASKPASS=~/.local/bin/swoq-askpass sudo -A dnf install ./target/generate-rpm/rustle-0.1.0-1.x86_64.rpm
```

Remove it with `sudo -A dnf remove rustle` the same way.
