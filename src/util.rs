//! Small pure helpers: sizes, dates, path normalisation, wildcards.

use std::path::Path;

/// Human readable size, e.g. `1.4 GB`.
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// `1234567` becomes `1,234,567`.
pub fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `75` becomes `1 min 15 s`; used for elapsed time and ETA.
pub fn fmt_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    }
}

/// `m:ss` for track lengths.
pub fn fmt_track(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Civil date and time from seconds since 1970-01-01 (already shifted to the
/// wanted time zone): `(year, month, day, hour, minute, second)`.
pub fn civil(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + if m <= 2 { 1 } else { 0 }) as i32;
    (
        y,
        m,
        d,
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    )
}

/// Seconds since 1970-01-01 for a civil date at midnight.
pub fn unix_from_date(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

/// Parses `YYYY-MM-DD` into seconds since 1970 (midnight). Empty or invalid text is `None`.
pub fn parse_date(s: &str) -> Option<i64> {
    let mut parts = s.trim().split('-');
    let y: i32 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some()
        || !(1970..=2200).contains(&y)
        || !(1..=12).contains(&m)
        || !(1..=31).contains(&d)
    {
        return None;
    }
    Some(unix_from_date(y, m, d))
}

/// `2026-01-31` for a time stamp in seconds since 1970 (0 = unknown).
pub fn fmt_date(secs: i64, offset_secs: i64) -> String {
    if secs <= 0 {
        return "unknown".into();
    }
    let (y, m, d, ..) = civil(secs + offset_secs);
    format!("{y:04}-{m:02}-{d:02}")
}

/// `2026-01-31 14:05` for a time stamp in seconds since 1970 (0 = unknown).
pub fn fmt_datetime(secs: i64, offset_secs: i64) -> String {
    if secs <= 0 {
        return "unknown".into();
    }
    let (y, m, d, h, mi, _) = civil(secs + offset_secs);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// Parses sizes such as `10`, `4 KB`, `1.5mb`, `2G` into bytes.
pub fn parse_size(s: &str) -> Option<u64> {
    let t = s.trim().to_lowercase().replace(' ', "");
    if t.is_empty() {
        return None;
    }
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let value: f64 = t[..split].parse().ok()?;
    let mult: f64 = match &t[split..] {
        "" | "b" => 1.0,
        "k" | "kb" => 1024.0,
        "m" | "mb" => 1024.0 * 1024.0,
        "g" | "gb" => 1024.0 * 1024.0 * 1024.0,
        "t" | "tb" => 1024.0_f64.powi(4),
        _ => return None,
    };
    Some((value * mult) as u64)
}

/// Lower-cased path with backslashes, no quotes, no `\\?\` prefix and no
/// trailing separator. `C:/Photos/Trip\` becomes `c:\photos\trip`.
pub fn norm_path(p: &str) -> String {
    let mut s: String = p
        .trim()
        .trim_matches('"')
        .trim()
        .replace('/', "\\")
        .to_lowercase();
    if let Some(rest) = s.strip_prefix("\\\\?\\unc\\") {
        s = format!("\\\\{rest}");
    } else if let Some(rest) = s.strip_prefix("\\\\?\\") {
        s = rest.to_string();
    }
    let unc = s.starts_with("\\\\");
    while s[usize::from(unc)..].contains("\\\\") {
        let tail = s[usize::from(unc)..].replace("\\\\", "\\");
        s = if unc { format!("\\{tail}") } else { tail };
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    if s.len() == 3 && s.ends_with(":\\") {
        s.pop();
    }
    s
}

/// True when `path` equals `dir` or lies inside it (both in any spelling).
pub fn path_within(path: &str, dir: &str) -> bool {
    let p = norm_path(path);
    let d = norm_path(dir);
    !d.is_empty() && (p == d || p.starts_with(&format!("{d}\\")))
}

/// [`path_within`] for `Path` values.
pub fn within(path: &Path, dir: &Path) -> bool {
    path_within(&path.to_string_lossy(), &dir.to_string_lossy())
}

/// The file name of a path as text (empty when there is none).
pub fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folder part of a path as text.
pub fn parent_text(path: &Path) -> String {
    path.parent()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Lower-cased extension without the dot (empty when there is none).
pub fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Case-insensitive wildcard match: `*` is any run of characters, `?` is one.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Splits a user-typed list (`jpg, .PNG;gif  tmp`) into clean lower-case extensions.
pub fn parse_extensions(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in s.split([',', ';', ' ', '\t', '\n']) {
        let e = part
            .trim()
            .trim_start_matches('*')
            .trim_start_matches('.')
            .to_lowercase();
        if !e.is_empty() && !out.contains(&e) {
            out.push(e);
        }
    }
    out
}

/// Escapes text for HTML.
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Quotes one CSV field when needed.
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1023), "1023 B");
        assert_eq!(fmt_size(1536), "1.5 KB");
        assert_eq!(fmt_size(250 * 1024 * 1024), "250 MB");
        assert_eq!(parse_size("4 KB"), Some(4096));
        assert_eq!(parse_size("1.5mb"), Some(1_572_864));
        assert_eq!(parse_size("2G"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_size("12"), Some(12));
        assert_eq!(parse_size("big"), None);
        assert_eq!(parse_size(""), None);
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1,000");
    }

    #[test]
    fn dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(unix_from_date(2000, 2, 29), 951_782_400);
        assert_eq!(parse_date("2024-03-01"), Some(unix_from_date(2024, 3, 1)));
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date("yesterday"), None);
        assert_eq!(
            fmt_datetime(951_782_400 + 3600 + 120, 0),
            "2000-02-29 01:02"
        );
        assert_eq!(fmt_date(951_782_400, 3 * 3600), "2000-02-29");
        assert_eq!(fmt_date(0, 0), "unknown");
        assert_eq!(fmt_duration(75), "1 min 15 s");
        assert_eq!(fmt_duration(3700), "1 h 1 min");
        assert_eq!(fmt_track(185), "3:05");
    }

    #[test]
    fn paths() {
        assert_eq!(norm_path("\"C:/Photos/Trip\\\""), "c:\\photos\\trip");
        assert_eq!(norm_path("C:\\"), "c:");
        assert_eq!(norm_path("C:\\a\\\\b\\"), "c:\\a\\b");
        assert_eq!(norm_path("\\\\?\\C:\\a"), "c:\\a");
        assert_eq!(norm_path("\\\\server\\share\\x\\"), "\\\\server\\share\\x");
        assert!(path_within("C:\\Data\\Foo\\bin\\x.bin", "c:\\data\\foo\\"));
        assert!(!path_within("C:\\Data\\FooBar", "c:\\data\\foo"));
        assert!(!path_within("C:\\Data", ""));
        assert!(path_within("C:\\Data", "C:\\"));
        assert_eq!(extension(Path::new("a\\b.JPG")), "jpg");
        assert_eq!(file_name(Path::new("a\\b.JPG")), "b.JPG");
    }

    #[test]
    fn wildcards_and_lists() {
        assert!(wildcard("*.tmp", "Report.TMP"));
        assert!(wildcard("~$*", "~$letter.docx"));
        assert!(wildcard("node_modules", "Node_Modules"));
        assert!(wildcard("a?c*", "abcdef"));
        assert!(!wildcard("*.tmp", "tmp.txt"));
        assert!(!wildcard("a?c", "ac"));
        assert!(wildcard("*", ""));
        assert_eq!(
            parse_extensions("jpg, .PNG;*.gif  jpg"),
            vec!["jpg", "png", "gif"]
        );
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(html_escape("<a & b>"), "&lt;a &amp; b&gt;");
    }
}
