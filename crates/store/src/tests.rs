use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use model::{AlbumRef, ArtistRef, Playlist, PlaylistSummary, Thumbnail, Track, TrackId};

use super::{NameCount, Store};

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

fn artist(id: Option<&str>, name: &str) -> ArtistRef {
    ArtistRef {
        id: id.map(Into::into),
        name: name.into(),
    }
}

fn song(id: &str, artists: Vec<ArtistRef>, album: Option<(&str, &str)>) -> Track {
    Track {
        id: id.into(),
        title: id.into(),
        artists,
        album: album.map(|(id, name)| AlbumRef {
            id: id.into(),
            name: name.into(),
        }),
        duration: None,
        thumbnails: vec![],
    }
}

fn nc(id: Option<&str>, name: &str, count: u32) -> NameCount {
    NameCount {
        id: id.map(Into::into),
        name: name.into(),
        count,
    }
}

fn save(s: &Store, n: usize, tracks: Vec<Track>) {
    s.save_playlist(&Playlist {
        summary: summary(n),
        tracks,
        entry_ids: Vec::new(),
    })
    .unwrap();
}

#[test]
fn entry_ids_round_trip() {
    let s = store();
    let mut pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..3),
        entry_ids: vec![Some("SET0".into()), None, Some("SET2".into())],
    };
    s.save_playlist(&pl).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));

    pl.entry_ids = vec![Some("A".into()), Some("B".into()), Some("C".into())];
    s.save_playlist(&pl).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));

    // All NULL reads back as "unknown".
    pl.entry_ids = vec![None, None, None];
    s.save_playlist(&pl).unwrap();
    assert!(s.playlist("PL1").unwrap().unwrap().entry_ids.is_empty());

    pl.entry_ids = Vec::new();
    s.save_playlist(&pl).unwrap();
    assert_eq!(s.playlist("PL1").unwrap(), Some(pl.clone()));

    // A short list is padded with None instead of panicking.
    pl.entry_ids = vec![Some("A".into())];
    s.save_playlist(&pl).unwrap();
    assert_eq!(
        s.playlist("PL1").unwrap().unwrap().entry_ids,
        vec![Some("A".into()), None, None]
    );
}

#[test]
fn main_playlist_set_and_replace() {
    let s = store();
    assert_eq!(s.main_playlist().unwrap(), None);
    s.set_main_playlist("PL1").unwrap();
    assert_eq!(s.main_playlist().unwrap().as_deref(), Some("PL1"));
    s.set_main_playlist("PL2").unwrap();
    assert_eq!(s.main_playlist().unwrap().as_deref(), Some("PL2"));
}

#[test]
fn last_played_returns_latest_play() {
    let s = store();
    s.record_play("a", 100, 30_000).unwrap();
    s.record_play("a", 300, 30_000).unwrap();
    s.record_play("a", 200, 30_000).unwrap();
    // Plays of uncached ids are fine, and huge durations do not overflow.
    s.record_play("b", 50, u64::MAX).unwrap();
    let got = s
        .last_played(&["a".into(), "b".into(), "never".into(), "a".into()])
        .unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got["a"], 300);
    assert_eq!(got["b"], 50);
    assert!(s.last_played(&[]).unwrap().is_empty());
}

#[test]
fn not_heard_since_filters_main_playlist() {
    let s = store();
    let ts = vec![
        song("old", vec![], None),
        song("never", vec![], None),
        song("recent", vec![], None),
        song("edge", vec![], None),
        song("old", vec![], None), // duplicate entry
        song("mixed", vec![], None),
    ];
    save(&s, 1, ts);
    save(&s, 2, vec![song("other", vec![], None)]);
    s.record_play("old", 500, 1).unwrap();
    s.record_play("recent", 1_500, 1).unwrap();
    s.record_play("edge", 1_000, 1).unwrap();
    s.record_play("mixed", 10, 1).unwrap();
    s.record_play("mixed", 2_000, 1).unwrap();

    // No main playlist yet.
    assert!(s.not_heard_since(1_000).unwrap().is_empty());

    s.set_main_playlist("PL1").unwrap();
    // A play at exactly the cutoff counts as heard; duplicates appear once.
    assert_eq!(s.not_heard_since(1_000).unwrap(), vec!["old", "never"]);
    assert_eq!(
        s.not_heard_since(1_001).unwrap(),
        vec!["old", "never", "edge"]
    );
    assert_eq!(
        s.not_heard_since(i64::MAX).unwrap(),
        vec!["old", "never", "recent", "edge", "mixed"]
    );

    // Main playlist whose tracks are not cached.
    s.set_main_playlist("PL9").unwrap();
    assert!(s.not_heard_since(1_000).unwrap().is_empty());
}

#[test]
fn art_colors_round_trip_and_overwrite() {
    let s = store();
    s.set_art_colors(&[("a".into(), [1, 2, 3]), ("b".into(), [255, 0, 128])])
        .unwrap();
    s.set_art_colors(&[("a".into(), [9, 8, 7])]).unwrap();
    let got = s
        .art_colors(&["a".into(), "b".into(), "none".into()])
        .unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got["a"], [9, 8, 7]);
    assert_eq!(got["b"], [255, 0, 128]);
    s.set_art_colors(&[]).unwrap();
}

#[test]
fn artist_counts_sort_and_featured_artists() {
    let s = store();
    let sj = || artist(Some("UCsj"), "Saint Juniper");
    let ts = vec![
        song("t1", vec![sj(), artist(None, "Feat Guest")], None),
        song("t2", vec![sj()], None),
        // Same channel under a different display name stays one group.
        song(
            "t3",
            vec![artist(Some("UCsj"), "Saint Juniper (Official)")],
            None,
        ),
        song("t4", vec![artist(Some("UCb"), "beta")], None),
        song("t5", vec![artist(Some("UCa"), "Alpha")], None),
        song("t6", vec![artist(Some("UCz"), "Zed")], None),
        // An unlinked credit with the same name as a channel stays separate.
        song("t7", vec![artist(None, "Alpha")], None),
        song("t8", vec![], None),
    ];
    let mut with_dup = ts.clone();
    with_dup.push(ts[1].clone());
    save(&s, 1, with_dup);
    save(&s, 2, vec![song("x", vec![sj()], None)]);

    assert_eq!(
        s.artist_counts("PL1").unwrap(),
        vec![
            nc(Some("UCsj"), "Saint Juniper", 3),
            nc(None, "Alpha", 1),
            nc(Some("UCa"), "Alpha", 1),
            nc(Some("UCb"), "beta", 1),
            nc(None, "Feat Guest", 1),
            nc(Some("UCz"), "Zed", 1),
        ]
    );
    assert!(s.artist_counts("missing").unwrap().is_empty());
}

#[test]
fn album_counts_sort_and_tie_break() {
    let s = store();
    let ts = vec![
        song("t1", vec![], Some(("MPa", "Night Songs"))),
        song("t2", vec![], Some(("MPa", "Night Songs"))),
        song("t3", vec![], Some(("MPb", "apple"))),
        song("t4", vec![], Some(("MPc", "Banana"))),
        song("t5", vec![], None),
        song("t1", vec![], Some(("MPa", "Night Songs"))), // duplicate entry
    ];
    save(&s, 1, ts);
    assert_eq!(
        s.album_counts("PL1").unwrap(),
        vec![
            nc(Some("MPa"), "Night Songs", 2),
            nc(Some("MPb"), "apple", 1),
            nc(Some("MPc"), "Banana", 1),
        ]
    );
    assert!(s.album_counts("missing").unwrap().is_empty());
}

#[test]
fn fetched_at_set_and_replace() {
    let s = store();
    assert_eq!(s.fetched_at("library").unwrap(), None);
    s.set_fetched_at("library", 100).unwrap();
    s.set_fetched_at("playlist:PL1", 200).unwrap();
    s.set_fetched_at("library", 300).unwrap();
    assert_eq!(s.fetched_at("library").unwrap(), Some(300));
    assert_eq!(s.fetched_at("playlist:PL1").unwrap(), Some(200));
    assert_eq!(s.fetched_at("liked").unwrap(), None);
}

#[test]
fn version_1_database_upgrades() {
    let dir = TempDir::new();
    let path = dir.0.join("cache.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(super::MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO tracks (id, title, artists, thumbnails)
                 VALUES ('t1', 'One', '[{\"id\":\"UCa\",\"name\":\"A\"}]', '[]');
             INSERT INTO playlists (id, name, thumbnails, has_tracks)
                 VALUES ('PL1', 'Main', '[]', 1);
             INSERT INTO playlist_tracks (playlist_id, position, track_id)
                 VALUES ('PL1', 0, 't1');",
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let version: i64 = s
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, super::MIGRATIONS.len() as i64);
    let pl = s.playlist("PL1").unwrap().unwrap();
    assert_eq!(ids(&pl.tracks), vec!["t1"]);
    assert!(pl.entry_ids.is_empty());

    s.set_main_playlist("PL1").unwrap();
    s.record_play("t1", 10, 1_000).unwrap();
    s.set_art_colors(&[("t1".into(), [1, 2, 3])]).unwrap();
    s.set_fetched_at("library", 5).unwrap();
    assert_eq!(s.not_heard_since(20).unwrap(), vec!["t1"]);
    assert_eq!(
        s.artist_counts("PL1").unwrap(),
        vec![nc(Some("UCa"), "A", 1)]
    );
    drop(s);

    // Reopening the upgraded file keeps everything.
    let s = Store::open(&path).unwrap();
    assert_eq!(s.main_playlist().unwrap().as_deref(), Some("PL1"));
    assert_eq!(s.last_played(&["t1".into()]).unwrap()["t1"], 10);
    assert_eq!(s.fetched_at("library").unwrap(), Some(5));
}

#[test]
fn queue_extras_round_trip() {
    let s = store();
    assert_eq!(s.queue_extras().unwrap(), (None, vec![]));

    let ts = tracks(0..4);
    s.upsert_tracks(&ts).unwrap();
    s.save_queue(&ids(&ts), Some(1)).unwrap();
    s.save_queue_extras(Some("Everything \u{b7} 2"), &[false, false, true, true])
        .unwrap();
    assert_eq!(
        s.queue_extras().unwrap(),
        (
            Some("Everything \u{b7} 2".into()),
            vec![false, false, true, true]
        )
    );
    // The queue itself is unchanged.
    assert_eq!(s.load_queue().unwrap(), (ts.clone(), Some(1)));

    // A new queue starts unmarked; extras replace the old ones.
    s.save_queue(&ids(&ts[..3]), Some(0)).unwrap();
    assert_eq!(s.queue_extras().unwrap().1, vec![false; 3]);
    s.save_queue_extras(None, &[true, false]).unwrap();
    assert_eq!(s.queue_extras().unwrap(), (None, vec![true, false, false]));

    // Flags past the end are dropped.
    s.save_queue_extras(Some("x"), &[false, false, true, true, true])
        .unwrap();
    assert_eq!(
        s.queue_extras().unwrap(),
        (Some("x".into()), vec![false, false, true])
    );

    // Extras before any save_queue keep the label.
    let s = store();
    s.save_queue_extras(Some("y"), &[true]).unwrap();
    assert_eq!(s.queue_extras().unwrap(), (Some("y".into()), vec![]));
    assert_eq!(s.load_queue().unwrap(), (vec![], None));
}

#[test]
fn queue_extras_skip_entries_load_queue_skips() {
    let s = store();
    let ts = tracks(0..3);
    s.upsert_tracks(&ts).unwrap();
    let q = vec![
        ts[0].id.clone(),
        "unknown".into(),
        ts[1].id.clone(),
        ts[2].id.clone(),
    ];
    s.save_queue(&q, Some(0)).unwrap();
    s.save_queue_extras(Some("l"), &[false, true, false, true])
        .unwrap();
    let (loaded, _) = s.load_queue().unwrap();
    let (_, radio) = s.queue_extras().unwrap();
    assert_eq!(radio.len(), loaded.len());
    assert_eq!(radio, vec![false, false, true]);
}

#[test]
fn queue_extras_survive_reopen() {
    let dir = TempDir::new();
    let path = dir.0.join("cache.sqlite");
    let ts = tracks(0..3);
    {
        let s = Store::open(&path).unwrap();
        s.upsert_tracks(&ts).unwrap();
        s.save_queue(&ids(&ts), Some(2)).unwrap();
        s.save_queue_extras(Some("Saint Juniper \u{b7} 2"), &[false, false, true])
            .unwrap();
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.load_queue().unwrap(), (ts, Some(2)));
    assert_eq!(
        s.queue_extras().unwrap(),
        (
            Some("Saint Juniper \u{b7} 2".into()),
            vec![false, false, true]
        )
    );
}

#[test]
fn version_2_database_upgrades() {
    let dir = TempDir::new();
    let path = dir.0.join("cache.sqlite");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(super::MIGRATIONS[0]).unwrap();
        conn.execute_batch(super::MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute_batch(
            "INSERT INTO tracks (id, title, artists, thumbnails)
                 VALUES ('t1', 'One', '[]', '[]'), ('t2', 'Two', '[]', '[]');
             INSERT INTO queue (position, track_id) VALUES (0, 't1'), (1, 't2');
             INSERT INTO queue_state (id, current) VALUES (0, 1);
             INSERT INTO settings (key, value) VALUES ('autoplay', 'false');",
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let version: i64 = s
        .conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 3);
    // Old rows survive: the queue, its current index, and settings.
    let (q, current) = s.load_queue().unwrap();
    assert_eq!(ids(&q), vec!["t1", "t2"]);
    assert_eq!(current, Some(1));
    assert_eq!(s.setting("autoplay").unwrap().as_deref(), Some("false"));
    // Old rows read as unmarked with no label.
    assert_eq!(s.queue_extras().unwrap(), (None, vec![false, false]));

    s.save_queue_extras(Some("Two"), &[false, true]).unwrap();
    drop(s);
    let s = Store::open(&path).unwrap();
    assert_eq!(
        s.queue_extras().unwrap(),
        (Some("Two".into()), vec![false, true])
    );
    assert_eq!(s.load_queue().unwrap().1, Some(1));
}

#[test]
fn queries_on_5000_tracks_and_50000_plays_are_fast() {
    let s = store();
    let pl = Playlist {
        summary: summary(1),
        tracks: tracks(0..5000),
        entry_ids: (0..5000).map(|n| Some(format!("SET{n}"))).collect(),
    };
    s.save_playlist(&pl).unwrap();
    s.set_main_playlist("PL1").unwrap();
    {
        let tx = s.conn.unchecked_transaction().unwrap();
        let mut insert = tx
            .prepare("INSERT INTO plays (track_id, played_at, played_ms) VALUES (?1, ?2, ?3)")
            .unwrap();
        // Ten plays per track for the first 4,000 tracks; the rest never played.
        for n in 0..50_000usize {
            let id = &pl.tracks[n % 4000].id;
            insert
                .execute(rusqlite::params![id, n as i64, 180_000])
                .unwrap();
        }
        drop(insert);
        tx.commit().unwrap();
    }
    let all = ids(&pl.tracks);
    let colors: Vec<_> = all.iter().map(|id| (id.clone(), [1, 2, 3])).collect();

    let mut timings = Vec::new();
    let mut time = |name: &'static str, f: &mut dyn FnMut()| {
        let start = Instant::now();
        f();
        timings.push((name, start.elapsed()));
    };
    time("playlist", &mut || {
        assert_eq!(s.playlist("PL1").unwrap().unwrap().entry_ids.len(), 5000)
    });
    time("last_played", &mut || {
        assert_eq!(s.last_played(&all).unwrap().len(), 4000)
    });
    time("not_heard_since", &mut || {
        // Track k's last play is 46,000 + k, so cutoff 48,000 leaves
        // tracks 0..2000 old plus 1,000 never played.
        assert_eq!(s.not_heard_since(48_000).unwrap().len(), 3000)
    });
    time("set_art_colors", &mut || s.set_art_colors(&colors).unwrap());
    time("art_colors", &mut || {
        assert_eq!(s.art_colors(&all).unwrap().len(), 5000)
    });
    time("artist_counts", &mut || {
        assert_eq!(s.artist_counts("PL1").unwrap().len(), 5001)
    });
    time("album_counts", &mut || {
        assert_eq!(s.album_counts("PL1").unwrap().len(), 3333)
    });
    time("record_play", &mut || {
        s.record_play("trk00000001", 1, 1).unwrap()
    });
    time("fetched_at", &mut || {
        s.set_fetched_at("playlist:PL1", 1).unwrap();
        s.fetched_at("playlist:PL1").unwrap();
    });
    eprintln!("5,000 tracks, 50,000 plays: {timings:?}");
    if !cfg!(debug_assertions) {
        for (name, t) in timings {
            assert!(t < Duration::from_millis(20), "{name} took {t:?}");
        }
    }
}

/// Two connections opening a fresh file at once must not both migrate.
/// CI once failed with "table tracks already exists" from this race.
#[test]
fn concurrent_opens_migrate_once() {
    for _ in 0..20 {
        let dir = TempDir::new();
        let path = dir.0.join("race.db");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (path, barrier) = (path.clone(), std::sync::Arc::clone(&barrier));
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open(&path).map(|_| ()).map_err(|e| e.to_string())
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().unwrap();
        }
        assert!(
            Store::open(&path)
                .unwrap()
                .main_playlist()
                .unwrap()
                .is_none()
        );
    }
}
