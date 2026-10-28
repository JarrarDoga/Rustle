# Rustle logo

Three leaves: gold `#E8C26A`, amber `#E0A04A`, rust `#C8643B` (with a cut
rib), on the D `bg` `#111214` tile for the app icon.

- `svg/rustle-icon.svg`: the app icon (rounded tile). Also `packaging/rustle.svg`.
- `svg/rustle-icon-small.svg`: the small-size variant (bigger leaves, no rib).
- `svg/rustle-glyph*.svg`: the mark alone, in color, white and black.
- `preview.png`: every variant side by side.
- `../icons/hicolor/`: the PNG sizes the RPM installs.

The app draws the mark itself (`crates/app/src/widgets/logo.rs`) from the
same geometry, so it needs no SVG renderer.
