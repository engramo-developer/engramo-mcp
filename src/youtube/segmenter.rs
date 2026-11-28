//! Caption segmenter: turns SRT, VTT or text copied from YouTube's "Show transcript" panel
//! into sentence-level clips. Pure, no I/O.
//!
//! A faithful port of the web app's `youtube-segmenter.ts` (engram-web-app,
//! `src/app/shared/utils/youtube-segmenter/`) so the MCP tool and the web wizard cut the same
//! clips. Parsing is hand-written (no regex crate) and bounded so long hostile lines stay linear.

use std::collections::{HashSet, VecDeque};

/// A concrete caption format (the TS `'auto'` is `Option::None` in [`SegmentOptions::format`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionFormat {
    Srt,
    Vtt,
    TranscriptPanel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitBy {
    Auto,
    Sentence,
    Line,
}

/// The split mode actually used (resolves `Auto`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Sentence,
    Line,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentWarning {
    CuesBeyondDuration,
    PanelFormatLowPrecision,
    TruncatedToMaxClips,
    DuplicatesRemoved,
    InputTruncated,
}

impl SegmentWarning {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CuesBeyondDuration => "cues_beyond_duration",
            Self::PanelFormatLowPrecision => "panel_format_low_precision",
            Self::TruncatedToMaxClips => "truncated_to_max_clips",
            Self::DuplicatesRemoved => "duplicates_removed",
            Self::InputTruncated => "input_truncated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// 0-based position in the segmented list.
    pub index: usize,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SegmentOptions {
    /// `None` = auto-detect.
    pub format: Option<CaptionFormat>,
    pub duration_ms: Option<f64>,
    /// Default 15000, clamped to 1000..=30000.
    pub max_clip_ms: Option<f64>,
    /// Default 200.
    pub max_clips: Option<f64>,
    /// Default `Auto`.
    pub split_by: Option<SplitBy>,
}

#[derive(Debug, Clone)]
pub struct SegmentResult {
    pub format: CaptionFormat,
    pub segments: Vec<Segment>,
    pub warnings: Vec<SegmentWarning>,
    pub split_by: Split,
}

/// A parsed cue; `lines` are the raw text lines (before cleaning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCue {
    pub start_ms: i64,
    pub end_ms: i64,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
struct Cue {
    start_ms: i64,
    end_ms: i64,
    text: String,
}

#[derive(Debug, Clone)]
struct Unit {
    start_ms: i64,
    end_ms: i64,
    text: String,
    cues: Vec<Cue>,
    /// Replaces the TS identity-based `stuck` set (joined units are new, so start unstuck).
    stuck: bool,
}

const DEFAULT_MAX_CLIP_MS: f64 = 15_000.0;
const HARD_MAX_CLIP_MS: f64 = 30_000.0;
const DEFAULT_MAX_CLIPS: f64 = 200.0;
const SENTENCE_GAP_MS: i64 = 700;
const MIN_CLIP_MS: i64 = 500;
const MIN_WORDS: usize = 3;
const LEAD_MAX_MS: f64 = 300.0;
const TAIL_MAX_MS: f64 = 400.0;
const TAIL_MIN_MS: f64 = 150.0;
const PANEL_LAST_CUE_MS: i64 = 3000;
const ASR_PUNCTUATED_MAX_RATIO: f64 = 0.2;
const ASR_OVERLAP_MIN_RATIO: f64 = 0.5;
/// Too few cues to judge punctuation reliably.
const ASR_MIN_SAMPLE: usize = 5;

pub const MAX_INPUT_CHARS: usize = 2_000_000;
const DETECT_SNIFF_LINES: usize = 500;
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

// ── small char helpers (JS regex semantics) ───────────────────────────────────

/// JS `\s`: Unicode whitespace plus the BOM.
fn is_ws(c: char) -> bool {
    c.is_whitespace() || c == '\u{FEFF}'
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

fn skip_ws(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && is_ws(chars[i]) {
        i += 1;
    }
    i
}

fn digits_at(chars: &[char], i: usize, max: usize) -> usize {
    let mut n = 0;
    while n < max && i + n < chars.len() && is_digit(chars[i + n]) {
        n += 1;
    }
    n
}

fn starts_with_at(chars: &[char], i: usize, pat: &str) -> bool {
    pat.chars()
        .enumerate()
        .all(|(n, p)| chars.get(i + n) == Some(&p))
}

fn word_count(text: &str) -> usize {
    text.split(is_ws).filter(|w| !w.is_empty()).count()
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

// ── timestamps ────────────────────────────────────────────────────────────────

/// Core of the TS timestamp pattern at `p`: `\d{1,2}:\d{2}(?:[.,]\d{1,3})?`. Returns the end.
fn ts_core(chars: &[char], p: usize) -> Option<usize> {
    for k in [2usize, 1] {
        if digits_at(chars, p, k) != k || chars.get(p + k) != Some(&':') {
            continue;
        }
        let q = p + k + 1;
        if digits_at(chars, q, 2) != 2 {
            continue;
        }
        let mut end = q + 2;
        if matches!(chars.get(end), Some('.') | Some(',')) {
            let f = digits_at(chars, end + 1, 3);
            if f >= 1 {
                end += 1 + f;
            }
        }
        return Some(end);
    }
    None
}

/// Candidate ends of a timestamp starting at `i`: with an hours prefix first, then without.
fn ts_ends(chars: &[char], i: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let a = digits_at(chars, i, 3);
    if a >= 1
        && chars.get(i + a) == Some(&':')
        && let Some(e) = ts_core(chars, i + a + 1)
    {
        out.push(e);
    }
    if let Some(e) = ts_core(chars, i) {
        out.push(e);
    }
    out
}

/// `(?<!\d)(TS)\s*-->\s*(TS)` — returns the two timestamp strings.
fn timed_match(line: &[char]) -> Option<(String, String)> {
    for i in 0..line.len() {
        if !is_digit(line[i]) || (i > 0 && is_digit(line[i - 1])) {
            continue;
        }
        for e in ts_ends(line, i) {
            let j = skip_ws(line, e);
            if !starts_with_at(line, j, "-->") {
                continue;
            }
            let j = skip_ws(line, j + 3);
            if let Some(e2) = ts_ends(line, j).first().copied() {
                let a: String = line[i..e].iter().collect();
                let b: String = line[j..e2].iter().collect();
                return Some((a, b));
            }
        }
    }
    None
}

/// `(?<!\d)\d{1,3}:\d{2}:\d{2},\d{1,3}\s*-->`
fn srt_timed(line: &[char]) -> bool {
    for i in 0..line.len() {
        if !is_digit(line[i]) || (i > 0 && is_digit(line[i - 1])) {
            continue;
        }
        let a = digits_at(line, i, 3);
        if a < 1 || line.get(i + a) != Some(&':') {
            continue;
        }
        let p = i + a + 1;
        if digits_at(line, p, 2) != 2 || line.get(p + 2) != Some(&':') {
            continue;
        }
        let q = p + 3;
        if digits_at(line, q, 2) != 2 || line.get(q + 2) != Some(&',') {
            continue;
        }
        let r = q + 3;
        let f = digits_at(line, r, 3);
        if f < 1 {
            continue;
        }
        if starts_with_at(line, skip_ws(line, r + f), "-->") {
            return true;
        }
    }
    false
}

fn parse_timestamp(ts: &str) -> i64 {
    let ts = ts.replacen(',', ".", 1);
    let mut it = ts.split('.');
    let clock = it.next().unwrap_or("");
    let frac = it.next().unwrap_or("");
    let parts: Vec<i64> = clock
        .split(':')
        .map(|p| p.parse::<i64>().unwrap_or(0))
        .collect();
    let (h, m, s) = if parts.len() == 3 {
        (parts[0], parts[1], parts[2])
    } else {
        (
            0,
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
        )
    };
    let ms = if frac.is_empty() {
        0
    } else {
        let mut f: String = frac.chars().take(3).collect();
        while f.len() < 3 {
            f.push('0');
        }
        f.parse::<i64>().unwrap_or(0)
    };
    (h * 3600 + m * 60 + s) * 1000 + ms
}

// ── format detection and parsing ──────────────────────────────────────────────

pub fn detect_format(input: &str) -> CaptionFormat {
    let text = js_trim(strip_bom(input));
    if text.starts_with("WEBVTT") {
        return CaptionFormat::Vtt;
    }
    let lines: Vec<Vec<char>> = text
        .split('\n')
        .take(DETECT_SNIFF_LINES)
        .map(|l| l.strip_suffix('\r').unwrap_or(l).chars().collect())
        .collect();
    if lines.iter().any(|l| srt_timed(l)) {
        return CaptionFormat::Srt;
    }
    if lines.iter().any(|l| timed_match(l).is_some()) {
        return CaptionFormat::Vtt;
    }
    CaptionFormat::TranscriptPanel
}

/// Replaces `\r\n?` with `\n`.
fn normalize_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\r' {
            if it.peek() == Some(&'\n') {
                it.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// `split(/\n\s*\n/)`.
fn split_blocks(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut blocks = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\n' {
            let w = skip_ws(&chars, i + 1);
            if let Some(k) = (i + 1..w).rev().find(|&k| chars[k] == '\n') {
                blocks.push(chars[start..i].iter().collect());
                start = k + 1;
                i = k + 1;
                continue;
            }
        }
        i += 1;
    }
    blocks.push(chars[start..].iter().collect());
    blocks
}

/// `/^(NOTE|STYLE|REGION)\b/`
fn is_vtt_meta_block(first: &str) -> bool {
    for kw in ["NOTE", "STYLE", "REGION"] {
        if let Some(rest) = first.strip_prefix(kw)
            && !rest.chars().next().is_some_and(is_word)
        {
            return true;
        }
    }
    false
}

fn parse_timed(input: &str) -> Vec<RawCue> {
    let mut cues = Vec::new();
    let normalized = normalize_newlines(strip_bom(input));
    for block in split_blocks(js_trim(&normalized)) {
        let lines: Vec<&str> = block.split('\n').map(js_trim).collect();
        let Some(ts_idx) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        if is_vtt_meta_block(lines.first().copied().unwrap_or("")) {
            continue;
        }
        let chars: Vec<char> = lines[ts_idx].chars().collect();
        let Some((a, b)) = timed_match(&chars) else {
            continue;
        };
        cues.push(RawCue {
            start_ms: parse_timestamp(&a),
            end_ms: parse_timestamp(&b),
            lines: lines[ts_idx + 1..]
                .iter()
                .filter(|l| !l.is_empty())
                .map(|l| (*l).to_owned())
                .collect(),
        });
    }
    cues
}

/// Candidate ends of `\d{1,2}:\d{2}(?::\d{2})?` anchored at 0 (with seconds first).
fn panel_prefix_ends(chars: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    for k in [2usize, 1] {
        if digits_at(chars, 0, k) != k
            || chars.get(k) != Some(&':')
            || digits_at(chars, k + 1, 2) != 2
        {
            continue;
        }
        let base = k + 3;
        if chars.get(base) == Some(&':') && digits_at(chars, base + 1, 2) == 2 {
            out.push(base + 3);
        }
        out.push(base);
        break;
    }
    out
}

/// `/^\d+\s+(?:hours?|minutes?|seconds?|horas?|minutos?|segundos?)\b.*$/i`
fn is_panel_a11y(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let d = digits_at(&chars, 0, usize::MAX);
    if d == 0 {
        return false;
    }
    let w = skip_ws(&chars, d);
    if w == d {
        return false;
    }
    let rest: String = chars[w..].iter().collect::<String>().to_lowercase();
    let rest: Vec<char> = rest.chars().collect();
    for base in ["hour", "minute", "second", "hora", "minuto", "segundo"] {
        if !starts_with_at(&rest, 0, base) {
            continue;
        }
        let n = base.chars().count();
        let boundary = |at: usize| !rest.get(at).copied().is_some_and(is_word);
        if rest.get(n) == Some(&'s') && boundary(n + 1) {
            return true;
        }
        if boundary(n) {
            return true;
        }
    }
    false
}

fn parse_panel(input: &str, duration_ms: Option<i64>) -> Vec<RawCue> {
    struct Start {
        start_ms: i64,
        lines: Vec<String>,
    }
    let mut starts: Vec<Start> = Vec::new();
    for raw in normalize_newlines(input).split('\n') {
        let line = js_trim(raw);
        if line.is_empty() {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let ends = panel_prefix_ends(&chars);
        if ends.contains(&chars.len()) {
            starts.push(Start {
                start_ms: parse_timestamp(line),
                lines: Vec::new(),
            });
            continue;
        }
        let inline = ends.iter().find_map(|&e| {
            let w = skip_ws(&chars, e);
            (w > e && w < chars.len()).then_some((e, w))
        });
        if let Some((e, w)) = inline {
            let ts: String = chars[..e].iter().collect();
            starts.push(Start {
                start_ms: parse_timestamp(&ts),
                lines: vec![chars[w..].iter().collect()],
            });
            continue;
        }
        if let Some(cur) = starts.last_mut()
            && !is_panel_a11y(line)
        {
            cur.lines.push(line.to_owned());
        }
    }
    let n = starts.len();
    let start_times: Vec<i64> = starts.iter().map(|c| c.start_ms).collect();
    starts
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let end_ms = if i + 1 < n {
                start_times[i + 1]
            } else if let Some(d) = duration_ms.filter(|&d| d > c.start_ms) {
                d
            } else {
                c.start_ms + PANEL_LAST_CUE_MS
            };
            RawCue {
                start_ms: c.start_ms,
                end_ms,
                lines: c.lines,
            }
        })
        .collect()
}

/// Parses captions of a concrete format into raw cues (start/end/lines).
pub fn parse_captions(input: &str, format: CaptionFormat, duration_ms: Option<i64>) -> Vec<RawCue> {
    if js_trim(input).is_empty() {
        return Vec::new();
    }
    if format == CaptionFormat::TranscriptPanel {
        parse_panel(input, duration_ms)
    } else {
        parse_timed(input)
    }
}

// ── cleaning ──────────────────────────────────────────────────────────────────

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => return None,
    })
}

fn code_point(value: u64) -> char {
    u32::try_from(value)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or(' ')
}

/// `&(#x[0-9a-f]+|#\d+|[a-z]+);` (case-insensitive), single pass.
fn decode_entities(chars: &[char]) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let rest = &chars[i + 1..];
        let mut handled = false;
        if rest.first() == Some(&'#') {
            // hex
            if matches!(rest.get(1), Some('x') | Some('X')) {
                let n = rest[2..]
                    .iter()
                    .take_while(|c| c.is_ascii_hexdigit())
                    .count();
                if n >= 1 && rest.get(2 + n) == Some(&';') {
                    let v = rest[2..2 + n].iter().fold(0u64, |acc, c| {
                        acc.saturating_mul(16)
                            .saturating_add(u64::from(c.to_digit(16).unwrap_or(0)))
                    });
                    out.push(code_point(v));
                    i += 1 + 2 + n + 1;
                    handled = true;
                }
            }
            if !handled {
                let n = digits_at(rest, 1, usize::MAX);
                if n >= 1 && rest.get(1 + n) == Some(&';') {
                    let v = rest[1..1 + n].iter().fold(0u64, |acc, c| {
                        acc.saturating_mul(10)
                            .saturating_add(u64::from(c.to_digit(10).unwrap_or(0)))
                    });
                    out.push(code_point(v));
                    i += 1 + 1 + n + 1;
                    handled = true;
                }
            }
        } else {
            let n = rest.iter().take_while(|c| c.is_ascii_alphabetic()).count();
            if n >= 1 && rest.get(n) == Some(&';') {
                let name: String = rest[..n].iter().collect::<String>().to_lowercase();
                match entity(&name) {
                    Some(c) => out.push(c),
                    None => out.extend(&chars[i..i + 1 + n + 1]),
                }
                i += 1 + n + 1;
                handled = true;
            }
        }
        if !handled {
            out.push('&');
            i += 1;
        }
    }
    out
}

/// Replaces `open [^open close]{0,200} close` spans via `replace` (bounded so it stays linear).
fn replace_bounded(
    chars: &[char],
    open: char,
    close: char,
    excluded: &[char],
    replace: Option<char>,
) -> Vec<char> {
    let mut out = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == open {
            let mut matched = None;
            for k in 0..=200usize {
                match chars.get(i + 1 + k) {
                    Some(&c) if c == close => {
                        matched = Some(i + 2 + k);
                        break;
                    }
                    Some(c) if excluded.contains(c) => break,
                    Some(_) => {}
                    None => break,
                }
            }
            if let Some(end) = matched {
                if let Some(r) = replace {
                    out.push(r);
                }
                i = end;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn is_unsafe_char(c: char) -> bool {
    matches!(c as u32,
        0x00..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F..=0x9F
        | 0x202A..=0x202E | 0x2066..=0x2069)
}

/// Returns UNTRUSTED plain text.
fn clean_line(line: &str) -> String {
    // Decode first, then strip tags, so encoded markup (&lt;tag&gt;) cannot survive.
    let chars: Vec<char> = line.chars().collect();
    let chars = decode_entities(&chars);
    let chars = replace_bounded(&chars, '<', '>', &['<', '>'], None);
    let chars: Vec<char> = chars.into_iter().filter(|&c| !is_unsafe_char(c)).collect();
    let chars = replace_bounded(&chars, '[', ']', &['[', ']'], Some(' '));
    // `>{2,}` and `♪+` become a space.
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '>' || c == '♪' {
            let mut j = i;
            while j < chars.len() && chars[j] == c {
                j += 1;
            }
            if c == '♪' || j - i >= 2 {
                out.push(' ');
            } else {
                out.push(c);
            }
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    // `\s+` -> ' ', trim.
    out.split(is_ws)
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lines that are only a parenthesised sound tag, e.g. "(applause)".
fn is_paren_tag(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    chars.len() >= 2
        && chars[0] == '('
        && chars[chars.len() - 1] == ')'
        && chars.len() - 2 <= 200
        && !chars[1..chars.len() - 1]
            .iter()
            .any(|&c| c == '(' || c == ')')
}

/// A lone ASCII letter that is not a common one-letter word (ASR noise such as "H").
fn is_junk_text(text: &str) -> bool {
    let mut it = text.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => {
            c.is_ascii_alphabetic()
                && !matches!(c.to_ascii_lowercase(), 'a' | 'i' | 'o' | 'y' | 'e' | 'u')
        }
        _ => false,
    }
}

/// Cleans cues and removes YouTube rolling-caption overlap (lines repeated from the previous cue).
fn clean_cues(raw: &[RawCue]) -> Vec<Cue> {
    let mut out: Vec<Cue> = Vec::new();
    let mut prev_lines: HashSet<String> = HashSet::new();
    for cue in raw {
        let lines: Vec<String> = cue
            .lines
            .iter()
            .map(|l| clean_line(l))
            .filter(|l| !l.is_empty() && !is_paren_tag(l))
            .collect();
        let fresh: Vec<&String> = lines.iter().filter(|l| !prev_lines.contains(*l)).collect();
        let no_fresh = fresh.is_empty();
        let text = fresh
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let has_lines = !lines.is_empty();
        prev_lines = lines.into_iter().collect();
        if no_fresh {
            if has_lines
                && let Some(last) = out.last_mut()
                && cue.start_ms - last.end_ms <= 100
            {
                last.end_ms = last.end_ms.max(cue.end_ms);
            }
            continue;
        }
        if is_junk_text(&text) {
            continue;
        }
        out.push(Cue {
            start_ms: cue.start_ms,
            end_ms: cue.end_ms,
            text,
        });
    }
    out
}

/// ASR captions overshoot: a line is spoken until the next one starts.
fn clamp_overlaps(cues: &[Cue]) -> Vec<Cue> {
    cues.iter()
        .enumerate()
        .map(|(i, c)| match cues.get(i + 1) {
            Some(n) if n.start_ms > c.start_ms && n.start_ms < c.end_ms => Cue {
                end_ms: n.start_ms,
                ..c.clone()
            },
            _ => c.clone(),
        })
        .collect()
}

/// `[.?!…]["'”’»)\]]*$`
fn ends_sentence(text: &str) -> bool {
    let t = text.trim_end_matches(['"', '\'', '”', '’', '»', ')', ']']);
    t.ends_with(['.', '?', '!', '…'])
}

/// `[,;:]["'”’)]*$`
fn ends_clause(text: &str) -> bool {
    let t = text.trim_end_matches(['"', '\'', '”', '’', ')']);
    t.ends_with([',', ';', ':'])
}

/// Picks `Line` for ASR-looking captions (little punctuation, or mostly overlapping cues).
fn detect_split(cues: &[Cue]) -> Split {
    if cues.len() < 2 {
        return Split::Sentence;
    }
    let overlaps = (1..cues.len())
        .filter(|&i| cues[i].start_ms < cues[i - 1].end_ms)
        .count();
    if overlaps as f64 / (cues.len() - 1) as f64 > ASR_OVERLAP_MIN_RATIO {
        return Split::Line;
    }
    let wordy: Vec<&Cue> = cues
        .iter()
        .filter(|c| word_count(&c.text) >= MIN_WORDS)
        .collect();
    if wordy.len() < ASR_MIN_SAMPLE {
        return Split::Sentence;
    }
    let punctuated = wordy.iter().filter(|c| ends_sentence(&c.text)).count();
    if (punctuated as f64 / wordy.len() as f64) < ASR_PUNCTUATED_MAX_RATIO {
        Split::Line
    } else {
        Split::Sentence
    }
}

// ── units ─────────────────────────────────────────────────────────────────────

fn to_unit(cues: Vec<Cue>) -> Unit {
    Unit {
        start_ms: cues[0].start_ms,
        end_ms: cues[cues.len() - 1].end_ms,
        text: cues
            .iter()
            .map(|c| c.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        cues,
        stuck: false,
    }
}

/// Joins cues until sentence punctuation or a gap above 700 ms.
fn merge_sentences(cues: &[Cue]) -> Vec<Unit> {
    let mut units = Vec::new();
    let mut buf: Vec<Cue> = Vec::new();
    for cue in cues {
        if let Some(prev) = buf.last()
            && cue.start_ms - prev.end_ms > SENTENCE_GAP_MS
        {
            units.push(to_unit(std::mem::take(&mut buf)));
        }
        buf.push(cue.clone());
        if ends_sentence(&cue.text) {
            units.push(to_unit(std::mem::take(&mut buf)));
        }
    }
    if !buf.is_empty() {
        units.push(to_unit(buf));
    }
    units
}

fn split_single_cue(cue: &Cue) -> Option<(Cue, Cue)> {
    let chars: Vec<char> = cue.text.chars().collect();
    // UTF-16 offsets keep distances and ratios identical to the JS string lengths.
    let mut off = Vec::with_capacity(chars.len() + 1);
    let mut acc = 0usize;
    for c in &chars {
        off.push(acc);
        acc += c.len_utf16();
    }
    off.push(acc);
    let total = acc;
    let mid = total as f64 / 2.0;

    let pick = |clause: bool| -> Option<usize> {
        let mut found = None;
        let mut best_dist = f64::INFINITY;
        let mut i = 0;
        while i < chars.len() {
            let (idx, ws_start) = if clause {
                if matches!(chars[i], ',' | ';' | ':') {
                    (i + 1, i + 1)
                } else {
                    i += 1;
                    continue;
                }
            } else if is_ws(chars[i]) {
                (i, i)
            } else {
                i += 1;
                continue;
            };
            let w = skip_ws(&chars, ws_start);
            if w == ws_start || w >= chars.len() {
                // no whitespace run, or only trailing whitespace (lookahead `\S` fails)
                i = if clause { i + 1 } else { w.max(i + 1) };
                continue;
            }
            if idx > 0 && idx < chars.len() {
                let dist = (off[idx] as f64 - mid).abs();
                if dist < best_dist {
                    best_dist = dist;
                    found = Some(idx);
                }
            }
            i = w;
        }
        found
    };
    let best = pick(true).or_else(|| pick(false))?;
    let left: String = chars[..best].iter().collect();
    let right: String = chars[best..].iter().collect();
    let (left, right) = (js_trim(&left).to_owned(), js_trim(&right).to_owned());
    if left.is_empty() || right.is_empty() {
        return None;
    }
    let (ll, rl) = (utf16_len(&left) as f64, utf16_len(&right) as f64);
    let ratio = ll / (ll + rl);
    let cut = js_round(cue.start_ms as f64 + (cue.end_ms - cue.start_ms) as f64 * ratio) as i64;
    Some((
        Cue {
            start_ms: cue.start_ms,
            end_ms: cut,
            text: left,
        },
        Cue {
            start_ms: cut,
            end_ms: cue.end_ms,
            text: right,
        },
    ))
}

fn split_cues(unit: &Unit) -> Option<(Unit, Unit)> {
    let cues = &unit.cues;
    if cues.len() == 1 {
        return split_single_cue(&cues[0]).map(|(a, b)| (to_unit(vec![a]), to_unit(vec![b])));
    }
    let mid = (unit.start_ms + unit.end_ms) as f64 / 2.0;
    let mut comma = 0usize;
    let mut comma_dist = f64::INFINITY;
    let mut gap_idx = 0usize;
    let mut gap_size = 0i64;
    let mut gap_dist = f64::INFINITY;
    let mut nearest = 1usize;
    let mut nearest_dist = f64::INFINITY;
    for i in 1..cues.len() {
        let (prev, cur) = (&cues[i - 1], &cues[i]);
        let dist = (cur.start_ms as f64 - mid).abs();
        if dist < nearest_dist {
            nearest_dist = dist;
            nearest = i;
        }
        if ends_clause(&prev.text) && dist < comma_dist {
            comma_dist = dist;
            comma = i;
        }
        let gap = cur.start_ms - prev.end_ms;
        if gap > gap_size || (gap == gap_size && gap > 0 && dist < gap_dist) {
            gap_size = gap;
            gap_dist = dist;
            gap_idx = i;
        }
    }
    let fallback = if gap_idx > 0 { gap_idx } else { nearest };
    let at = if comma > 0 { comma } else { fallback };
    Some((to_unit(cues[..at].to_vec()), to_unit(cues[at..].to_vec())))
}

fn split_long(unit: Unit, max_ms: i64) -> Vec<Unit> {
    if unit.end_ms - unit.start_ms <= max_ms {
        return vec![unit];
    }
    match split_cues(&unit) {
        None => vec![unit],
        Some((a, b)) => {
            let mut out = split_long(a, max_ms);
            out.extend(split_long(b, max_ms));
            out
        }
    }
}

fn is_short(u: &Unit) -> bool {
    u.end_ms - u.start_ms < MIN_CLIP_MS || word_count(&u.text) < MIN_WORDS
}

fn join_units(mut a: Unit, b: Unit) -> Unit {
    a.start_ms = a.start_ms.min(b.start_ms);
    a.end_ms = a.end_ms.max(b.end_ms);
    a.text.push(' ');
    a.text.push_str(&b.text);
    a.cues.extend(b.cues);
    a.stuck = false;
    a
}

/// Merges too-short units into the neighbour with the smaller gap (if the result fits max_ms).
///
/// Linear: every unit before the first short, non-stuck one is already settled, so `out` holds
/// the settled prefix and `rest` the unprocessed tail (same result as rescanning from index 0).
fn merge_short(input: Vec<Unit>, max_ms: i64) -> Vec<Unit> {
    let fits = |u: &Unit, o: &Unit| u.end_ms.max(o.end_ms) - u.start_ms.min(o.start_ms) <= max_ms;
    let mut out: Vec<Unit> = Vec::with_capacity(input.len());
    let mut rest: VecDeque<Unit> = input.into();
    while let Some(mut u) = rest.pop_front() {
        if u.stuck || !is_short(&u) {
            out.push(u);
            continue;
        }
        let prev_gap = out
            .last()
            .filter(|p| fits(&u, p))
            .map(|p| u.start_ms - p.end_ms);
        let next_gap = rest
            .front()
            .filter(|n| fits(&u, n))
            .map(|n| n.start_ms - u.end_ms);
        let use_prev = match (prev_gap, next_gap) {
            (None, None) => {
                u.stuck = true;
                out.push(u);
                continue;
            }
            (Some(pg), Some(ng)) => pg <= ng,
            (Some(_), None) => true,
            (None, Some(_)) => false,
        };
        let joined = match (use_prev, out.pop(), rest.pop_front()) {
            (true, Some(p), n) => {
                if let Some(n) = n {
                    rest.push_front(n);
                }
                join_units(p, u)
            }
            (false, p, Some(n)) => {
                if let Some(p) = p {
                    out.push(p);
                }
                join_units(u, n)
            }
            // Unreachable: the chosen neighbour exists by construction. Keep the unit as is.
            (_, p, n) => {
                if let Some(p) = p {
                    out.push(p);
                }
                u.stuck = true;
                out.push(u);
                if let Some(n) = n {
                    rest.push_front(n);
                }
                continue;
            }
        };
        rest.push_front(joined);
    }
    out
}

/// `text.toLowerCase().replace(/[^\p{L}\p{N}]+/gu, '')`
fn normalise(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphabetic() || c.is_numeric())
        .collect()
}

fn clamp_int(value: Option<f64>, def: f64, min: f64, max: f64) -> f64 {
    match value {
        Some(v) if v.is_finite() => js_round(v).clamp(min, max),
        _ => def,
    }
}

fn pad(units: &[Unit], duration_ms: Option<i64>) -> Vec<Segment> {
    let ceiling = duration_ms.map_or(f64::INFINITY, |d| d as f64);
    let mut segments = Vec::new();
    for (i, u) in units.iter().enumerate() {
        let prev_gap = if i > 0 {
            (u.start_ms - units[i - 1].end_ms).max(0) as f64
        } else {
            f64::INFINITY
        };
        let next_gap = units
            .get(i + 1)
            .map_or(f64::INFINITY, |n| (n.start_ms - u.end_ms).max(0) as f64);
        let lead = (prev_gap / 2.0).min(LEAD_MAX_MS);
        let tail = (next_gap / 2.0).clamp(TAIL_MIN_MS, TAIL_MAX_MS);
        let start = js_round(u.start_ms as f64 - lead).max(0.0) as i64;
        let end = ceiling.min(js_round(u.end_ms as f64 + tail)) as i64;
        if end > start {
            segments.push(Segment {
                index: segments.len(),
                start_ms: start,
                end_ms: end,
                text: u.text.clone(),
            });
        }
    }
    segments
}

pub fn segment_captions(input: &str, opts: &SegmentOptions) -> SegmentResult {
    let mut warnings: Vec<SegmentWarning> = Vec::new();
    let mut warn = |w: SegmentWarning| {
        if !warnings.contains(&w) {
            warnings.push(w);
        }
    };
    let text: &str = match input.char_indices().nth(MAX_INPUT_CHARS) {
        Some((cut, _)) => {
            warn(SegmentWarning::InputTruncated);
            &input[..cut]
        }
        None => input,
    };
    let format = opts.format.unwrap_or_else(|| detect_format(text));
    let max_clip_ms = clamp_int(
        opts.max_clip_ms,
        DEFAULT_MAX_CLIP_MS,
        1000.0,
        HARD_MAX_CLIP_MS,
    ) as i64;
    let max_clips = clamp_int(opts.max_clips, DEFAULT_MAX_CLIPS, 1.0, MAX_SAFE_INTEGER) as usize;
    let duration_ms = opts
        .duration_ms
        .filter(|d| d.is_finite() && *d > 0.0)
        .map(|d| js_round(d) as i64);

    let raw = parse_captions(text, format, duration_ms);
    if !raw.is_empty() && format == CaptionFormat::TranscriptPanel {
        warn(SegmentWarning::PanelFormatLowPrecision);
    }

    let mut cues: Vec<RawCue> = raw.into_iter().filter(|c| c.end_ms > c.start_ms).collect();
    if let Some(d) = duration_ms {
        let before = cues.len();
        cues.retain(|c| c.start_ms < d);
        if cues.len() < before {
            warn(SegmentWarning::CuesBeyondDuration);
        }
        for c in &mut cues {
            c.end_ms = c.end_ms.min(d);
        }
    }
    cues.sort_by_key(|c| c.start_ms);

    let cleaned = clean_cues(&cues);
    let split_by = match opts.split_by.unwrap_or(SplitBy::Auto) {
        SplitBy::Auto => detect_split(&cleaned),
        SplitBy::Sentence => Split::Sentence,
        SplitBy::Line => Split::Line,
    };
    let clamped = clamp_overlaps(&cleaned);
    let merged: Vec<Unit> = match split_by {
        Split::Line => clamped.into_iter().map(|c| to_unit(vec![c])).collect(),
        Split::Sentence => merge_sentences(&clamped),
    };
    let split: Vec<Unit> = merged
        .into_iter()
        .flat_map(|u| split_long(u, max_clip_ms))
        .collect();
    let mut units: Vec<Unit> = merge_short(split, max_clip_ms)
        .into_iter()
        .filter(|u| u.end_ms > u.start_ms)
        .collect();

    let before = units.len();
    let mut seen: HashSet<String> = HashSet::new();
    units.retain(|u| {
        let key = normalise(&u.text);
        key.is_empty() || seen.insert(key)
    });
    if units.len() < before {
        warn(SegmentWarning::DuplicatesRemoved);
    }

    if units.len() > max_clips {
        units.truncate(max_clips);
        warn(SegmentWarning::TruncatedToMaxClips);
    }

    SegmentResult {
        format,
        segments: pad(&units, duration_ms),
        warnings,
        split_by,
    }
}
