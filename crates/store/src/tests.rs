use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use model::{AlbumRef, ArtistRef, Playlist, PlaylistSummary, Thumbnail, Track, TrackId};

use super::Store;

fn track(n: usize) -> Track {
    Track {
        id: format!("trk{n:08}"),
        title: format!("Song {n} – “quoted” ünïcode"),
        artists: vec![
            ArtistRef {
                id: Some(format!("UC{n}")),
                name: format!("Artist {n}"),
            },
            ArtistRef {
                id: None,
                name: "Unlinked feat.".into(),
            },
        ],
        album: (!n.is_multiple_of(3)).then(|| AlbumRef {
            id: format!("MPRE{n}"),
            name: format!("Album {n}"),
        }),
        duration: (!n.is_multiple_of(5)).then(|| Duration::from_millis(180_000 + n as u64)),
        thumbnails: [60, 120, 544]
            .into_iter()
            .map(|s| Thumbnail {
                url: format!("https://lh3.googleusercontent.com/{n}=w{s}-h{s}"),
                width: s,
                height: s,
            })
            .collect(),
    }
}

fn tracks(range: std::ops::Range<usize>) -> Vec<Track> {
    range.map(track).collect()
}

fn summary(n: usize) -> PlaylistSummary {
    PlaylistSummary {
        id: format!("PL{n}"),
        name: format!("Playlist {n}"),
        owner: n.is_multiple_of(2).then(|| "Me".into()),
        track_count: (!n.is_multiple_of(3)).then_some(n as u64 * 10),
        thumbnails: vec![Thumbnail {
            url: format!("https://i.ytimg.com/{n}.jpg"),
            width: 226,
            height: 226,
        }],
    }
}

fn ids(tracks: &[Track]) -> Vec<TrackId> {
    tracks.iter().map(|t| t.id.clone()).collect()
}

fn store() -> Store {
    Store::open_in_memory().unwrap()
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rustle-store-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn track_round_trip_and_upsert_updates() {
    let s = store();
    let mut ts = tracks(0..10);
    s.upsert_tracks(&ts).unwrap();
    for t in &ts {
        assert_eq!(s.track(&t.id).unwrap().as_ref(), Some(t));
    }
    assert_eq!(s.track("missing").unwrap(), None);

    ts[3].title = "Renamed".into();
    ts[3].album = None;
    ts[3].artists.clear();
    ts[3].thumbnails.clear();
    ts[3].duration = None;
    s.upsert_tracks(&ts[3..4]).unwrap();
    assert_eq!(s.track(&ts[3].id).unwrap(), Some(ts[3].clone()));
    assert_eq!(s.track(&ts[4].id).unwrap(), Some(ts[4].clone()));
}

#[test]
fn upsert_with_duplicates_in_one_batch_keeps_last() {
    let s = store();
    let a = track(1);
    let mut b = a.clone();
    b.title = "Second".into();
    s.upsert_tracks(&[a, b.clone()]).unwrap();
    assert_eq!(s.track(&b.id).unwrap(), Some(b));
}

#[test]
fn upsert_does_not_break_references() {
    let s = store();
    let pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..5),
        entry_ids: Vec::new(),
    };
    s.save_playlist(&pl).unwrap();
    s.set_liked(&pl.tracks[..2]).unwrap();

    let mut changed = pl.tracks[1].clone();
    changed.title = "New title".into();
    s.upsert_tracks(std::slice::from_ref(&changed)).unwrap();

    let got = s.playlist("PL1").unwrap().unwrap();
    assert_eq!(got.tracks.len(), 5);
    assert_eq!(got.tracks[1], changed);
    assert_eq!(s.liked().unwrap()[1], changed);
}

#[test]
fn library_playlists_replace_and_order() {
    let s = store();
    assert!(s.library_playlists().unwrap().is_empty());

    let first = vec![summary(1), summary(2), summary(3)];
    s.set_library_playlists(&first).unwrap();
    assert_eq!(s.library_playlists().unwrap(), first);

    let mut second = vec![summary(3), summary(4), summary(1)];
    second[0].name = "Renamed".into();
    s.set_library_playlists(&second).unwrap();
    assert_eq!(s.library_playlists().unwrap(), second);

    s.set_library_playlists(&[]).unwrap();
    assert!(s.library_playlists().unwrap().is_empty());
}

#[test]
fn playlist_round_trip_and_replace() {
    let s = store();
    assert_eq!(s.playlist("PL1").unwrap(), None);

    let mut pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..20),
        entry_ids: Vec::new(),
    };
    // Duplicate entries are common in YouTube Music playlists.
    pl.tracks.push(pl.tracks[4].clone());
    s.save_playlist(&pl).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));

    // Replace: shorter, reordered, with a new track and a new summary.
    let mut replaced = Playlist {
        summary: summary(1),
        tracks: vec![track(7), track(100), track(2)],
        entry_ids: Vec::new(),
    };
    replaced.summary.name = "Renamed".into();
    replaced.summary.track_count = Some(3);
    s.save_playlist(&replaced).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(replaced.clone()));

    // Empty playlist is still cached, distinct from "not cached".
    let empty = Playlist {
        summary: summary(2),
        tracks: vec![],
        entry_ids: Vec::new(),
    };
    s.save_playlist(&empty).unwrap();
    assert_eq!(s.playlist("PL2").unwrap(), Some(empty));
    assert_eq!(s.playlist("PL1").unwrap(), Some(replaced));
}

#[test]
fn library_summary_alone_is_not_a_cached_playlist() {
    let s = store();
    s.set_library_playlists(&[summary(1)]).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), None);
}

#[test]
fn save_playlist_then_library_keeps_tracks() {
    let s = store();
    let pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..3),
        entry_ids: Vec::new(),
    };
    s.save_playlist(&pl).unwrap();
    s.set_library_playlists(&[summary(2), summary(1)]).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));

    // Removing it from the library keeps its cached tracks.
    s.set_library_playlists(&[summary(2)]).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl));
    assert_eq!(s.library_playlists().unwrap(), vec![summary(2)]);
}

#[test]
fn library_then_save_playlist_keeps_library_order() {
    let s = store();
    let lib = vec![summary(1), summary(2), summary(3)];
    s.set_library_playlists(&lib).unwrap();

    let mut pl = Playlist {
        summary: summary(2),
        tracks: tracks(0..4),
        entry_ids: Vec::new(),
    };
    pl.summary.name = "Fresh name".into();
    s.save_playlist(&pl).unwrap();

    let got = s.library_playlists().unwrap();
    assert_eq!(got, vec![summary(1), pl.summary.clone(), summary(3)]);
    assert_eq!(s.playlist("PL2").unwrap(), Some(pl));
}

#[test]
fn liked_round_trip_and_replace() {
    let s = store();
    assert!(s.liked().unwrap().is_empty());

    let first = tracks(0..50);
    s.set_liked(&first).unwrap();
    assert_eq!(s.liked().unwrap(), first);

    let second: Vec<Track> = tracks(40..45)
        .into_iter()
        .rev()
        .chain(tracks(200..202))
        .collect();
    s.set_liked(&second).unwrap();
    assert_eq!(s.liked().unwrap(), second);

    s.set_liked(&[]).unwrap();
    assert!(s.liked().unwrap().is_empty());
    // Tracks stay cached after leaving the liked list.
    assert_eq!(s.track(&first[0].id).unwrap(), Some(first[0].clone()));
}

#[test]
fn queue_round_trip() {
    let s = store();
    assert_eq!(s.load_queue().unwrap(), (vec![], None));

    let ts = tracks(0..10);
    s.upsert_tracks(&ts).unwrap();
    s.save_queue(&ids(&ts), Some(4)).unwrap();
    assert_eq!(s.load_queue().unwrap(), (ts.clone(), Some(4)));

    s.save_queue(&ids(&ts[..3]), None).unwrap();
    assert_eq!(s.load_queue().unwrap(), (ts[..3].to_vec(), None));

    // Out-of-range index is stored as None.
    s.save_queue(&ids(&ts[..3]), Some(3)).unwrap();
    assert_eq!(s.load_queue().unwrap(), (ts[..3].to_vec(), None));

    s.save_queue(&[], Some(0)).unwrap();
    assert_eq!(s.load_queue().unwrap(), (vec![], None));
}

#[test]
fn queue_allows_duplicate_ids() {
    let s = store();
    let ts = tracks(0..2);
    s.upsert_tracks(&ts).unwrap();
    let q = vec![ts[0].id.clone(), ts[1].id.clone(), ts[0].id.clone()];
    s.save_queue(&q, Some(2)).unwrap();
    assert_eq!(
        s.load_queue().unwrap(),
        (vec![ts[0].clone(), ts[1].clone(), ts[0].clone()], Some(2))
    );
}

#[test]
fn queue_skips_unknown_tracks_and_keeps_current() {
    let s = store();
    let known = tracks(0..6);
    s.upsert_tracks(&known).unwrap();
    let k = |i: usize| known[i].id.clone();
    let unknown = |n: usize| format!("unknown{n}");

    // Missing tracks before the current one shift the index down.
    s.save_queue(&[unknown(0), k(0), unknown(1), k(1), k(2)], Some(3))
        .unwrap();
    let (q, cur) = s.load_queue().unwrap();
    assert_eq!(q, known[..3].to_vec());
    assert_eq!(cur, Some(1));
    assert_eq!(q[cur.unwrap()], known[1]);

    // Missing tracks after the current one leave it alone.
    s.save_queue(&[k(0), k(1), unknown(0), k(2), unknown(1)], Some(1))
        .unwrap();
    let (q, cur) = s.load_queue().unwrap();
    assert_eq!(q, known[..3].to_vec());
    assert_eq!(cur, Some(1));

    // Current track missing: point at the next surviving track.
    s.save_queue(&[k(0), k(1), unknown(0), k(2), k(3)], Some(2))
        .unwrap();
    let (q, cur) = s.load_queue().unwrap();
    assert_eq!(q, known[..4].to_vec());
    assert_eq!(cur, Some(2));
    assert_eq!(q[2], known[2]);

    // Current track missing and nothing follows: point at the last track.
    s.save_queue(&[k(0), k(1), unknown(0), unknown(1)], Some(3))
        .unwrap();
    let (q, cur) = s.load_queue().unwrap();
    assert_eq!(q, known[..2].to_vec());
    assert_eq!(cur, Some(1));

    // Everything missing.
    s.save_queue(&[unknown(0), unknown(1)], Some(1)).unwrap();
    assert_eq!(s.load_queue().unwrap(), (vec![], None));

    // Queue ids cached later become loadable.
    let late = track(99);
    s.save_queue(std::slice::from_ref(&late.id), Some(0))
        .unwrap();
    assert_eq!(s.load_queue().unwrap(), (vec![], None));
    s.upsert_tracks(std::slice::from_ref(&late)).unwrap();
    assert_eq!(s.load_queue().unwrap(), (vec![late], Some(0)));
}

#[test]
fn file_reopen_keeps_data_and_reruns_migrations_safely() {
    let dir = TempDir::new();
    let path = dir.0.join("cache.sqlite");
    let pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..10),
        entry_ids: Vec::new(),
    };
    {
        let s = Store::open(&path).unwrap();
        s.save_playlist(&pl).unwrap();
        s.set_library_playlists(&[summary(1)]).unwrap();
        s.set_liked(&pl.tracks[..3]).unwrap();
        s.save_queue(&ids(&pl.tracks), Some(5)).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }
    for _ in 0..2 {
        let s = Store::open(&path).unwrap();
        let version: i64 = s
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, super::MIGRATIONS.len() as i64);
        let fk: bool = s
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert!(fk);
        assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));
        assert_eq!(s.library_playlists().unwrap(), vec![summary(1)]);
        assert_eq!(s.liked().unwrap(), pl.tracks[..3].to_vec());
        assert_eq!(s.load_queue().unwrap(), (pl.tracks.clone(), Some(5)));
    }
}

#[test]
fn store_works_from_another_thread() {
    let s = store();
    let handle = std::thread::spawn(move || {
        s.upsert_tracks(&[track(1)]).unwrap();
        s
    });
    let s = handle.join().unwrap();
    assert_eq!(s.track(&track(1).id).unwrap(), Some(track(1)));
}

#[test]
fn save_5000_track_playlist_is_fast() {
    let s = store();
    let pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..5000),
        entry_ids: Vec::new(),
    };
    let start = Instant::now();
    s.save_playlist(&pl).unwrap();
    let save = start.elapsed();
    let start = Instant::now();
    let loaded = s.playlist("PL1").unwrap().unwrap();
    let load = start.elapsed();
    assert_eq!(loaded, pl);
    eprintln!("5,000-track playlist: save {save:?}, load {load:?}");
    if !cfg!(debug_assertions) {
        assert!(save < Duration::from_secs(1), "save took {save:?}");
    }
}
