# Rustle UI design: Direction D ("One playlist")

This folder is the source of truth for the UI overhaul. The brief below is
the designer's, copied as written. The boards are from the same design
session.

- `boards/*.html`: the 12 D boards as static HTML (scripts stripped). Open in
  a browser to inspect exact markup and CSS values.
- `png/*.png`: the same boards rendered at 1280x800 (D-Tokens 1040x920,
  D-States 1280x1080). Compare screenshots of Rustle against these.

| Board | Shows |
| --- | --- |
| D-Collection | Main screen: rail, browse panel, header, filter row, track table, spine, player bar |
| D-Filter | Artist filter active: chip in the filter field, "Shuffle 41", scope chip in the player |
| D-Range | Spine range #380 to #520 selected, "Shuffle 141" |
| D-Search | Search results with the "In Everything" column |
| D-Album | Album page with "Add 4 missing to Everything" |
| D-Artist | Artist page: top songs, albums, singles |
| D-Queue | Queue drawer over the collection |
| D-Login | Cookie paste sign-in |
| D-LoginError | Sign-in with an invalid cookie |
| D-Empty | First run: pick your main playlist; nothing playing |
| D-States | Loading skeleton, offline banner, session-expired banner, buffering |
| D-Tokens | Colors, type, spacing, radius, sizes, row, spine and button states |

Known board defect: the Cookie field in D-Login and D-LoginError shows
`[object Object]`. That is a bug in the mockup. The real field is empty with
placeholder text.

---

## Who this is for
The user listens from one giant playlist (~1,500 songs, every genre and mood, kept in the
order they were added). He doesn't curate lots of playlists, doesn't browse "Listen again"
shelves, and mostly hits shuffle. Spotify, YouTube Music and Apple Music are all built around
many playlists plus recommendation shelves. Rustle is built around the one big playlist instead.

## Core idea
The main playlist is the app. Rustle opens straight into it (the "Collection" screen).
There is no Home screen. Every other screen (Search, Album, Artist, Library) exists either to
add songs to that playlist or to play part of it.

## Why each piece exists
- No Home, no shelves: recommendation rows assume you pick a playlist for each mood.
  This user doesn't, so Home was cut entirely.
- Icon rail instead of a playlists sidebar: other playlists are rarely used, so they live
  under Library. The 52px rail (Collection, Search, Library) gives the track table more room.
- Browse panel (every artist / album in the playlist, with song counts): with 1,500 mixed
  songs, "who" is the fastest way to narrow down. Clicking an artist filters the table.
- Filter field: instant local filter on title, artist and album. Active filters show as
  removable chips inside the field.
- Scope: whatever the table currently shows (whole playlist, a filter, or a spine range) is
  the scope. The header buttons act on the scope ("Shuffle all" becomes "Shuffle 41"), and
  the player's "Playing from" chip shows which scope the shuffle is drawing from.
  This is the core interaction: narrow down, then shuffle what you see.
- Shuffle is the primary button and "Play in order" is secondary, because shuffle is how a
  giant mixed playlist actually gets played.
- The spine: a 20px strip on the right edge of the table that stands for the whole playlist,
  colored by album art (one 4px segment = ~10 songs). The playlist is in add order, so songs
  added around the same time sit together and listening "eras" show up as color bands.
  Click to jump; drag to select a stretch (e.g. #380 to #520) and shuffle only that.
  It's the cheapest way to get mood-like control without tagging songs or analyzing audio.
  It also shows what's on screen (white box), the playing song (accent line) and filter
  matches (white ticks).
- Locate button plus "#412" in the player: in a 1,500-row list you constantly lose the
  playing song, so one click scrolls back to it.
- "Last played" column and the "Not heard in 3 months" quick filter: rediscovery inside the
  one playlist. This replaces what "Listen again" shelves do in other apps.
- "In Everything" column on Search, Album and Artist: the main job on those screens is
  adding songs to the one playlist, so every row shows whether a song is already in it
  (check plus its #) or offers an Add button.

## Visual language and why
- Structure comes from direction B ("Hairline"): edge-to-edge regions split by 1px lines,
  no floating panels, dense 32px rows. Dense because the main content is a 1,500-row table,
  not cards. It should feel like a tool, not a toy.
- Color comes from direction C ("Tinted"): the chrome stays neutral gray. The only hue is the
  accent, taken from the now-playing album art (OKLCH L 0.80, chroma clamped 0.09 to 0.11),
  and the player bar gets a 14% tint of it. Album art is the only color source, and there is
  no fixed brand color. With nothing playing, the accent falls back to text-1 (white).
- Accent means "playing / active / primary action" only. Never use it for decoration.
- Everything must map cleanly to egui: flat fills, rounded rects (radius 0 / 2 / 4),
  1px strokes, a single drop shadow (queue drawer only). No blur, gradients, glow, pulsing,
  shimmer or decorative animation. Loading uses determinate progress and static skeleton rows.
- One font (Inter) with 5 sizes: 22 / 16 / 13 / 12 / 11. One icon set (Phosphor).

## Rules for agents
- Do not bring back Home, "Listen again", recommendation shelves, or a playlists list in
  the sidebar.
- Every song list uses the same track table component and the same row states:
  default, hover, selected, playing.
- Any screen that shows songs from outside the main playlist shows the "In Everything" status.
- Take exact values from the D tokens board (D-Tokens). Boards A, B and C are earlier
  explorations; don't pull values from them.
- "Everything" is a placeholder name for whichever playlist the user picks as main on first run.

## Data this design depends on
- Local play history (Rustle records plays itself) for "Last played" and "Not heard in
  3 months". YouTube Music's own history doesn't go back far enough.
- Playlist order roughly matching the order songs were added, so the spine's bands mean
  "eras". True for the user's migrated playlist.
- A dominant color per album art, computed once per track and cached, for the accent and
  the spine segments.
- Artist and album counts computed locally from the playlist.

## Open / not designed yet
- Mood-aware shuffle (avoiding jarring genre jumps) would need audio analysis or
  similar-song data. Not designed.
