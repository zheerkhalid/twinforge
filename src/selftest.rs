//! End-to-end self-test (`TwinForge.exe --selftest`).
//!
//! Builds a throwaway folder tree inside the temp folder, runs every finder
//! over it, applies selection rules, removes / moves / links duplicates through
//! the real action engine, tries the things that must be refused, and cleans
//! up. Nothing outside that one temp folder is touched, except that with
//! `--recycle` three tiny generated files are sent to the Recycle Bin.

use crate::actions::{self, Method, Plan, Status};
use crate::history::{self, Entry};
use crate::model::{Detail, Finder, Group, Progress, ScanConfig, ScanResult};
use crate::select::{self, Rule};
use crate::sys::{self, Guard};
use crate::util::{file_name, fmt_size, within};
use crate::{engine, export, lnk, music};
use std::fs;
use std::path::{Path, PathBuf};

fn ensure(ok: bool, what: &str) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("FAILED: {what}"))
    }
}

/// Deterministic filler bytes (a small xorshift generator).
pub fn filler(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A generated picture: smooth colour fields with a few shapes, different for every seed.
pub fn picture(seed: u32, width: u32, height: u32) -> image::RgbImage {
    let s = seed as f32;
    image::RgbImage::from_fn(width, height, |x, y| {
        let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
        let wave = ((u * (3.0 + s % 4.0) + s).sin() + (v * (2.0 + s % 3.0) - s * 0.7).cos()) * 0.25;
        let (cx, cy) = (0.25 + (s * 0.37) % 0.5, 0.25 + (s * 0.61) % 0.5);
        let d = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
        let disc = if d < 0.18 + (s % 5.0) * 0.02 {
            0.5
        } else {
            0.0
        };
        let bar = if (u + v * (0.5 + s % 2.0)) % 0.5 < 0.08 {
            0.3
        } else {
            0.0
        };
        let r = (0.45 + wave + disc - bar).clamp(0.0, 1.0);
        let g =
            (0.5 + wave * 0.6 - disc * 0.5 + (v - 0.5) * (0.6 - (s % 3.0) * 0.3)).clamp(0.0, 1.0);
        let b = (0.55 - wave + bar + (u - 0.5) * 0.5).clamp(0.0, 1.0);
        image::Rgb([(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8])
    })
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.to_string_lossy()))
}

fn scan(finder: Finder, cfg: &ScanConfig) -> ScanResult {
    engine::run(finder, cfg, &Progress::new())
}

fn names(group: &Group) -> Vec<String> {
    let mut v: Vec<String> = group
        .items
        .iter()
        .map(|i| file_name(&i.file.path))
        .collect();
    v.sort();
    v
}

fn group_with<'a>(result: &'a ScanResult, name: &str) -> Option<&'a Group> {
    result
        .groups
        .iter()
        .find(|g| g.items.iter().any(|i| file_name(&i.file.path) == name))
}

fn invariant_holds(result: &ScanResult) -> bool {
    result.groups.iter().all(|g| {
        g.items.iter().any(|i| !i.selected) && g.items.iter().all(|i| !(i.selected && i.reference))
    })
}

/// Runs the self-test. Returns the log on success, the first failure otherwise.
pub fn run(recycle: bool) -> Result<Vec<String>, String> {
    if sys::is_demo() {
        return Err("The self-test cannot run in demo mode.".into());
    }
    let base = std::env::temp_dir().join(format!("twinforge-selftest-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let result = run_in(&base, recycle);
    // Junctions first, so that removing the tree can never reach through one.
    let _ = fs::remove_dir(base.join("tree\\link-to-outside"));
    let cleaned = fs::remove_dir_all(&base);
    let mut log = result?;
    ensure(
        cleaned.is_ok() && !base.exists(),
        "the temp folder was removed again",
    )?;
    log.push("Clean-up: the temp folder is gone".into());
    Ok(log)
}

fn run_in(base: &Path, recycle: bool) -> Result<Vec<String>, String> {
    let mut log: Vec<String> = Vec::new();
    let tree = base.join("tree");
    let outside = base.join("outside");
    let app = base.join("app");

    // ---------------------------------------------------------------- the tree
    let big = filler(1, 300_000);
    let mut near = big.clone();
    near[150_000] ^= 0xFF; // same size, same first and last blocks, different middle
    write(&tree.join("docs\\report.bin"), &big)?;
    write(&tree.join("backup\\report copy.bin"), &big)?;
    write(&tree.join("backup\\old\\report.bin"), &big)?;
    write(&tree.join("docs\\almost.bin"), &near)?;
    // Keeps this folder from becoming empty once its duplicate is deleted.
    write(&tree.join("backup\\old\\list of old backups.txt"), b"kept")?;
    write(&tree.join("docs\\note.txt"), b"hello")?;
    write(&tree.join("backup\\note (1).txt"), b"hello")?;
    write(&tree.join("masters\\report-master.bin"), &big)?;
    write(&tree.join("protected\\report-system.bin"), &big)?;
    write(&outside.join("report-outside.bin"), &big)?;
    fs::hard_link(
        tree.join("docs\\report.bin"),
        tree.join("docs\\report-hardlink.bin"),
    )
    .map_err(|e| format!("hard link: {e}"))?;
    ensure(
        sys::make_junction(&tree.join("link-to-outside"), &outside),
        "a junction could be created in the temp folder",
    )?;

    let base_pic = picture(3, 640, 480);
    let save = |img: &image::RgbImage, path: PathBuf| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        image::DynamicImage::ImageRgb8(img.clone())
            .save(&path)
            .map_err(|e| format!("{}: {e}", path.to_string_lossy()))
    };
    save(&base_pic, tree.join("pics\\harbour.png"))?;
    save(
        &image::imageops::resize(&base_pic, 320, 240, image::imageops::FilterType::Triangle),
        tree.join("pics\\harbour-small.jpg"),
    )?;
    save(
        &image::imageops::rotate90(&base_pic),
        tree.join("pics\\export\\harbour-rotated.png"),
    )?;
    save(&picture(11, 640, 480), tree.join("pics\\meadow.png"))?;

    music::write_test_wav(
        &tree.join("music\\a\\Low Sun.wav"),
        "Tidewater",
        "Low Sun",
        "Shorelines",
        2,
    )?;
    music::write_test_wav(
        &tree.join("music\\b\\tidewater - low sun (remastered).wav"),
        "tidewater",
        "Low Sun (Remastered)",
        "Shorelines",
        2,
    )?;
    music::write_test_wav(
        &tree.join("music\\b\\Other.wav"),
        "Paper Kites",
        "Glass Morning",
        "Atlas",
        2,
    )?;

    write(&tree.join("misc\\zero.dat"), b"")?;
    write(&tree.join("misc\\draft.tmp"), b"temp")?;
    write(&tree.join("misc\\~$letter.docx"), b"lock")?;
    write(&tree.join("misc\\keep.txt"), b"keep me")?;
    write(
        &tree.join("misc\\other\\keep.txt"),
        b"another file with the same name",
    )?;
    fs::create_dir_all(tree.join("hollow\\inner\\deep")).map_err(|e| e.to_string())?;
    write(&tree.join("not-hollow\\.hidden"), b"x")?;
    sys::set_attributes(
        &tree.join("not-hollow\\.hidden"),
        sys::FILE_ATTRIBUTE_HIDDEN,
    );
    write(
        &tree.join("misc\\Broken.lnk"),
        &lnk::build(&tree.join("misc\\gone.exe").to_string_lossy()),
    )?;
    write(
        &tree.join("misc\\Working.lnk"),
        &lnk::build(&tree.join("misc\\keep.txt").to_string_lossy()),
    )?;
    log.push(format!(
        "Built a test tree in the temp folder ({} files)",
        count_files(&tree)
    ));

    let mut cfg = ScanConfig::for_paths(&[&tree]);
    cfg.reference = vec![tree.join("masters")];
    cfg.cache_path = Some(app.join("hash-cache.bin"));
    let guard = Guard::with_roots(&[tree.join("protected")]);

    // ---------------------------------------------------------------- exact duplicates
    let mut dupes = scan(Finder::Duplicates, &cfg);
    ensure(
        dupes.groups.len() == 2,
        &format!("two duplicate groups (got {})", dupes.groups.len()),
    )?;
    let g = group_with(&dupes, "report-master.bin").ok_or("FAILED: the big group exists")?;
    let found = names(g);
    ensure(
        found.len() == 5,
        &format!("the big group holds the five real copies and not the hard link: {found:?}"),
    )?;
    ensure(
        !found.contains(&"almost.bin".to_string()),
        "a file that differs in the middle is not a duplicate",
    )?;
    ensure(
        !found.contains(&"report-outside.bin".to_string()),
        "the junction was not followed",
    )?;
    ensure(
        g.items[0].reference && g.items.iter().filter(|i| i.reference).count() == 1,
        "the reference copy is marked and leads its group",
    )?;
    ensure(dupes.bytes_read > 0, "files were read")?;
    log.push(format!(
        "Duplicates: 2 groups, {} could be freed; hard link, near-twin and junction correctly left out",
        fmt_size(dupes.wasted_bytes())
    ));

    let again = scan(Finder::Duplicates, &cfg);
    ensure(
        again.groups.len() == 2,
        "a second scan finds the same groups",
    )?;
    ensure(
        again.cache_hits > 0 && again.bytes_read < dupes.bytes_read,
        &format!(
            "the second scan used the hash cache (hits {}, read {} vs {})",
            again.cache_hits, again.bytes_read, dupes.bytes_read
        ),
    )?;
    log.push(format!(
        "Hash cache: second scan read {} instead of {}",
        fmt_size(again.bytes_read),
        fmt_size(dupes.bytes_read)
    ));

    // ---------------------------------------------------------------- selection rules
    for rule in [
        Rule::KeepNewest,
        Rule::KeepOldest,
        Rule::KeepLargest,
        Rule::KeepShortestPath,
        Rule::KeepLongestPath,
    ] {
        select::apply(&mut dupes.groups, &rule);
        ensure(
            invariant_holds(&dupes),
            &format!("{rule:?} keeps one file per group and never selects a reference"),
        )?;
        ensure(
            dupes.selected_count() > 0,
            &format!("{rule:?} selects something"),
        )?;
    }
    for g in &mut dupes.groups {
        for i in &mut g.items {
            i.selected = true; // a buggy caller ticks everything
        }
    }
    ensure(
        select::enforce(&mut dupes.groups) > 0 && invariant_holds(&dupes),
        "enforce repairs an everything-selected state",
    )?;
    log.push("Selection rules: every rule leaves at least one file per group; references are never ticked".into());

    // ---------------------------------------------------------------- refusals
    let mut forced = dupes.clone();
    for g in &mut forced.groups {
        for i in &mut g.items {
            i.selected = true;
            i.reference = false;
        }
    }
    let before = count_files(&tree);
    let report = actions::execute(
        &Plan::from_result(&forced, Method::Delete, true),
        &guard,
        &Progress::new(),
    );
    ensure(
        report.done() == 0 && report.refused() == report.planned,
        "a plan that removes every copy is refused entirely",
    )?;
    ensure(
        count_files(&tree) == before,
        "nothing was deleted by the refused plan",
    )?;
    log.push(format!(
        "Safety: a plan to delete all {} copies was refused; nothing was touched",
        report.planned
    ));

    // ---------------------------------------------------------------- real removal
    select::apply(&mut dupes.groups, &Rule::KeepShortestPath);
    let plan = Plan::from_result(&dupes, Method::Delete, true);
    // One selected file changes after the scan; it must survive.
    let changed = plan
        .groups
        .iter()
        .flat_map(|g| g.items.iter())
        .find(|i| i.remove && file_name(&i.path) == "note (1).txt")
        .map(|i| i.path.clone());
    if let Some(p) = &changed {
        write(p, b"hello, edited")?;
    }
    let report = actions::execute(&plan, &guard, &Progress::new());
    let protected_refused = report.outcomes.iter().any(|o| {
        within(&o.path, &tree.join("protected")) && matches!(o.status, Status::Refused(_))
    });
    ensure(
        protected_refused && tree.join("protected\\report-system.bin").exists(),
        "the copy in the protected folder was refused",
    )?;
    ensure(
        tree.join("masters\\report-master.bin").exists(),
        "the reference copy is untouched",
    )?;
    if let Some(p) = &changed {
        ensure(
            p.exists(),
            "the file that changed after the scan was left alone",
        )?;
    }
    ensure(
        report.done() >= 1,
        &format!("duplicates were deleted: {:?}", report.outcomes),
    )?;
    ensure(report.failed() == 0, "no unexpected failures")?;
    for o in report.outcomes.iter().filter(|o| o.status == Status::Done) {
        ensure(!o.path.exists(), "deleted files are gone")?;
        ensure(
            o.kept.as_ref().map(|k| k.exists()).unwrap_or(false),
            "each deleted file has a kept copy that still exists",
        )?;
    }
    ensure(
        outside.join("report-outside.bin").exists(),
        "the file behind the junction is untouched",
    )?;
    let entry = Entry::from_report(&report);
    history::save(&app, &entry)?;
    ensure(history::load_all(&app).len() == 1, "the action was logged")?;
    log.push(format!(
        "Delete: {} removed, {} refused (protected folder, changed file); every kept copy verified",
        report.done(),
        report.refused()
    ));

    // ---------------------------------------------------------------- move + undo, links
    write(&tree.join("round2\\a\\twin.bin"), &filler(7, 50_000))?;
    write(&tree.join("round2\\b\\twin.bin"), &filler(7, 50_000))?;
    write(&tree.join("round2\\c\\twin.bin"), &filler(7, 50_000))?;
    let mut cfg2 = ScanConfig::for_paths(&[tree.join("round2")]);
    cfg2.cache_path = cfg.cache_path.clone();
    let mut r2 = scan(Finder::Duplicates, &cfg2);
    ensure(
        r2.groups.len() == 1 && r2.groups[0].items.len() == 3,
        "the second round finds its three twins",
    )?;
    ensure(
        select::set_selected(&mut r2.groups[0], 2, true),
        "one file can be ticked",
    )?;
    let moved_from = r2.groups[0].items[2].file.path.clone();
    let dest = base.join("moved");
    let report = actions::execute(
        &Plan::from_result(&r2, Method::MoveTo(dest.clone()), true),
        &guard,
        &Progress::new(),
    );
    ensure(
        report.done() == 1 && !moved_from.exists(),
        "the selected twin was moved away",
    )?;
    let mut entry = Entry::from_report(&report);
    history::save(&app, &entry)?;
    let undo = history::undo_moves(&app, &mut entry, &guard)?;
    ensure(
        undo.restored == 1 && moved_from.exists(),
        "undo moved it back",
    )?;
    log.push("Move: one twin moved to another folder, then put back with Undo".into());

    let mut r2 = scan(Finder::Duplicates, &cfg2);
    ensure(
        r2.groups.len() == 1,
        "the twins are found again after the undo",
    )?;
    select::apply(&mut r2.groups, &Rule::KeepShortestPath);
    let report = actions::execute(
        &Plan::from_result(&r2, Method::HardLink, false),
        &guard,
        &Progress::new(),
    );
    ensure(
        report.done() == 2,
        &format!("two twins became hard links: {:?}", report.outcomes),
    )?;
    let ids: Vec<_> = ["a", "b", "c"]
        .iter()
        .filter_map(|d| sys::file_id(&tree.join(format!("round2\\{d}\\twin.bin"))))
        .collect();
    ensure(
        ids.len() == 3 && ids[0].same_file(&ids[1]) && ids[1].same_file(&ids[2]),
        "all three names now share one file",
    )?;
    let r2 = scan(Finder::Duplicates, &cfg2);
    ensure(
        r2.groups.is_empty(),
        "hard-linked names are no longer reported as duplicates",
    )?;
    log.push("Hard links: two twins replaced by links; a new scan no longer reports them".into());

    if recycle {
        write(&tree.join("round3\\a\\bin-me.bin"), &filler(9, 2_000))?;
        write(&tree.join("round3\\b\\bin-me.bin"), &filler(9, 2_000))?;
        write(&tree.join("round3\\c\\bin-me.bin"), &filler(9, 2_000))?;
        write(&tree.join("round3\\zero.dat"), b"")?;
        // So that these folders are not empty once their twin is in the bin.
        write(&tree.join("round3\\b\\stays in b.txt"), b"b")?;
        write(&tree.join("round3\\c\\stays in c.txt"), b"c")?;
        let cfg3 = ScanConfig::for_paths(&[tree.join("round3")]);
        let mut r3 = scan(Finder::Duplicates, &cfg3);
        select::apply(&mut r3.groups, &Rule::KeepShortestPath);
        let report = actions::execute(
            &Plan::from_result(&r3, Method::Recycle, true),
            &guard,
            &Progress::new(),
        );
        ensure(
            report.done() == 2,
            &format!("two files went to the Recycle Bin: {:?}", report.outcomes),
        )?;
        let mut cfg3z = cfg3.clone();
        cfg3z.min_size = 0;
        let mut z = scan(Finder::EmptyFiles, &cfg3z);
        for g in &mut z.groups {
            g.items[0].selected = true;
        }
        let report = actions::execute(
            &Plan::from_result(&z, Method::Recycle, true),
            &guard,
            &Progress::new(),
        );
        ensure(report.done() == 1, "the empty file went to the Recycle Bin")?;
        log.push(
            "Recycle Bin: three generated files were sent there (restore or empty at will)".into(),
        );
    }

    // ---------------------------------------------------------------- similar pictures
    let pics = scan(Finder::SimilarImages, &cfg);
    let g = group_with(&pics, "harbour.png").ok_or(format!(
        "FAILED: the harbour pictures form a group ({} groups)",
        pics.groups.len()
    ))?;
    let found = names(g);
    ensure(
        found.contains(&"harbour-small.jpg".to_string()),
        &format!("the resized JPEG matches: {found:?}"),
    )?;
    ensure(
        found.contains(&"harbour-rotated.png".to_string()),
        &format!("the rotated copy matches: {found:?}"),
    )?;
    ensure(
        !found.contains(&"meadow.png".to_string()),
        "a different picture does not match",
    )?;
    ensure(
        g.items
            .iter()
            .all(|i| matches!(i.detail, Detail::Image { width, .. } if width > 0)),
        "picture sizes are reported",
    )?;
    log.push(format!("Similar pictures: resized, recompressed and rotated copies grouped ({found:?}); a different picture kept apart"));

    // ---------------------------------------------------------------- similar music
    let tracks = scan(Finder::SimilarMusic, &cfg);
    let g = group_with(&tracks, "Low Sun.wav").ok_or(format!(
        "FAILED: the two versions of the track form a group ({} groups)",
        tracks.groups.len()
    ))?;
    ensure(
        g.items.len() == 2 && !names(g).contains(&"Other.wav".to_string()),
        "only the matching tracks are grouped",
    )?;
    log.push(
        "Similar music: two taggings of the same track grouped; a different track kept apart"
            .into(),
    );

    // ---------------------------------------------------------------- the small finders
    let same = scan(Finder::SameName, &cfg);
    ensure(
        group_with(&same, "keep.txt").map(|g| g.items.len()) == Some(2),
        "same-name files are grouped",
    )?;
    let mut any_size = cfg.clone();
    any_size.min_size = 0;
    let empty_files = scan(Finder::EmptyFiles, &any_size);
    ensure(
        empty_files.groups.len() == 1 && group_with(&empty_files, "zero.dat").is_some(),
        &format!("the one empty file is found ({})", empty_files.groups.len()),
    )?;
    let mut empty_dirs = scan(Finder::EmptyFolders, &cfg);
    let dir_names: Vec<String> = empty_dirs
        .groups
        .iter()
        .map(|g| file_name(&g.items[0].file.path))
        .collect();
    ensure(
        dir_names == ["hollow"],
        &format!("exactly the hollow folder is reported: {dir_names:?}"),
    )?;
    let shortcuts = scan(Finder::BrokenShortcuts, &cfg);
    ensure(
        shortcuts.groups.len() == 1 && group_with(&shortcuts, "Broken.lnk").is_some(),
        &format!(
            "only the broken shortcut is reported ({})",
            shortcuts.groups.len()
        ),
    )?;
    let temp = scan(Finder::TempFiles, &cfg);
    let temp_names: Vec<String> = temp
        .groups
        .iter()
        .map(|g| file_name(&g.items[0].file.path))
        .collect();
    ensure(
        temp_names.contains(&"draft.tmp".to_string())
            && temp_names.contains(&"~$letter.docx".to_string())
            && !temp_names.contains(&"keep.txt".to_string()),
        &format!("temporary files are recognised: {temp_names:?}"),
    )?;
    let mut big_cfg = cfg.clone();
    big_cfg.big_count = 3;
    let biggest = scan(Finder::BigFiles, &big_cfg);
    ensure(
        biggest.groups.len() == 3
            && biggest
                .groups
                .windows(2)
                .all(|w| w[0].items[0].file.size >= w[1].items[0].file.size),
        "the biggest files come back largest first",
    )?;
    log.push("Other finders: same name, empty files, empty folders, broken shortcuts, temporary files, biggest files all correct".into());

    // An empty folder that gets a file after the scan must stay.
    empty_dirs.groups[0].items[0].selected = true;
    let plan = Plan::from_result(&empty_dirs, Method::Delete, true);
    write(&tree.join("hollow\\inner\\late.txt"), b"surprise")?;
    let report = actions::execute(&plan, &guard, &Progress::new());
    ensure(
        report.refused() == 1 && tree.join("hollow\\inner\\late.txt").exists(),
        "a folder that is no longer empty is refused",
    )?;
    fs::remove_file(tree.join("hollow\\inner\\late.txt")).map_err(|e| e.to_string())?;
    let report = actions::execute(&plan, &guard, &Progress::new());
    ensure(
        report.done() == 1 && !tree.join("hollow").exists(),
        "the empty folder was removed",
    )?;
    log.push(
        "Empty folders: removed when empty, refused when a file appeared after the scan".into(),
    );

    // ---------------------------------------------------------------- export
    for name in ["list.csv", "list.json", "list.html"] {
        let path = app.join(name);
        export::write(&same, &path, 0)?;
        ensure(
            fs::metadata(&path).map(|m| m.len() > 50).unwrap_or(false),
            &format!("{name} was written"),
        )?;
    }
    log.push("Export: CSV, JSON and HTML written".into());

    ensure(
        outside.join("report-outside.bin").exists()
            && fs::read(outside.join("report-outside.bin"))
                .map(|b| b == big)
                .unwrap_or(false),
        "the folder behind the junction is exactly as it was",
    )?;
    ensure(
        tree.join("docs\\almost.bin").exists() && tree.join("masters\\report-master.bin").exists(),
        "unique and reference files are still there",
    )?;
    Ok(log)
}

/// The same idea through the window: `launch` opens TwinForge on a generated
/// folder with the autopilot switched on (scan, select by rule, delete, close);
/// afterwards the disk is checked. Settings, cache and history live in the
/// temp folder too.
pub fn run_ui(
    launch: impl FnOnce(
        PathBuf,
        PathBuf,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> Result<(), String>,
) -> Result<Vec<String>, String> {
    let base = std::env::temp_dir().join(format!("twinforge-selftest-ui-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let (tree, app) = (base.join("tree"), base.join("app"));
    let run = || -> Result<Vec<String>, String> {
        for (n, name) in ["alpha", "beta", "gamma"].iter().enumerate() {
            let data = filler(20 + n as u64, 40_000 + n * 1000);
            write(&tree.join(format!("{name}.bin")), &data)?;
            write(&tree.join(format!("copies\\{name} copy.bin")), &data)?;
            write(
                &tree.join(format!("copies\\deeper\\{name} copy 2.bin")),
                &data,
            )?;
        }
        write(&tree.join("unique.bin"), &filler(99, 40_000))?;
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        launch(tree.clone(), app.clone(), log.clone())?;
        let mut lines = log.lock().map(|l| l.clone()).unwrap_or_default();
        let has = |lines: &[String], text: &str| lines.iter().any(|l| l.contains(text));
        ensure(
            has(
                &lines,
                "scan finished with 3 groups; the rule selected 6 files",
            ),
            &format!("the window found 3 groups and selected 6 files: {lines:?}"),
        )?;
        ensure(
            has(&lines, "confirmation shows 6 files"),
            &format!("the confirmation listed 6 files: {lines:?}"),
        )?;
        ensure(
            has(&lines, "6 deleted, 0 refused, 0 failed"),
            &format!("six files were deleted: {lines:?}"),
        )?;
        ensure(
            has(&lines, "0 groups left"),
            "the list emptied itself after the action",
        )?;
        // On disk: the shortest path of each group stayed, the copies are gone.
        for name in ["alpha", "beta", "gamma"] {
            ensure(
                tree.join(format!("{name}.bin")).exists(),
                "the kept copies are still there",
            )?;
            ensure(
                !tree.join(format!("copies\\{name} copy.bin")).exists(),
                "the selected copies are gone",
            )?;
        }
        ensure(
            count_files(&tree) == 4,
            "exactly the three kept files and the unique file remain",
        )?;
        ensure(
            history::load_all(&app).len() == 1,
            "the action is in the history",
        )?;
        ensure(
            app.join("hash-cache.bin").exists(),
            "the hash cache was written to the test folder",
        )?;
        lines.push(
            "Disk check: 3 kept copies and the unique file remain; history and cache written"
                .into(),
        );
        Ok(lines)
    };
    let result = run();
    let cleaned = fs::remove_dir_all(&base);
    let mut log = result?;
    ensure(
        cleaned.is_ok() && !base.exists(),
        "the temp folder was removed again",
    )?;
    log.push("Clean-up: the temp folder is gone".into());
    Ok(log)
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    let Ok(reader) = fs::read_dir(dir) else {
        return 0;
    };
    for e in reader.flatten() {
        let Ok(meta) = fs::symlink_metadata(e.path()) else {
            continue;
        };
        if meta.is_dir() && !sys::is_reparse_point(&e.path()) {
            n += count_files(&e.path());
        } else if meta.is_file() {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generators_are_deterministic() {
        assert_eq!(filler(5, 1000), filler(5, 1000));
        assert_ne!(filler(5, 1000), filler(6, 1000));
        assert_eq!(filler(5, 13).len(), 13);
        assert_eq!(picture(3, 64, 48), picture(3, 64, 48));
        assert_ne!(picture(3, 64, 48), picture(11, 64, 48));
    }
}
