//! The small finders: same name, empty files, empty folders, broken
//! shortcuts, biggest files, temporary files. All of them only list and read.

use crate::hashing;
use crate::lnk;
use crate::model::{Detail, FileEntry, Finder, Group, Item, Progress, ScanConfig, ScanResult};
use crate::sys::Guard;
use crate::util::{extension, file_name};
use crate::walk::{self, Tree, WalkOptions};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::time::Instant;

/// No shortcut needs more than this to name its target.
const SHORTCUT_READ_LIMIT: u64 = 1024 * 1024;
const MAX_ERRORS: usize = 200;

/// Runs one of the small finders (`SameName`, `EmptyFiles`, `EmptyFolders`,
/// `BrokenShortcuts`, `BigFiles`, `TempFiles`).
pub fn scan(finder: Finder, cfg: &ScanConfig, progress: &Progress) -> ScanResult {
    scan_with(finder, cfg, &Guard::real(), progress)
}

/// [`scan`] with an explicit guard (tests use their own).
pub fn scan_with(
    finder: Finder,
    cfg: &ScanConfig,
    guard: &Guard,
    progress: &Progress,
) -> ScanResult {
    let started = Instant::now();
    let mut result = ScanResult::empty(finder);
    let options = match finder {
        Finder::SameName | Finder::BigFiles => WalkOptions::ALL,
        Finder::EmptyFiles | Finder::TempFiles => WalkOptions::ANY_SIZE,
        Finder::EmptyFolders => WalkOptions::NONE,
        // Shortcuts are tiny and the extension is fixed, so neither filter applies.
        Finder::BrokenShortcuts => WalkOptions {
            size_filter: false,
            extension_filter: false,
            date_filter: true,
        },
        // The other finders have engines of their own.
        Finder::Duplicates | Finder::SimilarImages | Finder::SimilarMusic => return result,
    };
    progress.set_stage_count(2);
    let tree = walk::walk_with(cfg, options, guard, progress);
    result.files_scanned = tree.files.len() as u64;
    result.bytes_scanned = tree.bytes;
    result.errors = tree.errors.clone();
    // A listing that was cut short proves nothing: a folder that was not
    // read yet would look empty, the biggest files might be missing.
    if !progress.cancelled() {
        result.groups = match finder {
            Finder::SameName => same_name(tree, cfg, progress),
            Finder::EmptyFiles => empty_files(tree, progress),
            Finder::EmptyFolders => empty_folders(&tree, progress),
            Finder::BrokenShortcuts => {
                broken_shortcuts(tree, progress, &mut result.errors, &mut result.bytes_read)
            }
            Finder::BigFiles => big_files(tree, cfg.big_count, progress),
            Finder::TempFiles => temp_files(tree, progress),
            Finder::Duplicates | Finder::SimilarImages | Finder::SimilarMusic => Vec::new(),
        };
    }
    result.cancelled = progress.cancelled();
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    result
}

/// A flat list: one item per group.
fn flat(items: impl IntoIterator<Item = Item>) -> Vec<Group> {
    items
        .into_iter()
        .map(|item| Group::new(vec![item], ""))
        .collect()
}

fn by_path(a: &FileEntry, b: &FileEntry) -> Ordering {
    a.path.cmp(&b.path)
}

fn largest_first(a: &FileEntry, b: &FileEntry) -> Ordering {
    b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path))
}

// --------------------------------------------------------------------------------------
// Same name
// --------------------------------------------------------------------------------------

/// The name as it is compared: lower case, optionally without the extension.
fn name_key(path: &Path, ignore_extension: bool) -> String {
    let name = if ignore_extension {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        file_name(path)
    };
    name.to_lowercase()
}

fn same_name(tree: Tree, cfg: &ScanConfig, progress: &Progress) -> Vec<Group> {
    progress.set_stage("Comparing names", tree.files.len() as u64, 0);
    let mut files = tree.files;
    files.sort_by(by_path);
    let mut by_name: HashMap<String, Vec<FileEntry>> = HashMap::new();
    for file in files {
        let key = name_key(&file.path, cfg.name_ignore_extension);
        progress.add_item();
        if !key.is_empty() {
            by_name.entry(key).or_default().push(file);
        }
    }
    let mut sets: Vec<Vec<FileEntry>> =
        by_name.into_values().filter(|set| set.len() >= 2).collect();
    sets.sort_by(|a, b| a[0].path.cmp(&b[0].path));
    sets.into_iter()
        .map(|set| {
            // The label shows the name the way the first file spells it.
            let label = if cfg.name_ignore_extension {
                set[0]
                    .path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                file_name(&set[0].path)
            };
            Group::new(set.into_iter().map(Item::new).collect(), label)
        })
        .collect()
}

// --------------------------------------------------------------------------------------
// Empty files and folders
// --------------------------------------------------------------------------------------

fn empty_files(tree: Tree, progress: &Progress) -> Vec<Group> {
    progress.set_stage("Looking for empty files", tree.files.len() as u64, 0);
    let mut empty: Vec<FileEntry> = tree.files.into_iter().filter(|f| f.size == 0).collect();
    empty.sort_by(by_path);
    progress.add_items(empty.len() as u64);
    flat(empty.into_iter().map(Item::new))
}

/// Folders that hold nothing but (possibly nested) empty folders. Of every
/// such subtree only the top-most folder is listed, and never a folder the
/// user picked for scanning.
fn empty_folders(tree: &Tree, progress: &Progress) -> Vec<Group> {
    let dirs = &tree.dirs;
    progress.set_stage("Looking for empty folders", dirs.len() as u64, 0);
    // Children come after their parents, so going backwards every folder
    // knows about all of its children by the time it is looked at.
    let mut has_content = vec![false; dirs.len()];
    for i in (0..dirs.len()).rev() {
        let dir = &dirs[i];
        if dir.files > 0 || dir.skipped_dirs > 0 {
            has_content[i] = true;
        }
        if has_content[i] {
            if let Some(parent) = dir.parent {
                if parent < i {
                    has_content[parent] = true;
                }
            }
        }
    }
    let mut found: Vec<FileEntry> = Vec::new();
    for (i, dir) in dirs.iter().enumerate() {
        progress.add_item();
        let Some(parent) = dir.parent else {
            continue; // a scan root
        };
        // A parent listed after its child would break the rule above; then
        // nothing about this folder is certain.
        if has_content[i] || parent >= i {
            continue;
        }
        let parent_is_listed = !has_content[parent] && dirs[parent].parent.is_some();
        if !parent_is_listed {
            found.push(FileEntry {
                path: dir.path.clone(),
                size: 0,
                modified: dir.modified,
                created: dir.created,
                is_dir: true,
            });
        }
    }
    found.sort_by(by_path);
    flat(found.into_iter().map(Item::new))
}

// --------------------------------------------------------------------------------------
// Broken shortcuts
// --------------------------------------------------------------------------------------

/// `C:\` for a target like `C:\Tools\app.exe`; `None` for anything that is
/// not a path on a drive letter (network places, relative paths, ...).
fn drive_root(target: &str) -> Option<String> {
    let b = target.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/'))
        .then(|| format!("{}:\\", (b[0] as char).to_ascii_uppercase()))
}

fn read_start(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let file = hashing::open_shared(path)?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn broken_shortcuts(
    tree: Tree,
    progress: &Progress,
    errors: &mut Vec<String>,
    bytes_read: &mut u64,
) -> Vec<Group> {
    let mut shortcuts: Vec<FileEntry> = tree
        .files
        .into_iter()
        .filter(|f| extension(&f.path) == "lnk")
        .collect();
    shortcuts.sort_by(by_path);
    progress.set_stage("Checking shortcuts", shortcuts.len() as u64, 0);
    // Asking for a drive that is not there can be slow: ask once per letter.
    let mut drive_present: HashMap<String, bool> = HashMap::new();
    let mut broken = Vec::new();
    for (n, file) in shortcuts.into_iter().enumerate() {
        if progress.cancelled() {
            break;
        }
        if n.is_multiple_of(16) {
            progress.set_current(&file.path);
        }
        progress.add_item();
        let bytes = match read_start(&file.path, SHORTCUT_READ_LIMIT) {
            Ok(b) => b,
            Err(e) => {
                if errors.len() < MAX_ERRORS {
                    errors.push(format!(
                        "{}: could not be read and was skipped ({e}).",
                        file.path.to_string_lossy()
                    ));
                }
                continue;
            }
        };
        *bytes_read += bytes.len() as u64;
        let Some(target) = lnk::target(&bytes) else {
            continue; // no local target recorded: nothing can be said
        };
        let Some(root) = drive_root(&target) else {
            continue;
        };
        let present = *drive_present
            .entry(root.clone())
            .or_insert_with(|| Path::new(&root).exists());
        if !present {
            continue; // a drive that is not connected right now
        }
        // Only "not found" counts; a target that merely cannot be looked at
        // (no permission, for example) may well exist.
        let missing = matches!(
            std::fs::metadata(&target),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        );
        if missing {
            broken.push(Item::with_detail(file, Detail::Shortcut { target }));
        }
    }
    flat(broken)
}

// --------------------------------------------------------------------------------------
// Biggest files
// --------------------------------------------------------------------------------------

fn big_files(tree: Tree, count: usize, progress: &Progress) -> Vec<Group> {
    progress.set_stage("Sorting by size", tree.files.len() as u64, 0);
    let mut files = tree.files;
    progress.add_items(files.len() as u64);
    if count == 0 {
        return Vec::new();
    }
    // Only the wanted part is sorted, so a huge listing stays cheap.
    if files.len() > count {
        files.select_nth_unstable_by(count - 1, largest_first);
        files.truncate(count);
    }
    files.sort_by(largest_first);
    flat(files.into_iter().map(Item::new))
}

// --------------------------------------------------------------------------------------
// Temporary files
// --------------------------------------------------------------------------------------

/// Why a file with this name counts as a leftover, or `None` when it does not.
pub fn temp_reason(name: &str) -> Option<&'static str> {
    let name = name.to_lowercase();
    if name.starts_with("~$") && name.len() > 2 {
        return Some("Office lock file");
    }
    if name.starts_with(".~lock.") && name.ends_with('#') {
        return Some("Document lock file");
    }
    if matches!(name.as_str(), "thumbs.db" | "ehthumbs.db" | ".ds_store")
        || (name.starts_with("._") && name.len() > 2)
    {
        return Some("Thumbnail / desktop cache");
    }
    let ext = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext,
        _ => "",
    };
    match ext {
        "tmp" | "temp" => Some("Temporary file"),
        "bak" | "old" => Some("Backup copy"),
        "dmp" => Some("Crash dump"),
        "crdownload" | "part" | "partial" => Some("Unfinished download"),
        "chk" => Some("Recovered file fragment"),
        "swp" => Some("Editor swap file"),
        _ if name.ends_with('~') && name.len() > 1 => Some("Editor backup"),
        _ => None,
    }
}

fn temp_files(tree: Tree, progress: &Progress) -> Vec<Group> {
    progress.set_stage("Checking names", tree.files.len() as u64, 0);
    let mut found: Vec<(FileEntry, &'static str)> = Vec::new();
    for file in tree.files {
        progress.add_item();
        if let Some(reason) = temp_reason(&file_name(&file.path)) {
            found.push((file, reason));
        }
    }
    found.sort_by(|a, b| largest_first(&a.0, &b.0));
    flat(
        found
            .into_iter()
            .map(|(file, reason)| Item::with_detail(file, Detail::Note(reason.to_string()))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hashing::testkit::{filler, flat_names, put, shape, temp_dir};
    use crate::sys;
    use std::fs;
    use std::path::PathBuf;

    fn run(finder: Finder, cfg: &ScanConfig) -> ScanResult {
        scan_with(finder, cfg, &Guard::none(), &Progress::new())
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn same_name_groups() {
        let dir = temp_dir("finders-names");
        put(&dir.join("a\\Song.mp3"), b"one");
        put(&dir.join("b\\song.MP3"), b"two two");
        put(&dir.join("c\\song.flac"), b"three three three");
        put(&dir.join("c\\other.txt"), b"x");
        put(&dir.join("a\\notes.txt"), b"y");
        put(&dir.join("b\\notes.txt"), b"z");
        put(&dir.join("b\\empty.txt"), b"");
        put(&dir.join("c\\empty.txt"), b"");
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let result = run(Finder::SameName, &cfg);
        assert_eq!(result.finder, Finder::SameName);
        assert_eq!(
            shape(&result),
            [
                strings(&["Song.mp3", "song.MP3"]),
                strings(&["notes.txt", "notes.txt"])
            ]
        );
        assert_eq!(
            result.files_scanned, 6,
            "empty files are below the smallest size"
        );
        assert_eq!(result.bytes_scanned, 3 + 7 + 17 + 3);
        let labels: Vec<&str> = result.groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["Song.mp3", "notes.txt"]);
        assert!(result
            .groups
            .iter()
            .all(|g| g.items.windows(2).all(|w| w[0].file.path < w[1].file.path)));

        cfg.name_ignore_extension = true;
        let result = run(Finder::SameName, &cfg);
        assert_eq!(
            shape(&result),
            [
                strings(&["Song.mp3", "song.MP3", "song.flac"]),
                strings(&["notes.txt", "notes.txt"])
            ]
        );
        let labels: Vec<&str> = result.groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["Song", "notes"]);

        // The filters of the configuration apply.
        cfg.only_extensions = vec!["mp3".into()];
        assert_eq!(
            shape(&run(Finder::SameName, &cfg)),
            [strings(&["Song.mp3", "song.MP3"])]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_files_ignore_the_size_filter() {
        let dir = temp_dir("finders-empty-files");
        put(&dir.join("z\\zero.dat"), b"");
        put(&dir.join("a\\nothing.txt"), b"");
        put(&dir.join("a\\full.txt"), b"content");
        put(&dir.join("hidden-zero.txt"), b"");
        assert!(sys::set_attributes(
            &dir.join("hidden-zero.txt"),
            sys::FILE_ATTRIBUTE_HIDDEN
        ));
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.min_size = 1_000;
        let result = run(Finder::EmptyFiles, &cfg);
        assert_eq!(flat_names(&result), ["nothing.txt", "zero.dat"]);
        assert!(result
            .groups
            .iter()
            .all(|g| g.items[0].file.size == 0 && !g.items[0].file.is_dir));
        assert_eq!(result.files_scanned, 3);
        cfg.only_extensions = vec!["dat".into()];
        assert_eq!(flat_names(&run(Finder::EmptyFiles, &cfg)), ["zero.dat"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_folders_top_most_only() {
        let dir = temp_dir("finders-empty-folders");
        fs::create_dir_all(dir.join("hollow\\inner\\deep")).unwrap();
        fs::create_dir_all(dir.join("hollow\\second")).unwrap();
        fs::create_dir_all(dir.join("plain-empty")).unwrap();
        fs::create_dir_all(dir.join("used\\empty-child")).unwrap();
        fs::create_dir_all(dir.join("used\\full-child\\empty-grandchild")).unwrap();
        put(&dir.join("used\\full-child\\file.txt"), b"x");
        put(&dir.join("only-hidden\\secret.txt"), b"x");
        assert!(sys::set_attributes(
            &dir.join("only-hidden\\secret.txt"),
            sys::FILE_ATTRIBUTE_HIDDEN
        ));
        put(&dir.join("only-zero\\zero.dat"), b"");
        // A folder that only holds a hidden folder, and one that only holds a link.
        fs::create_dir_all(dir.join("hidden-inside\\ghost")).unwrap();
        assert!(sys::set_attributes(
            &dir.join("hidden-inside\\ghost"),
            sys::FILE_ATTRIBUTE_HIDDEN | sys::FILE_ATTRIBUTE_DIRECTORY
        ));
        let outside = temp_dir("finders-empty-folders-outside");
        fs::create_dir_all(dir.join("link-inside")).unwrap();
        assert!(sys::make_junction(&dir.join("link-inside\\link"), &outside));

        let mut cfg = ScanConfig::for_paths(&[&dir]);
        // Filters on files must not make a folder look empty.
        cfg.min_size = 1_000_000;
        cfg.only_extensions = vec!["nothing".into()];
        let result = run(Finder::EmptyFolders, &cfg);
        assert_eq!(
            flat_names(&result),
            ["hollow", "plain-empty", "empty-child", "empty-grandchild"]
        );
        for group in &result.groups {
            let file = &group.items[0].file;
            assert!(file.is_dir && file.size == 0 && file.modified > 0);
            assert!(walk::is_empty_tree(&file.path));
        }

        // An empty scan root is never listed itself; its empty folders are.
        let cfg = ScanConfig::for_paths(&[dir.join("hollow")]);
        assert_eq!(
            flat_names(&run(Finder::EmptyFolders, &cfg)),
            ["inner", "second"]
        );
        let cfg = ScanConfig::for_paths(&[dir.join("plain-empty")]);
        assert!(run(Finder::EmptyFolders, &cfg).groups.is_empty());

        // A listing that was cancelled says nothing about emptiness.
        let cfg = ScanConfig::for_paths(&[&dir]);
        let progress = Progress::new();
        progress.cancel();
        let result = scan_with(Finder::EmptyFolders, &cfg, &Guard::none(), &progress);
        assert!(result.cancelled && result.groups.is_empty());

        fs::remove_dir(dir.join("link-inside\\link")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    /// A drive letter that is not in use on this machine, if there is one.
    fn unused_drive() -> Option<char> {
        let taken: Vec<String> = sys::drives()
            .iter()
            .map(|d| d.to_string_lossy().to_uppercase())
            .collect();
        ('D'..='Z')
            .rev()
            .find(|c| !taken.iter().any(|t| t.starts_with(*c)))
    }

    #[test]
    fn broken_shortcuts_only() {
        let dir = temp_dir("finders-shortcuts");
        put(&dir.join("apps\\tool.exe"), b"not really a program");
        fs::create_dir_all(dir.join("apps\\folder-target")).unwrap();
        let link = |name: &str, target: PathBuf| {
            put(&dir.join(name), &lnk::build(&target.to_string_lossy()));
        };
        link("Working.lnk", dir.join("apps\\tool.exe"));
        link("Folder.lnk", dir.join("apps\\folder-target"));
        link("Broken.lnk", dir.join("apps\\gone.exe"));
        link("sub\\Also Broken.LNK", dir.join("missing-folder\\gone.exe"));
        put(
            &dir.join("Network.lnk"),
            &lnk::build("\\\\made-up-server\\share\\gone.exe"),
        );
        put(&dir.join("Garbage.lnk"), &filler(1, 400));
        put(&dir.join("Empty.lnk"), b"");
        put(
            &dir.join("not-a-shortcut.txt"),
            &lnk::build("C:\\made-up\\gone.exe"),
        );
        if let Some(letter) = unused_drive() {
            put(
                &dir.join("Unplugged.lnk"),
                &lnk::build(&format!("{letter}:\\made-up\\gone.exe")),
            );
        }
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        // Neither the size nor the extension filter hides shortcuts.
        cfg.min_size = 1_000_000;
        cfg.only_extensions = vec!["txt".into()];
        let result = run(Finder::BrokenShortcuts, &cfg);
        assert_eq!(flat_names(&result), ["Broken.lnk", "Also Broken.LNK"]);
        let gone = dir.join("apps\\gone.exe").to_string_lossy().into_owned();
        assert_eq!(
            result.groups[0].items[0].detail,
            Detail::Shortcut { target: gone }
        );
        assert!(result.errors.is_empty(), "{:?}", result.errors);
        assert!(result.bytes_read > 0);

        // Once the target exists the shortcut is fine.
        put(&dir.join("apps\\gone.exe"), b"back again");
        assert_eq!(
            flat_names(&run(Finder::BrokenShortcuts, &cfg)),
            ["Also Broken.LNK"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drive_roots() {
        assert_eq!(drive_root("c:\\tools\\app.exe").as_deref(), Some("C:\\"));
        assert_eq!(drive_root("Q:/x").as_deref(), Some("Q:\\"));
        assert_eq!(drive_root("Q:\\").as_deref(), Some("Q:\\"));
        assert_eq!(drive_root("\\\\server\\share\\x"), None);
        assert_eq!(drive_root("relative\\x"), None);
        assert_eq!(drive_root("Q:"), None);
        assert_eq!(drive_root("1:\\x"), None);
        assert_eq!(drive_root(""), None);
    }

    #[test]
    fn biggest_files_in_order() {
        let dir = temp_dir("finders-big");
        for (name, size) in [
            ("a\\five.bin", 5_000usize),
            ("a\\one.bin", 1_000),
            ("b\\nine.bin", 9_000),
            ("b\\three.bin", 3_000),
            ("b\\also-five.bin", 5_000),
            ("seven.txt", 7_000),
            ("tiny.bin", 10),
        ] {
            put(&dir.join(name), &filler(size as u64, size));
        }
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.big_count = 4;
        let result = run(Finder::BigFiles, &cfg);
        assert_eq!(
            flat_names(&result),
            ["nine.bin", "seven.txt", "five.bin", "also-five.bin"]
        );
        assert_eq!(result.files_scanned, 7);
        assert_eq!(result.bytes_scanned, 30_010);
        cfg.big_count = 100;
        assert_eq!(run(Finder::BigFiles, &cfg).groups.len(), 7);
        cfg.big_count = 1;
        assert_eq!(flat_names(&run(Finder::BigFiles, &cfg)), ["nine.bin"]);
        cfg.big_count = 0;
        assert!(run(Finder::BigFiles, &cfg).groups.is_empty());
        // Filters apply.
        cfg.big_count = 2;
        cfg.only_extensions = vec!["bin".into()];
        cfg.max_size = Some(5_000);
        assert_eq!(
            flat_names(&run(Finder::BigFiles, &cfg)),
            ["five.bin", "also-five.bin"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn biggest_of_many() {
        let files: Vec<FileEntry> = (0..50_000u64)
            .map(|n| FileEntry {
                path: PathBuf::from(format!("Q:\\made-up\\{n:05}.bin")),
                size: (n * 7_919) % 10_007,
                ..Default::default()
            })
            .collect();
        let mut expected = files.clone();
        expected.sort_by(largest_first);
        expected.truncate(25);
        let tree = Tree {
            files,
            ..Default::default()
        };
        let groups = big_files(tree, 25, &Progress::new());
        let got: Vec<FileEntry> = groups
            .into_iter()
            .map(|g| g.items[0].file.clone())
            .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn temp_patterns() {
        for (name, reason) in [
            ("draft.tmp", "Temporary file"),
            ("Draft.TEMP", "Temporary file"),
            ("~$letter.docx", "Office lock file"),
            ("report.bak", "Backup copy"),
            ("report.docx.old", "Backup copy"),
            ("notes.txt~", "Editor backup"),
            ("notes~", "Editor backup"),
            (".~lock.budget.ods#", "Document lock file"),
            ("Thumbs.db", "Thumbnail / desktop cache"),
            ("ehthumbs.db", "Thumbnail / desktop cache"),
            (".DS_Store", "Thumbnail / desktop cache"),
            ("._photo.jpg", "Thumbnail / desktop cache"),
            ("crash.dmp", "Crash dump"),
            ("movie.mkv.crdownload", "Unfinished download"),
            ("archive.zip.part", "Unfinished download"),
            ("setup.exe.partial", "Unfinished download"),
            ("FILE0001.CHK", "Recovered file fragment"),
            (".main.rs.swp", "Editor swap file"),
        ] {
            assert_eq!(temp_reason(name), Some(reason), "{name}");
        }
        for name in [
            "desktop.ini",
            "setup.log",
            "keep.txt",
            "tmp.txt",
            "template.docx",
            "bak",
            ".tmp",
            ".bak",
            "old",
            "thumbs.db.txt",
            "photo.jpg",
            "~",
            "~$",
            "._",
            "partial.zip",
            "backup.old.zip",
            "",
        ] {
            assert_eq!(temp_reason(name), None, "{name}");
        }
    }

    #[test]
    fn temp_files_largest_first() {
        let dir = temp_dir("finders-temp");
        put(&dir.join("a\\draft.tmp"), &filler(1, 300));
        put(&dir.join("a\\~$letter.docx"), &filler(2, 162));
        put(&dir.join("b\\huge.dmp"), &filler(3, 9_000));
        put(&dir.join("b\\zero.bak"), b"");
        put(&dir.join("b\\keep.txt"), &filler(4, 20_000));
        put(&dir.join("desktop.ini"), b"[.ShellClassInfo]");
        put(&dir.join("setup.log"), b"all fine");
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let result = run(Finder::TempFiles, &cfg);
        assert_eq!(
            flat_names(&result),
            ["huge.dmp", "draft.tmp", "~$letter.docx", "zero.bak"]
        );
        let notes: Vec<&Detail> = result.groups.iter().map(|g| &g.items[0].detail).collect();
        assert_eq!(
            notes,
            [
                &Detail::Note("Crash dump".into()),
                &Detail::Note("Temporary file".into()),
                &Detail::Note("Office lock file".into()),
                &Detail::Note("Backup copy".into()),
            ]
        );
        assert_eq!(result.files_scanned, 7);
        // The extension filter applies, the size filter does not.
        cfg.min_size = 1_000;
        cfg.skip_extensions = vec!["dmp".into()];
        assert_eq!(
            flat_names(&run(Finder::TempFiles, &cfg)),
            ["draft.tmp", "~$letter.docx", "zero.bak"]
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cancelled_and_foreign_finders_return_nothing() {
        let dir = temp_dir("finders-cancel");
        put(&dir.join("a\\draft.tmp"), b"x");
        put(&dir.join("b\\draft.tmp"), b"y");
        put(&dir.join("zero.dat"), b"");
        let cfg = ScanConfig::for_paths(&[&dir]);
        for finder in Finder::ALL {
            let progress = Progress::new();
            progress.cancel();
            let result = scan_with(finder, &cfg, &Guard::none(), &progress);
            assert_eq!(result.finder, finder);
            assert!(result.groups.is_empty(), "{finder:?}");
        }
        for finder in [
            Finder::Duplicates,
            Finder::SimilarImages,
            Finder::SimilarMusic,
        ] {
            let result = run(finder, &cfg);
            assert!(result.groups.is_empty() && !result.cancelled);
        }
        assert_eq!(run(Finder::SameName, &cfg).groups.len(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
