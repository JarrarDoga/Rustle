# Stream fallbacks (removed)

Until issue #12, `RustyPipeBackend` resolved a stream through four paths in
order: the visionOS client, rustypipe's web clients with botguard, yt-dlp,
and rustypipe's iOS client. After #12 only the visionOS client is left. This
page records what the other three were, what they cost, and how to bring one
back. All measurements are from issue #1, taken 2026-10-28.

The last commit that contains every piece below is **b677706** (main,
2026-10-28). Line numbers are from that commit.

## Measured times

| Path | Time | Result |
| --- | --- | --- |
| Old signed-in chain, click to audio in the app | 5.6 to 6.5 s | yt-dlp won every time |
| Old signed-in chain run outside the app, per track | 6.0 to 7.9 s | yt-dlp |
| rustypipe web client + botguard | no number | cached failure: the 0.11.4 URL deobfuscator fails on the live player JS |
| yt-dlp with cookies, no JS runtime (what Rustle ran) | 4.7 to 5.0 s | **failed**: "n challenge solving failed ... Only images are available" |
| yt-dlp anonymous | 1.7 to 2.0 s | Opus 251, through yt-dlp's own `visionos` client |
| yt-dlp with cookies and `--js-runtimes node` | 4.8 to 6.2 s | Premium Opus 774 (253 kbps) through TVHTML5 |
| rustypipe iOS client | 626 to 814 ms cold, 92 to 116 ms warm | URL in one request, but **403 past the first 1 MiB** without a PO token |
| Native visionOS `player` request | 102 to 163 ms | Opus 251, whole file served |
| Native visionOS, click to audio in the app | 163 to 226 ms | |

A signed-in cold play used to be: web attempt (cheap once its failure was
cached) + yt-dlp with cookies failing after about 5 s + yt-dlp anonymous
about 2 s. None of the three fallbacks has produced a stream in use since the
visionOS path landed (ed1e37a).

## The four paths

### 1. visionOS client (kept)

- **What:** one Innertube `player` POST as YouTube's visionOS app
  (`clientName VISIONOS`, `clientVersion 1.02`, `X-YouTube-Client-Name: 101`),
  the client yt-dlp's anonymous path uses.
- **Needs:** a visitor id (`X-Goog-Visitor-Id`), taken from rustypipe's
  visitor data cache. No cookies, no PO token, no player JS, no
  deobfuscation; URLs are direct.
- **Gives:** Opus 251 (or AAC 140), full file, Range requests anywhere.
- **Code:** `crates/ytm-core/src/visionos.rs` (`stream`, `parse_player`),
  `RustyPipeBackend::visionos_stream` in `rustypipe_backend.rs`.
- **Dependencies:** `reqwest`, `serde`, `serde_json` (all already in the tree
  through rustypipe).

### 2. rustypipe web clients with botguard (removed)

- **What:** `rp.query().player_from_clients(id, [DesktopMusic, Desktop])`
  when logged in (authenticated), `player_from_client(id, Desktop)` when not.
- **How:** rustypipe asks the `rustypipe-botguard` helper binary for PO
  tokens (session-bound tokens cached with `po_token_cache()`), then
  deobfuscates the stream URLs by running YouTube's player JS in its
  in-process QuickJS (`rquickjs`). Logged in, it could return Premium
  itag 774.
- **Why it went:** rustypipe 0.11.4 (the newest release) fails to find the
  signature function in the current player JS ("could not extract sig fn
  name"), so every attempt was a cached failure. It also needed the
  botguard helper shipped in the RPM.
- **Code at b677706:**
  - `rustypipe_backend.rs`: `BOTGUARD_BIN` (line 30), the builder in
    `RustyPipeBackend::with_storage_dir` (line 85: `find_bin(BOTGUARD_BIN)`,
    then `botguard_bin(bin).po_token_cache()` or `no_botguard()`), the
    `botguard: bool` field, `web_stream` (line 206), `pick_audio` (line 994)
    and `map_stream` (line 1014) with their tests `stream_preference_order`,
    `stream_skips_drm_and_dubbed_tracks`, `stream_source_fields`.
  - Binary lookup: `LIBEXEC_DIR`, `find_bin`, `bin_dirs`, `find_in`
    (lines 559 to 582) with tests `bin_dirs_end_with_libexec` and
    `find_in_takes_the_first_dir_holding_the_file`. `find_bin` searched PATH,
    then `~/.local/bin`, then `/usr/libexec/rustle`, because a desktop
    launcher's PATH often lacks `~/.local/bin` and rustypipe's own probe only
    searches PATH.
- **Cargo:** nothing extra. `rquickjs` is a non-optional dependency of
  rustypipe 0.11.4 (it has no feature to turn it off), so it stays in the
  binary either way.
- **RPM:** `/usr/libexec/rustle/rustypipe-botguard` and
  `/usr/share/licenses/rustle/LICENSE.rustypipe-botguard` in
  `[package.metadata.generate-rpm] assets` of `crates/app/Cargo.toml`;
  `packaging/build-rpm.sh` copied the binary from `~/.local/bin` (or
  `$RUSTYPIPE_BOTGUARD`) and its LICENSE from the cargo registry into
  `target/release/`. `rustypipe-botguard` is MIT and statically links a
  stripped Deno/V8; it pulled in libssl/libcrypto through auto-req.

### 3. yt-dlp, with cookies then anonymous (removed)

- **What:** `yt-dlp -f 774/251 -j --no-playlist --no-warnings --ignore-config
  [--cookies <file>] https://music.youtube.com/watch?v=<id>`, 15 s timeout,
  `kill_on_drop`, JSON output parsed into a `StreamSource`.
- **How:** when logged in, `set_cookies` converted the pasted login to a
  Netscape cookies.txt body (`ytdlp::to_netscape`) and kept it in memory. Each
  call wrote it to a 0600 file in `$XDG_RUNTIME_DIR`
  (`rustle-ytdlp-<pid>-<n>.txt`, deleted on drop, also on timeout). With
  cookies, yt-dlp switches to clients that need a JS runtime for YouTube's
  "n" challenge; Rustle passed none, so that call failed and
  `ytdlp_stream` retried without cookies. The anonymous call succeeds through
  yt-dlp's `visionos` client, the same request path 1 now makes natively.
- **Code at b677706:** all of `crates/ytm-core/src/ytdlp.rs` (`YtDlp::find`,
  `YtDlp::stream`, `parse_output`, `to_netscape`, `CookieFile`,
  `valid_video_id`, and `url_param` / `url_expiry`, which moved to
  `stream_cache.rs`); in `rustypipe_backend.rs` the `ytdlp` and
  `ytdlp_cookies` fields, `ytdlp_stream` (line 243) and the `to_netscape` line
  in `set_cookies` (line 320); fixture `crates/ytm-core/fixtures/ytdlp_251.json`
  (real yt-dlp output) and its line in `fixtures/README.md`.
- **Cargo:** `tokio` feature `process` in `crates/ytm-core/Cargo.toml` (only
  yt-dlp spawned processes); `serde` derive was then justified as
  "deserializes yt-dlp's JSON output" (it stays for the visionOS parser).
- **RPM:** `yt-dlp = "*"` under `[package.metadata.generate-rpm.requires]`
  in `crates/app/Cargo.toml`.

### 4. rustypipe iOS client (removed)

- **What:** `rp.query().player_from_client(id, ClientType::Ios)`, mapped by
  `map_stream`; `pick_audio` refused iOS Opus (its URLs answer 403 on the
  first request) so it returned AAC 140.
- **Why it went:** without a PO token YouTube serves only the first 1 MiB of
  an iOS URL: `Range: bytes=0-1048575` answers 206, any range starting at or
  above 1 MiB answers 403. It would play about a minute of AAC and fail. It
  never worked as a last resort.
- **Code at b677706:** `ios_stream` (line 263), the iOS branch of
  `pick_audio`, test `ios_client_never_picks_opus`.
- **Cargo / RPM:** nothing of its own.

### The ordering

`resolve_in_order` (line 278 at b677706) took the four futures and returned
`(StreamSource, StreamOrigin)`: visionOS, then web, then yt-dlp, then iOS. A
visionOS "not found" and any web error except an extraction failure (auth,
not found, network) ended the search; other visionOS errors and every yt-dlp
error moved on. `StreamOrigin` (`VisionOs`, `Web`, `YtDlp`, `Ios`) was stored
next to each entry in the URL cache and reported by `stream_info` for the
`spike` example. Its test was `fallback_order`.

After #12, `resolve_visionos` takes the visitor id source, the drop
callback and the `player` request as closures. On a bot check
(`LOGIN_REQUIRED`) it drops the visitor id, fetches a fresh one and retries
once; a second bot check, or any other failure, is the error the app shows.
`StreamOrigin` is gone; `StreamInfo` keeps `source` and `cached`.

## Putting one back

Start from the code at b677706:

```sh
git show b677706:crates/ytm-core/src/ytdlp.rs > crates/ytm-core/src/ytdlp.rs
git show b677706:crates/ytm-core/fixtures/ytdlp_251.json > crates/ytm-core/fixtures/ytdlp_251.json
git show b677706:crates/ytm-core/src/rustypipe_backend.rs   # resolve_in_order, web_stream, ytdlp_stream, ios_stream, find_bin
git show b677706:packaging/build-rpm.sh
```

Then, per path:

- **yt-dlp:** restore `ytdlp.rs` (drop its `url_param` / `url_expiry`, which
  now live in `stream_cache.rs`), `mod ytdlp;` in `lib.rs`, the `ytdlp` and
  `ytdlp_cookies` fields, `ytdlp_stream`, the `to_netscape` line in
  `set_cookies`, `find_bin` (yt-dlp is looked up with it), a step and a
  `StreamOrigin` variant in the resolver, tokio's `process` feature, and
  `yt-dlp = "*"` in the RPM requires. Give it a JS runtime
  (`--js-runtimes node:<path>`) or the cookie call fails as above; anonymous
  only, it is just a slower visionOS client.
- **Web + botguard:** restore `BOTGUARD_BIN`, `find_bin` with the libexec
  helpers, the builder branch (`botguard_bin(bin).po_token_cache()`), the
  `botguard` field, `web_stream`, `pick_audio` and `map_stream`; restore the
  two botguard assets in the RPM manifest, the botguard part of
  `build-rpm.sh` and its rows in `packaging/README.md`. Only worth it once a
  rustypipe release can deobfuscate the current player JS again.
- **iOS:** not worth restoring as a fallback while YouTube caps it at 1 MiB
  without a PO token.

Also restore the README Install note (yt-dlp dependency, botguard helper in
`/usr/libexec/rustle`) and the AGENTS.md line about `rustypipe-botguard`
running as a separate helper binary.

## The risk this accepts

visionOS is one undocumented client. YouTube can change or block it (new
client version, a PO token requirement, a 1 MiB cap like iOS) at any time,
and after #12 there is no second path: every play fails with "Could not play
this track" until a release with an updated client definition (or a restored
fallback) ships. The client fields follow yt-dlp's
`INNERTUBE_CLIENTS['visionos']`, so yt-dlp's changes to that entry are the
first place to look when it breaks.
