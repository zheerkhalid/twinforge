//! Writes a result list as CSV, HTML or JSON.

use crate::model::{Detail, ExportFormat, ScanResult};
use crate::util::{csv_field, fmt_count, fmt_datetime, fmt_size, fmt_track, html_escape};
use std::path::Path;

fn detail_text(d: &Detail) -> String {
    match d {
        Detail::None => String::new(),
        Detail::Image {
            width,
            height,
            similarity,
        } => format!("{width} x {height}, {similarity}% similar"),
        Detail::Audio {
            artist,
            title,
            album,
            duration_secs,
            bitrate_kbps,
        } => {
            let mut parts: Vec<String> = Vec::new();
            for (label, v) in [("artist", artist), ("title", title), ("album", album)] {
                if !v.is_empty() {
                    parts.push(format!("{label}: {v}"));
                }
            }
            if *duration_secs > 0 {
                parts.push(fmt_track(*duration_secs));
            }
            if *bitrate_kbps > 0 {
                parts.push(format!("{bitrate_kbps} kbps"));
            }
            parts.join(", ")
        }
        Detail::Shortcut { target } => format!("points to {target}"),
        Detail::Note(n) => n.clone(),
    }
}

/// CSV with a header row; one row per file. Starts with a byte-order mark so
/// spreadsheet programs read non-English names correctly.
pub fn to_csv(result: &ScanResult, tz_offset: i64) -> String {
    let mut out = String::from(
        "\u{feff}Group,Path,Size (bytes),Modified,Selected,Reference,Details,Group note\r\n",
    );
    for (n, group) in result.groups.iter().enumerate() {
        for item in &group.items {
            let fields = [
                (n + 1).to_string(),
                csv_field(&item.file.path.to_string_lossy()),
                item.file.size.to_string(),
                fmt_datetime(item.file.modified, tz_offset),
                if item.selected { "yes" } else { "no" }.to_string(),
                if item.reference { "yes" } else { "no" }.to_string(),
                csv_field(&detail_text(&item.detail)),
                csv_field(&group.label),
            ];
            out.push_str(&fields.join(","));
            out.push_str("\r\n");
        }
    }
    out
}

/// Pretty JSON: summary plus groups.
pub fn to_json(result: &ScanResult) -> String {
    let groups: Vec<serde_json::Value> = result
        .groups
        .iter()
        .map(|g| {
            serde_json::json!({
                "note": g.label,
                "wasted_bytes": if result.finder.is_grouped() { g.wasted_bytes() } else { g.total_bytes() },
                "files": g.items.iter().map(|i| serde_json::json!({
                    "path": i.file.path.to_string_lossy(),
                    "size": i.file.size,
                    "modified": i.file.modified,
                    "is_folder": i.file.is_dir,
                    "selected": i.selected,
                    "reference": i.reference,
                    "details": detail_text(&i.detail),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "tool": "TwinForge",
        "version": env!("CARGO_PKG_VERSION"),
        "finder": result.finder.title(),
        "files_scanned": result.files_scanned,
        "bytes_scanned": result.bytes_scanned,
        "elapsed_ms": result.elapsed_ms,
        "cancelled": result.cancelled,
        "group_count": result.groups.len(),
        "file_count": result.item_count(),
        "wasted_bytes": result.wasted_bytes(),
        "groups": groups,
        "errors": result.errors,
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// A self-contained HTML page.
pub fn to_html(result: &ScanResult, tz_offset: i64) -> String {
    let mut rows = String::new();
    for (n, group) in result.groups.iter().enumerate() {
        if result.finder.is_grouped() {
            rows.push_str(&format!(
                "<tr class=\"g\"><td colspan=\"5\">Group {} &middot; {} files &middot; {} could be freed{}</td></tr>\n",
                n + 1,
                group.items.len(),
                fmt_size(group.wasted_bytes()),
                if group.label.is_empty() {
                    String::new()
                } else {
                    format!(" &middot; {}", html_escape(&group.label))
                }
            ));
        }
        for item in &group.items {
            let mark = if item.reference {
                "reference"
            } else if item.selected {
                "selected"
            } else {
                ""
            };
            rows.push_str(&format!(
                "<tr><td>{}</td><td class=\"n\">{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                html_escape(&item.file.path.to_string_lossy()),
                fmt_size(item.file.size),
                fmt_datetime(item.file.modified, tz_offset),
                html_escape(&detail_text(&item.detail)),
                mark
            ));
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>TwinForge: {title}</title>\n<style>\nbody{{font:14px 'Segoe UI',sans-serif;margin:32px;color:#221c18;background:#f7f5f2}}\nh1{{font-size:22px;margin:0 0 4px}}p{{color:#665e58;margin:0 0 18px}}\ntable{{border-collapse:collapse;width:100%;background:#fff;border:1px solid #e2dcd5}}\nth,td{{text-align:left;padding:6px 10px;border-bottom:1px solid #eee7df;vertical-align:top}}\nth{{background:#fff3e6}}td.n{{white-space:nowrap}}tr.g td{{background:#fbf3ea;font-weight:600}}\n</style></head><body>\n<h1>{title}</h1>\n<p>{groups} groups &middot; {files} files &middot; {wasted} could be freed &middot; {scanned} files scanned</p>\n<table><tr><th>Path</th><th>Size</th><th>Modified</th><th>Details</th><th>Mark</th></tr>\n{rows}</table>\n</body></html>\n",
        title = html_escape(result.finder.title()),
        groups = fmt_count(result.groups.len() as u64),
        files = fmt_count(result.item_count() as u64),
        wasted = fmt_size(result.wasted_bytes()),
        scanned = fmt_count(result.files_scanned),
    )
}

pub fn render(result: &ScanResult, format: ExportFormat, tz_offset: i64) -> String {
    match format {
        ExportFormat::Csv => to_csv(result, tz_offset),
        ExportFormat::Html => to_html(result, tz_offset),
        ExportFormat::Json => to_json(result),
    }
}

/// Writes the list to `path`; the format follows the file extension (CSV when unknown).
pub fn write(result: &ScanResult, path: &Path, tz_offset: i64) -> Result<ExportFormat, String> {
    crate::sys::guard_write()?;
    let format = ExportFormat::from_path(path).unwrap_or(ExportFormat::Csv);
    std::fs::write(path, render(result, format, tz_offset))
        .map_err(|e| format!("Could not write {}: {e}", path.to_string_lossy()))?;
    Ok(format)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Finder, Group, Item};
    use std::path::PathBuf;

    fn sample() -> ScanResult {
        let file = |p: &str, size| FileEntry {
            path: PathBuf::from(p),
            size,
            modified: 951_782_400,
            ..Default::default()
        };
        let mut r = ScanResult::empty(Finder::SimilarImages);
        let mut a = Item::with_detail(
            file("X:\\Pics\\a, \"best\".jpg", 2048),
            Detail::Image {
                width: 800,
                height: 600,
                similarity: 100,
            },
        );
        a.reference = true;
        let mut b = Item::new(file("X:\\Pics\\<b>.jpg", 1024));
        b.selected = true;
        r.groups.push(Group::new(vec![a, b], "97% similar"));
        r.files_scanned = 12;
        r
    }

    #[test]
    fn csv_quotes_and_lists_every_file() {
        let csv = to_csv(&sample(), 0);
        let lines: Vec<&str> = csv.trim_end().split("\r\n").collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("\u{feff}Group,Path"));
        assert_eq!(
            lines[1],
            "1,\"X:\\Pics\\a, \"\"best\"\".jpg\",2048,2000-02-29 00:00,no,yes,\"800 x 600, 100% similar\",97% similar"
        );
        assert!(lines[2].contains(",yes,no,"));
    }

    #[test]
    fn json_and_html_are_well_formed() {
        let r = sample();
        let v: serde_json::Value = serde_json::from_str(&to_json(&r)).unwrap();
        assert_eq!(v["group_count"], 1);
        assert_eq!(v["wasted_bytes"], 1024);
        assert_eq!(v["groups"][0]["files"][1]["selected"], true);
        let html = to_html(&r, 0);
        assert!(html.contains("&lt;b&gt;.jpg"));
        assert!(!html.contains("<b>.jpg"));
        assert!(html.contains("1.0 KB could be freed"));
        assert!(html.ends_with("</html>\n"));
    }

    #[test]
    fn write_picks_the_format_from_the_name() {
        let dir = std::env::temp_dir().join(format!("twinforge-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = sample();
        assert_eq!(write(&r, &dir.join("list.json"), 0), Ok(ExportFormat::Json));
        assert_eq!(write(&r, &dir.join("list.HTM"), 0), Ok(ExportFormat::Html));
        assert_eq!(write(&r, &dir.join("list.txt"), 0), Ok(ExportFormat::Csv));
        assert!(std::fs::read_to_string(dir.join("list.json"))
            .unwrap()
            .starts_with('{'));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
