//! Client-side re-ranking of search results. YouTube's filtered album search
//! is weak for generic titles (twenty unknown singles named after the query),
//! while its song ranking is good, so the album list borrows from the songs.

use std::collections::HashMap;

use model::{AlbumSummary, AlbumType, Track};

/// Reorders `albums` in place, with no extra requests:
/// 1. albums by an artist of one of the `tracks`, in the order those songs
///    appear (an album by song #1's artist outranks one by song #5's);
/// 2. albums named exactly like `query` (ignoring case and punctuation),
///    Album before EP before Single;
/// 3. the rest.
///
/// The sort is stable, so ties keep YouTube's order.
pub(crate) fn rank_albums(albums: &mut [AlbumSummary], tracks: &[Track], query: &str) {
    let mut song_rank: HashMap<&str, usize> = HashMap::new();
    for id in tracks
        .iter()
        .flat_map(|t| &t.artists)
        .filter_map(|a| a.id.as_deref())
    {
        let next = song_rank.len();
        song_rank.entry(id).or_insert(next);
    }
    let query = normalize(query);
    albums.sort_by_cached_key(|a| {
        let by_song_artist = a
            .artists
            .iter()
            .filter_map(|ar| song_rank.get(ar.id.as_deref()?))
            .min();
        match by_song_artist {
            Some(&rank) => (0, rank),
            None if !query.is_empty() && normalize(&a.name) == query => {
                (1, type_rank(a.album_type))
            }
            None => (2, 0),
        }
    });
}

fn type_rank(t: AlbumType) -> usize {
    match t {
        AlbumType::Album => 0,
        AlbumType::Ep => 1,
        AlbumType::Single => 2,
        AlbumType::Unknown => 3,
    }
}

/// Lowercase words without punctuation: "Heaven Knows!" and "heaven  knows"
/// are equal.
fn normalize(s: &str) -> String {
    let kept: String = s
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .flat_map(char::to_lowercase)
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::ArtistRef;

    fn artist(id: Option<&str>) -> ArtistRef {
        ArtistRef {
            id: id.map(Into::into),
            name: id.unwrap_or("unlinked").into(),
        }
    }

    fn album(id: &str, name: &str, by: Option<&str>, album_type: AlbumType) -> AlbumSummary {
        AlbumSummary {
            id: id.into(),
            name: name.into(),
            artists: vec![artist(by)],
            year: None,
            thumbnails: Vec::new(),
            album_type,
        }
    }

    fn track(by: &[Option<&str>]) -> Track {
        Track {
            id: "aaaaaaaaaaa".into(),
            title: "song".into(),
            artists: by.iter().map(|&a| artist(a)).collect(),
            album: None,
            duration: None,
            thumbnails: Vec::new(),
        }
    }

    fn ids(albums: &[AlbumSummary]) -> Vec<&str> {
        albums.iter().map(|a| a.id.as_str()).collect()
    }

    #[test]
    fn normalize_ignores_case_and_punctuation() {
        assert_eq!(normalize("Heaven Knows!"), "heaven knows");
        assert_eq!(normalize("  heaven-knows "), "heaven knows");
        assert_eq!(normalize("HEAVEN, KNOWS"), "heaven knows");
        assert_ne!(normalize("Heaven Knows Me"), "heaven knows");
    }

    #[test]
    fn song_artists_first_in_song_order() {
        use AlbumType::*;
        let tracks = [
            track(&[Some("UCfirst")]),
            track(&[None, Some("UCsecond")]),
            track(&[Some("UCfirst")]),
        ];
        let mut albums = [
            album("other", "Other", Some("UCnobody"), Album),
            album("exact", "heaven knows", Some("UCnobody"), Album),
            album("second", "B", Some("UCsecond"), Single),
            album("first", "A", Some("UCfirst"), Single),
            album("unlinked", "C", None, Album),
            album("second2", "D", Some("UCsecond"), Album),
        ];
        rank_albums(&mut albums, &tracks, "Heaven Knows");
        assert_eq!(
            ids(&albums),
            ["first", "second", "second2", "exact", "other", "unlinked"]
        );
    }

    #[test]
    fn exact_names_by_type_then_youtube_order() {
        use AlbumType::*;
        let mut albums = [
            album("s1", "Heaven Knows", Some("UCa"), Single),
            album("near", "Heaven Knows Me", Some("UCb"), Album),
            album("u", "Heaven Knows", Some("UCc"), Unknown),
            album("e", "Heaven Knows", Some("UCd"), Ep),
            album("s2", "heaven knows", Some("UCe"), Single),
            album("a", "Heaven Knows", Some("UCf"), Album),
        ];
        rank_albums(&mut albums, &[], "heaven knows");
        assert_eq!(ids(&albums), ["a", "e", "s1", "s2", "u", "near"]);
    }

    #[test]
    fn empty_inputs_keep_order() {
        let mut albums = [
            album("x", "", None, AlbumType::Single),
            album("y", "", None, AlbumType::Album),
        ];
        rank_albums(&mut albums, &[], "");
        assert_eq!(ids(&albums), ["x", "y"]);
    }
}
