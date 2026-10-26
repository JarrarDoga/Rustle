//! Plays a URL and prints the player state once per second.
//!
//! ```text
//! cargo run -p player --example play -- <url> [--codec opus|aac] [--secs N]
//!     [--volume V] [--len BYTES] [--dur SECS] [--loudness DB]
//!     [--seek AT=TO] [--pause AT=FOR]
//! ```
//!
//! `--len` and `--dur` default to the `clen=` and `dur=` query parameters of
//! a googlevideo URL. `--seek 2=10` seeks to 10 s at 2 s of wall time;
//! `--pause 3=1` pauses at 3 s for 1 s. The run ends after `--secs`
//! (default 5) or when the track ends.

use std::time::{Duration, Instant};

use model::{AudioCodec, StreamSource};
use player::{Player, PlayerEvent};

struct Args {
    url: String,
    codec: AudioCodec,
    secs: f64,
    volume: f32,
    len: Option<u64>,
    dur: Option<f64>,
    loudness: Option<f32>,
    seek: Option<(f64, f64)>,
    pause: Option<(f64, f64)>,
}

fn pair(v: &str) -> (f64, f64) {
    let (a, b) = v.split_once('=').expect("expected AT=VALUE");
    (a.parse().expect("number"), b.parse().expect("number"))
}

fn parse() -> Args {
    let mut it = std::env::args().skip(1);
    let mut args = Args {
        url: String::new(),
        codec: AudioCodec::Opus,
        secs: 5.0,
        volume: 0.1,
        len: None,
        dur: None,
        loudness: None,
        seek: None,
        pause: None,
    };
    while let Some(a) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "--codec" => {
                args.codec = match val().as_str() {
                    "aac" => AudioCodec::Aac,
                    _ => AudioCodec::Opus,
                }
            }
            "--secs" => args.secs = val().parse().expect("--secs"),
            "--volume" => args.volume = val().parse().expect("--volume"),
            "--len" => args.len = Some(val().parse().expect("--len")),
            "--dur" => args.dur = Some(val().parse().expect("--dur")),
            "--loudness" => args.loudness = Some(val().parse().expect("--loudness")),
            "--seek" => args.seek = Some(pair(&val())),
            "--pause" => args.pause = Some(pair(&val())),
            _ if args.url.is_empty() => args.url = a.clone(),
            _ => panic!("unknown argument {a}"),
        }
    }
    assert!(!args.url.is_empty(), "usage: play <url> [options]");
    args
}

fn query<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    q.split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
}

fn main() {
    let args = parse();
    let len = args
        .len
        .or_else(|| query(&args.url, "clen")?.parse().ok())
        .unwrap_or(0);
    let dur = args
        .dur
        .or_else(|| query(&args.url, "dur")?.parse().ok())
        .map(Duration::from_secs_f64);

    let player = Player::new().expect("audio output");
    let events = player.events();
    player.set_volume(args.volume);
    player.load(StreamSource {
        track_id: "example".into(),
        url: args.url.clone(),
        codec: args.codec,
        bitrate_bps: 0,
        content_length: len,
        duration: dur,
        loudness_db: args.loudness,
    });

    let start = Instant::now();
    let mut next_print = 0.0;
    let mut seek = args.seek;
    let mut pause = args.pause;
    let mut resume_at = None;
    loop {
        let t = start.elapsed().as_secs_f64();
        if t >= args.secs {
            break;
        }
        while let Ok(ev) = events.try_recv() {
            println!("[{t:5.2}] event {ev:?}");
            if let PlayerEvent::TrackEnded { .. } = ev {
                print_state(&player, t);
                return;
            }
        }
        if let Some((at, to)) = seek
            && t >= at
        {
            println!("[{t:5.2}] seek to {to} s");
            player.seek(Duration::from_secs_f64(to));
            seek = None;
        }
        if let Some((at, len)) = pause
            && t >= at
        {
            println!("[{t:5.2}] pause for {len} s");
            player.pause();
            resume_at = Some(t + len);
            pause = None;
        }
        if let Some(r) = resume_at
            && t >= r
        {
            println!("[{t:5.2}] resume");
            player.play();
            resume_at = None;
        }
        if t >= next_print {
            print_state(&player, t);
            next_print += 1.0;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    print_state(&player, start.elapsed().as_secs_f64());
}

fn print_state(player: &Player, t: f64) {
    let s = player.state();
    println!(
        "[{t:5.2}] {:?} pos {:6.2} s / {} vol {:.2}",
        s.status,
        s.position.as_secs_f64(),
        s.duration
            .map_or("?".to_string(), |d| format!("{:.2} s", d.as_secs_f64())),
        s.volume
    );
}
