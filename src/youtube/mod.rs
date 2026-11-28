//! YouTube helpers for the pure `parse_youtube_captions` tool: video id extraction from a URL
//! (or bare id), time-string parsing, and the caption segmenter. No network access, ever.

pub mod segmenter;

#[cfg(test)]
mod segmenter_tests;

/// Extracts the 11-character video id from a YouTube URL (watch?v=, youtu.be/, shorts/, embed/,
/// live/, v/, m. and music. hosts, extra query params) or accepts a bare id.
pub fn extract_video_id(video: &str) -> Result<String, String> {
    let err = || {
        "video must be a YouTube URL (watch?v=, youtu.be/, shorts/, embed/) or an 11-character \
         video id ([A-Za-z0-9_-])"
            .to_owned()
    };
    let video = video.trim();
    if is_video_id(video) {
        return Ok(video.to_owned());
    }
    let lower = video.to_ascii_lowercase();
    let without_scheme = if lower.starts_with("https://") {
        &video[8..]
    } else if lower.starts_with("http://") {
        &video[7..]
    } else if let Some(rest) = video.strip_prefix("//") {
        rest
    } else {
        video
    };
    let (authority, rest) = match without_scheme.find(['/', '?', '#']) {
        Some(i) => without_scheme.split_at(i),
        None => (without_scheme, ""),
    };
    if authority.contains('@') {
        return Err(err());
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let host = ["www.", "m.", "music."]
        .iter()
        .find_map(|p| host.strip_prefix(p))
        .unwrap_or(&host);
    // Drop the fragment, then split path and query.
    let rest = rest.split('#').next().unwrap_or("");
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let candidate = match host {
        "youtu.be" => segments.first().copied(),
        "youtube.com" | "youtube-nocookie.com" => match segments.first().copied() {
            Some("watch") => query.split('&').find_map(|kv| kv.strip_prefix("v=")),
            Some("shorts" | "embed" | "live" | "v") => segments.get(1).copied(),
            _ => None,
        },
        _ => None,
    };
    match candidate {
        Some(id) if is_video_id(id) => Ok(id.to_owned()),
        _ => Err(err()),
    }
}

fn is_video_id(s: &str) -> bool {
    s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Parses `m:ss`, `h:mm:ss` or plain seconds (`90`, `90.5`) into milliseconds.
pub fn parse_time_ms(input: &str) -> Result<i64, String> {
    const MAX_TIME_LEN: usize = 32;
    let err =
        || "invalid time: use m:ss, h:mm:ss or plain seconds (e.g. 1:30, 0:01:30, 90)".to_owned();
    let s = input.trim();
    if s.len() > MAX_TIME_LEN {
        return Err(err());
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return Err(err());
    }
    let digits = |p: &str| !p.is_empty() && p.len() <= 7 && p.bytes().all(|b| b.is_ascii_digit());
    let last = parts[parts.len() - 1];
    let (sec_whole, frac) = match last.split_once('.') {
        Some((w, f)) => (w, f),
        None => (last, ""),
    };
    if !digits(sec_whole) || !frac.bytes().all(|b| b.is_ascii_digit()) || frac.len() > 3 {
        return Err(err());
    }
    let mut total: i64 = 0;
    for p in &parts[..parts.len() - 1] {
        if !digits(p) {
            return Err(err());
        }
    }
    let secs: i64 = sec_whole.parse().map_err(|_| err())?;
    if parts.len() > 1 && secs >= 60 {
        return Err(err());
    }
    let mut frac_s = frac.to_owned();
    while frac_s.len() < 3 {
        frac_s.push('0');
    }
    let frac_ms: i64 = frac_s.parse().map_err(|_| err())?;
    match parts.len() {
        3 => {
            let h: i64 = parts[0].parse().map_err(|_| err())?;
            let m: i64 = parts[1].parse().map_err(|_| err())?;
            if m >= 60 {
                return Err(err());
            }
            total += h * 3600 + m * 60;
        }
        2 => {
            let m: i64 = parts[0].parse().map_err(|_| err())?;
            total += m * 60;
        }
        _ => {}
    }
    Ok((total + secs) * 1000 + frac_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_id_from_urls_and_bare_id() {
        let id = "dQw4w9WgXcQ";
        for v in [
            "dQw4w9WgXcQ",
            "  dQw4w9WgXcQ ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=90s",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ#t=5",
            "http://youtube.com/watch?v=dQw4w9WgXcQ",
            "youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ?feature=share",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ?si=x",
            "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
            "//www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/v/dQw4w9WgXcQ",
            "https://www.youtube.com:443/watch?v=dQw4w9WgXcQ",
            "HTTPS://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ#t=5",
        ] {
            assert_eq!(extract_video_id(v).as_deref(), Ok(id), "{v}");
        }
    }

    #[test]
    fn video_id_rejects_bad_input() {
        for v in [
            "https://youtube.com/Watch?v=dQw4w9WgXcQ",
            "",
            "short",
            "dQw4w9WgXcQ1",
            "dQw4w9WgXc!",
            "https://example.com/watch?v=dQw4w9WgXcQ",
            "https://evil.com/youtu.be/dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=tooshort",
            "https://www.youtube.com/watch",
            "https://www.youtube.com/playlist?list=PL12345678901",
            "https://user@youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/",
        ] {
            assert!(extract_video_id(v).is_err(), "{v}");
        }
    }

    #[test]
    fn time_parsing() {
        assert_eq!(parse_time_ms("90"), Ok(90_000));
        assert_eq!(parse_time_ms("0"), Ok(0));
        assert_eq!(parse_time_ms("1:30"), Ok(90_000));
        assert_eq!(parse_time_ms("0:01:30"), Ok(90_000));
        assert_eq!(parse_time_ms("1:02:03"), Ok(3_723_000));
        assert_eq!(parse_time_ms(" 12.5 "), Ok(12_500));
        assert_eq!(parse_time_ms("1:05.25"), Ok(65_250));
        for bad in [
            "", "abc", "1:60", "1:2:3:4", "-5", "1:", ":30", "1:99:00", "1.2345", "1e3",
        ] {
            assert!(parse_time_ms(bad).is_err(), "{bad}");
        }
        // Overlong input is rejected without being echoed back.
        let long = "9".repeat(10_000);
        let e = parse_time_ms(&long).unwrap_err();
        assert!(e.len() < 200 && !e.contains("9999"));
    }
}
