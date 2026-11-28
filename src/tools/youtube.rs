//! `parse_youtube_captions`: a pure, offline tool that turns caption text (SRT, VTT or text
//! copied from YouTube's transcript panel) into numbered, timed clips. It never fetches
//! YouTube and never calls the EngrAmo API; see `crate::youtube` for the segmenter.

use rmcp::{
    ErrorData, handler::server::wrapper::Parameters, model::CallToolResult, schemars, tool,
    tool_router,
};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use tokio::sync::Semaphore;

use crate::server::EngramoMcpServer;
use crate::tools::catalogs::{err_result, ok_json};
use crate::youtube::segmenter::{
    CaptionFormat, MAX_INPUT_CHARS, SegmentOptions, SegmentWarning, SplitBy, segment_captions,
};
use crate::youtube::{extract_video_id, parse_time_ms};

/// Maximum accepted size of the `captions` parameter, in bytes (same 2 MB figure as the
/// segmenter's char cap, so the segmenter's own truncation is never reached from the tool).
pub(crate) const MAX_CAPTIONS_BYTES: usize = MAX_INPUT_CHARS;

/// Maximum clips returned per call (web default); applied AFTER the from/to filter.
const MAX_CLIPS: usize = 200;

/// Max concurrent caption parses (each holds tens of MB on a 2 MB input).
const PARSE_CONCURRENCY: usize = 2;
static PARSE_PERMITS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(PARSE_CONCURRENCY));

#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionsFormat {
    #[default]
    Auto,
    Srt,
    Vtt,
    Panel,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionsSplit {
    /// Default, same as the web wizard.
    #[default]
    Line,
    Sentence,
    Auto,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ParseYoutubeCaptionsParams {
    #[schemars(
        description = "YouTube video URL (watch?v=, youtu.be/, shorts/, embed/, m.youtube.com, extra \
        query params such as t= are fine) or a bare 11-character video id."
    )]
    pub video: String,
    #[schemars(
        description = "The caption text: SRT, WebVTT, or text copied from YouTube's 'Show transcript' \
        panel. Max 2 MB."
    )]
    pub captions: String,
    #[schemars(
        description = "Caption format: 'auto' (default, detected), 'srt', 'vtt' or 'panel' (YouTube \
        transcript panel text)."
    )]
    pub format: Option<CaptionsFormat>,
    #[schemars(
        description = "How to cut clips: 'line' (default; each caption line is a clip), 'sentence' \
        (join cues until sentence punctuation) or 'auto' (picks 'line' for YouTube automatic \
        captions that have no punctuation, else 'sentence')."
    )]
    pub split: Option<CaptionsSplit>,
    #[schemars(
        description = "Optional start of the range to keep: 'm:ss', 'h:mm:ss' or plain seconds \
        (e.g. '90'). A clip is kept only if it lies fully inside [from, to]."
    )]
    pub from: Option<String>,
    #[schemars(
        description = "Optional end of the range to keep: 'm:ss', 'h:mm:ss' or plain seconds. A clip \
        is kept only if it lies fully inside [from, to]. Must be greater than from."
    )]
    pub to: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YoutubeClip {
    /// 1-based position in the full segmented list, stable under the from/to filter.
    pub index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedYoutubeCaptions {
    pub video_id: String,
    pub clips: Vec<YoutubeClip>,
    pub warnings: Vec<&'static str>,
}

/// Pure core of the tool (no I/O): validates the inputs and segments the captions.
pub fn parse_youtube_captions(
    p: &ParseYoutubeCaptionsParams,
) -> Result<ParsedYoutubeCaptions, String> {
    let video_id = extract_video_id(&p.video)?;
    if p.captions.len() > MAX_CAPTIONS_BYTES {
        return Err(format!(
            "captions are too large ({} bytes); the limit is {MAX_CAPTIONS_BYTES} bytes (2 MB). \
             Send a shorter excerpt.",
            p.captions.len()
        ));
    }
    if p.captions.trim().is_empty() {
        return Err(
            "captions are empty: paste SRT, VTT or YouTube transcript-panel text".to_owned(),
        );
    }
    let from_ms = p
        .from
        .as_deref()
        .map(|s| parse_time_ms(s).map_err(|e| format!("from: {e}")))
        .transpose()?;
    let to_ms =
        p.to.as_deref()
            .map(|s| parse_time_ms(s).map_err(|e| format!("to: {e}")))
            .transpose()?;
    if let (Some(f), Some(t)) = (from_ms, to_ms)
        && f >= t
    {
        return Err("invalid range: from must be earlier than to".to_owned());
    }

    let opts = SegmentOptions {
        format: match p.format.unwrap_or_default() {
            CaptionsFormat::Auto => None,
            CaptionsFormat::Srt => Some(CaptionFormat::Srt),
            CaptionsFormat::Vtt => Some(CaptionFormat::Vtt),
            CaptionsFormat::Panel => Some(CaptionFormat::TranscriptPanel),
        },
        split_by: Some(match p.split.unwrap_or_default() {
            CaptionsSplit::Auto => SplitBy::Auto,
            CaptionsSplit::Sentence => SplitBy::Sentence,
            CaptionsSplit::Line => SplitBy::Line,
        }),
        // Uncapped here: the 200-clip cap is applied after the from/to filter so a range past
        // clip 200 stays reachable.
        max_clips: Some(f64::MAX),
        ..SegmentOptions::default()
    };
    let result = segment_captions(&p.captions, &opts);

    // Numbering is fixed on the full list; the range filter only hides clips.
    let mut warnings: Vec<&'static str> = result.warnings.iter().map(|w| w.as_str()).collect();
    let mut clips: Vec<YoutubeClip> = result
        .segments
        .into_iter()
        .filter(|s| from_ms.is_none_or(|f| s.start_ms >= f) && to_ms.is_none_or(|t| s.end_ms <= t))
        .map(|s| YoutubeClip {
            index: s.index + 1,
            start_ms: s.start_ms,
            end_ms: s.end_ms,
            text: s.text,
        })
        .collect();
    if clips.len() > MAX_CLIPS {
        clips.truncate(MAX_CLIPS);
        let w = SegmentWarning::TruncatedToMaxClips.as_str();
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    }
    Ok(ParsedYoutubeCaptions {
        video_id,
        clips,
        warnings,
    })
}

#[tool_router(router = youtube_tools_router, vis = "pub(crate)")]
impl EngramoMcpServer {
    #[tool(
        title = "Parse YouTube captions",
        annotations(
            title = "Parse YouTube captions",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        description = "Turn YouTube captions into numbered, timed clips (startMs/endMs). Pure and \
        offline: it never fetches YouTube and makes no API call. Clip times come ONLY from this \
        tool: never invent or edit them. Captions come from the user (SRT, VTT, or text copied \
        from YouTube's 'Show transcript' panel) or, if you can watch the YouTube video yourself \
        (e.g. Gemini), you may write SRT captions yourself first. After parsing, ALWAYS show the \
        user the numbered clips (`#index  m:ss-m:ss  text`) and ask which to keep (\"all\", \
        numbers/ranges, a time range, or a wish) before translating or creating cards. Optional \
        from/to keep only clips fully inside that range; clip numbers do not change when \
        filtering. At most 200 clips are returned per call (after the from/to filter): on long \
        videos use from/to to reach later parts."
    )]
    pub async fn parse_youtube_captions(
        &self,
        Parameters(p): Parameters<ParseYoutubeCaptionsParams>,
    ) -> Result<CallToolResult, ErrorData> {
        // Bound concurrency: each parse holds tens of MB on a 2 MB input.
        let Ok(permit) = PARSE_PERMITS.acquire().await else {
            return Ok(err_result("failed to parse captions"));
        };
        // CPU-bound over up to 2 MB: keep it off the async executor.
        Ok(
            match tokio::task::spawn_blocking(move || {
                let _permit = permit; // held until the parse really ends
                parse_youtube_captions(&p)
            })
            .await
            {
                Ok(Ok(out)) => ok_json(&out),
                Ok(Err(e)) => err_result(e),
                Err(_) => err_result("failed to parse captions"),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:03,000\nHola, ¿cómo estás?\n\n\
        2\n00:00:03,200 --> 00:00:05,500\nMuy bien, gracias.\n\n\
        3\n00:00:20,000 --> 00:00:22,000\nVamos a la playa hoy.\n";

    fn params(captions: &str) -> ParseYoutubeCaptionsParams {
        ParseYoutubeCaptionsParams {
            video: "https://youtu.be/dQw4w9WgXcQ?t=5".to_owned(),
            captions: captions.to_owned(),
            format: None,
            split: None,
            from: None,
            to: None,
        }
    }

    #[test]
    fn parses_clips_one_based_with_camel_case_json() {
        let out = parse_youtube_captions(&params(SRT)).unwrap();
        assert_eq!(out.video_id, "dQw4w9WgXcQ");
        assert_eq!(out.clips.len(), 3);
        assert_eq!(out.clips[0].index, 1);
        assert_eq!(out.clips[2].index, 3);
        assert!(out.warnings.is_empty());
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["videoId"], "dQw4w9WgXcQ");
        assert_eq!(json["clips"][0]["startMs"], out.clips[0].start_ms);
        assert!(json["clips"][0]["endMs"].is_i64());
        assert_eq!(json["clips"][0]["text"], "Hola, ¿cómo estás?");
        assert!(json["warnings"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_parse_youtube_captions_range_filter_stable_numbering() {
        let mut p = params(SRT);
        p.from = Some("0:15".into());
        p.to = Some("30".into());
        let out = parse_youtube_captions(&p).unwrap();
        assert_eq!(out.clips.len(), 1);
        assert_eq!(out.clips[0].index, 3);
        assert_eq!(out.clips[0].text, "Vamos a la playa hoy.");

        // A clip that merely overlaps the range edge is dropped.
        let mut p = params(SRT);
        p.from = Some("2".into());
        let out = parse_youtube_captions(&p).unwrap();
        assert_eq!(
            out.clips.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn default_split_is_line_like_the_web_wizard() {
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nHola mi buen\n\n\
            2\n00:00:02,600 --> 00:00:04,000\namigo de siempre.\n";
        let out = parse_youtube_captions(&params(srt)).unwrap();
        assert_eq!(out.clips.len(), 2);
        let mut p = params(srt);
        p.split = Some(CaptionsSplit::Sentence);
        assert_eq!(parse_youtube_captions(&p).unwrap().clips.len(), 1);
    }

    #[test]
    fn test_parse_youtube_captions_explicit_format_split_and_warnings() {
        let mut p = params("0:01\nHola a todos.\n0:04\nAdios amigos mios.");
        p.format = Some(CaptionsFormat::Panel);
        p.split = Some(CaptionsSplit::Line);
        let out = parse_youtube_captions(&p).unwrap();
        assert_eq!(out.warnings, vec!["panel_format_low_precision"]);
        assert!(!out.clips.is_empty());
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        let mut p = params(SRT);
        p.video = "https://example.com/x".into();
        assert!(parse_youtube_captions(&p).unwrap_err().contains("video"));

        let mut p = params(SRT);
        p.from = Some("bogus".into());
        assert!(parse_youtube_captions(&p).unwrap_err().starts_with("from:"));

        let mut p = params(SRT);
        p.to = Some("1:xx".into());
        assert!(parse_youtube_captions(&p).unwrap_err().starts_with("to:"));

        let mut p = params(SRT);
        p.from = Some("30".into());
        p.to = Some("0:30".into());
        assert!(parse_youtube_captions(&p).unwrap_err().contains("earlier"));

        assert!(
            parse_youtube_captions(&params("   \n "))
                .unwrap_err()
                .contains("empty")
        );

        let big = "x".repeat(MAX_CAPTIONS_BYTES + 1);
        assert!(
            parse_youtube_captions(&params(&big))
                .unwrap_err()
                .contains("too large")
        );
        // exactly at the limit is accepted
        let ok = "x".repeat(MAX_CAPTIONS_BYTES);
        assert!(parse_youtube_captions(&params(&ok)).is_ok());
    }

    #[test]
    fn params_deserialize_snake_case_enums() {
        let p: ParseYoutubeCaptionsParams = serde_json::from_value(serde_json::json!({
            "video": "dQw4w9WgXcQ", "captions": "x", "format": "panel", "split": "sentence"
        }))
        .unwrap();
        assert!(matches!(p.format, Some(CaptionsFormat::Panel)));
        assert!(matches!(p.split, Some(CaptionsSplit::Sentence)));
        let bad = serde_json::from_value::<ParseYoutubeCaptionsParams>(serde_json::json!({
            "video": "dQw4w9WgXcQ", "captions": "x", "format": "transcript_panel"
        }));
        assert!(bad.is_err());
    }

    fn text_of(r: &CallToolResult) -> String {
        serde_json::to_value(&r.content[0]).unwrap()["text"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[tokio::test]
    async fn test_parse_youtube_captions_handler_json_and_error_results() {
        let client = crate::client::EngramoClient::new("http://localhost", "engramo_test");
        let server = EngramoMcpServer::new(client, false);
        let ok = server
            .parse_youtube_captions(Parameters(params(SRT)))
            .await
            .unwrap();
        assert_ne!(ok.is_error, Some(true));
        let json: serde_json::Value = serde_json::from_str(&text_of(&ok)).unwrap();
        assert_eq!(json["videoId"], "dQw4w9WgXcQ");
        let mut bad = params(SRT);
        bad.video = "nope".into();
        let err = server
            .parse_youtube_captions(Parameters(bad))
            .await
            .unwrap();
        assert_eq!(err.is_error, Some(true));
        assert!(text_of(&err).contains("video must be a YouTube URL"));
    }

    #[tokio::test]
    async fn test_parse_youtube_captions_concurrent_calls_beyond_limit_all_succeed() {
        let client = crate::client::EngramoClient::new("http://localhost", "engramo_test");
        let server = std::sync::Arc::new(EngramoMcpServer::new(client, false));
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..=PARSE_CONCURRENCY {
            let server = std::sync::Arc::clone(&server);
            set.spawn(async move { server.parse_youtube_captions(Parameters(params(SRT))).await });
        }
        let mut done = 0;
        while let Some(joined) = set.join_next().await {
            assert_ne!(joined.unwrap().unwrap().is_error, Some(true));
            done += 1;
        }
        assert_eq!(done, PARSE_CONCURRENCY + 1);
    }

    #[test]
    fn test_parse_youtube_captions_explicit_formats_and_auto_split_mapped() {
        // An explicit format overrides detection: panel text forced to SRT gives no clips.
        let mut p = params("0:01\nHola a todos.\n0:04\nAdios amigos mios.");
        p.format = Some(CaptionsFormat::Srt);
        let out = parse_youtube_captions(&p).unwrap();
        assert!(out.clips.is_empty());
        assert!(out.warnings.is_empty());

        // Header-less VTT forced to Vtt parses.
        let mut p = params("00:00:01.000 --> 00:00:03.000\nHello there my friend.\n");
        p.format = Some(CaptionsFormat::Vtt);
        assert_eq!(parse_youtube_captions(&p).unwrap().clips.len(), 1);

        // split=auto on punctuated captions picks sentence mode (unlike the line default).
        let srt = "1\n00:00:01,000 --> 00:00:02,500\nHola mi buen\n\n\
                   2\n00:00:02,600 --> 00:00:04,000\namigo de siempre.\n";
        let mut p = params(srt);
        p.split = Some(CaptionsSplit::Auto);
        assert_eq!(parse_youtube_captions(&p).unwrap().clips.len(), 1);
    }

    #[test]
    fn test_parse_youtube_captions_range_to_only_and_inclusive_boundaries() {
        let all = parse_youtube_captions(&params(SRT)).unwrap();
        let (c2_end, c3_start) = (all.clips[1].end_ms, all.clips[2].start_ms);

        let mut p = params(SRT);
        p.to = Some(format!("{}", c2_end as f64 / 1000.0));
        let out = parse_youtube_captions(&p).unwrap();
        assert_eq!(
            out.clips.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![1, 2]
        );

        let mut p = params(SRT);
        p.from = Some(format!("{}", c3_start as f64 / 1000.0));
        let out = parse_youtube_captions(&p).unwrap();
        assert_eq!(
            out.clips.iter().map(|c| c.index).collect::<Vec<_>>(),
            vec![3]
        );
    }

    #[test]
    fn test_parse_youtube_captions_limit_counts_bytes_not_chars() {
        let big = "é".repeat(MAX_CAPTIONS_BYTES / 2 + 1); // chars < limit, bytes > limit
        assert!(
            parse_youtube_captions(&params(&big))
                .unwrap_err()
                .contains("too large")
        );
        let ok = "é".repeat(MAX_CAPTIONS_BYTES / 2); // exactly at the limit in bytes
        assert!(parse_youtube_captions(&params(&ok)).is_ok());
    }

    fn many_cues(n: usize) -> String {
        let ts = |ms: usize| {
            format!(
                "{:02}:{:02}:{:02},{:03}",
                ms / 3_600_000,
                ms / 60_000 % 60,
                ms / 1000 % 60,
                ms % 1000
            )
        };
        (0..n)
            .map(|i| {
                format!(
                    "{}\n{} --> {}\nFrase numero {i} del video\n",
                    i + 1,
                    ts(i * 5000),
                    ts(i * 5000 + 3000)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn test_parse_youtube_captions_cap_applies_after_range_filter() {
        let captions = many_cues(500);
        let out = parse_youtube_captions(&params(&captions)).unwrap();
        assert_eq!(out.clips.len(), MAX_CLIPS);
        assert!(out.warnings.contains(&"truncated_to_max_clips"));

        // A range past clip 200 stays reachable, with stable numbering and no truncation.
        let mut p = params(&captions);
        p.from = Some("1500".into()); // cue #301 starts at 1500 s
        p.to = Some("1700".into());
        let out = parse_youtube_captions(&p).unwrap();
        assert!(!out.clips.is_empty());
        assert!(out.clips[0].index > MAX_CLIPS);
        assert!(!out.warnings.contains(&"truncated_to_max_clips"));
    }
}
