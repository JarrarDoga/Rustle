# ytm-core fixtures

Recorded with `spike record` (real responses, no cookies):
`rp_search.json`, `rp_playlist.json`. `search.json` and `playlist.json` are
their model mapping (`UPDATE_FIXTURES=1 cargo test -p ytm-core`).
`ytdlp_251.json` is real yt-dlp output.

Recorded with `spike record-pages fixtures UCRr1xG_2WIDs18a6cIiCxeA`
(2026-10-27, no cookies): `rp_artist.json` is rustypipe's `MusicArtist` for
Daft Punk (one page, without the albums behind "More"), and `rp_album.json`
its `MusicAlbum` for the artist's first full album, `MPREb_OgeEnoHTsCm`
(Random Access Memories (Drumless Edition), 13 tracks). `album.json` and
`artist.json` are their model mapping (`UPDATE_FIXTURES=1 cargo test -p
ytm-core`).

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

Lyrics, recorded 2026-10-27 without cookies:

- `lyrics_timed_*.json`: the `ANDROID_MUSIC` `browse` response for a lyrics
  id (`POST https://music.youtube.com/youtubei/v1/browse`, body
  `{"context":{"client":{"clientName":"ANDROID_MUSIC","clientVersion":"7.21.50","hl":"en","gl":"US"}},"browseId":"<MPLYt…>"}`,
  the request `innertube::timed_lyrics` sends), trimmed to the
  `contents…componentType.model` fields the parser reads (the full
  responses are 430 to 650 KB of layout data). The lyrics ids come from
  `music_details` (`next`).
  - `lyrics_timed_synced.json`: `LxgnIhjQFuE`, Harder, Better, Faster,
    Stronger (Alive Radio Edit 2007), `MPLYt_wNhKTKb809x-1`: 103 lines, all
    with `cueRange`, "Source: LyricFind".
  - `lyrics_timed_unsynced.json`: `sFZjqVnWBhc`, Robot Rock,
    `MPLYt_NcFzLIYCtwP`: 40 lines in `timedLyricsModel` without `cueRange`
    (plain lyrics only).
  - `lyrics_timed_unavailable.json`: `PwILkY9gRrc`, Da Funk,
    `MPLYt_YT86wj2Ahaj`: the song has a lyrics id, but the answer is a
    `musicMessageModel` "Lyrics not available at this time." (no lyrics).
- `rp_lyrics_plain.json`: rustypipe's `music_lyrics` for Robot Rock (the web
  client's plain text, CRLF line ends), from
  `spike record-lyrics fixtures sFZjqVnWBhc`. Tests use it for the plain
  fallback.
