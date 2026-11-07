//! Who a song credits: its artist list, anyone named in a featuring clause
//! of its title ("Another Late Night (feat. Lil Yachty)"), and a known
//! artist named before " - " in the title ("glaive - crynomore", uploaded
//! by a fan channel). YouTube Music often lists only the main artist or the
//! uploader, so the title is the only place the real artist shows up.
//!
//! The browse panel counts and the artist chip filter both use this module,
//! so a row's count always equals the number of songs its chip shows.

use std::collections::HashMap;

use model::Track;

/// True if `track` credits `name`: an artist-list name, a name in a
/// featuring clause of the title, or a name in the title's "ARTIST - SONG"
/// prefix, compared case-insensitively ("glaive" and "Glaive" are one
/// artist). A name must match a whole credit: "Future" does not match
/// "Future Islands".
///
/// `name` is a browse-panel row, and every row is a known name, so a
/// prefix name matches here exactly when [`credited_names`] counts it.
pub fn credits(track: &Track, name: &str) -> bool {
    track.artists.iter().any(|a| eq_fold(&a.name, name))
        || clauses(&track.title).any(|c| runs(c).iter().any(|r| eq_fold(r, name)))
        || prefix_runs(&track.title).iter().any(|r| eq_fold(r, name))
}

/// Every name `track` credits, each once ignoring case, appended to `out`.
/// Artist-list names come first, then each featured name, then names in
/// the title's "ARTIST - SONG" prefix. A name that matches one in `known`
/// (folded name to display name) is added under that display name,
/// otherwise as written. A prefix name counts only if it is in `known`:
/// "Intro - Live" credits nobody unless "Intro" is an artist.
pub fn credited_names<'a>(
    track: &'a Track,
    known: &HashMap<String, &'a str>,
    out: &mut Vec<&'a str>,
) {
    let add = |name: &'a str, out: &mut Vec<&'a str>| {
        if !out.iter().any(|n| eq_fold(n, name)) {
            out.push(name);
        }
    };
    for a in &track.artists {
        if !a.name.is_empty() {
            add(known.get(&fold(&a.name)).copied().unwrap_or(&a.name), out);
        }
    }
    for clause in clauses(&track.title) {
        // A run of pieces that is a known name ("Tyler, The Creator")
        // covers its pieces; pieces no known name covers count as written.
        let pieces = pieces(clause);
        let mut covered = vec![false; pieces.len()];
        for i in 0..pieces.len() {
            for j in i..pieces.len() {
                let run = &clause[pieces[i].0..pieces[j].1];
                if let Some(&display) = known.get(&fold(run)) {
                    add(display, out);
                    covered[i..=j].fill(true);
                }
            }
        }
        for (&(s, e), covered) in pieces.iter().zip(covered) {
            if !covered {
                add(&clause[s..e], out);
            }
        }
    }
    for run in prefix_runs(&track.title) {
        if let Some(&display) = known.get(&fold(run)) {
            add(display, out);
        }
    }
}

/// Lowercased `s`, the key for case-insensitive name lookups.
pub fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

fn eq_fold(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// The featuring clauses of a title: the text after "feat." in
/// "(feat. A & B)", "[ft. A]", "(featuring A)", "(with A)", or an
/// unbracketed "- feat. A" / " ft. A" up to the next bracket.
///
/// Inside a bracket the keyword may lack its dot ("(feat A)"). Outside one
/// it needs the dot or the full word "featuring", so a title like
/// "Great Feat of Strength" is not read as a credit. "with" counts only
/// right after an opening bracket.
fn clauses(title: &str) -> impl Iterator<Item = &str> {
    // ASCII lowercasing keeps every byte offset, so offsets found in
    // `lower` slice `title` at the same places.
    let lower = title.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let (bracket, after) = match b[i] {
            b'(' | b'[' => {
                let mut k = i + 1;
                while b.get(k) == Some(&b' ') {
                    k += 1;
                }
                (Some(if b[i] == b'(' { b')' } else { b']' }), k)
            }
            b' ' if i > 0 => (None, i + 1),
            _ => {
                i += 1;
                continue;
            }
        };
        let keywords: &[&str] = if bracket.is_some() {
            &["featuring ", "feat. ", "feat ", "ft. ", "ft ", "with "]
        } else {
            &["featuring ", "feat. ", "ft. "]
        };
        let Some(kw) = keywords.iter().find(|k| lower[after..].starts_with(**k)) else {
            i += 1;
            continue;
        };
        let start = after + kw.len();
        let end = match bracket {
            Some(close) => b[start..].iter().position(|&c| c == close),
            None => b[start..]
                .iter()
                .position(|&c| matches!(c, b'(' | b'[' | b')' | b']'))
                .into_iter()
                .chain(lower[start..].find(" - "))
                .min(),
        }
        .map_or(b.len(), |n| start + n);
        let clause = title[start..end].trim();
        if !clause.is_empty() {
            out.push(clause);
        }
        i = end.max(i + 1);
    }
    out.into_iter()
}

/// Candidate names from the part of a title before its first " - " (or
/// " – ", " — "): "glaive - crynomore" gives "glaive". Leading tags in
/// brackets ("[clean] glaive") are skipped. Text inside brackets is its own
/// candidate group ("ovine hall (glaive)" gives "ovine hall" and
/// "glaive"). Each group is split like a featuring clause, and also on
/// " x "; every contiguous run of pieces is a candidate. Callers keep only
/// candidates that are known artist names.
fn prefix_runs(title: &str) -> Vec<&str> {
    let Some(end) = [" - ", " – ", " — "]
        .iter()
        .filter_map(|d| title.find(d))
        .min()
    else {
        return Vec::new();
    };
    let prefix = &title[..end];
    let b = prefix.as_bytes();
    let mut groups = Vec::new();
    let mut i = 0;
    let mut start = 0;
    while i < b.len() {
        let close = match b[i] {
            b'(' => b')',
            b'[' => b']',
            _ => {
                i += 1;
                continue;
            }
        };
        // A bracket with only spaces before it is a leading tag
        // ("[clean]"), not a name.
        let leading = prefix[..i].trim().is_empty();
        groups.push(&prefix[start..i]);
        let inner = i + 1;
        let stop = b[inner..]
            .iter()
            .position(|&c| c == close)
            .map_or(b.len(), |n| inner + n);
        if !leading {
            groups.push(&prefix[inner..stop]);
        }
        i = (stop + 1).min(b.len());
        start = i;
    }
    groups.push(&prefix[start..]);
    let mut out = Vec::new();
    for g in groups {
        let p = split(g, true);
        for (k, &(s, _)) in p.iter().enumerate() {
            for &(_, e) in &p[k..] {
                out.push(&g[s..e]);
            }
        }
    }
    out
}

/// Byte ranges of the names in a clause, split on ",", "&" and " and ",
/// trimmed, empty ones dropped.
fn pieces(clause: &str) -> Vec<(usize, usize)> {
    split(clause, false)
}

/// [`pieces`], also splitting on " x " when `x` is set.
fn split(clause: &str, x: bool) -> Vec<(usize, usize)> {
    let lower = clause.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut push = |s: usize, e: usize| {
        let raw = &clause[s..e];
        let s = s + (raw.len() - raw.trim_start().len());
        let e = e - (raw.len() - raw.trim_end().len());
        if s < e {
            out.push((s, e));
        }
    };
    let mut start = 0;
    let mut i = 0;
    while i < lower.len() {
        let sep = match lower.as_bytes()[i] {
            b',' | b'&' => 1,
            b' ' if lower[i..].starts_with(" and ") => 5,
            b' ' if x && lower[i..].starts_with(" x ") => 3,
            _ => 0,
        };
        if sep > 0 {
            push(start, i);
            // ", and " leaves an empty piece between the two, which
            // `push` drops.
            i += sep;
            start = i;
        } else {
            i += 1;
        }
    }
    push(start, lower.len());
    out
}

/// Every contiguous run of pieces, as written ("A", "A, B", "B").
fn runs(clause: &str) -> Vec<&str> {
    let p = pieces(clause);
    let mut out = Vec::new();
    for (i, &(s, _)) in p.iter().enumerate() {
        for &(_, e) in &p[i..] {
            out.push(&clause[s..e]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::ArtistRef;

    fn track(title: &str, artists: &[&str]) -> Track {
        Track {
            id: "t".into(),
            title: title.into(),
            artists: artists
                .iter()
                .map(|n| ArtistRef {
                    id: None,
                    name: (*n).into(),
                })
                .collect(),
            album: None,
            duration: None,
            thumbnails: Vec::new(),
        }
    }

    fn names(title: &str) -> Vec<&str> {
        clauses(title)
            .flat_map(|c| pieces(c).into_iter().map(move |(s, e)| &c[s..e]))
            .collect()
    }

    #[test]
    fn featuring_forms() {
        assert_eq!(
            names("Another Late Night (feat. Lil Yachty)"),
            ["Lil Yachty"]
        );
        assert_eq!(names("SIDE EFFExT (FEAT. Lil Yachty)"), ["Lil Yachty"]);
        assert_eq!(names("Song [ft. A]"), ["A"]);
        assert_eq!(names("Song (ft A)"), ["A"]);
        assert_eq!(names("Song (feat A) [Remix]"), ["A"]);
        assert_eq!(names("Song (featuring A)"), ["A"]);
        assert_eq!(names("Song ( with A )"), ["A"]);
        assert_eq!(names("Song - feat. A"), ["A"]);
        assert_eq!(names("Song feat. A (Live)"), ["A"]);
        assert_eq!(names("Song ft. A - Remastered"), ["A"]);
        assert_eq!(names("Song featuring A"), ["A"]);
        assert_eq!(names("Song (feat. A"), ["A"]);
    }

    #[test]
    fn not_featuring() {
        assert!(names("Great Feat of Strength").is_empty());
        assert!(names("Feat. Nobody").is_empty());
        assert!(names("Left Hand").is_empty());
        assert!(names("Soft (Interlude)").is_empty());
        assert!(names("Defeat (Live)").is_empty());
        assert!(names("Live with Friends").is_empty());
        assert!(names("Song (Live with Orchestra)").is_empty());
        assert!(names("Song (feat. )").is_empty());
    }

    #[test]
    fn splits_several_names() {
        assert_eq!(names("Song (feat. A, B & C)"), ["A", "B", "C"]);
        assert_eq!(names("Song (feat. A, B, and C)"), ["A", "B", "C"]);
        assert_eq!(
            names("Song (feat. Pharrell Williams and Nile Rodgers)"),
            ["Pharrell Williams", "Nile Rodgers"]
        );
        assert_eq!(names("Song (feat. A AND B)"), ["A", "B"]);
        // No split inside a word.
        assert_eq!(names("Song (feat. Brandon Flowers)"), ["Brandon Flowers"]);
        assert_eq!(names("Song (feat. A) [with B]"), ["A", "B"]);
    }

    #[test]
    fn credits_whole_names_only() {
        let t = track("Song (feat. Future Islands)", &["X"]);
        assert!(credits(&t, "Future Islands"));
        assert!(credits(&t, "future islands"));
        assert!(!credits(&t, "Future"));
        assert!(!credits(&t, "Islands"));
        assert!(credits(&t, "X"));
        // Artist-list names ignore case too.
        assert!(credits(&t, "x"));
        assert!(!credits(&t, "X Y"));

        let t = track("Song (feat. Tyler, The Creator & Kali Uchis)", &["X"]);
        assert!(credits(&t, "Tyler, The Creator"));
        assert!(credits(&t, "Kali Uchis"));
        assert!(!credits(&t, "Tyler, The"));
        assert!(!credits(&t, "Creator"));
    }

    #[test]
    fn title_prefix_candidates() {
        let has = |title: &str, name: &str| prefix_runs(title).contains(&name);
        let t = "Glaive - hit the function like larry david (Ovine Hall) (Official Audio)";
        assert_eq!(prefix_runs(t), ["Glaive"]);
        let t = "ovine hall (glaive) - pree me at the party [가사해석]";
        assert!(has(t, "ovine hall") && has(t, "glaive"));
        assert_eq!(prefix_runs("[clean] glaive - crynomore"), ["glaive"]);
        assert_eq!(
            prefix_runs("glaive - i hate 2 see u cry (lyric video)"),
            ["glaive"]
        );
        assert_eq!(prefix_runs("A x B - Song"), ["A", "A x B", "B"]);
        assert_eq!(prefix_runs("A – Song"), ["A"]);
        assert_eq!(prefix_runs("A — Song - Remix"), ["A"]);
        assert!(prefix_runs("No Dash").is_empty());
        assert!(prefix_runs("Half-Life").is_empty());
    }

    #[test]
    fn title_prefix_credits() {
        for title in [
            "Glaive - hit the function like larry david (Ovine Hall) (Official Audio)",
            "ovine hall (glaive) - pree me at the party [가사해석]",
            "[clean] glaive - crynomore",
            "glaive - i hate 2 see u cry (lyric video)",
        ] {
            let t = track(title, &["uploader"]);
            assert!(credits(&t, "glaive"), "{title}");
            assert!(credits(&t, "Glaive"), "{title}");
            assert!(!credits(&t, "glai"), "{title}");
        }
        // Prefix names go through `known` only.
        let known: HashMap<String, &str> = [("glaive".to_owned(), "glaive")].into();
        let intro = track("Intro - Live", &["Band"]);
        let mut out = Vec::new();
        credited_names(&intro, &known, &mut out);
        assert_eq!(out, ["Band"]);
        let clean = track("[clean] GLAIVE - crynomore", &["X"]);
        let mut out = Vec::new();
        credited_names(&clean, &known, &mut out);
        assert_eq!(out, ["X", "glaive"]);
    }

    #[test]
    fn credited_names_merge_into_known_names() {
        let t = track(
            "Song (feat. LIL YACHTY, Tyler, The Creator & New Guest)",
            &["Drake", "Drake"],
        );
        let known: HashMap<String, &str> = ["Lil Yachty", "Tyler, The Creator", "Drake"]
            .into_iter()
            .map(|n| (fold(n), n))
            .collect();
        let mut out = Vec::new();
        credited_names(&t, &known, &mut out);
        assert_eq!(
            out,
            ["Drake", "Lil Yachty", "Tyler, The Creator", "New Guest"]
        );

        // A featured name already in the artist list counts once.
        let t = track("Song (feat. Drake)", &["Drake"]);
        out.clear();
        credited_names(&t, &known, &mut out);
        assert_eq!(out, ["Drake"]);
    }
}
