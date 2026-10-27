//! Local SQLite cache: library, metadata, queue. The UI renders from here so
//! the library opens instantly, even offline.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

use model::{AlbumRef, Playlist, PlaylistSummary, Track, TrackId};
use rusqlite::types::Type;
use rusqlite::{Connection, OptionalExtension, Row, params};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Schema migrations, applied in order. `PRAGMA user_version` records how many
/// have run, so each one runs exactly once per database file. Append only.
///
/// Artists and thumbnails are stored as JSON text on the owning row instead of
/// in child tables: they are always read together with their track or
/// playlist and never queried on their own, so one column keeps a 5,000-track
/// save to one row per track instead of five or six.
const MIGRATIONS: &[&str] = &[
    "
    CREATE TABLE tracks (
        id          TEXT PRIMARY KEY NOT NULL,
        title       TEXT NOT NULL,
        artists     TEXT NOT NULL,  -- JSON [ArtistRef]
        album_id    TEXT,           -- NULL together with album_name
        album_name  TEXT,
        duration_ms INTEGER,        -- whole milliseconds; sub-ms precision is dropped
        thumbnails  TEXT NOT NULL   -- JSON [Thumbnail], smallest first
    ) STRICT;

    CREATE TABLE playlists (
        id           TEXT PRIMARY KEY NOT NULL,
        name         TEXT NOT NULL,
        owner        TEXT,
        track_count  INTEGER,
        thumbnails   TEXT NOT NULL,             -- JSON [Thumbnail]
        in_library   INTEGER NOT NULL DEFAULT 0,
        library_pos  INTEGER,                   -- order in the library list; NULL if not in it
        has_tracks   INTEGER NOT NULL DEFAULT 0 -- 1 once save_playlist cached the track order
    ) STRICT;
    CREATE INDEX playlists_library ON playlists (library_pos) WHERE in_library = 1;

    CREATE TABLE playlist_tracks (
        playlist_id TEXT NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        track_id    TEXT NOT NULL REFERENCES tracks (id),
        PRIMARY KEY (playlist_id, position)
    ) STRICT, WITHOUT ROWID;
    CREATE INDEX playlist_tracks_track ON playlist_tracks (track_id);

    CREATE TABLE liked (
        position INTEGER PRIMARY KEY NOT NULL,
        track_id TEXT NOT NULL REFERENCES tracks (id)
    ) STRICT;
    CREATE INDEX liked_track ON liked (track_id);

    -- No foreign key: save_queue takes bare ids, which may not be cached yet.
    CREATE TABLE queue (
        position INTEGER PRIMARY KEY NOT NULL,
        track_id TEXT NOT NULL
    ) STRICT;

    CREATE TABLE queue_state (
        id      INTEGER PRIMARY KEY CHECK (id = 0),
        current INTEGER
    ) STRICT;
",
    "
    -- YouTube's setVideoId for this entry; NULL when the source gave none.
    ALTER TABLE playlist_tracks ADD COLUMN entry_id TEXT;

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY NOT NULL,
        value TEXT NOT NULL
    ) STRICT, WITHOUT ROWID;

    -- No foreign key: record_play takes bare ids (radio, search), which may
    -- not be cached.
    CREATE TABLE plays (
        track_id  TEXT NOT NULL,
        played_at INTEGER NOT NULL, -- Unix seconds
        played_ms INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX plays_track ON plays (track_id, played_at);

    -- No foreign key, for the same reason as plays.
    CREATE TABLE art_colors (
        track_id TEXT PRIMARY KEY NOT NULL,
        r INTEGER NOT NULL CHECK (r BETWEEN 0 AND 255),
        g INTEGER NOT NULL CHECK (g BETWEEN 0 AND 255),
        b INTEGER NOT NULL CHECK (b BETWEEN 0 AND 255)
    ) STRICT, WITHOUT ROWID;

    CREATE TABLE fetched_at (
        key TEXT PRIMARY KEY NOT NULL,
        at  INTEGER NOT NULL -- Unix seconds
    ) STRICT, WITHOUT ROWID;
",
];

/// `settings` key holding the main playlist id.
const MAIN_PLAYLIST_KEY: &str = "main_playlist";

const TRACK_COLUMNS: &str =
    "t.id, t.title, t.artists, t.album_id, t.album_name, t.duration_ms, t.thumbnails";

const UPSERT_TRACK: &str = "
    INSERT INTO tracks (id, title, artists, album_id, album_name, duration_ms, thumbnails)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
    ON CONFLICT (id) DO UPDATE SET
        title = excluded.title,
        artists = excluded.artists,
        album_id = excluded.album_id,
        album_name = excluded.album_name,
        duration_ms = excluded.duration_ms,
        thumbnails = excluded.thumbnails";

/// Upserts summary fields only; `in_library`, `library_pos` and `has_tracks`
/// keep their values on conflict.
const UPSERT_PLAYLIST: &str = "
    INSERT INTO playlists (id, name, owner, track_count, thumbnails)
    VALUES (?1, ?2, ?3, ?4, ?5)
    ON CONFLICT (id) DO UPDATE SET
        name = excluded.name,
        owner = excluded.owner,
        track_count = excluded.track_count,
        thumbnails = excluded.thumbnails";

/// The SQLite cache.
///
/// Owns one connection. `Store` is `Send`, so it can move to the app's backend
/// thread, but it is not `Sync`: keep it on that one thread and send it
/// commands instead of sharing it. Every write method runs in a single
/// transaction, so a crash leaves either the old or the new data.
pub struct Store {
    conn: Connection,
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<Store>();
};

impl Store {
    /// Opens or creates the database and runs migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        // journal_mode returns the resulting mode as a row, so read it.
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        // With WAL, NORMAL only risks the last commits on power loss; this is a cache.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        // Per-connection setting; SQLite does not persist it.
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.set_prepared_statement_cache_capacity(32);
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Insert or update track metadata.
    pub fn upsert_tracks(&self, tracks: &[Track]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        upsert_tracks(&tx, tracks)?;
        tx.commit()?;
        Ok(())
    }

    pub fn track(&self, id: &str) -> Result<Option<Track>> {
        let sql = format!("SELECT {TRACK_COLUMNS} FROM tracks t WHERE t.id = ?1");
        let mut stmt = self.conn.prepare_cached(&sql)?;
        Ok(stmt.query_row([id], track_from_row).optional()?)
    }

    /// Replace the cached list of the user's saved playlists.
    pub fn set_library_playlists(&self, playlists: &[PlaylistSummary]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE playlists SET in_library = 0, library_pos = NULL WHERE in_library = 1",
            [],
        )?;
        {
            let mut mark = tx.prepare_cached(
                "UPDATE playlists SET in_library = 1, library_pos = ?2 WHERE id = ?1",
            )?;
            for (pos, p) in playlists.iter().enumerate() {
                upsert_playlist(&tx, p)?;
                mark.execute(params![p.id, pos as i64])?;
            }
        }
        // Drop summaries that are neither listed nor hold a cached track order.
        tx.execute(
            "DELETE FROM playlists WHERE in_library = 0 AND has_tracks = 0",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, name, owner, track_count, thumbnails FROM playlists
             WHERE in_library = 1 ORDER BY library_pos",
        )?;
        let rows = stmt.query_map([], summary_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Cache one playlist with its full track order.
    pub fn save_playlist(&self, playlist: &Playlist) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        upsert_tracks(&tx, &playlist.tracks)?;
        let id = &playlist.summary.id;
        upsert_playlist(&tx, &playlist.summary)?;
        tx.prepare_cached("UPDATE playlists SET has_tracks = 1 WHERE id = ?1")?
            .execute([id])?;
        tx.prepare_cached("DELETE FROM playlist_tracks WHERE playlist_id = ?1")?
            .execute([id])?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO playlist_tracks (playlist_id, position, track_id, entry_id)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for (pos, t) in playlist.tracks.iter().enumerate() {
                let entry_id = playlist.entry_ids.get(pos).and_then(|e| e.as_deref());
                insert.execute(params![id, pos as i64, t.id, entry_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Returns `None` unless `save_playlist` has cached this playlist's tracks.
    ///
    /// `entry_ids` is empty when no entry has a stored id, otherwise it holds
    /// one entry per track.
    pub fn playlist(&self, id: &str) -> Result<Option<Playlist>> {
        let summary = self
            .conn
            .prepare_cached(
                "SELECT id, name, owner, track_count, thumbnails FROM playlists
                 WHERE id = ?1 AND has_tracks = 1",
            )?
            .query_row([id], summary_from_row)
            .optional()?;
        let Some(summary) = summary else {
            return Ok(None);
        };
        let sql = format!(
            "SELECT p.entry_id, {TRACK_COLUMNS} FROM playlist_tracks p
             JOIN tracks t ON t.id = p.track_id
             WHERE p.playlist_id = ?1 ORDER BY p.position"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut rows = stmt.query([id])?;
        let mut tracks = Vec::new();
        let mut entry_ids = Vec::new();
        while let Some(row) = rows.next()? {
            entry_ids.push(row.get::<_, Option<String>>(0)?);
            tracks.push(track_from_row_at(row, 1)?);
        }
        if entry_ids.iter().all(Option::is_none) {
            entry_ids.clear();
        }
        Ok(Some(Playlist {
            summary,
            tracks,
            entry_ids,
        }))
    }

    pub fn set_liked(&self, tracks: &[Track]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        upsert_tracks(&tx, tracks)?;
        tx.execute("DELETE FROM liked", [])?;
        {
            let mut insert =
                tx.prepare_cached("INSERT INTO liked (position, track_id) VALUES (?1, ?2)")?;
            for (pos, t) in tracks.iter().enumerate() {
                insert.execute(params![pos as i64, t.id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn liked(&self) -> Result<Vec<Track>> {
        let sql = format!(
            "SELECT {TRACK_COLUMNS} FROM liked l JOIN tracks t ON t.id = l.track_id ORDER BY l.position"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([], track_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Persist the play queue and the index of the current track.
    ///
    /// A `current` index past the end of `queue` is stored as `None`.
    pub fn save_queue(&self, queue: &[TrackId], current: Option<usize>) -> Result<()> {
        let current = current.filter(|&i| i < queue.len());
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM queue", [])?;
        {
            let mut insert =
                tx.prepare_cached("INSERT INTO queue (position, track_id) VALUES (?1, ?2)")?;
            for (pos, id) in queue.iter().enumerate() {
                insert.execute(params![pos as i64, id])?;
            }
        }
        tx.execute(
            "INSERT INTO queue_state (id, current) VALUES (0, ?1)
             ON CONFLICT (id) DO UPDATE SET current = excluded.current",
            [current.map(|i| i as i64)],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Loads the queue, skipping ids with no cached track metadata.
    ///
    /// The current index is remapped to the same track in the shortened list.
    /// If the current track itself was skipped, it points at the next track
    /// that survived, or the last one if none follow.
    pub fn load_queue(&self) -> Result<(Vec<Track>, Option<usize>)> {
        let current: Option<i64> = self
            .conn
            .prepare_cached("SELECT current FROM queue_state WHERE id = 0")?
            .query_row([], |r| r.get(0))
            .optional()?
            .flatten();

        let sql = format!(
            "SELECT q.position, {TRACK_COLUMNS} FROM queue q JOIN tracks t ON t.id = q.track_id
             ORDER BY q.position"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let mut rows = stmt.query([])?;
        let mut tracks = Vec::new();
        // Surviving tracks at positions before the saved current index.
        let mut before_current = 0;
        while let Some(row) = rows.next()? {
            let pos: i64 = row.get(0)?;
            if current.is_some_and(|c| pos < c) {
                before_current += 1;
            }
            tracks.push(track_from_row_at(row, 1)?);
        }

        let index = match current {
            Some(_) if !tracks.is_empty() => Some(before_current.min(tracks.len() - 1)),
            _ => None,
        };
        Ok((tracks, index))
    }

    /// One app setting by key, as plain text. `None` when never set.
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?
            .execute([key, value])?;
        Ok(())
    }

    /// Saves what `save_queue` does not: the "Playing from" label and which
    /// entries are autoplay radio, parallel to the saved queue.
    pub fn save_queue_extras(&self, label: Option<&str>, radio: &[bool]) -> Result<()> {
        // b4-settings
        let _ = (label, radio);
        Ok(())
    }

    /// The label and radio flags saved with the queue. Empty when none.
    pub fn queue_extras(&self) -> Result<(Option<String>, Vec<bool>)> {
        // b4-settings
        Ok((None, Vec::new()))
    }

    /// Remember which playlist is the user's main playlist ("Everything").
    pub fn set_main_playlist(&self, id: &str) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            )?
            .execute([MAIN_PLAYLIST_KEY, id])?;
        Ok(())
    }

    pub fn main_playlist(&self) -> Result<Option<String>> {
        Ok(self
            .conn
            .prepare_cached("SELECT value FROM settings WHERE key = ?1")?
            .query_row([MAIN_PLAYLIST_KEY], |r| r.get(0))
            .optional()?)
    }

    /// Record one play of `track_id` that started at `at` (Unix seconds) and
    /// lasted `played_ms`. The app decides what counts as a play.
    pub fn record_play(&self, track_id: &str, at: i64, played_ms: u64) -> Result<()> {
        let played_ms = i64::try_from(played_ms).unwrap_or(i64::MAX);
        self.conn
            .prepare_cached(
                "INSERT INTO plays (track_id, played_at, played_ms) VALUES (?1, ?2, ?3)",
            )?
            .execute(params![track_id, at, played_ms])?;
        Ok(())
    }

    /// Latest play time (Unix seconds) per id. Ids never played are absent.
    pub fn last_played(&self, ids: &[TrackId]) -> Result<HashMap<TrackId, i64>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut out = HashMap::new();
        {
            let mut stmt =
                tx.prepare_cached("SELECT max(played_at) FROM plays WHERE track_id = ?1")?;
            for id in ids {
                let at: Option<i64> = stmt.query_row([id], |r| r.get(0))?;
                if let Some(at) = at {
                    out.insert(id.clone(), at);
                }
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Main-playlist tracks never played, or last played before `cutoff`
    /// (Unix seconds), in playlist order. A play at exactly `cutoff` counts
    /// as heard. A track listed twice appears once, at its first position.
    /// Empty if no main playlist is set or its tracks are not cached.
    pub fn not_heard_since(&self, cutoff: i64) -> Result<Vec<TrackId>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT p.track_id FROM playlist_tracks p
             WHERE p.playlist_id = (SELECT value FROM settings WHERE key = ?1)
               AND NOT EXISTS (
                   SELECT 1 FROM plays WHERE track_id = p.track_id AND played_at >= ?2
               )
             ORDER BY p.position",
        )?;
        let mut rows = stmt.query(params![MAIN_PLAYLIST_KEY, cutoff])?;
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let id: TrackId = row.get(0)?;
            if seen.insert(id.clone()) {
                out.push(id);
            }
        }
        Ok(out)
    }

    /// Store art accent colors (sRGB) in one transaction. A new color for an
    /// id replaces the old one.
    pub fn set_art_colors(&self, colors: &[(TrackId, [u8; 3])]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO art_colors (track_id, r, g, b) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (track_id) DO UPDATE SET r = excluded.r, g = excluded.g, b = excluded.b",
            )?;
            for (id, [r, g, b]) in colors {
                stmt.execute(params![id, r, g, b])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Stored art colors. Ids with no color are absent.
    pub fn art_colors(&self, ids: &[TrackId]) -> Result<HashMap<TrackId, [u8; 3]>> {
        let tx = self.conn.unchecked_transaction()?;
        let mut out = HashMap::new();
        {
            let mut stmt =
                tx.prepare_cached("SELECT r, g, b FROM art_colors WHERE track_id = ?1")?;
            for id in ids {
                let rgb = stmt
                    .query_row([id], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?]))
                    .optional()?;
                if let Some(rgb) = rgb {
                    out.insert(id.clone(), rgb);
                }
            }
        }
        tx.commit()?;
        Ok(out)
    }

    /// Songs per artist in a playlist, most first, then by name
    /// (case-insensitive, ASCII only), then by id. A song with several
    /// artists counts once under each; a song listed twice in the playlist
    /// counts once. Artists group by channel id, or by name for unlinked
    /// credits; a group with differing names shows the smallest one.
    pub fn artist_counts(&self, playlist_id: &str) -> Result<Vec<NameCount>> {
        self.name_counts(
            "SELECT id, min(name) AS name, count(DISTINCT track_id) AS n FROM (
                 SELECT p.track_id,
                        json_extract(a.value, '$.id') AS id,
                        json_extract(a.value, '$.name') AS name
                 FROM playlist_tracks p
                 JOIN tracks t ON t.id = p.track_id, json_each(t.artists) a
                 WHERE p.playlist_id = ?1
             )
             WHERE name IS NOT NULL
             -- Within a group, id is constant: 'n' groups only hold NULL ids.
             GROUP BY coalesce('i' || id, 'n' || name)",
            playlist_id,
        )
    }

    /// Songs per album in a playlist, most first, then by name
    /// (case-insensitive, ASCII only), then by id. A song listed twice in the
    /// playlist counts once; songs with no album are left out.
    pub fn album_counts(&self, playlist_id: &str) -> Result<Vec<NameCount>> {
        self.name_counts(
            "SELECT t.album_id AS id, min(t.album_name) AS name,
                    count(DISTINCT t.id) AS n
             FROM playlist_tracks p JOIN tracks t ON t.id = p.track_id
             WHERE p.playlist_id = ?1 AND t.album_id IS NOT NULL
             GROUP BY t.album_id",
            playlist_id,
        )
    }

    /// Runs a grouped `(id, name, n)` query and sorts it for display.
    fn name_counts(&self, grouped: &str, playlist_id: &str) -> Result<Vec<NameCount>> {
        let sql = format!(
            "SELECT id, name, n FROM ({grouped})
             ORDER BY n DESC, name COLLATE NOCASE, name, id"
        );
        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map([playlist_id], |r| {
            Ok(NameCount {
                id: r.get(0)?,
                name: r.get(1)?,
                count: r.get(2)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// When `key` ("library", "liked", "playlist:<id>") was last fetched, in
    /// Unix seconds.
    ///
    /// Refresh rule for callers: show the cached data at once, then refresh
    /// in the background if the key was never fetched or `fetched_at` is
    /// more than 10 minutes old.
    pub fn fetched_at(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .prepare_cached("SELECT at FROM fetched_at WHERE key = ?1")?
            .query_row([key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_fetched_at(&self, key: &str, at: i64) -> Result<()> {
        self.conn
            .prepare_cached(
                "INSERT INTO fetched_at (key, at) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET at = excluded.at",
            )?
            .execute(params![key, at])?;
        Ok(())
    }
}

/// One row of [`Store::artist_counts`] or [`Store::album_counts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameCount {
    /// Channel or album id; `None` for unlinked artist credits.
    pub id: Option<String>,
    pub name: String,
    pub count: u32,
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

fn upsert_tracks(conn: &Connection, tracks: &[Track]) -> Result<()> {
    let mut stmt = conn.prepare_cached(UPSERT_TRACK)?;
    for t in tracks {
        let (album_id, album_name) = match &t.album {
            Some(a) => (Some(a.id.as_str()), Some(a.name.as_str())),
            None => (None, None),
        };
        stmt.execute(params![
            t.id,
            t.title,
            to_json(&t.artists)?,
            album_id,
            album_name,
            t.duration.map(|d| d.as_millis() as i64),
            to_json(&t.thumbnails)?,
        ])?;
    }
    Ok(())
}

fn upsert_playlist(conn: &Connection, p: &PlaylistSummary) -> Result<()> {
    conn.prepare_cached(UPSERT_PLAYLIST)?.execute(params![
        p.id,
        p.name,
        p.owner,
        p.track_count.map(|n| n as i64),
        to_json(&p.thumbnails)?,
    ])?;
    Ok(())
}

fn track_from_row(row: &Row<'_>) -> rusqlite::Result<Track> {
    track_from_row_at(row, 0)
}

/// Reads the `TRACK_COLUMNS` starting at column `base`.
fn track_from_row_at(row: &Row<'_>, base: usize) -> rusqlite::Result<Track> {
    let album_id: Option<String> = row.get(base + 3)?;
    let album_name: Option<String> = row.get(base + 4)?;
    let duration_ms: Option<i64> = row.get(base + 5)?;
    Ok(Track {
        id: row.get(base)?,
        title: row.get(base + 1)?,
        artists: from_json(row, base + 2)?,
        album: album_id
            .zip(album_name)
            .map(|(id, name)| AlbumRef { id, name }),
        duration: duration_ms.map(|ms| Duration::from_millis(ms as u64)),
        thumbnails: from_json(row, base + 6)?,
    })
}

fn summary_from_row(row: &Row<'_>) -> rusqlite::Result<PlaylistSummary> {
    let track_count: Option<i64> = row.get(3)?;
    Ok(PlaylistSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        owner: row.get(2)?,
        track_count: track_count.map(|n| n as u64),
        thumbnails: from_json(row, 4)?,
    })
}

fn to_json<T: serde::Serialize + ?Sized>(value: &T) -> rusqlite::Result<String> {
    serde_json::to_string(value).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

fn from_json<T: serde::de::DeserializeOwned>(row: &Row<'_>, idx: usize) -> rusqlite::Result<T> {
    let text: String = row.get(idx)?;
    serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

#[cfg(test)]
mod tests;
