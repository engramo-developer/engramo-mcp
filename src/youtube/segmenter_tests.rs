//! Port of `youtube-segmenter.spec.ts` (+ `fixtures/captions.fixtures.ts`) from the web app.

use super::segmenter::*;

const SRT_ES: &str = "1
00:00:01,000 --> 00:00:03,000
Hola, ¿cómo estás?

2
00:00:03,200 --> 00:00:05,500
Muy bien, gracias.

3
00:00:06,000 --> 00:00:07,500
Vamos a la playa
y después comemos.
";

const VTT_EN: &str = "WEBVTT
Kind: captions
Language: en

NOTE this is a comment
spanning lines

STYLE
::cue { color: red }

00:00:01.000 --> 00:00:03.000 align:start position:0%
<i>Welcome to the show.</i>

intro-cue
00:00:03.400 --> 00:00:05.000
[Music] &gt;&gt; Today we talk about travel.
";

const VTT_ROLLING: &str = "WEBVTT
Kind: captions
Language: en

00:00:00.000 --> 00:00:02.000 align:start position:0%
we<00:00:00.300><c> are</c><00:00:00.600><c> going</c>

00:00:02.000 --> 00:00:02.010 align:start position:0%
we are going
 

00:00:02.010 --> 00:00:04.000 align:start position:0%
we are going
to<00:00:02.500><c> the</c><c> beach.</c>

00:00:04.000 --> 00:00:04.010 align:start position:0%
to the beach.
 

00:00:04.010 --> 00:00:06.000 align:start position:0%
to the beach.
it<c> is</c><c> very</c><c> sunny</c><c> today.</c>
";

const PANEL_MSS: &str = "0:01
Hola a todos.
0:04
Hoy vamos a aprender
español juntos.
0:09 Empezamos con los saludos.
";

const PANEL_HMMSS: &str = "0:59:58
Almost at the end of the hour.
1:00:03
And now we continue.
";

/// YouTube automatic (ASR) captions: no punctuation, rolling overlapping cues, junk cues.
const SRT_ASR: &str = "1
00:00:00,120 --> 00:00:03,120
H

2
00:00:04,890 --> 00:00:34,760
[Música]

3
00:00:31,359 --> 00:00:38,280
Tengo la camisa negra hoy mi amor está

4
00:00:34,760 --> 00:00:41,559
de luto hoy tengo en el alma una pena y

5
00:00:38,280 --> 00:00:45,000
es por culpa de tu embrujo pues sé que

6
00:00:41,559 --> 00:00:49,320
tú ya no me quieres eso es lo que más me

7
00:00:45,000 --> 00:00:52,320
ere que tengo la camisa negra y una pena

8
00:00:49,320 --> 00:00:55,760
que me duele mal paré as que Sola me

9
00:00:52,320 --> 00:00:59,359
quedé y fuei pura toita tu mentira Que

10
00:00:55,760 --> 00:01:00,480
 mala suerte la mía aquel día t

11
00:00:59,359 --> 00:01:05,360
en

12
00:01:00,480 --> 00:01:07,159
por beber del veneno malevo de tu amor

13
00:01:05,360 --> 00:01:11,720
yo quedé

14
00:01:07,159 --> 00:01:16,360
moribundo y lleno de dolor respiré de
";

fn ts(ms: u32) -> String {
    let (h, m, s, f) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    format!("{h:02}:{m:02}:{s:02},{f:03}")
}

/// `srt([...])` helper: cues are (start, end, text) with timestamps as SRT strings.
fn srt(cues: &[(&str, &str, &str)]) -> String {
    cues.iter()
        .enumerate()
        .map(|(i, (a, b, t))| format!("{}\n{a} --> {b}\n{t}\n", i + 1))
        .collect::<Vec<_>>()
        .join("\n")
}

fn seg(input: &str) -> SegmentResult {
    segment_captions(input, &SegmentOptions::default())
}

fn seg_with(input: &str, opts: SegmentOptions) -> SegmentResult {
    segment_captions(input, &opts)
}

fn texts(r: &SegmentResult) -> Vec<&str> {
    r.segments.iter().map(|s| s.text.as_str()).collect()
}

fn dur(d: f64) -> SegmentOptions {
    SegmentOptions {
        duration_ms: Some(d),
        ..Default::default()
    }
}

fn words(n: usize, prefix: &str) -> String {
    (0..n)
        .map(|i| format!("{prefix}{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn raw(start: i64, end: i64, lines: &[&str]) -> RawCue {
    RawCue {
        start_ms: start,
        end_ms: end,
        lines: lines.iter().map(|s| (*s).to_owned()).collect(),
    }
}

// ── detectFormat ──────────────────────────────────────────────────────────────

#[test]
fn detects_formats() {
    for (label, input, expected) in [
        ("SRT", SRT_ES, CaptionFormat::Srt),
        ("WEBVTT header", VTT_EN, CaptionFormat::Vtt),
        (
            "header-less timed",
            "00:00:01.000 --> 00:00:02.000\nhi",
            CaptionFormat::Vtt,
        ),
        ("panel", PANEL_MSS, CaptionFormat::TranscriptPanel),
        ("empty", "", CaptionFormat::TranscriptPanel),
    ] {
        assert_eq!(detect_format(input), expected, "{label}");
    }
}

#[test]
fn detect_stays_linear_on_a_long_digit_run() {
    let t = std::time::Instant::now();
    assert_eq!(
        detect_format(&"1".repeat(1_000_000)),
        CaptionFormat::TranscriptPanel
    );
    assert!(t.elapsed().as_secs() < 5);
}

// ── parseCaptions ─────────────────────────────────────────────────────────────

#[test]
fn parses_srt_cues_with_multi_line_text() {
    let cues = parse_captions(SRT_ES, CaptionFormat::Srt, None);
    assert_eq!(cues.len(), 3);
    assert_eq!(cues[0], raw(1000, 3000, &["Hola, ¿cómo estás?"]));
    assert_eq!(
        cues[2].lines,
        vec!["Vamos a la playa", "y después comemos."]
    );
}

#[test]
fn skips_headers_note_style_blocks_and_cue_identifiers_in_vtt() {
    let cues = parse_captions(VTT_EN, CaptionFormat::Vtt, None);
    assert_eq!(cues.len(), 2);
    assert_eq!(cues[1].start_ms, 3400);
}

#[test]
fn parses_panel_timestamps_with_end_equal_next_start() {
    let cues = parse_captions(PANEL_MSS, CaptionFormat::TranscriptPanel, Some(12000));
    let pairs: Vec<(i64, i64)> = cues.iter().map(|c| (c.start_ms, c.end_ms)).collect();
    assert_eq!(pairs, vec![(1000, 4000), (4000, 9000), (9000, 12000)]);
    assert_eq!(cues[2].lines, vec!["Empezamos con los saludos."]);
    let h = parse_captions(PANEL_HMMSS, CaptionFormat::TranscriptPanel, None);
    assert_eq!(h[0].start_ms, 3_598_000);
    assert_eq!(h[1].start_ms, 3_603_000);
    assert_eq!(h[1].end_ms, 3_606_000);
}

#[test]
fn ignores_junk_before_first_timestamp_and_accessibility_lines() {
    let cues = parse_captions(
        "Transcript\n0:01\n1 minute, 5 seconds\nHello there friend.",
        CaptionFormat::TranscriptPanel,
        None,
    );
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].lines, vec!["Hello there friend."]);
}

#[test]
fn skips_note_style_region_blocks_even_with_an_arrow() {
    let vtt = [
        "WEBVTT",
        "",
        "NOTE timing 00:00:00.000 --> 00:00:09.000 is approximate",
        "",
        "REGION",
        "id:r1 00:00:00.000 --> 00:00:01.000",
        "",
        "00:00:01.000 --> 00:00:03.000",
        "Real cue text here.",
    ]
    .join("\n");
    assert_eq!(
        parse_captions(&vtt, CaptionFormat::Vtt, None),
        vec![raw(1000, 3000, &["Real cue text here."])]
    );
}

#[test]
fn panel_last_cue_falls_back_to_start_plus_3000() {
    let cues = parse_captions(
        "0:10\nLast line here.",
        CaptionFormat::TranscriptPanel,
        Some(10000),
    );
    assert_eq!((cues[0].start_ms, cues[0].end_ms), (10000, 13000));
}

#[test]
fn parse_empty_crlf_and_fractions() {
    assert!(parse_captions("  ", CaptionFormat::Srt, None).is_empty());
    let cues = parse_captions(
        "1\r\n00:00:01,5 --> 00:00:02,25\r\nHi\r\n",
        CaptionFormat::Srt,
        None,
    );
    assert_eq!((cues[0].start_ms, cues[0].end_ms), (1500, 2250));
}

// ── formats ───────────────────────────────────────────────────────────────────

#[test]
fn segments_srt_into_sentences() {
    let r = seg_with(SRT_ES, dur(20000.0));
    assert_eq!(r.format, CaptionFormat::Srt);
    assert!(r.warnings.is_empty());
    assert_eq!(
        texts(&r),
        vec![
            "Hola, ¿cómo estás?",
            "Muy bien, gracias.",
            "Vamos a la playa y después comemos."
        ]
    );
    assert_eq!(
        r.segments.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn segments_vtt_with_tag_entity_bracket_and_speaker_marker_cleaning() {
    let r = seg(VTT_EN);
    assert_eq!(r.format, CaptionFormat::Vtt);
    assert_eq!(
        texts(&r),
        vec!["Welcome to the show.", "Today we talk about travel."]
    );
}

#[test]
fn removes_rolling_overlap_duplication() {
    let r = seg(VTT_ROLLING);
    assert_eq!(
        texts(&r),
        vec!["we are going to the beach.", "it is very sunny today."]
    );
}

#[test]
fn segments_panel_text_with_warning_and_duration_end() {
    let r = seg_with(PANEL_MSS, dur(12000.0));
    assert_eq!(r.format, CaptionFormat::TranscriptPanel);
    assert!(
        r.warnings
            .contains(&SegmentWarning::PanelFormatLowPrecision)
    );
    assert_eq!(
        texts(&r),
        vec![
            "Hola a todos.",
            "Hoy vamos a aprender español juntos.",
            "Empezamos con los saludos."
        ]
    );
    assert_eq!(r.segments[2].end_ms, 12000);
}

#[test]
fn panel_last_cue_without_duration() {
    let r = seg("0:10\nThis is the last line.");
    assert_eq!(
        (r.segments[0].start_ms, r.segments[0].end_ms),
        (9700, 13400)
    );
}

#[test]
fn handles_hmmss_panel() {
    let r = seg_with(
        PANEL_HMMSS,
        SegmentOptions {
            format: None,
            ..Default::default()
        },
    );
    let got: Vec<(i64, i64, &str)> = r
        .segments
        .iter()
        .map(|s| (s.start_ms, s.end_ms, s.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (3_597_700, 3_603_150, "Almost at the end of the hour."),
            (3_603_000, 3_606_400, "And now we continue."),
        ]
    );
}

#[test]
fn explicit_format_wins_over_detection() {
    let r = seg_with(
        SRT_ES,
        SegmentOptions {
            format: Some(CaptionFormat::Vtt),
            ..Default::default()
        },
    );
    assert_eq!(r.format, CaptionFormat::Vtt);
}

#[test]
fn panel_lines_with_timestamp_and_text_on_the_same_line() {
    let r = seg("0:01 Hello my dear friends.\n0:05 How are you doing today?");
    assert_eq!(
        texts(&r),
        vec!["Hello my dear friends.", "How are you doing today?"]
    );
}

// ── cleaning ──────────────────────────────────────────────────────────────────

#[test]
fn cleaning_cases() {
    let one = |text: &str| -> Vec<String> {
        seg(&srt(&[("00:00:01,000", "00:00:04,000", text)]))
            .segments
            .into_iter()
            .map(|s| s.text)
            .collect()
    };
    let cases: [(&str, &str, Vec<&str>); 9] = [
        (
            "bare ampersand, empty numeric refs, uppercase and nbsp entities",
            "Tom & Jerry&nbsp;say &AMP; &#; &#x; ok.",
            vec!["Tom & Jerry say & &#; &#x; ok."],
        ),
        (
            "bracket tags",
            "[Applause] Thank you all so much.",
            vec!["Thank you all so much."],
        ),
        (
            "speaker markers",
            ">> Where are we going today?",
            vec!["Where are we going today?"],
        ),
        (
            "html tags and entities",
            "<b>Tom &amp; Jerry</b> are here&#33; &#x41; ok.",
            vec!["Tom & Jerry are here! A ok."],
        ),
        (
            "whitespace and music notes",
            "♪  la   la    la   canción ♪",
            vec!["la la la canción"],
        ),
        ("tag-only cue", "[Music]", vec![]),
        (
            "bad numeric and unknown named entities",
            "Bad &#99999999; entity &foo; here today.",
            vec!["Bad entity &foo; here today."],
        ),
        (
            "encoded markup (decoded then stripped)",
            "&lt;img src=x onerror=alert(1)&gt; hello there friend.",
            vec!["hello there friend."],
        ),
        (
            "bidi override and control chars",
            "hello&#x202E; there&#1; friend.",
            vec!["hello there friend."],
        ),
    ];
    for (label, input, expected) in cases {
        assert_eq!(one(input), expected, "{label}");
    }
}

// ── sentence merge ────────────────────────────────────────────────────────────

#[test]
fn joins_cues_until_sentence_punctuation_including_ellipsis() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:02,000", "Dime, ¿qué"),
        ("00:00:02,100", "00:00:03,000", "quieres hacer hoy?"),
        ("00:00:03,100", "00:00:04,000", "No lo sé…"),
        ("00:00:04,100", "00:00:05,000", "Tal vez nada hoy."),
    ]));
    assert_eq!(
        texts(&r),
        vec![
            "Dime, ¿qué quieres hacer hoy?",
            "No lo sé…",
            "Tal vez nada hoy."
        ]
    );
}

#[test]
fn breaks_on_a_gap_above_700ms_without_punctuation() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:02,000", "one two three"),
        ("00:00:02,600", "00:00:03,500", "four five six"),
        ("00:00:04,300", "00:00:05,000", "seven eight nine"),
    ]));
    assert_eq!(
        texts(&r),
        vec!["one two three four five six", "seven eight nine"]
    );
}

// ── long split ────────────────────────────────────────────────────────────────

#[test]
fn splits_at_a_comma_cue_boundary() {
    let (a, b, c) = (
        format!("{},", words(4, "a")),
        words(4, "b"),
        format!("{}.", words(4, "c")),
    );
    let r = seg(&srt(&[
        ("00:00:00,000", "00:00:08,000", &a),
        ("00:00:08,100", "00:00:16,000", &b),
        ("00:00:16,100", "00:00:20,000", &c),
    ]));
    assert_eq!(texts(&r), vec!["a0 a1 a2 a3,", "b0 b1 b2 b3 c0 c1 c2 c3."]);
}

#[test]
fn splits_at_the_largest_internal_gap_when_there_is_no_comma() {
    let (a, b, c, d) = (
        words(4, "a"),
        words(4, "b"),
        words(4, "c"),
        format!("{}.", words(4, "d")),
    );
    let r = seg(&srt(&[
        ("00:00:00,000", "00:00:05,000", &a),
        ("00:00:05,100", "00:00:09,000", &b),
        ("00:00:09,600", "00:00:14,000", &c),
        ("00:00:14,100", "00:00:18,000", &d),
    ]));
    assert_eq!(
        texts(&r),
        vec!["a0 a1 a2 a3 b0 b1 b2 b3", "c0 c1 c2 c3 d0 d1 d2 d3."]
    );
}

#[test]
fn splits_at_the_middle_cue_boundary_without_commas_or_gaps() {
    let (a, b) = (words(4, "a"), format!("{}.", words(4, "b")));
    let r = seg(&srt(&[
        ("00:00:00,000", "00:00:09,000", &a),
        ("00:00:09,000", "00:00:18,000", &b),
    ]));
    assert_eq!(texts(&r), vec!["a0 a1 a2 a3", "b0 b1 b2 b3."]);
}

#[test]
fn breaks_equal_gap_ties_by_distance_to_the_midpoint() {
    let w = |p: &str| format!("{p}0 {p}1 {p}2 {p}3");
    let c = format!("{}.", w("c"));
    let r = seg(&srt(&[
        ("00:00:00,000", "00:00:02,000", &w("a")),
        ("00:00:02,500", "00:00:09,000", &w("b")),
        ("00:00:09,500", "00:00:19,000", &c),
    ]));
    assert_eq!(texts(&r), vec!["a0 a1 a2 a3 b0 b1 b2 b3", "c0 c1 c2 c3."]);
}

#[test]
fn splits_a_single_long_cue_at_a_comma_with_time_proportional_to_characters() {
    let r = seg_with(
        &srt(&[(
            "00:00:00,000",
            "00:00:20,000",
            "aaaa bbbb cccc dddd, eeee ffff gggg hhhh.",
        )]),
        dur(60000.0),
    );
    assert_eq!(
        texts(&r),
        vec!["aaaa bbbb cccc dddd,", "eeee ffff gggg hhhh."]
    );
    assert!(r.segments[0].end_ms > 9500);
    assert!(r.segments[0].end_ms < 11000);
}

#[test]
fn splits_a_single_long_cue_at_a_word_boundary_without_comma() {
    let t = format!("{}.", words(8, "w"));
    let r = seg(&srt(&[("00:00:00,000", "00:00:20,000", &t)]));
    assert_eq!(r.segments.len(), 2);
    assert_eq!(texts(&r).join(" "), "w0 w1 w2 w3 w4 w5 w6 w7.");
}

#[test]
fn leaves_an_unsplittable_word_alone_and_caps_max_clip_at_30s() {
    let big = SegmentOptions {
        max_clip_ms: Some(99999.0),
        ..Default::default()
    };
    let r = seg_with(
        &srt(&[("00:00:00,000", "00:00:40,000", "Supercalifragilistic")]),
        big.clone(),
    );
    assert_eq!(r.segments.len(), 1);
    let t = format!("{}.", words(20, "x"));
    let split = seg_with(&srt(&[("00:00:00,000", "00:00:40,000", &t)]), big);
    assert_eq!(
        texts(&split),
        vec![
            "x0 x1 x2 x3 x4 x5 x6 x7 x8 x9 x10",
            "x11 x12 x13 x14 x15 x16 x17 x18 x19."
        ]
    );
}

#[test]
fn recursively_splits_very_long_text() {
    let t = format!("{}.", words(40, "x"));
    let r = seg(&srt(&[("00:00:00,000", "00:01:00,000", &t)]));
    assert_eq!(texts(&r).join(" "), t);
    assert!(r.segments.len() >= 4);
    for s in &r.segments {
        assert!(s.end_ms - s.start_ms <= 15_000 + 700);
    }
}

// ── short merge ───────────────────────────────────────────────────────────────

#[test]
fn merges_a_one_word_piece_into_the_previous_neighbour_when_closer() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:03,000", "Esto es una frase larga."),
        ("00:00:03,100", "00:00:03,800", "Sí."),
        ("00:00:08,000", "00:00:10,000", "Otra frase aparte aquí."),
    ]));
    assert_eq!(
        texts(&r),
        vec!["Esto es una frase larga. Sí.", "Otra frase aparte aquí."]
    );
}

#[test]
fn merges_a_short_piece_into_the_next_neighbour_when_closer() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:03,000", "Esto es una frase larga."),
        ("00:00:08,000", "00:00:08,300", "Oye."),
        ("00:00:08,400", "00:00:10,000", "Otra frase aparte aquí."),
    ]));
    assert_eq!(
        texts(&r),
        vec!["Esto es una frase larga.", "Oye. Otra frase aparte aquí."]
    );
}

#[test]
fn merges_a_trailing_short_unit_into_the_previous_one() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:03,000", "Esto es una frase larga."),
        ("00:00:03,100", "00:00:03,500", "Vale."),
    ]));
    assert_eq!(texts(&r), vec!["Esto es una frase larga. Vale."]);
}

#[test]
fn keeps_a_lone_short_segment_and_does_not_merge_beyond_max_clip() {
    assert_eq!(
        seg(&srt(&[("00:00:01,000", "00:00:01,300", "Hi.")]))
            .segments
            .len(),
        1
    );
    let r = seg_with(
        &srt(&[
            ("00:00:00,000", "00:00:02,000", "Una frase de prueba aquí."),
            ("00:00:03,000", "00:00:03,300", "Ok."),
        ]),
        SegmentOptions {
            max_clip_ms: Some(2500.0),
            ..Default::default()
        },
    );
    assert_eq!(r.segments.len(), 2);
}

// ── padding ───────────────────────────────────────────────────────────────────

#[test]
fn applies_lead_and_tail_padding() {
    let r = seg(&srt(&[
        ("00:00:10,000", "00:00:12,000", "First sentence is here."),
        ("00:00:13,000", "00:00:15,000", "Second sentence is here."),
    ]));
    assert_eq!(
        (r.segments[0].start_ms, r.segments[0].end_ms),
        (9700, 12400)
    );
    assert_eq!(
        (r.segments[1].start_ms, r.segments[1].end_ms),
        (12700, 15400)
    );
}

#[test]
fn uses_half_gap_when_the_gap_is_small_voronoi() {
    let r = seg(&srt(&[
        ("00:00:10,000", "00:00:12,000", "First sentence is here."),
        ("00:00:12,600", "00:00:15,000", "Second sentence is here."),
    ]));
    assert_eq!(r.segments[0].end_ms, 12300);
    assert_eq!(r.segments[1].start_ms, 12300);
}

#[test]
fn keeps_a_150ms_tail_floor_even_with_no_gap() {
    let r = seg(&srt(&[
        ("00:00:10,000", "00:00:12,000", "First sentence is here."),
        ("00:00:12,000", "00:00:15,000", "Second sentence is here."),
    ]));
    assert_eq!(r.segments[0].end_ms, 12150);
    assert_eq!(r.segments[1].start_ms, 12000);
}

#[test]
fn clamps_to_zero_and_to_duration() {
    let r = seg_with(
        &srt(&[("00:00:00,100", "00:00:09,900", "A sentence near the edges.")]),
        dur(10000.0),
    );
    assert_eq!((r.segments[0].start_ms, r.segments[0].end_ms), (0, 10000));
}

#[test]
fn does_not_clamp_the_end_without_duration() {
    let r = seg(&srt(&[(
        "00:00:01,000",
        "00:00:03,000",
        "A sentence near the end.",
    )]));
    assert_eq!((r.segments[0].start_ms, r.segments[0].end_ms), (700, 3400));
}

#[test]
fn produces_sorted_clips_with_end_after_start() {
    let r = seg(&srt(&[
        ("00:00:01,001", "00:00:02,003", "Alpha beta gamma delta."),
        ("00:00:02,507", "00:00:04,001", "Epsilon zeta eta theta."),
    ]));
    let mut prev = -1;
    for s in &r.segments {
        assert!(s.end_ms > s.start_ms);
        assert!(s.start_ms >= prev);
        prev = s.start_ms;
    }
}

// ── dedupe, cap, warnings ─────────────────────────────────────────────────────

#[test]
fn removes_duplicates_by_normalised_text_and_warns() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:03,000", "Hola, ¿cómo estás?"),
        ("00:00:05,000", "00:00:07,000", "Adiós, amigos míos."),
        ("00:00:09,000", "00:00:11,000", "HOLA cómo estás"),
    ]));
    assert_eq!(texts(&r), vec!["Hola, ¿cómo estás?", "Adiós, amigos míos."]);
    assert_eq!(
        r.segments.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1]
    );
    assert_eq!(r.warnings, vec![SegmentWarning::DuplicatesRemoved]);
}

#[test]
fn caps_at_max_clips_and_warns() {
    let owned: Vec<(String, String, String)> = (0..10u32)
        .map(|i| {
            (
                ts(i * 3000),
                ts(i * 3000 + 2000),
                format!("Sentence number {} here.", "x".repeat(i as usize + 1)),
            )
        })
        .collect();
    let cues: Vec<(&str, &str, &str)> = owned
        .iter()
        .map(|(a, b, c)| (a.as_str(), b.as_str(), c.as_str()))
        .collect();
    let r = seg_with(
        &srt(&cues),
        SegmentOptions {
            max_clips: Some(4.0),
            ..Default::default()
        },
    );
    assert_eq!(r.segments.len(), 4);
    assert!(r.warnings.contains(&SegmentWarning::TruncatedToMaxClips));
    assert_eq!(
        r.segments.iter().map(|s| s.index).collect::<Vec<_>>(),
        vec![0, 1, 2, 3]
    );
}

#[test]
fn does_not_dedupe_units_whose_normalised_text_is_empty() {
    let r = seg(&srt(&[
        ("00:00:01,000", "00:00:03,000", "¿? ¿? ¿?"),
        ("00:00:20,000", "00:00:22,000", "¡! ¡! ¡!"),
    ]));
    assert_eq!(r.segments.len(), 2);
    assert!(!r.warnings.contains(&SegmentWarning::DuplicatesRemoved));
}

#[test]
fn clamps_max_clips_to_at_least_one_and_max_clip_ms_to_at_least_1000() {
    let r = seg_with(
        SRT_ES,
        SegmentOptions {
            max_clips: Some(0.0),
            ..Default::default()
        },
    );
    assert_eq!(r.segments.len(), 1);
    assert!(r.warnings.contains(&SegmentWarning::TruncatedToMaxClips));
    let s = seg_with(
        &srt(&[(
            "00:00:00,000",
            "00:00:02,000",
            "one two three four five six.",
        )]),
        SegmentOptions {
            max_clip_ms: Some(1.0),
            ..Default::default()
        },
    );
    assert_eq!(texts(&s), vec!["one two three", "four five", "six."]);
}

#[test]
fn truncates_oversized_input_and_warns() {
    let r = seg(&format!(
        "0:01 Hello there my friend.\n{}",
        "x".repeat(2_000_001)
    ));
    assert!(r.warnings.contains(&SegmentWarning::InputTruncated));
}

#[test]
fn ignores_invalid_options() {
    let r = seg_with(
        SRT_ES,
        SegmentOptions {
            max_clips: Some(f64::NAN),
            max_clip_ms: Some(f64::NAN),
            duration_ms: Some(-5.0),
            ..Default::default()
        },
    );
    assert_eq!(r.segments.len(), 3);
}

#[test]
fn drops_cues_past_duration_and_warns() {
    let r = seg_with(
        &srt(&[
            ("00:00:01,000", "00:00:03,000", "Inside the video here."),
            ("00:01:00,000", "00:01:03,000", "Outside the video here."),
        ]),
        dur(10000.0),
    );
    assert_eq!(texts(&r), vec!["Inside the video here."]);
    assert_eq!(r.warnings, vec![SegmentWarning::CuesBeyondDuration]);
}

#[test]
fn truncates_a_cue_straddling_duration_without_warning() {
    let r = seg_with(
        &srt(&[("00:00:08,000", "00:00:14,000", "Straddling the end here.")]),
        dur(10000.0),
    );
    assert_eq!(r.segments[0].end_ms, 10000);
    assert!(r.warnings.is_empty());
}

#[test]
fn does_not_extend_previous_cue_when_overlap_only_cue_starts_over_100ms_later() {
    let vtt = [
        "WEBVTT",
        "",
        "00:00:00.000 --> 00:00:02.000",
        "We are going to the beach.",
        "",
        "00:00:05.000 --> 00:00:06.000",
        "We are going to the beach.",
    ]
    .join("\n");
    let r = seg(&vtt);
    assert_eq!(r.segments.len(), 1);
    assert_eq!(r.segments[0].end_ms, 2400);
}

#[test]
fn drops_zero_length_cues_and_sorts_unordered_cues() {
    let r = seg(&srt(&[
        ("00:00:05,000", "00:00:07,000", "Second sentence goes here."),
        ("00:00:01,000", "00:00:01,000", "Zero length cue ignored."),
        ("00:00:01,000", "00:00:03,000", "First sentence goes here."),
    ]));
    assert_eq!(
        texts(&r),
        vec!["First sentence goes here.", "Second sentence goes here."]
    );
}

// ── empty and garbage input ───────────────────────────────────────────────────

#[test]
fn returns_no_segments_for_empty_and_garbage_input() {
    for input in [
        "",
        "   \n  ",
        "just some random text without timestamps",
        "\u{0}\u{1} ???",
        "0:0\n1:",
    ] {
        let r = seg(input);
        assert!(r.segments.is_empty(), "{input:?}");
        assert!(r.warnings.is_empty(), "{input:?}");
    }
}

#[test]
fn ignores_blocks_with_an_unparsable_timing_line() {
    assert!(seg("1\nfoo --> bar\nhello there world").segments.is_empty());
}

// ── cleanLine regexes ─────────────────────────────────────────────────────────

fn one_cue(line: &str) -> String {
    format!("1\n00:00:01,000 --> 00:00:03,000\n{line}\n")
}

#[test]
fn stays_linear_on_a_long_line_of_unclosed_brackets() {
    let t = std::time::Instant::now();
    let _ = seg(&one_cue(&"<".repeat(200_000)));
    let _ = seg(&one_cue(&"[".repeat(200_000)));
    assert!(t.elapsed().as_secs() < 10);
}

#[test]
fn still_strips_ordinary_tags_and_bracketed_sound_cues() {
    let r = seg(&one_cue("<c.colorE5E5E5>Hola</c> [Música] amigo"));
    assert_eq!(r.segments[0].text, "Hola amigo");
}

// ── split mode (ASR captions) ─────────────────────────────────────────────────

fn seq(items: &[&str]) -> String {
    let owned: Vec<(String, String)> = (0..items.len() as u32)
        .map(|i| (ts(i * 3000), ts(i * 3000 + 2000)))
        .collect();
    let cues: Vec<(&str, &str, &str)> = owned
        .iter()
        .zip(items)
        .map(|((a, b), t)| (a.as_str(), b.as_str(), *t))
        .collect();
    srt(&cues)
}

fn split(sb: SplitBy) -> SegmentOptions {
    SegmentOptions {
        split_by: Some(sb),
        ..Default::default()
    }
}

#[test]
fn auto_picks_line_for_asr_captions_and_reports_it() {
    assert_eq!(seg(SRT_ASR).split_by, Split::Line);
}

#[test]
fn clamps_each_clip_to_the_next_cue_start_and_never_mixes_cues() {
    let r = seg(SRT_ASR);
    let first = &r.segments[0];
    assert_eq!(first.text, "Tengo la camisa negra hoy mi amor está");
    assert!(first.start_ms >= 31_359 - 300);
    assert!(first.start_ms <= 31_359);
    assert!(first.end_ms <= 34_760 + 400);
    for s in &r.segments {
        let t = s.text.to_lowercase();
        let hits = [
            t.contains("camisa negra hoy"),
            t.contains("de luto hoy"),
            t.contains("es por culpa"),
        ]
        .iter()
        .filter(|b| **b)
        .count();
        assert!(hits <= 1, "{t}");
    }
}

#[test]
fn drops_junk_cues_like_h_and_musica() {
    let r = seg(SRT_ASR);
    let all = texts(&r).join(" ");
    assert!(!all.contains("Música"));
    assert!(!r.segments.iter().any(|s| s.text.trim() == "H"));
    assert!(r.segments[0].start_ms > 30_000);
}

#[test]
fn drops_parenthesised_sound_tags() {
    let r = seg(&srt(&[("00:00:01,000", "00:00:03,000", "(applause)")]));
    assert!(r.segments.is_empty());
}

#[test]
fn auto_picks_line_for_unpunctuated_non_overlapping_captions() {
    let items: Vec<String> = (0..6)
        .map(|i| format!("this is caption number {i} here"))
        .collect();
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    assert_eq!(seg(&seq(&refs)).split_by, Split::Line);
}

#[test]
fn auto_stays_on_sentence_with_too_few_wordy_cues() {
    let r = seg(&seq(&[
        "one two three four",
        "five six seven eight",
        "nine ten eleven",
    ]));
    assert_eq!(r.split_by, Split::Sentence);
}

#[test]
fn auto_stays_on_sentence_for_a_single_cue() {
    assert_eq!(seg(&seq(&["just one cue here"])).split_by, Split::Sentence);
}

#[test]
fn keeps_one_character_real_words_and_drops_lone_letter_asr_noise() {
    let r = seg_with(&seq(&["好", "I", "H"]), split(SplitBy::Line));
    let t = texts(&r);
    assert!(t.join(" ").contains('好'));
    assert!(t.iter().any(|x| x.split_whitespace().any(|w| w == "I")));
    assert!(!t.iter().any(|x| x.split_whitespace().any(|w| w == "H")));
}

#[test]
fn keeps_punctuated_captions_on_sentence_mode_in_auto() {
    assert_eq!(seg(SRT_ES).split_by, Split::Sentence);
}

#[test]
fn honours_explicit_sentence_mode_on_asr_captions_and_still_clamps_overshoot() {
    let r = seg_with(SRT_ASR, split(SplitBy::Sentence));
    assert_eq!(r.split_by, Split::Sentence);
    assert!(
        r.segments
            .iter()
            .all(|s| s.end_ms - s.start_ms <= 15_000 + 700)
    );
}

#[test]
fn honours_explicit_line_mode_on_punctuated_captions() {
    let r = seg_with(SRT_ES, split(SplitBy::Line));
    assert_eq!(r.split_by, Split::Line);
    assert!(r.segments.len() >= 3);
}

#[test]
fn panel_accessibility_lines_spanish_and_word_boundary() {
    let cues = parse_captions(
        "0:01\n2 minutos, 3 segundos\n1 hora\nHola amigos míos.\n10 secondsy tail",
        CaptionFormat::TranscriptPanel,
        None,
    );
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].lines, vec!["Hola amigos míos.", "10 secondsy tail"]);
}

#[test]
fn bom_prefixed_vtt_and_srt_are_detected_and_parsed() {
    let vtt = "\u{FEFF}WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nHello there friend.\n";
    assert_eq!(detect_format(vtt), CaptionFormat::Vtt);
    assert_eq!(
        parse_captions(vtt, CaptionFormat::Vtt, None),
        vec![raw(1000, 3000, &["Hello there friend."])]
    );
    let srt = "\u{FEFF}1\n00:00:01,000 --> 00:00:03,000\nHello there friend.\n";
    assert_eq!(detect_format(srt), CaptionFormat::Srt);
    assert_eq!(parse_captions(srt, CaptionFormat::Srt, None).len(), 1);
}

#[test]
fn merge_short_is_linear_on_many_tiny_cues() {
    let ts = |ms: usize| {
        format!(
            "{:02}:{:02}:{:02},{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    };
    let input: String = (0..30_000)
        .map(|i| {
            format!(
                "{}\n{} --> {}\nw{i}\n\n",
                i + 1,
                ts(i * 400),
                ts(i * 400 + 300)
            )
        })
        .collect();
    let t = std::time::Instant::now();
    let r = seg(&input);
    assert!(!r.segments.is_empty());
    assert!(
        t.elapsed() < std::time::Duration::from_secs(20),
        "segmenting took {:?}",
        t.elapsed()
    );
}
