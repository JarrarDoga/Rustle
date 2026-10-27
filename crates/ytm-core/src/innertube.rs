//! Innertube calls rustypipe 0.11.4 lacks: like, unlike, playlist edits,
//! each playlist entry's `setVideoId`, and timed lyrics.
//!
//! Requests go through `RustyPipeQuery::raw`. On an `.authenticated()` query
//! with `ClientType::DesktopMusic`, rustypipe's request builder adds the
//! login cookie, the SAPISIDHASH `Authorization` header and the
//! `X-Goog-AuthUser` / `X-Goog-PageId` headers. Bodies follow ytmusicapi.
//! Timed lyrics need the `ANDROID_MUSIC` client, which `raw` cannot send, so
//! [`timed_lyrics`] is a plain POST of its own.
//! The parsers below are pure so they run offline against recorded fixtures.

use std::sync::OnceLock;
use std::time::Duration;

use model::{LyricLine, Lyrics};
use rustypipe::client::{ClientType, RustyPipeQuery};
use rustypipe::error::Error as RpError;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{BackendError, Result};

/// `like/like` or `like/removelike`.
pub(crate) async fn rate(q: &RustyPipeQuery, video_id: &str, like: bool) -> Result<()> {
    let endpoint = if like { "like/like" } else { "like/removelike" };
    let body = json!({ "target": { "videoId": video_id } });
    let resp = post(q, endpoint, &body, video_id).await?;
    check_rating(&resp)
}

/// Appends `video_ids` to a playlist; returns the new entry ids in order.
pub(crate) async fn add_videos(
    q: &RustyPipeQuery,
    playlist_id: &str,
    video_ids: &[String],
) -> Result<Vec<Option<String>>> {
    if video_ids.is_empty() {
        return Ok(Vec::new());
    }
    // No `dedupeOption`: YouTube then refuses to add a song that is already
    // in the playlist, which also keeps a retried request from adding twice.
    let actions: Vec<Value> = video_ids
        .iter()
        .map(|v| json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": v }))
        .collect();
    let body = json!({ "playlistId": bare_playlist_id(playlist_id), "actions": actions });
    let resp = post(q, "browse/edit_playlist", &body, playlist_id).await?;
    let v = check_edit(&resp)?;
    Ok(added_entry_ids(&v, video_ids))
}

/// Removes playlist entries given as `(video_id, set_video_id)`.
pub(crate) async fn remove_entries(
    q: &RustyPipeQuery,
    playlist_id: &str,
    entries: &[(String, String)],
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let actions: Vec<Value> = entries
        .iter()
        .map(|(video, set)| {
            json!({
                "setVideoId": set,
                "removedVideoId": video,
                "action": "ACTION_REMOVE_VIDEO",
            })
        })
        .collect();
    let body = json!({ "playlistId": bare_playlist_id(playlist_id), "actions": actions });
    let resp = post(q, "browse/edit_playlist", &body, playlist_id).await?;
    check_edit(&resp).map(drop)
}

/// The `setVideoId` of each entry of a playlist the user owns, in playlist
/// order, following continuations up to `max_items` entries.
///
/// Returns an empty list when the first page has no `setVideoId` at all
/// (the user does not own the playlist), after one request.
pub(crate) async fn playlist_entries(
    q: &RustyPipeQuery,
    playlist_id: &str,
    max_items: usize,
) -> Result<Vec<RawEntry>> {
    let body = json!({ "browseId": format!("VL{}", bare_playlist_id(playlist_id)) });
    let resp = post(q, "browse", &body, playlist_id).await?;
    let mut page = parse_entry_page(&parse_json(&resp)?)?;
    if page.entries.iter().all(|e| e.set_video_id.is_none()) {
        return Ok(Vec::new());
    }
    let mut entries = std::mem::take(&mut page.entries);
    while let Some(token) = page.ctoken.take() {
        if entries.len() >= max_items {
            break;
        }
        let resp = post(q, "browse", &json!({ "continuation": token }), playlist_id).await?;
        page = parse_entry_page(&parse_json(&resp)?)?;
        if page.entries.is_empty() {
            break;
        }
        entries.append(&mut page.entries);
    }
    entries.truncate(max_items);
    Ok(entries)
}

async fn post(q: &RustyPipeQuery, endpoint: &str, body: &Value, id: &str) -> Result<String> {
    // `raw` sends the `X-Goog-EOM-Visitor-Id` header only when the query has
    // visitor data set; rustypipe's typed calls always send it.
    let vd = q
        .get_visitor_data(false)
        .await
        .map_err(|e| map_write_error(e, id))?;
    q.clone()
        .visitor_data(vd)
        .raw(ClientType::DesktopMusic, endpoint, body)
        .await
        .map_err(|e| map_write_error(e, id))
}

/// Error mapping for raw calls. `raw` skips the status translation rustypipe's
/// typed calls do, so 401/403/404 arrive as plain `HttpStatus`.
///
/// Unlike `map_error`, 403 means a rejected session here: these endpoints
/// need no PO token, so there is no bot check to fail.
pub(crate) fn map_write_error(e: RpError, id: &str) -> BackendError {
    match e {
        RpError::Auth(_) | RpError::HttpStatus(401 | 403, _) => BackendError::Auth,
        RpError::HttpStatus(404, _) => BackendError::NotFound(id.to_owned()),
        RpError::HttpStatus(400, msg) => BackendError::Extraction(format!("bad request: {msg}")),
        RpError::Http(_) | RpError::HttpStatus(..) => BackendError::Network(e.to_string()),
        RpError::Extraction(_) | RpError::Other(_) => BackendError::Extraction(e.to_string()),
    }
}

/// Parses a 2xx body. YouTube's throttling answers with an empty body, which
/// is a network condition, not a parse failure and not a logout.
pub(crate) fn parse_json(body: &str) -> Result<Value> {
    if body.trim().is_empty() {
        return Err(BackendError::Network(
            "empty response from YouTube (rate limited)".to_owned(),
        ));
    }
    serde_json::from_str(body).map_err(|e| BackendError::Extraction(format!("bad JSON: {e}")))
}

/// The `logged_in` flag YouTube reports in `responseContext`, if present.
fn logged_in(v: &Value) -> Option<bool> {
    v.pointer("/responseContext/serviceTrackingParams")?
        .as_array()?
        .iter()
        .filter_map(|p| p.get("params")?.as_array())
        .flatten()
        .find(|kv| kv.get("key").and_then(Value::as_str) == Some("logged_in"))
        .and_then(|kv| kv.get("value")?.as_str())
        .map(|v| v == "1")
}

/// `like/*` responses carry no `status`; success is a logged-in response
/// (the same check the migration job used for its likes).
pub(crate) fn check_rating(body: &str) -> Result<()> {
    let v = parse_json(body)?;
    match logged_in(&v) {
        Some(true) => Ok(()),
        Some(false) => Err(BackendError::Auth),
        None => Err(BackendError::Extraction(
            "like response has no login state".to_owned(),
        )),
    }
}

/// `browse/edit_playlist` succeeded only if `status` is `STATUS_SUCCEEDED`.
pub(crate) fn check_edit(body: &str) -> Result<Value> {
    let v = parse_json(body)?;
    if logged_in(&v) == Some(false) {
        return Err(BackendError::Auth);
    }
    match v.get("status").and_then(Value::as_str) {
        Some("STATUS_SUCCEEDED") => Ok(v),
        Some(status) => {
            let mut msg = format!("playlist edit {status}");
            if let Some(text) = dialog_text(&v) {
                msg.push_str(": ");
                msg.push_str(&text);
            }
            Err(BackendError::Extraction(msg))
        }
        None => Err(BackendError::Extraction(
            "playlist edit response has no status".to_owned(),
        )),
    }
}

/// The first human-readable message in a failed edit (YouTube explains a
/// refused duplicate in a confirm dialog or toast).
fn dialog_text(v: &Value) -> Option<String> {
    fn runs(text: &Value) -> Option<String> {
        let s: String = text
            .get("runs")?
            .as_array()?
            .iter()
            .filter_map(|r| r.get("text")?.as_str())
            .collect();
        (!s.is_empty()).then_some(s)
    }
    match v {
        Value::Object(map) => {
            for key in ["dialogMessages", "title", "text", "successResponseText"] {
                if let Some(t) = map.get(key) {
                    let t = match t {
                        Value::Array(a) => a.first().unwrap_or(t),
                        _ => t,
                    };
                    if let Some(s) = runs(t) {
                        return Some(s);
                    }
                }
            }
            map.iter()
                .filter(|(k, _)| k.as_str() != "responseContext")
                .find_map(|(_, v)| dialog_text(v))
        }
        Value::Array(a) => a.iter().find_map(dialog_text),
        _ => None,
    }
}

/// New entry ids from an add response, one per requested video in order.
/// A result whose `videoId` does not match the request position yields `None`.
pub(crate) fn added_entry_ids(v: &Value, video_ids: &[String]) -> Vec<Option<String>> {
    let results: Vec<&Value> = v
        .get("playlistEditResults")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| r.get("playlistEditVideoAddedResultData"))
                .collect()
        })
        .unwrap_or_default();
    video_ids
        .iter()
        .enumerate()
        .map(|(i, want)| {
            let r = results.get(i)?;
            if r.get("videoId")
                .and_then(Value::as_str)
                .is_some_and(|got| got != want)
            {
                return None;
            }
            r.get("setVideoId")?.as_str().map(str::to_owned)
        })
        .collect()
}

/// ytmusicapi strips the `VL` browse prefix before playlist edits.
fn bare_playlist_id(id: &str) -> &str {
    id.strip_prefix("VL").unwrap_or(id)
}

/// One row of a playlist browse response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawEntry {
    /// Empty when the row has no video id (rustypipe skips such rows too).
    pub video_id: String,
    pub set_video_id: Option<String>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct EntryPage {
    pub entries: Vec<RawEntry>,
    pub ctoken: Option<String>,
}

/// Rows and the next continuation token from a playlist `browse` response:
/// the first page (`musicPlaylistShelfRenderer` / `musicShelfRenderer`) or a
/// continuation (`continuationContents` or `onResponseReceivedActions`).
pub(crate) fn parse_entry_page(v: &Value) -> Result<EntryPage> {
    if logged_in(v) == Some(false) {
        return Err(BackendError::Auth);
    }
    let (items, shelf) = find_items(v).ok_or_else(|| {
        BackendError::Extraction("playlist response has no track list".to_owned())
    })?;
    let mut page = EntryPage::default();
    for item in items {
        if let Some(row) = item.get("musicResponsiveListItemRenderer") {
            page.entries.push(parse_row(row));
        } else if let Some(t) = item
            .pointer("/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
            .and_then(Value::as_str)
        {
            page.ctoken = Some(t.to_owned());
        }
    }
    if page.ctoken.is_none() {
        page.ctoken = shelf
            .and_then(|s| s.pointer("/continuations/0/nextContinuationData/continuation"))
            .and_then(Value::as_str)
            .map(str::to_owned);
    }
    Ok(page)
}

const SHELF_KEYS: [&str; 4] = [
    "musicPlaylistShelfRenderer",
    "musicShelfRenderer",
    "musicPlaylistShelfContinuation",
    "musicShelfContinuation",
];

/// The first track list in `v`, with the shelf object that holds it (for
/// old-style `continuations`).
fn find_items(v: &Value) -> Option<(&Vec<Value>, Option<&Value>)> {
    match v {
        Value::Object(map) => {
            for key in SHELF_KEYS {
                if let Some(shelf) = map.get(key)
                    && let Some(items) = shelf.get("contents").and_then(Value::as_array)
                {
                    return Some((items, Some(shelf)));
                }
            }
            if let Some(items) = map
                .get("appendContinuationItemsAction")
                .and_then(|a| a.get("continuationItems"))
                .and_then(Value::as_array)
            {
                return Some((items, None));
            }
            // The track list never sits in the header or the tracking data;
            // skipping them avoids walking large subtrees.
            map.iter()
                .filter(|(k, _)| {
                    !matches!(
                        k.as_str(),
                        "responseContext" | "header" | "frameworkUpdates"
                    )
                })
                .find_map(|(_, v)| find_items(v))
        }
        Value::Array(a) => a.iter().find_map(find_items),
        _ => None,
    }
}

fn parse_row(row: &Value) -> RawEntry {
    // ytmusicapi reads both ids from the "Remove from playlist" menu item.
    let edit = row
        .pointer("/menu/menuRenderer/items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find_map(|i| {
            i.pointer("/menuServiceItemRenderer/serviceEndpoint/playlistEditEndpoint/actions/0")
        });
    let str_at = |v: Option<&Value>, ptr: &str| {
        v.and_then(|v| v.pointer(ptr))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let data = row.get("playlistItemData");
    let video_id = str_at(data, "/videoId")
        .or_else(|| str_at(edit, "/removedVideoId"))
        .unwrap_or_default();
    let set_video_id = str_at(data, "/playlistSetVideoId").or_else(|| str_at(edit, "/setVideoId"));
    RawEntry {
        video_id,
        set_video_id,
    }
}

/// Entry ids parallel to `track_ids`, or an empty list when the two lists
/// cannot be matched without guessing.
///
/// rustypipe drops some rows (livestreams, rows without a title), so the raw
/// list may be longer. Rows are matched in order by video id; if a dropped row
/// has the video id of a kept track, the match is ambiguous and nothing is
/// returned rather than a list that might point at the wrong copy.
pub(crate) fn align_entry_ids(track_ids: &[&str], entries: &[RawEntry]) -> Vec<Option<String>> {
    if entries.iter().all(|e| e.set_video_id.is_none()) {
        return Vec::new();
    }
    if entries.len() == track_ids.len()
        && entries.iter().zip(track_ids).all(|(e, t)| e.video_id == *t)
    {
        return entries.iter().map(|e| e.set_video_id.clone()).collect();
    }
    let mut out = Vec::with_capacity(track_ids.len());
    let mut skipped: Vec<&str> = Vec::new();
    let mut rest = entries.iter();
    for &t in track_ids {
        loop {
            let Some(e) = rest.next() else {
                return Vec::new();
            };
            if e.video_id == t {
                out.push(e.set_video_id.clone());
                break;
            }
            skipped.push(&e.video_id);
        }
    }
    if skipped
        .iter()
        .any(|s| !s.is_empty() && track_ids.contains(s))
    {
        return Vec::new();
    }
    out
}

// ---------------------------------------------------------------- lyrics

const BROWSE_URL: &str = "https://music.youtube.com/youtubei/v1/browse?prettyPrint=false";
/// The Android app is the only client YouTube Music sends timed lyrics to;
/// ytmusicapi's `get_lyrics(timestamps=True)` uses this version.
const ANDROID_MUSIC_VERSION: &str = "7.21.50";

/// What the `ANDROID_MUSIC` lyrics `browse` says about a song.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TimedLyrics {
    /// The lines, timed or not.
    Found(Lyrics),
    /// YouTube answered "Lyrics not available at this time."
    Unavailable,
    /// Neither shape: the caller falls back to the web client's plain lyrics.
    Unknown,
}

/// Lyrics for a lyrics browse id (`MPLYt…`, from `music_details`) as the
/// `ANDROID_MUSIC` client sees them, with line start times when YouTube has
/// them. Sent without cookies: lyrics are public.
pub(crate) async fn timed_lyrics(user_agent: &str, lyrics_id: &str) -> Result<TimedLyrics> {
    let body = json!({
        "context": { "client": {
            "clientName": "ANDROID_MUSIC",
            "clientVersion": ANDROID_MUSIC_VERSION,
            "hl": "en",
            "gl": "US",
        } },
        "browseId": lyrics_id,
    });
    let resp = http()?
        .post(BROWSE_URL)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ORIGIN, "https://music.youtube.com")
        .header(reqwest::header::USER_AGENT, user_agent)
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| BackendError::Network(e.without_url().to_string()))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| BackendError::Network(e.without_url().to_string()))?;
    match status {
        200..=299 => parse_timed_lyrics(&text),
        404 => Err(BackendError::NotFound(lyrics_id.to_owned())),
        400 => Err(BackendError::Extraction(format!(
            "bad request: HTTP {status}"
        ))),
        // No cookies were sent, so 401 and 403 are not a session problem.
        _ => Err(BackendError::Network(format!("HTTP {status}"))),
    }
}

/// One client for the lyrics POST, built on first use. rustypipe keeps its
/// own client private.
fn http() -> Result<&'static reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| BackendError::Extraction(format!("HTTP client: {e}")))?;
    Ok(CLIENT.get_or_init(|| c))
}

/// Parses the `ANDROID_MUSIC` lyrics `browse` response. The response is
/// several hundred KB of layout data; typed structs skip it without building
/// a JSON tree.
pub(crate) fn parse_timed_lyrics(body: &str) -> Result<TimedLyrics> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        contents: Option<Contents>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Contents {
        element_renderer: Option<ElementRenderer>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ElementRenderer {
        new_element: Option<NewElement>,
    }
    #[derive(Deserialize)]
    struct NewElement {
        #[serde(rename = "type")]
        kind: Option<ElementType>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ElementType {
        component_type: Option<ComponentType>,
    }
    #[derive(Deserialize)]
    struct ComponentType {
        model: Option<Model>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Model {
        timed_lyrics_model: Option<TimedLyricsModel>,
        music_message_model: Option<serde::de::IgnoredAny>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct TimedLyricsModel {
        lyrics_data: Option<LyricsData>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LyricsData {
        #[serde(default)]
        timed_lyrics_data: Vec<RawLine>,
        source_message: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RawLine {
        #[serde(default)]
        lyric_line: String,
        cue_range: Option<CueRange>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CueRange {
        /// A decimal string, as Innertube writes 64-bit numbers.
        start_time_milliseconds: Option<Value>,
    }

    if body.trim().is_empty() {
        return Err(BackendError::Network(
            "empty response from YouTube (rate limited)".to_owned(),
        ));
    }
    let resp: Response = serde_json::from_str(body)
        .map_err(|e| BackendError::Extraction(format!("bad JSON: {e}")))?;
    let Some(model) = resp
        .contents
        .and_then(|c| c.element_renderer)
        .and_then(|e| e.new_element)
        .and_then(|e| e.kind)
        .and_then(|t| t.component_type)
        .and_then(|c| c.model)
    else {
        return Ok(TimedLyrics::Unknown);
    };
    let data = model.timed_lyrics_model.and_then(|m| m.lyrics_data);
    let Some(data) = data.filter(|d| !d.timed_lyrics_data.is_empty()) else {
        return Ok(if model.music_message_model.is_some() {
            TimedLyrics::Unavailable
        } else {
            TimedLyrics::Unknown
        });
    };
    let mut lines: Vec<LyricLine> = data
        .timed_lyrics_data
        .into_iter()
        .map(|l| LyricLine {
            start: l
                .cue_range
                .and_then(|c| c.start_time_milliseconds)
                .and_then(|v| match v {
                    Value::String(s) => s.parse().ok(),
                    v => v.as_u64(),
                })
                .map(Duration::from_millis),
            text: l.lyric_line,
        })
        .collect();
    let synced = lines.iter().all(|l| l.start.is_some());
    if synced {
        // The view finds the current line by binary search.
        lines.sort_by_key(|l| l.start);
    }
    let source = data
        .source_message
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    Ok(TimedLyrics::Found(Lyrics {
        lines,
        synced,
        source,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(video: &str, set: Option<&str>) -> RawEntry {
        RawEntry {
            video_id: video.to_owned(),
            set_video_id: set.map(str::to_owned),
        }
    }

    #[test]
    fn write_errors() {
        use rustypipe::error::{AuthError, ExtractionError};
        let id = "PLx";
        for e in [
            RpError::Auth(AuthError::NoLogin),
            RpError::HttpStatus(401, "".into()),
            RpError::HttpStatus(403, "".into()),
        ] {
            assert!(matches!(map_write_error(e, id), BackendError::Auth));
        }
        for e in [
            RpError::HttpStatus(429, "".into()),
            RpError::HttpStatus(503, "".into()),
            RpError::Http("reset".into()),
        ] {
            assert!(matches!(map_write_error(e, id), BackendError::Network(_)));
        }
        assert!(matches!(
            map_write_error(RpError::HttpStatus(404, "".into()), id),
            BackendError::NotFound(p) if p == id
        ));
        for e in [
            RpError::HttpStatus(400, "Request contains an invalid argument.".into()),
            RpError::Extraction(ExtractionError::InvalidData("x".into())),
        ] {
            assert!(matches!(
                map_write_error(e, id),
                BackendError::Extraction(_)
            ));
        }
    }

    #[test]
    fn throttled_bodies_are_network_errors() {
        for body in ["", "  \n"] {
            assert!(matches!(check_rating(body), Err(BackendError::Network(_))));
            assert!(matches!(check_edit(body), Err(BackendError::Network(_))));
        }
        assert!(matches!(
            check_edit("<html>"),
            Err(BackendError::Extraction(_))
        ));
    }

    #[test]
    fn rating_responses() {
        assert!(check_rating(include_str!("../fixtures/like.json")).is_ok());
        assert!(check_rating(include_str!("../fixtures/removelike.json")).is_ok());
        assert!(matches!(
            check_rating(include_str!("../fixtures/like_logged_out.json")),
            Err(BackendError::Auth)
        ));
        assert!(matches!(
            check_rating(r#"{"actions": []}"#),
            Err(BackendError::Extraction(_))
        ));
    }

    #[test]
    fn add_response_gives_entry_ids_in_order() {
        let v = check_edit(include_str!("../fixtures/edit_playlist_add.json")).unwrap();
        let ids = ["lYBUbBu4W08".to_owned(), "dQw4w9WgXcQ".to_owned()];
        assert_eq!(
            added_entry_ids(&v, &ids),
            [
                Some("1F3E5D7C9B0A2468".to_owned()),
                Some("525CFB6D9DD9F1E0".to_owned())
            ]
        );
        // A result for a different video, or a missing result, is None.
        let swapped = [ids[1].clone(), ids[0].clone(), "zzzzzzzzzzz".to_owned()];
        assert_eq!(added_entry_ids(&v, &swapped), [None, None, None]);
    }

    #[test]
    fn edit_status() {
        assert!(check_edit(include_str!("../fixtures/edit_playlist_remove.json")).is_ok());
        match check_edit(include_str!("../fixtures/edit_playlist_duplicate.json")) {
            Err(BackendError::Extraction(m)) => {
                assert!(m.starts_with("playlist edit STATUS_FAILED"), "{m}");
                assert!(m.contains("already"), "{m}");
            }
            other => panic!("got {other:?}"),
        }
        assert!(matches!(
            check_edit(r#"{"responseContext": {}}"#),
            Err(BackendError::Extraction(_))
        ));
        let logged_out = include_str!("../fixtures/like_logged_out.json");
        assert!(matches!(check_edit(logged_out), Err(BackendError::Auth)));
    }

    #[test]
    fn browse_pages_give_set_video_ids() {
        let first = parse_entry_page(
            &parse_json(include_str!("../fixtures/browse_owned_playlist.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            first.entries,
            [
                entry("D8K90hX4PrE", Some("1F3E5D7C9B0A2468")),
                // Only the "Remove from playlist" menu item has this one.
                entry("K0HSD_i2DvA", Some("D0A0EF93DCE5742B")),
                entry("FGBhQbmPwH8", Some("8D79FEDC0AA7D0F1")),
            ]
        );
        // The shelf's own token, not the suggestions token of the section.
        assert_eq!(first.ctoken.as_deref(), Some("4qmFsgKbARIkVkxQTHRlc3Q"));

        let next = parse_entry_page(
            &parse_json(include_str!(
                "../fixtures/browse_owned_playlist_continuation.json"
            ))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            next.entries,
            [
                entry("5NV6Rdv1a3I", Some("2D1A2B9E8E4D21F5")),
                entry("D8K90hX4PrE", Some("4A6C8E0B1D3F5A7C")),
            ]
        );
        assert_eq!(next.ctoken, None);

        // Both pages against the tracks rustypipe would return: the repeated
        // song keeps the id of its own copy.
        let all: Vec<RawEntry> = first.entries.into_iter().chain(next.entries).collect();
        let tracks = [
            "D8K90hX4PrE",
            "K0HSD_i2DvA",
            "FGBhQbmPwH8",
            "5NV6Rdv1a3I",
            "D8K90hX4PrE",
        ];
        let ids = align_entry_ids(&tracks, &all);
        assert_eq!(ids.len(), 5);
        assert_eq!(ids[4].as_deref(), Some("4A6C8E0B1D3F5A7C"));
    }

    #[test]
    fn old_style_continuation() {
        let v = json!({
            "continuationContents": {"musicPlaylistShelfContinuation": {
                "contents": [{"musicResponsiveListItemRenderer": {
                    "playlistItemData": {"videoId": "aaaaaaaaaaa", "playlistSetVideoId": "S1"}
                }}],
                "continuations": [{"nextContinuationData": {"continuation": "TOKEN"}}]
            }}
        });
        let page = parse_entry_page(&v).unwrap();
        assert_eq!(page.entries, [entry("aaaaaaaaaaa", Some("S1"))]);
        assert_eq!(page.ctoken.as_deref(), Some("TOKEN"));
    }

    #[test]
    fn not_owned_playlist_has_no_set_video_ids() {
        let v = json!({"contents": {"musicPlaylistShelfRenderer": {"contents": [
            {"musicResponsiveListItemRenderer": {"playlistItemData": {"videoId": "aaaaaaaaaaa"}}}
        ]}}});
        let page = parse_entry_page(&v).unwrap();
        assert_eq!(page.entries, [entry("aaaaaaaaaaa", None)]);
        assert_eq!(
            align_entry_ids(&["aaaaaaaaaaa"], &page.entries),
            Vec::<Option<String>>::new()
        );
        assert!(matches!(
            parse_entry_page(&json!({"contents": {}})),
            Err(BackendError::Extraction(_))
        ));
    }

    #[test]
    fn alignment() {
        let a = entry("A", Some("s1"));
        let b = entry("B", Some("s2"));
        let a2 = entry("A", Some("s3"));
        let live = entry("L", Some("s4"));
        let none = Vec::<Option<String>>::new();
        let s = |x: &str| Some(x.to_owned());

        // Same rows: positional, duplicates keep their own ids.
        let entries = [a.clone(), b.clone(), a2.clone()];
        assert_eq!(
            align_entry_ids(&["A", "B", "A"], &entries),
            [s("s1"), s("s2"), s("s3")]
        );
        // rustypipe dropped a livestream row.
        let entries = [a.clone(), live.clone(), b.clone()];
        assert_eq!(align_entry_ids(&["A", "B"], &entries), [s("s1"), s("s2")]);
        // A dropped row shares a video id with a kept track: ambiguous.
        let entries = [a.clone(), b.clone(), a2.clone()];
        assert_eq!(align_entry_ids(&["B", "A"], &entries), none);
        // A track the raw list lacks.
        assert_eq!(align_entry_ids(&["A", "C"], &[a.clone(), b.clone()]), none);
        // Different order.
        assert_eq!(align_entry_ids(&["B", "A"], &[a, b]), none);
        assert_eq!(align_entry_ids(&[], &[]), none);
    }

    fn found(body: &str) -> Lyrics {
        match parse_timed_lyrics(body).unwrap() {
            TimedLyrics::Found(l) => l,
            other => panic!("expected lyrics, got {other:?}"),
        }
    }

    #[test]
    fn timed_lyrics_synced() {
        let l = found(include_str!("../fixtures/lyrics_timed_synced.json"));
        assert!(l.synced);
        assert_eq!(l.lines.len(), 103);
        assert_eq!(l.source.as_deref(), Some("Source: LyricFind"));
        assert_eq!(
            l.lines[0],
            LyricLine {
                start: Some(Duration::from_millis(840)),
                text: "Television".to_owned(),
            }
        );
        assert_eq!(l.lines[102].start, Some(Duration::from_millis(196_120)));
        assert!(l.lines.windows(2).all(|w| w[0].start <= w[1].start));
    }

    #[test]
    fn timed_lyrics_without_cues_are_plain() {
        let l = found(include_str!("../fixtures/lyrics_timed_unsynced.json"));
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 40);
        assert!(l.lines.iter().all(|l| l.start.is_none()));
        assert_eq!(l.lines[0].text, "Rock, robot rock");
        assert_eq!(l.source.as_deref(), Some("Source: LyricFind"));
    }

    #[test]
    fn timed_lyrics_one_missing_cue_is_not_synced() {
        let body = json!({ "contents": { "elementRenderer": { "newElement": { "type": {
            "componentType": { "model": { "timedLyricsModel": { "lyricsData": {
                "timedLyricsData": [
                    { "lyricLine": "b", "cueRange": { "startTimeMilliseconds": "2000" } },
                    { "lyricLine": "a" },
                ],
            } } } } } } } } });
        let l = found(&body.to_string());
        assert!(!l.synced);
        // Unsynced lines keep YouTube's order.
        assert_eq!(l.lines[0].text, "b");
        assert_eq!(l.source, None);
    }

    #[test]
    fn timed_lyrics_unavailable_and_unknown() {
        assert_eq!(
            parse_timed_lyrics(include_str!("../fixtures/lyrics_timed_unavailable.json")).unwrap(),
            TimedLyrics::Unavailable
        );
        assert_eq!(
            parse_timed_lyrics(r#"{"contents":{"sectionListRenderer":{}}}"#).unwrap(),
            TimedLyrics::Unknown
        );
        assert!(matches!(
            parse_timed_lyrics(""),
            Err(BackendError::Network(_))
        ));
        assert!(matches!(
            parse_timed_lyrics("<html>"),
            Err(BackendError::Extraction(_))
        ));
    }
}
