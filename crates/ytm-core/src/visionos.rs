//! Stream extraction as YouTube's visionOS app (`VISIONOS`, client 101).
//!
//! This client needs no player JavaScript, no PO token and no cookies, and
//! its URLs serve whole files: checked 2026-10-28, Range requests at the
//! start, middle and end of a track all answered 206, where the iOS client
//! stops at 1 MiB. One `player` POST takes about 100 ms against 2 s for
//! yt-dlp. It is the only stream path: the fallbacks it replaced are in
//! `docs/design/stream-fallbacks.md`. The client fields follow yt-dlp's
//! `INNERTUBE_CLIENTS['visionos']`; when YouTube changes the client, start
//! there.
//!
//! The one thing YouTube does check is a visitor id: without
//! `X-Goog-Visitor-Id` the endpoint answers `LOGIN_REQUIRED` ("Sign in to
//! confirm you're not a bot"). rustypipe caches one per install.

use std::time::Duration;

use model::{AudioCodec, StreamSource};
use serde::Deserialize;
use serde_json::json;

use crate::innertube;
use crate::rustypipe_backend::ITAG_PREFERENCE;
use crate::stream_cache::url_param;

const PLAYER_URL: &str = "https://www.youtube.com/youtubei/v1/player?prettyPrint=false";
const CLIENT_NAME: &str = "VISIONOS";
const CLIENT_VERSION: &str = "1.02";
/// `INNERTUBE_CONTEXT_CLIENT_NAME`, sent as `X-YouTube-Client-Name`.
const CLIENT_ID: &str = "101";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 15_7_3) AppleWebKit/605.1.15 \
                          (KHTML, like Gecko) Version/26.0 Safari/605.1.15";
/// Well above the ~100 ms a reply takes.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub(crate) enum VisionOsError {
    #[error("network error: {0}")]
    Network(String),
    /// YouTube says the video does not exist.
    #[error("video not found")]
    NotFound,
    /// `LOGIN_REQUIRED`: YouTube wants a sign-in for this visitor id, which
    /// usually means the id is burnt. The caller drops it and retries once.
    #[error("bot check: {0}")]
    BotCheck(String),
    /// Anything else: unplayable, no direct URL, no audio format we take.
    #[error("{0}")]
    Unusable(String),
}

/// Resolves the best audio stream for `track_id` as the visionOS client.
pub(crate) async fn stream(
    visitor_data: &str,
    track_id: &str,
) -> Result<StreamSource, VisionOsError> {
    let body = json!({
        "context": { "client": {
            "clientName": CLIENT_NAME,
            "clientVersion": CLIENT_VERSION,
            "deviceMake": "Apple",
            "deviceModel": "RealityDevice17,1",
            "osName": "visionOS",
            "osVersion": "26.5.23O471",
            "hl": "en",
            "visitorData": visitor_data,
        } },
        "videoId": track_id,
        "contentCheckOk": true,
        "racyCheckOk": true,
    });
    let net = |e: reqwest::Error| VisionOsError::Network(e.without_url().to_string());
    let resp = innertube::http()
        .map_err(|e| VisionOsError::Unusable(e.to_string()))?
        .post(PLAYER_URL)
        .timeout(TIMEOUT)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header("X-YouTube-Client-Name", CLIENT_ID)
        .header("X-YouTube-Client-Version", CLIENT_VERSION)
        .header("X-Goog-Visitor-Id", visitor_data)
        .body(body.to_string())
        .send()
        .await
        .map_err(net)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.map_err(net)?;
    if !(200..300).contains(&status) {
        return Err(VisionOsError::Unusable(format!("HTTP {status}")));
    }
    parse_player(track_id, &text)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response {
    playability_status: Option<Playability>,
    streaming_data: Option<StreamingData>,
}

#[derive(Deserialize)]
struct Playability {
    status: Option<String>,
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamingData {
    #[serde(default)]
    adaptive_formats: Vec<Format>,
}

/// The fields we read from one `adaptiveFormats` entry.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Format {
    itag: u32,
    /// Absent when the URL is ciphered (`signatureCipher`), which this client
    /// does not do; such a format is skipped rather than deobfuscated.
    url: Option<String>,
    mime_type: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    average_bitrate: u32,
    /// Bytes, as a decimal string.
    content_length: Option<String>,
    /// Milliseconds, as a decimal string.
    approx_duration_ms: Option<String>,
    loudness_db: Option<f32>,
    /// Set on videos with several audio languages.
    audio_track: Option<AudioTrack>,
    #[serde(default)]
    drm_families: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AudioTrack {
    #[serde(default)]
    audio_is_default: bool,
}

impl Format {
    fn codec(&self) -> Option<AudioCodec> {
        let (kind, codecs) = self.mime_type.split_once(';')?;
        if !kind.trim().starts_with("audio/") {
            return None;
        }
        if codecs.contains("opus") {
            Some(AudioCodec::Opus)
        } else if codecs.contains("mp4a") {
            Some(AudioCodec::Aac)
        } else {
            None
        }
    }

    fn usable(&self) -> bool {
        self.url.is_some() && self.drm_families.is_empty() && self.codec().is_some()
    }

    fn default_track(&self) -> bool {
        self.audio_track.as_ref().is_none_or(|t| t.audio_is_default)
    }
}

/// Parses a `player` response into the best audio stream, in
/// [`ITAG_PREFERENCE`] order, preferring the default audio track.
pub(crate) fn parse_player(track_id: &str, body: &str) -> Result<StreamSource, VisionOsError> {
    let r: Response =
        serde_json::from_str(body).map_err(|e| VisionOsError::Unusable(format!("player: {e}")))?;
    let status = r.playability_status.unwrap_or(Playability {
        status: None,
        reason: None,
    });
    let reason = status.reason.unwrap_or_default();
    match status.status.as_deref() {
        Some("OK") => {}
        Some("LOGIN_REQUIRED") => return Err(VisionOsError::BotCheck(reason)),
        Some("ERROR") if reason.to_lowercase().contains("unavailable") => {
            return Err(VisionOsError::NotFound);
        }
        other => {
            return Err(VisionOsError::Unusable(format!(
                "playability {}: {reason}",
                other.unwrap_or("missing")
            )));
        }
    }
    let formats = r
        .streaming_data
        .map(|d| d.adaptive_formats)
        .unwrap_or_default();
    let best = |only_default: bool| {
        ITAG_PREFERENCE.iter().find_map(|&itag| {
            formats
                .iter()
                .filter(|f| f.usable() && (!only_default || f.default_track()))
                .find(|f| f.itag == itag)
        })
    };
    let f = best(true).or_else(|| best(false)).ok_or_else(|| {
        VisionOsError::Unusable(format!(
            "no usable audio format for {track_id} (got itags {:?})",
            formats.iter().map(|f| f.itag).collect::<Vec<_>>()
        ))
    })?;
    let url = f.url.clone().expect("usable formats have a URL");
    let content_length = f
        .content_length
        .as_deref()
        .and_then(|n| n.parse().ok())
        .or_else(|| url_param(&url, "clen")?.parse().ok())
        .ok_or_else(|| {
            VisionOsError::Unusable("no contentLength and no clen= in url".to_owned())
        })?;
    let duration = f
        .approx_duration_ms
        .as_deref()
        .and_then(|ms| ms.parse().ok())
        .map(Duration::from_millis)
        .or_else(|| {
            let secs: f64 = url_param(&url, "dur")?.parse().ok()?;
            Duration::try_from_secs_f64(secs).ok()
        });
    Ok(StreamSource {
        track_id: track_id.to_owned(),
        url,
        codec: f.codec().expect("usable formats have a codec"),
        bitrate_bps: if f.average_bitrate > 0 {
            f.average_bitrate
        } else {
            f.bitrate
        },
        content_length,
        duration,
        loudness_db: f.loudness_db,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/visionos_player.json");

    fn without_itags(itags: &[u32]) -> String {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let formats = v["streamingData"]["adaptiveFormats"]
            .as_array_mut()
            .unwrap();
        formats.retain(|f| !itags.contains(&(f["itag"].as_u64().unwrap() as u32)));
        v.to_string()
    }

    #[test]
    fn picks_opus_251_from_recorded_response() {
        let s = parse_player("jvm6DpqqbLk", FIXTURE).unwrap();
        assert_eq!(s.track_id, "jvm6DpqqbLk");
        assert_eq!(s.codec, AudioCodec::Opus);
        assert_eq!(url_param(&s.url, "itag"), Some("251"));
        assert_eq!(s.content_length, 5_114_228);
        assert_eq!(s.duration, Some(Duration::from_millis(317_781)));
        assert_eq!(s.loudness_db, Some(5.29));
        assert_eq!(s.bitrate_bps, 128_748);
    }

    #[test]
    fn falls_back_to_aac_then_rejects_video_only() {
        // Without 251 the next accepted itag is 140, never the lower Opus 250.
        let s = parse_player("jvm6DpqqbLk", &without_itags(&[251])).unwrap();
        assert_eq!(s.codec, AudioCodec::Aac);
        assert_eq!(url_param(&s.url, "itag"), Some("140"));
        assert_eq!(s.content_length, 5_145_975);
        // Only the video format and Opus 250 left: nothing we take.
        let e = parse_player("jvm6DpqqbLk", &without_itags(&[251, 140])).unwrap_err();
        assert!(
            matches!(e, VisionOsError::Unusable(ref m) if m.contains("[136, 250]")),
            "{e}"
        );
    }

    #[test]
    fn skips_ciphered_and_drm_formats_and_prefers_default_track() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let formats = v["streamingData"]["adaptiveFormats"]
            .as_array_mut()
            .unwrap();
        for f in formats.iter_mut() {
            match f["itag"].as_u64().unwrap() {
                251 => {
                    f.as_object_mut().unwrap().remove("url");
                    f["signatureCipher"] = "s=abc&url=x".into();
                }
                140 => f["drmFamilies"] = serde_json::json!(["widevine"]),
                _ => {}
            }
        }
        let e = parse_player("jvm6DpqqbLk", &v.to_string()).unwrap_err();
        assert!(matches!(e, VisionOsError::Unusable(_)), "{e}");

        // Two Opus 251 tracks (dubbed audio): the default one wins even
        // though it is listed second.
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        let formats = v["streamingData"]["adaptiveFormats"]
            .as_array_mut()
            .unwrap();
        let mut dub = formats.iter().find(|f| f["itag"] == 251).unwrap().clone();
        let mut original = dub.clone();
        dub["audioTrack"] = serde_json::json!({ "audioIsDefault": false });
        dub["url"] = "https://example.invalid/dub?clen=1".into();
        original["audioTrack"] = serde_json::json!({ "audioIsDefault": true });
        original["url"] = "https://example.invalid/original?clen=1".into();
        formats.retain(|f| f["itag"] != 251);
        formats.insert(0, dub);
        formats.push(original);
        let s = parse_player("jvm6DpqqbLk", &v.to_string()).unwrap();
        assert!(s.url.contains("/original"), "{}", s.url);
    }

    #[test]
    fn length_and_duration_fall_back_to_url_params() {
        let mut v: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
        for f in v["streamingData"]["adaptiveFormats"]
            .as_array_mut()
            .unwrap()
        {
            let o = f.as_object_mut().unwrap();
            o.remove("contentLength");
            o.remove("approxDurationMs");
            o.remove("averageBitrate");
        }
        let s = parse_player("jvm6DpqqbLk", &v.to_string()).unwrap();
        assert_eq!(s.content_length, 5_114_228);
        assert_eq!(s.duration, Some(Duration::from_secs_f64(317.781)));
        assert_eq!(s.bitrate_bps, 135_567);
    }

    #[test]
    fn playability_statuses() {
        let body = |status: &str, reason: &str| {
            serde_json::json!({ "playabilityStatus": { "status": status, "reason": reason } })
                .to_string()
        };
        let e = parse_player(
            "x",
            &body("LOGIN_REQUIRED", "Sign in to confirm you’re not a bot"),
        );
        assert!(matches!(e, Err(VisionOsError::BotCheck(ref r)) if r.contains("bot")));
        let e = parse_player("x", &body("ERROR", "Video unavailable"));
        assert!(matches!(e, Err(VisionOsError::NotFound)));
        let e = parse_player("x", &body("UNPLAYABLE", "This video is private"));
        assert!(matches!(e, Err(VisionOsError::Unusable(ref m)) if m.contains("UNPLAYABLE")));
        // OK but no streams, and plain garbage.
        let e = parse_player("x", &body("OK", ""));
        assert!(matches!(e, Err(VisionOsError::Unusable(ref m)) if m.contains("got itags []")));
        assert!(matches!(
            parse_player("x", "<html>"),
            Err(VisionOsError::Unusable(_))
        ));
    }
}
