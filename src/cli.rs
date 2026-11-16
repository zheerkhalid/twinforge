//! The command line: read-only scans with an optional export, the self-test,
//! and the help text. The command line never deletes or moves anything.

use crate::model::{Finder, HashAlgo, Progress, ScanConfig, ScanResult};
use crate::select::{self, Rule};
use crate::util::{fmt_count, fmt_duration, fmt_size, parse_extensions, parse_size};
use crate::{engine, export, sys};
use std::path::PathBuf;

pub const USAGE: &str = "TwinForge: finds duplicate files, similar pictures and other clutter

  TwinForge.exe                      open the program
  TwinForge.exe <folder> ...         open the program with these folders
  TwinForge.exe --scan <path> ...    scan from the command line (read-only) and print a summary
        --finder <name>              duplicates (default), pictures, music, names, empty-files,
                                     empty-folders, shortcuts, big, temp
        --export <file>              write the list as .csv, .json or .html
        --reference <folder>         files there are originals: compared, never marked (repeatable)
        --exclude <folder>           skip this folder (repeatable)
        --skip-name <wildcard>       skip files and folders with this name (repeatable)
        --min-size <size>            e.g. 500KB, 2MB           --max-size <size>
        --ext <list>                 only these types, e.g. jpg,png
        --similarity <60-100>        for pictures (default 90)
        --hash blake3|xxh3           for duplicates (default blake3)
        --verify                     also compare duplicates byte by byte
        --keep <rule>                mark what a rule would remove: newest, oldest, largest,
                                     smallest, best-resolution, best-audio, shortest-path,
                                     longest-path
        --no-cache  --no-recursive  --include-hidden  --include-system  --list
  TwinForge.exe --demo               show invented data; nothing on this PC is changed
  TwinForge.exe --screenshot <png>   render the window to a PNG and exit
        [--view duplicates|pictures|music|names|empty-files|empty-folders|shortcuts|big|temp|
                setup|scanning|confirm|report|history|settings|about|keys] [--pose <name>] [--light]
  TwinForge.exe --selftest [--recycle]
                                     build a throwaway folder tree in the temp folder, scan it,
                                     remove duplicates, check every safety rule, clean up
  TwinForge.exe --selftest-ui        the same idea through the window: scan, select and delete in a
                                     throwaway folder without any clicks
  TwinForge.exe --version";

/// What `--scan` was asked to do.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanRequest {
    pub finder: Finder,
    pub cfg: ScanConfig,
    pub export: Option<PathBuf>,
    pub keep: Option<Rule>,
    pub list: bool,
}

fn rule_from(word: &str) -> Option<Rule> {
    Some(match word {
        "newest" => Rule::KeepNewest,
        "oldest" => Rule::KeepOldest,
        "largest" => Rule::KeepLargest,
        "smallest" => Rule::KeepSmallest,
        "best-resolution" => Rule::KeepHighestResolution,
        "best-audio" => Rule::KeepBestAudio,
        "shortest-path" => Rule::KeepShortestPath,
        "longest-path" => Rule::KeepLongestPath,
        _ => return None,
    })
}

/// Parses the arguments of a `--scan` run.
pub fn parse_scan(args: &[String]) -> Result<ScanRequest, String> {
    let mut req = ScanRequest {
        finder: Finder::Duplicates,
        cfg: ScanConfig::default(),
        export: None,
        keep: None,
        list: false,
    };
    let mut no_cache = false;
    let mut i = 0;
    let value = |i: &mut usize, flag: &str| -> Result<String, String> {
        *i += 1;
        args.get(*i)
            .filter(|v| !v.starts_with("--"))
            .cloned()
            .ok_or(format!("{flag} needs a value."))
    };
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "--scan" => {
                while args
                    .get(i + 1)
                    .map(|v| !v.starts_with("--"))
                    .unwrap_or(false)
                {
                    i += 1;
                    req.cfg.include.push(PathBuf::from(&args[i]));
                }
            }
            "--finder" => {
                let v = value(&mut i, a)?;
                req.finder =
                    Finder::from_key(&v).ok_or(format!("Unknown finder \"{v}\". See --help."))?;
            }
            "--export" => req.export = Some(PathBuf::from(value(&mut i, a)?)),
            "--reference" => req.cfg.reference.push(PathBuf::from(value(&mut i, a)?)),
            "--exclude" => req.cfg.exclude.push(PathBuf::from(value(&mut i, a)?)),
            "--skip-name" => req.cfg.exclude_patterns.push(value(&mut i, a)?),
            "--min-size" | "--max-size" => {
                let v = value(&mut i, a)?;
                let n =
                    parse_size(&v).ok_or(format!("\"{v}\" is not a size (try 500KB or 2MB)."))?;
                if a == "--min-size" {
                    req.cfg.min_size = n;
                } else {
                    req.cfg.max_size = Some(n);
                }
            }
            "--ext" => req.cfg.only_extensions = parse_extensions(&value(&mut i, a)?),
            "--similarity" => {
                let v = value(&mut i, a)?;
                req.cfg.similarity = v
                    .parse::<u8>()
                    .ok()
                    .filter(|n| (60..=100).contains(n))
                    .ok_or("--similarity takes a number from 60 to 100.")?;
            }
            "--hash" => {
                req.cfg.hash = match value(&mut i, a)?.as_str() {
                    "blake3" => HashAlgo::Blake3,
                    "xxh3" => HashAlgo::Xxh3,
                    other => return Err(format!("Unknown hash \"{other}\" (blake3 or xxh3).")),
                }
            }
            "--keep" => {
                let v = value(&mut i, a)?;
                req.keep = Some(rule_from(&v).ok_or(format!("Unknown rule \"{v}\". See --help."))?);
            }
            "--verify" => req.cfg.byte_verify = true,
            "--no-cache" => no_cache = true,
            "--no-recursive" => req.cfg.recursive = false,
            "--include-hidden" => req.cfg.skip_hidden = false,
            "--include-system" => req.cfg.skip_system = false,
            "--list" => req.list = true,
            other => return Err(format!("Unknown option \"{other}\". See --help.")),
        }
        i += 1;
    }
    if req.cfg.include.is_empty() {
        return Err("--scan needs at least one folder.".into());
    }
    for p in req.cfg.include.iter().chain(req.cfg.reference.iter()) {
        if !p.exists() {
            return Err(format!("{} does not exist.", p.to_string_lossy()));
        }
    }
    // Relative paths would make "is this inside that folder" checks unreliable.
    let absolute = |p: &PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone());
    req.cfg.include = req.cfg.include.iter().map(absolute).collect();
    req.cfg.reference = req.cfg.reference.iter().map(absolute).collect();
    req.cfg.exclude = req.cfg.exclude.iter().map(absolute).collect();
    if matches!(req.finder, Finder::EmptyFiles) {
        req.cfg.min_size = 0;
    }
    if !no_cache {
        req.cfg.cache_path = Some(sys::app_dir().join("hash-cache.bin"));
    }
    Ok(req)
}

/// The printed summary of a finished scan.
pub fn summary(result: &ScanResult, list: bool) -> Vec<String> {
    let mut out = vec![format!(
        "{}: {} files looked at ({}) in {}",
        result.finder.title(),
        fmt_count(result.files_scanned),
        fmt_size(result.bytes_scanned),
        fmt_duration(result.elapsed_ms / 1000)
    )];
    if result.finder.is_grouped() {
        out.push(format!(
            "{} groups, {} files, {} could be freed",
            fmt_count(result.groups.len() as u64),
            fmt_count(result.item_count() as u64),
            fmt_size(result.wasted_bytes())
        ));
    } else {
        out.push(format!(
            "{} found, {} in total",
            fmt_count(result.item_count() as u64),
            fmt_size(result.wasted_bytes())
        ));
    }
    if result.cache_hits > 0 {
        out.push(format!(
            "{} files were unchanged since the last scan and were not read again",
            fmt_count(result.cache_hits)
        ));
    }
    if result.selected_count() > 0 {
        out.push(format!(
            "The rule marks {} files ({}) for removal",
            fmt_count(result.selected_count() as u64),
            fmt_size(result.selected_bytes())
        ));
    }
    if !result.errors.is_empty() {
        out.push(format!("{} places could not be read", result.errors.len()));
    }
    if list {
        for (n, g) in result.groups.iter().enumerate() {
            if result.finder.is_grouped() {
                out.push(format!("[{}] {}", n + 1, g.label));
            }
            for item in &g.items {
                let mark = if item.reference {
                    "ref"
                } else if item.selected {
                    " x "
                } else {
                    "   "
                };
                out.push(format!(
                    "  {mark} {:>10}  {}",
                    fmt_size(item.file.size),
                    item.file.path.to_string_lossy()
                ));
            }
        }
    }
    out
}

/// Runs a command-line request and returns the exit code.
pub fn run(args: &[String]) -> i32 {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("--help") || has("-h") || has("/?") {
        println!("{USAGE}");
        return 0;
    }
    if has("--version") {
        println!("TwinForge {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }
    if has("--selftest") {
        return match crate::selftest::run(has("--recycle")) {
            Ok(log) => {
                for line in log {
                    println!("{line}");
                }
                println!("SELFTEST PASSED");
                0
            }
            Err(e) => {
                println!("{e}");
                println!("SELFTEST FAILED");
                1
            }
        };
    }
    let req = match parse_scan(args) {
        Ok(r) => r,
        Err(e) => {
            println!("{e}");
            return 2;
        }
    };
    let mut result = engine::run(req.finder, &req.cfg, &Progress::new());
    if let Some(rule) = &req.keep {
        if req.finder.is_grouped() {
            select::apply(&mut result.groups, rule);
        }
    }
    for line in summary(&result, req.list) {
        println!("{line}");
    }
    if let Some(path) = &req.export {
        match export::write(&result, path, sys::local_offset_secs()) {
            Ok(_) => println!("List written to {}", path.to_string_lossy()),
            Err(e) => {
                println!("{e}");
                return 1;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_a_full_scan_request() {
        let temp = std::env::temp_dir();
        let t = temp.to_string_lossy().into_owned();
        let req = parse_scan(&args(&[
            "--scan",
            &t,
            &t,
            "--finder",
            "pictures",
            "--export",
            "out.json",
            "--reference",
            &t,
            "--min-size",
            "2MB",
            "--ext",
            "JPG,.png",
            "--similarity",
            "85",
            "--hash",
            "xxh3",
            "--keep",
            "best-resolution",
            "--no-cache",
            "--include-hidden",
            "--list",
            "--verify",
            "--skip-name",
            "*.iso",
        ]))
        .unwrap();
        assert_eq!(req.finder, Finder::SimilarImages);
        assert_eq!(req.cfg.include.len(), 2);
        assert_eq!(req.cfg.reference.len(), 1);
        assert_eq!(req.cfg.min_size, 2 * 1024 * 1024);
        assert_eq!(req.cfg.only_extensions, ["jpg", "png"]);
        assert_eq!(req.cfg.similarity, 85);
        assert_eq!(req.cfg.hash, HashAlgo::Xxh3);
        assert_eq!(req.keep, Some(Rule::KeepHighestResolution));
        assert_eq!(req.cfg.cache_path, None);
        assert!(!req.cfg.skip_hidden && req.cfg.skip_system && req.list && req.cfg.byte_verify);
        assert_eq!(req.cfg.exclude_patterns, ["*.iso"]);
        assert_eq!(req.export, Some(PathBuf::from("out.json")));
    }

    #[test]
    fn rejects_mistakes_with_a_reason() {
        let t = std::env::temp_dir().to_string_lossy().into_owned();
        let err = |list: &[&str]| parse_scan(&args(list)).unwrap_err();
        assert!(err(&["--scan"]).contains("at least one folder"));
        assert!(err(&["--scan", "Q:\\twinforge\\no\\such\\folder"]).contains("does not exist"));
        assert!(err(&["--scan", &t, "--finder", "everything"]).contains("Unknown finder"));
        assert!(err(&["--scan", &t, "--similarity", "20"]).contains("60 to 100"));
        assert!(err(&["--scan", &t, "--min-size"]).contains("needs a value"));
        assert!(err(&["--scan", &t, "--delete"]).contains("Unknown option"));
        assert!(err(&["--scan", &t, "--keep", "random"]).contains("Unknown rule"));
        // Defaults: cache on, duplicates.
        let req = parse_scan(&args(&["--scan", &t])).unwrap();
        assert!(req.cfg.cache_path.is_some());
        assert_eq!(req.finder, Finder::Duplicates);
        assert_eq!(run(&args(&["--version"])), 0);
        assert_eq!(run(&args(&["--scan"])), 2);
    }

    #[test]
    fn summary_lines() {
        let mut r = ScanResult::empty(Finder::Duplicates);
        r.files_scanned = 1200;
        r.elapsed_ms = 2500;
        let lines = summary(&r, true);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("1,200 files looked at"));
        assert!(lines[1].starts_with("0 groups"));
    }
}
