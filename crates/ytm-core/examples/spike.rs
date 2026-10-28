//! Live smoke test for `RustyPipeBackend`.
//!
//! ```text
//! cargo run -p ytm-core --example spike -- search <query> [--json]
//! cargo run -p ytm-core --example spike -- stream <video_id>   # resolves twice: path, then cache
//! cargo run -p ytm-core --example spike -- playlist <playlist_id> [--json]
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- library
//! cargo run -p ytm-core --example spike -- record <fixtures_dir> [playlist_id]
//! cargo run -p ytm-core --example spike -- radio <video_id>
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- like <video_id>
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- unlike <video_id>
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- add <playlist_id> <video_id>...
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- remove <playlist_id> <video_id>...
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- record-owned <fixtures_dir> <playlist_id>
//! cargo run -p ytm-core --example spike -- record-radio <fixtures_dir> <video_id>
//! cargo run -p ytm-core --example spike -- album <album_id> [--json]
//! cargo run -p ytm-core --example spike -- artist <channel_id> [--json]
//! cargo run -p ytm-core --example spike -- record-pages <fixtures_dir> <channel_id> [album_id]
//! cargo run -p ytm-core --example spike -- lyrics <video_id>
//! cargo run -p ytm-core --example spike -- record-lyrics <fixtures_dir> <video_id>
//! ```
//!
//! `record` saves raw rustypipe responses (search "daft punk", one page per
//! type, and one public playlist; no cookies) as `rp_*.json`. Then run
//! `UPDATE_FIXTURES=1 cargo test -p ytm-core` to regenerate the model fixtures.
//!
//! `record-owned` saves the first two raw `browse` pages of a playlist the
//! account owns (trimmed to three rows each, without header or tracking data)
//! for the entry id tests. `record-radio` saves one radio page, no cookies.
//! `record-pages` saves one artist page and one album page, no cookies; the
//! album defaults to the artist's first full album. `record-lyrics` saves
//! the web client's plain lyrics of one song (`rp_lyrics_plain.json`), no
//! cookies; the timed `ANDROID_MUSIC` fixtures are recorded as described in
//! `fixtures/README.md`.
//!
//! `add` and `remove` refuse any playlist not named "Rustle test" unless
//! `--force` is given. `remove` looks up each video's entry id itself and
//! removes its first copy. With `RUSTLE_COOKIES_FILE` set, `playlist` and
//! `radio` run logged in; `playlist` then also prints entry ids.

use std::path::Path;
use std::time::Instant;

use ytm_core::model::{
    Album, AlbumSummary, Artist, Playlist, PlaylistSummary, SearchResults, Track,
};
use ytm_core::{MusicBackend, RustyPipeBackend, StreamOrigin};

type AnyResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> AnyResult<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing_subscriber::filter::LevelFilter::WARN)
        .with_writer(std::io::stderr)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let force = args.iter().any(|a| a == "--force");
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json" && *a != "--force")
        .collect();

    match args.as_slice() {
        ["search", query @ ..] if !query.is_empty() => {
            let res = RustyPipeBackend::new()?.search(&query.join(" ")).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&res)?);
            } else {
                print_search(&res);
            }
        }
        ["stream", id] => {
            let backend = RustyPipeBackend::new()?;
            // The second call shows the URL cache: no extractor runs.
            for call in ["first", "second"] {
                let start = Instant::now();
                let info = backend.stream_info(id).await?;
                let path = match info.origin {
                    StreamOrigin::VisionOs => "visionos",
                    StreamOrigin::Web => "web",
                    StreamOrigin::YtDlp => "yt-dlp",
                    StreamOrigin::Ios => "ios",
                };
                let s = &info.source;
                println!(
                    "{call} call: path {path}{}, {:.2} s",
                    if info.cached { " (cached)" } else { "" },
                    start.elapsed().as_secs_f64()
                );
                if call == "second" {
                    break;
                }
                println!(
                    "itag {} {:?} {} kbps, {} bytes, {:?}, loudness {:?} dB",
                    url_param(&s.url, "itag").unwrap_or("?"),
                    s.codec,
                    s.bitrate_bps / 1000,
                    s.content_length,
                    s.duration,
                    s.loudness_db
                );
                println!("{}", s.url);
            }
        }
        ["playlist", id] => {
            let pl = maybe_logged_in().await?.playlist(id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pl)?);
            } else {
                print_playlist(&pl);
            }
        }
        ["radio", id] => {
            let tracks = maybe_logged_in().await?.radio(id).await?;
            println!("{} radio tracks after {id}:", tracks.len());
            for t in &tracks {
                println!("  {}", track_line(t));
            }
        }
        ["like", id] => {
            logged_in().await?.like(id).await?;
            println!("liked {id}");
        }
        ["unlike", id] => {
            logged_in().await?.unlike(id).await?;
            println!("removed like from {id}");
        }
        ["add", playlist_id, ids @ ..] if !ids.is_empty() => {
            let backend = logged_in().await?;
            check_scratch(&backend.playlist(playlist_id).await?, force)?;
            let ids: Vec<String> = ids.iter().map(|s| (*s).to_owned()).collect();
            let entry_ids = backend.add_to_playlist(playlist_id, &ids).await?;
            for (id, entry) in ids.iter().zip(&entry_ids) {
                println!("added {id} as entry {}", entry.as_deref().unwrap_or("?"));
            }
        }
        ["remove", playlist_id, ids @ ..] if !ids.is_empty() => {
            let backend = logged_in().await?;
            let pl = backend.playlist(playlist_id).await?;
            check_scratch(&pl, force)?;
            let mut entries = Vec::new();
            for id in ids {
                let entry = pl
                    .tracks
                    .iter()
                    .zip(&pl.entry_ids)
                    .find(|(t, _)| t.id == *id)
                    .and_then(|(_, e)| e.clone())
                    .ok_or_else(|| format!("{id}: not in the playlist or no entry id"))?;
                entries.push(((*id).to_owned(), entry));
            }
            backend.remove_from_playlist(playlist_id, &entries).await?;
            for (id, entry) in &entries {
                println!("removed {id} (entry {entry})");
            }
        }
        ["album", id] => {
            let album = RustyPipeBackend::new()?.album(id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&album)?);
            } else {
                print_album(&album);
            }
        }
        ["artist", id] => {
            let artist = RustyPipeBackend::new()?.artist(id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&artist)?);
            } else {
                print_artist(&artist);
            }
        }
        ["record-pages", dir, artist_id, rest @ ..] => {
            record_pages(Path::new(dir), artist_id, rest.first().copied()).await?
        }
        ["record-owned", dir, playlist_id] => record_owned(Path::new(dir), playlist_id).await?,
        ["record-radio", dir, video_id] => record_radio(Path::new(dir), video_id).await?,
        ["record-lyrics", dir, video_id] => record_lyrics(Path::new(dir), video_id).await?,
        ["lyrics", id] => {
            let start = Instant::now();
            let lyrics = RustyPipeBackend::new()?.lyrics(id).await?;
            println!("{:.2} s", start.elapsed().as_secs_f64());
            match lyrics {
                None => println!("no lyrics for {id}"),
                Some(l) => {
                    println!(
                        "{} lines, synced: {}, source: {}",
                        l.lines.len(),
                        l.synced,
                        l.source.as_deref().unwrap_or("-")
                    );
                    for line in &l.lines {
                        match line.start {
                            Some(t) => println!(
                                "  [{}:{:05.2}] {}",
                                t.as_secs() / 60,
                                t.as_secs_f64() % 60.0,
                                line.text
                            ),
                            None => println!("  {}", line.text),
                        }
                    }
                }
            }
        }
        ["library"] => {
            let path = std::env::var("RUSTLE_COOKIES_FILE")
                .map_err(|_| "set RUSTLE_COOKIES_FILE to a cookies.txt or Cookie header file")?;
            let backend = RustyPipeBackend::new()?;
            backend.set_cookies(&std::fs::read_to_string(path)?).await?;
            println!("cookies accepted");
            let playlists = backend.library_playlists().await?;
            println!("{} library playlists:", playlists.len());
            for p in &playlists {
                println!("  {}", playlist_line(p));
            }
            let liked = backend.liked_tracks().await?;
            println!("{} liked tracks, first 10:", liked.len());
            for t in liked.iter().take(10) {
                println!("  {}", track_line(t));
            }
        }
        ["record", dir, rest @ ..] => record(Path::new(dir), rest.first().copied()).await?,
        _ => {
            eprintln!(
                "usage: spike search <query> [--json] | stream <id> | playlist <id> [--json] \
                 | library | record <dir> [playlist_id] | radio <id> | like <id> | unlike <id> \
                 | add <playlist_id> <id>... [--force] | remove <playlist_id> <id>... [--force] \
                 | record-owned <dir> <playlist_id> | record-radio <dir> <id> \
                 | album <id> [--json] | artist <id> [--json] \
                 | record-pages <dir> <artist_id> [album_id] | lyrics <id> \
                 | record-lyrics <dir> <id>"
            );
            std::process::exit(2);
        }
    }
    Ok(())
}

/// The cookies file named by `RUSTLE_COOKIES_FILE`, if set.
fn cookies_from_env() -> AnyResult<Option<String>> {
    match std::env::var("RUSTLE_COOKIES_FILE") {
        Ok(path) => Ok(Some(std::fs::read_to_string(path)?)),
        Err(_) => Ok(None),
    }
}

async fn logged_in() -> AnyResult<RustyPipeBackend> {
    let cookies = cookies_from_env()?
        .ok_or("set RUSTLE_COOKIES_FILE to a cookies.txt or Cookie header file")?;
    let backend = RustyPipeBackend::new()?;
    backend.set_cookies(&cookies).await?;
    eprintln!("cookies accepted");
    Ok(backend)
}

async fn maybe_logged_in() -> AnyResult<RustyPipeBackend> {
    match cookies_from_env()? {
        Some(_) => logged_in().await,
        None => Ok(RustyPipeBackend::new()?),
    }
}

/// Writes go only to the scratch playlist the live test creates.
fn check_scratch(pl: &Playlist, force: bool) -> AnyResult<()> {
    if pl.summary.name == "Rustle test" || force {
        Ok(())
    } else {
        Err(format!(
            "refusing to edit {:?}: only \"Rustle test\" without --force",
            pl.summary.name
        )
        .into())
    }
}

fn url_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn track_line(t: &Track) -> String {
    let artists: Vec<&str> = t.artists.iter().map(|a| a.name.as_str()).collect();
    let album = t.album.as_ref().map_or("-", |a| a.name.as_str());
    let secs = t.duration.map_or(0, |d| d.as_secs());
    format!(
        "{}  {} - {} [{}] {}:{:02}",
        t.id,
        artists.join(", "),
        t.title,
        album,
        secs / 60,
        secs % 60
    )
}

fn playlist_line(p: &PlaylistSummary) -> String {
    format!(
        "{}  {} (by {}, {} tracks)",
        p.id,
        p.name,
        p.owner.as_deref().unwrap_or("?"),
        p.track_count.map_or("?".to_owned(), |n| n.to_string())
    )
}

fn print_search(res: &SearchResults) {
    println!("tracks ({}):", res.tracks.len());
    for t in &res.tracks {
        println!("  {}", track_line(t));
    }
    println!("albums ({}):", res.albums.len());
    for a in &res.albums {
        let artists: Vec<&str> = a.artists.iter().map(|a| a.name.as_str()).collect();
        println!(
            "  {}  {} - {} ({:?})",
            a.id,
            artists.join(", "),
            a.name,
            a.year
        );
    }
    println!("artists ({}):", res.artists.len());
    for a in &res.artists {
        println!("  {}  {}", a.id, a.name);
    }
    println!("playlists ({}):", res.playlists.len());
    for p in &res.playlists {
        println!("  {}", playlist_line(p));
    }
}

fn print_playlist(pl: &Playlist) {
    println!("{}", playlist_line(&pl.summary));
    println!(
        "{} tracks fetched, {} entry ids:",
        pl.tracks.len(),
        pl.entry_ids.iter().flatten().count()
    );
    for (i, t) in pl.tracks.iter().enumerate() {
        match pl.entry_ids.get(i).cloned().flatten() {
            Some(e) => println!("  {}  entry {e}", track_line(t)),
            None => println!("  {}", track_line(t)),
        }
    }
}

fn album_line(a: &AlbumSummary) -> String {
    let artists: Vec<&str> = a.artists.iter().map(|a| a.name.as_str()).collect();
    let year = a.year.map_or("?".to_owned(), |y| y.to_string());
    format!("{}  {} - {} ({year})", a.id, artists.join(", "), a.name)
}

fn thumb_sizes(thumbs: &[ytm_core::model::Thumbnail]) -> String {
    let sizes: Vec<String> = thumbs
        .iter()
        .map(|t| format!("{}x{}", t.width, t.height))
        .collect();
    sizes.join(" ")
}

fn print_album(album: &Album) {
    println!("{}", album_line(&album.summary));
    println!("art: {}", thumb_sizes(&album.summary.thumbnails));
    println!("{} tracks:", album.tracks.len());
    for t in &album.tracks {
        println!("  {}", track_line(t));
    }
}

fn print_artist(artist: &Artist) {
    println!("{}  {}", artist.summary.id, artist.summary.name);
    println!("art: {}", thumb_sizes(&artist.summary.thumbnails));
    println!("top songs ({}):", artist.top_songs.len());
    for t in &artist.top_songs {
        println!("  {}", track_line(t));
    }
    println!("albums ({}):", artist.albums.len());
    for a in &artist.albums {
        println!("  {}", album_line(a));
    }
    println!("singles and EPs ({}):", artist.singles.len());
    for a in &artist.singles {
        println!("  {}", album_line(a));
    }
}

/// Saves one artist page and one album page, fetched without cookies.
async fn record_pages(dir: &Path, artist_id: &str, album_id: Option<&str>) -> AnyResult<()> {
    use rustypipe::model::AlbumType;

    let rp = rustypipe::client::RustyPipe::builder()
        .no_storage()
        .no_reporter()
        .no_botguard()
        .build()?;
    let q = rp.query().unauthenticated();
    let artist = q.music_artist(artist_id, false).await?;
    let album_id = match album_id {
        Some(id) => id.to_owned(),
        None => artist
            .albums
            .iter()
            .find(|a| a.album_type == AlbumType::Album)
            .map(|a| a.id.clone())
            .ok_or("artist page has no album; pass an album id")?,
    };
    let album = q.music_album(&album_id).await?;
    std::fs::write(
        dir.join("rp_artist.json"),
        serde_json::to_string_pretty(&artist)? + "\n",
    )?;
    std::fs::write(
        dir.join("rp_album.json"),
        serde_json::to_string_pretty(&album)? + "\n",
    )?;
    println!(
        "recorded artist {artist_id} ({} tracks, {} albums) and album {album_id} ({} tracks)",
        artist.tracks.len(),
        artist.albums.len(),
        album.tracks.len()
    );
    Ok(())
}

/// Saves raw, cookie-free rustypipe responses for the offline tests.
async fn record(dir: &Path, playlist_id: Option<&str>) -> AnyResult<()> {
    let rp = rustypipe::client::RustyPipe::builder()
        .no_storage()
        .no_reporter()
        .no_botguard()
        .build()?;
    let q = rp.query().unauthenticated();

    let query = "daft punk";
    let (tracks, albums, artists, playlists) = tokio::try_join!(
        q.music_search_tracks(query),
        q.music_search_albums(query),
        q.music_search_artists(query),
        q.music_search_playlists(query, false),
    )?;
    let playlist_id = match playlist_id {
        Some(id) => id.to_owned(),
        None => playlists
            .items
            .items
            .first()
            .map(|p| p.id.clone())
            .ok_or("search returned no playlist; pass a playlist id")?,
    };
    let search = serde_json::json!({
        "tracks": tracks.items.items,
        "albums": albums.items.items,
        "artists": artists.items.items,
        "playlists": playlists.items.items,
    });
    let mut playlist = q.music_playlist(&playlist_id).await?;
    playlist.tracks.visitor_data = None;
    playlist.tracks.ctoken = None;
    playlist.related_playlists = Default::default();

    std::fs::write(
        dir.join("rp_search.json"),
        serde_json::to_string_pretty(&search)? + "\n",
    )?;
    std::fs::write(
        dir.join("rp_playlist.json"),
        serde_json::to_string_pretty(&playlist)? + "\n",
    )?;
    println!(
        "recorded search ({} tracks) and playlist {playlist_id} ({} tracks)",
        search["tracks"].as_array().map_or(0, Vec::len),
        playlist.tracks.items.len()
    );
    Ok(())
}

/// Saves two raw `browse` pages of an owned playlist (after one typed
/// `music_playlist` call that checks the playlist is readable). Reads only.
async fn record_owned(dir: &Path, playlist_id: &str) -> AnyResult<()> {
    use rustypipe::client::ClientType;
    use serde_json::{Value, json};

    let cookies = cookies_from_env()?.ok_or("set RUSTLE_COOKIES_FILE")?;
    let rp = rustypipe::client::RustyPipe::builder()
        .no_storage()
        .no_reporter()
        .no_botguard()
        .build()?;
    let login = if cookies.contains('\t') {
        rp.user_auth_set_cookie_txt(&cookies).await
    } else {
        rp.user_auth_set_cookie(cookies.trim()).await
    };
    login.map_err(|e| format!("login: {e}"))?;
    let q = rp.query().authenticated();
    let vd = q.get_visitor_data(false).await?;
    let q = q.visitor_data(vd);

    let typed = q
        .music_playlist(playlist_id)
        .await
        .map_err(|e| format!("typed browse: {e}"))?;
    println!(
        "typed browse ok: {} tracks on page 1",
        typed.tracks.items.len()
    );

    let first: Value = serde_json::from_str(
        &q.raw(
            ClientType::DesktopMusic,
            "browse",
            &json!({ "browseId": format!("VL{playlist_id}") }),
        )
        .await
        .map_err(|e| format!("browse: {e}"))?,
    )?;
    let token = find_token(&first).ok_or("first page has no continuation token")?;
    let next: Value = serde_json::from_str(
        &q.raw(
            ClientType::DesktopMusic,
            "browse",
            &json!({ "continuation": token }),
        )
        .await
        .map_err(|e| format!("continuation: {e}"))?,
    )?;

    for (name, mut v) in [
        ("browse_owned_playlist.json", first),
        ("browse_owned_playlist_continuation.json", next),
    ] {
        trim(&mut v);
        std::fs::write(dir.join(name), serde_json::to_string_pretty(&v)? + "\n")?;
    }
    println!("recorded 2 browse pages of {playlist_id}");
    Ok(())
}

/// Saves one radio page, fetched without cookies.
async fn record_radio(dir: &Path, video_id: &str) -> AnyResult<()> {
    let rp = rustypipe::client::RustyPipe::builder()
        .no_storage()
        .no_reporter()
        .no_botguard()
        .build()?;
    let mut radio = rp
        .query()
        .unauthenticated()
        .music_radio_track(video_id)
        .await?;
    radio.visitor_data = None;
    radio.ctoken = None;
    std::fs::write(
        dir.join("rp_radio.json"),
        serde_json::to_string_pretty(&radio)? + "\n",
    )?;
    println!("recorded {} radio tracks for {video_id}", radio.items.len());
    Ok(())
}

/// Saves the web client's plain lyrics of one song, fetched without cookies.
async fn record_lyrics(dir: &Path, video_id: &str) -> AnyResult<()> {
    let rp = rustypipe::client::RustyPipe::builder()
        .no_storage()
        .no_reporter()
        .no_botguard()
        .build()?;
    let q = rp.query().unauthenticated();
    let lyrics_id = q
        .music_details(video_id)
        .await?
        .lyrics_id
        .ok_or("the song has no lyrics id")?;
    let lyrics = q.music_lyrics(&lyrics_id).await?;
    std::fs::write(
        dir.join("rp_lyrics_plain.json"),
        serde_json::to_string_pretty(&lyrics)? + "\n",
    )?;
    println!(
        "recorded plain lyrics {lyrics_id} for {video_id} ({} lines)",
        lyrics.body.lines().count()
    );
    Ok(())
}

fn find_token(v: &serde_json::Value) -> Option<String> {
    use serde_json::Value;
    match v {
        Value::Object(m) => m
            .get("continuationCommand")
            .and_then(|c| c.get("token"))
            .or_else(|| {
                m.get("nextContinuationData")
                    .and_then(|c| c.get("continuation"))
            })
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| m.values().find_map(find_token)),
        Value::Array(a) => a.iter().find_map(find_token),
        _ => None,
    }
}

/// Drops account and tracking data and keeps three rows (plus the
/// continuation item) of each track list.
fn trim(v: &mut serde_json::Value) {
    use serde_json::Value;
    const DROP: [&str; 8] = [
        "responseContext",
        "trackingParams",
        "header",
        "musicEditablePlaylistDetailHeaderRenderer",
        "musicResponsiveHeaderRenderer",
        "microformat",
        "frameworkUpdates",
        "clickTrackingParams",
    ];
    match v {
        Value::Object(m) => {
            for k in DROP {
                m.remove(k);
            }
            for (k, child) in m.iter_mut() {
                let track_list = (k == "contents" || k == "continuationItems")
                    && child.as_array().is_some_and(|items| {
                        items
                            .iter()
                            .any(|i| i.get("musicResponsiveListItemRenderer").is_some())
                    });
                if track_list && let Value::Array(items) = child {
                    let (rows, cont): (Vec<Value>, Vec<Value>) = std::mem::take(items)
                        .into_iter()
                        .partition(|i| i.get("continuationItemRenderer").is_none());
                    items.extend(rows.into_iter().take(3));
                    items.extend(cont);
                }
                trim(child);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(trim),
        _ => {}
    }
}
