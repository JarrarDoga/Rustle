# ytm-core fixtures

Recorded with `spike record` (real responses, no cookies):
`rp_search.json`, `rp_playlist.json`. `search.json` and `playlist.json` are
their model mapping (`UPDATE_FIXTURES=1 cargo test -p ytm-core`).
`ytdlp_251.json` is real yt-dlp output.

Constructed by hand, not recorded (2026-10-26). Live writes were not allowed
while the like migration ran, and YouTube answered every read attempt that
day with HTTP 403, so the shapes come from ytmusicapi 1.12.3's parsers and
docs and from what the migration job saw live:

- `like.json`, `removelike.json`, `like_logged_out.json`: `like/like` and
  `like/removelike` answer with `actions` and `responseContext` only (no
  `status`); `logged_in` in `serviceTrackingParams` tells success from a
  logged-out session.
- `edit_playlist_add.json`, `edit_playlist_remove.json`,
  `edit_playlist_duplicate.json`: `browse/edit_playlist` with `status`, and
  `playlistEditResults[].playlistEditVideoAddedResultData` for adds.
- `browse_owned_playlist.json`, `browse_owned_playlist_continuation.json`:
  `browse` `VL<id>` of an owned playlist, trimmed to three rows; entry ids
  sit in `playlistItemData.playlistSetVideoId` and in the "Remove from
  playlist" menu item.
- `rp_radio.json`: rustypipe's radio paginator, built from tracks in
  `rp_search.json` and `rp_playlist.json` (seed first, one repeat).

Re-record the browse pages with `spike record-owned` and the radio page with
`spike record-radio` when YouTube allows it, and replace the constructed files.
