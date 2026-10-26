//! Live smoke test for `RustyPipeBackend`.
//!
//! ```text
//! cargo run -p ytm-core --example spike -- search <query> [--json]
//! cargo run -p ytm-core --example spike -- stream <video_id>   # resolves twice: path, then cache
//! cargo run -p ytm-core --example spike -- playlist <playlist_id> [--json]
//! RUSTLE_COOKIES_FILE=cookies.txt cargo run -p ytm-core --example spike -- library
//! cargo run -p ytm-core --example spike -- record <fixtures_dir> [playlist_id]
//! ```
//!
//! `record` saves raw rustypipe responses (search "daft punk", one page per
//! type, and one public playlist; no cookies) as `rp_*.json`. Then run
//! `UPDATE_FIXTURES=1 cargo test -p ytm-core` to regenerate the model fixtures.

use std::path::Path;
use std::time::Instant;

use ytm_core::model::{Playlist, PlaylistSummary, SearchResults, Track};
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
    let args: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--json")
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
            let pl = RustyPipeBackend::new()?.playlist(id).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&pl)?);
            } else {
                print_playlist(&pl);
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
                 | library | record <dir> [playlist_id]"
            );
            std::process::exit(2);
        }
    }
    Ok(())
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
    println!("{} tracks fetched:", pl.tracks.len());
    for t in &pl.tracks {
        println!("  {}", track_line(t));
    }
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
