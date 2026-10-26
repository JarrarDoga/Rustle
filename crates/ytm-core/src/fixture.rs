use std::time::Duration;

use model::{
    AlbumRef, AlbumSummary, ArtistRef, ArtistSummary, AudioCodec, Playlist, PlaylistSummary,
    SearchResults, StreamSource, Track,
};

use crate::{BackendError, MusicBackend, Result};

/// Offline backend for `--demo` and tests. No network, no account.
///
/// The ytm-core worker may replace this static data with recorded JSON
/// fixtures, but must keep it network-free and keep the same type.
#[derive(Default)]
pub struct FixtureBackend {
    _private: (),
}

impl FixtureBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

fn artist(name: &str) -> ArtistRef {
    ArtistRef {
        id: Some(format!("UC_{}", name.to_lowercase().replace(' ', "_"))),
        name: name.to_owned(),
    }
}

fn tracks() -> Vec<Track> {
    let rows: [(&str, &str, &str, &str, u64); 6] = [
        (
            "aaaaaaaaaa1",
            "Neon Rivers",
            "The Lanterns",
            "Night Signals",
            214,
        ),
        (
            "aaaaaaaaaa2",
            "Paper Satellites",
            "The Lanterns",
            "Night Signals",
            187,
        ),
        ("aaaaaaaaaa3", "Glasshouse", "Mira Vale", "Glasshouse", 243),
        ("aaaaaaaaaa4", "Slow Static", "Mira Vale", "Glasshouse", 198),
        ("aaaaaaaaaa5", "Copper Tide", "Ondine", "Low Orbit", 276),
        ("aaaaaaaaaa6", "Lowlight", "Ondine", "Low Orbit", 161),
    ];
    rows.iter()
        .map(|&(id, title, by, album, secs)| Track {
            id: id.to_owned(),
            title: title.to_owned(),
            artists: vec![artist(by)],
            album: Some(AlbumRef {
                id: format!("MPRE_{}", album.to_lowercase().replace(' ', "_")),
                name: album.to_owned(),
            }),
            duration: Some(Duration::from_secs(secs)),
            thumbnails: Vec::new(),
        })
        .collect()
}

fn playlists() -> Vec<PlaylistSummary> {
    vec![
        PlaylistSummary {
            id: "PLdemo_focus".to_owned(),
            name: "Focus".to_owned(),
            owner: Some("You".to_owned()),
            track_count: Some(4),
            thumbnails: Vec::new(),
        },
        PlaylistSummary {
            id: "PLdemo_evening".to_owned(),
            name: "Evening".to_owned(),
            owner: Some("You".to_owned()),
            track_count: Some(2),
            thumbnails: Vec::new(),
        },
    ]
}

impl MusicBackend for FixtureBackend {
    async fn set_cookies(&self, _cookies: &str) -> Result<()> {
        Ok(())
    }

    async fn search(&self, query: &str) -> Result<SearchResults> {
        let q = query.to_lowercase();
        let hit = |s: &str| s.to_lowercase().contains(&q);
        let tracks: Vec<Track> = tracks()
            .into_iter()
            .filter(|t| hit(&t.title) || t.artists.iter().any(|a| hit(&a.name)))
            .collect();
        let mut albums: Vec<AlbumSummary> = Vec::new();
        let mut artists: Vec<ArtistSummary> = Vec::new();
        for t in &tracks {
            if let Some(a) = &t.album
                && !albums.iter().any(|x| x.id == a.id)
            {
                albums.push(AlbumSummary {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    artists: t.artists.clone(),
                    year: Some(2025),
                    thumbnails: Vec::new(),
                });
            }
            for ar in &t.artists {
                let id = ar.id.clone().unwrap_or_default();
                if !artists.iter().any(|x| x.id == id) {
                    artists.push(ArtistSummary {
                        id,
                        name: ar.name.clone(),
                        thumbnails: Vec::new(),
                    });
                }
            }
        }
        let playlists = playlists().into_iter().filter(|p| hit(&p.name)).collect();
        Ok(SearchResults {
            tracks,
            albums,
            artists,
            playlists,
        })
    }

    async fn library_playlists(&self) -> Result<Vec<PlaylistSummary>> {
        Ok(playlists())
    }

    async fn playlist(&self, id: &str) -> Result<Playlist> {
        let summary = playlists()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| BackendError::NotFound(id.to_owned()))?;
        let all = tracks();
        let tracks = if id == "PLdemo_focus" {
            all[..4].to_vec()
        } else {
            all[4..].to_vec()
        };
        Ok(Playlist { summary, tracks })
    }

    async fn liked_tracks(&self) -> Result<Vec<Track>> {
        Ok(tracks().into_iter().step_by(2).collect())
    }

    async fn stream(&self, track_id: &str) -> Result<StreamSource> {
        let track = tracks()
            .into_iter()
            .find(|t| t.id == track_id)
            .ok_or_else(|| BackendError::NotFound(track_id.to_owned()))?;
        // Demo mode has no audio. The player must treat this URL as silence.
        Ok(StreamSource {
            track_id: track.id,
            url: "demo://silence".to_owned(),
            codec: AudioCodec::Opus,
            bitrate_bps: 0,
            content_length: 0,
            duration: track.duration,
            loudness_db: None,
        })
    }
}
