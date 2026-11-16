//! Walks the chosen folders and lists what is there. Read-only.
//!
//! Rules that hold for every finder:
//! * junctions and symbolic links are never followed (and linked files are skipped);
//! * excluded folders, name patterns and, by default, hidden and system items
//!   and protected system locations are skipped;
//! * cloud placeholders that are not on this disk are skipped, so that a scan
//!   never triggers downloads.

use crate::model::{FileEntry, Progress, ScanConfig};
use crate::sys::{self, Guard};
use crate::util::{file_name, norm_path, wildcard, within};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Which of the per-file filters of [`ScanConfig`] the walk applies.
#[derive(Clone, Copy, Debug)]
pub struct WalkOptions {
    pub size_filter: bool,
    pub extension_filter: bool,
    pub date_filter: bool,
}

impl WalkOptions {
    /// All filters on: what the duplicate finders want.
    pub const ALL: WalkOptions = WalkOptions {
        size_filter: true,
        extension_filter: true,
        date_filter: true,
    };
    /// No size filter: for finders that look for empty or tiny files.
    pub const ANY_SIZE: WalkOptions = WalkOptions {
        size_filter: false,
        extension_filter: true,
        date_filter: true,
    };
    /// No per-file filters at all.
    pub const NONE: WalkOptions = WalkOptions {
        size_filter: false,
        extension_filter: false,
        date_filter: false,
    };
}

/// One folder that was walked. The counts are raw: they include hidden and
/// filtered files, so that "empty" really means empty.
#[derive(Clone, Debug)]
pub struct DirInfo {
    pub path: PathBuf,
    /// Index of the parent in [`Tree::dirs`]; `None` for a scan root.
    pub parent: Option<usize>,
    /// Everything directly inside that is not a folder (files, links, ...).
    pub files: u32,
    /// Sub-folders that were not walked (links, excluded, hidden, protected,
    /// unreadable). A folder with skipped children is never reported as empty.
    pub skipped_dirs: u32,
    pub modified: i64,
    pub created: i64,
}

/// The result of a walk.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    /// Files that passed the filters.
    pub files: Vec<FileEntry>,
    /// Every folder that was walked, parents before children.
    pub dirs: Vec<DirInfo>,
    /// Total size of `files`.
    pub bytes: u64,
    /// Folders and files that could not be read.
    pub errors: Vec<String>,
}

const MAX_ERRORS: usize = 200;

fn push_error(errors: &mut Vec<String>, text: String) {
    if errors.len() < MAX_ERRORS {
        errors.push(text);
    }
}

/// The scan roots: include and reference folders, without duplicates and
/// without folders that lie inside another root (which would list files twice).
pub fn roots(cfg: &ScanConfig) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = Vec::new();
    for p in cfg.include.iter().chain(cfg.reference.iter()) {
        if p.as_os_str().is_empty() {
            continue;
        }
        let n = norm_path(&p.to_string_lossy());
        if !all.iter().any(|q| norm_path(&q.to_string_lossy()) == n) {
            all.push(p.clone());
        }
    }
    let keep: Vec<PathBuf> = all
        .iter()
        .filter(|p| {
            !all.iter().any(|q| {
                norm_path(&q.to_string_lossy()) != norm_path(&p.to_string_lossy()) && within(p, q)
            })
        })
        .cloned()
        .collect();
    keep
}

fn name_excluded(cfg: &ScanConfig, name: &str) -> bool {
    cfg.exclude_patterns
        .iter()
        .any(|p| !p.trim().is_empty() && wildcard(p.trim(), name))
}

/// Walks with the protected locations of this PC (skipped when
/// `cfg.skip_system` is on).
pub fn walk(cfg: &ScanConfig, opts: WalkOptions, progress: &Progress) -> Tree {
    walk_with(cfg, opts, &Guard::real(), progress)
}

/// Walks with an explicit guard (tests use their own).
pub fn walk_with(cfg: &ScanConfig, opts: WalkOptions, guard: &Guard, progress: &Progress) -> Tree {
    let mut tree = Tree::default();
    progress.set_stage("Listing files", 0, 0);

    // (folder, index in tree.dirs)
    let mut stack: Vec<(PathBuf, usize)> = Vec::new();
    for root in roots(cfg) {
        let meta = match std::fs::symlink_metadata(&root) {
            Ok(m) => m,
            Err(e) => {
                push_error(&mut tree.errors, format!("{}: {e}", root.to_string_lossy()));
                continue;
            }
        };
        // A drive root reports odd attributes; only a real link is refused.
        if root.parent().is_some()
            && meta.file_attributes() & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            push_error(
                &mut tree.errors,
                format!(
                    "{}: this is a link to another place and is not followed.",
                    root.to_string_lossy()
                ),
            );
            continue;
        }
        if cfg.skip_system && guard.is_protected(&root) {
            push_error(
                &mut tree.errors,
                format!(
                    "{}: protected system location, skipped.",
                    root.to_string_lossy()
                ),
            );
            continue;
        }
        if meta.is_dir() {
            tree.dirs.push(DirInfo {
                path: root.clone(),
                parent: None,
                files: 0,
                skipped_dirs: 0,
                modified: sys::unix_secs(meta.modified()),
                created: sys::unix_secs(meta.created()),
            });
            stack.push((root, tree.dirs.len() - 1));
        } else {
            let entry = FileEntry {
                path: root,
                size: meta.len(),
                modified: sys::unix_secs(meta.modified()),
                created: sys::unix_secs(meta.created()),
                is_dir: false,
            };
            if passes(cfg, opts, &entry) {
                tree.bytes += entry.size;
                tree.files.push(entry);
            }
        }
    }

    while let Some((dir, index)) = stack.pop() {
        if progress.cancelled() {
            break;
        }
        progress.set_current(&dir);
        let reader = match std::fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                push_error(&mut tree.errors, format!("{}: {e}", dir.to_string_lossy()));
                // Unknown contents: make sure nobody calls it (or its parent) empty.
                tree.dirs[index].skipped_dirs += 1;
                continue;
            }
        };
        for entry in reader {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    push_error(&mut tree.errors, format!("{}: {e}", dir.to_string_lossy()));
                    tree.dirs[index].skipped_dirs += 1;
                    continue;
                }
            };
            // On Windows this comes from the directory listing itself: no extra disk access.
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    tree.dirs[index].files += 1;
                    continue;
                }
            };
            let attrs = meta.file_attributes();
            let path = entry.path();
            let name = file_name(&path);
            let is_link = attrs & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0
                && meta.file_type().is_symlink()
                || (attrs & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0
                    && attrs & sys::FILE_ATTRIBUTE_DIRECTORY != 0);
            let hidden = cfg.skip_hidden && attrs & sys::FILE_ATTRIBUTE_HIDDEN != 0;
            let system = cfg.skip_system && attrs & sys::FILE_ATTRIBUTE_SYSTEM != 0;

            if attrs & sys::FILE_ATTRIBUTE_DIRECTORY != 0 {
                let skip = is_link
                    || hidden
                    || system
                    || !cfg.recursive
                    || cfg.is_excluded(&path)
                    || name_excluded(cfg, &name)
                    || (cfg.skip_system && guard.is_protected(&path));
                if skip {
                    tree.dirs[index].skipped_dirs += 1;
                    continue;
                }
                tree.dirs.push(DirInfo {
                    path: path.clone(),
                    parent: Some(index),
                    files: 0,
                    skipped_dirs: 0,
                    modified: sys::unix_secs(meta.modified()),
                    created: sys::unix_secs(meta.created()),
                });
                stack.push((path, tree.dirs.len() - 1));
                continue;
            }

            tree.dirs[index].files += 1;
            let offline = attrs
                & (sys::FILE_ATTRIBUTE_OFFLINE | sys::FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
                != 0;
            if is_link
                || hidden
                || system
                || offline
                || name_excluded(cfg, &name)
                || (cfg.skip_system && guard.is_protected(&path))
            {
                continue;
            }
            let file = FileEntry {
                path,
                size: meta.len(),
                modified: sys::unix_secs(meta.modified()),
                created: sys::unix_secs(meta.created()),
                is_dir: false,
            };
            if passes(cfg, opts, &file) {
                tree.bytes += file.size;
                tree.files.push(file);
                progress.add_item();
            }
        }
    }
    tree
}

fn passes(cfg: &ScanConfig, opts: WalkOptions, file: &FileEntry) -> bool {
    (!opts.size_filter || cfg.size_passes(file.size))
        && (!opts.extension_filter || cfg.extension_passes(&file.path))
        && (!opts.date_filter || cfg.date_passes(file))
}

/// Does the folder (walked earlier) still contain nothing but empty folders?
/// Reads the disk again; links and unreadable entries count as "not empty".
pub fn is_empty_tree(dir: &Path) -> bool {
    if sys::is_reparse_point(dir) {
        return false;
    }
    let Ok(reader) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in reader {
        let Ok(entry) = entry else { return false };
        let Ok(meta) = entry.metadata() else {
            return false;
        };
        let attrs = meta.file_attributes();
        if attrs & sys::FILE_ATTRIBUTE_DIRECTORY == 0
            || attrs & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0
            || !is_empty_tree(&entry.path())
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("twinforge-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(tree: &Tree) -> Vec<String> {
        let mut v: Vec<String> = tree.files.iter().map(|f| file_name(&f.path)).collect();
        v.sort();
        v
    }

    #[test]
    fn lists_files_and_applies_filters() {
        let dir = temp("filters");
        fs::create_dir_all(dir.join("sub\\deep")).unwrap();
        fs::create_dir_all(dir.join("node_modules")).unwrap();
        fs::create_dir_all(dir.join("skipme")).unwrap();
        fs::write(dir.join("a.txt"), b"12345").unwrap();
        fs::write(dir.join("empty.txt"), b"").unwrap();
        fs::write(dir.join("sub\\b.JPG"), b"1234567890").unwrap();
        fs::write(dir.join("sub\\deep\\c.jpg"), b"123").unwrap();
        fs::write(dir.join("node_modules\\d.txt"), b"12345").unwrap();
        fs::write(dir.join("skipme\\e.txt"), b"12345").unwrap();
        fs::write(dir.join("hidden.txt"), b"12345").unwrap();
        assert!(sys::set_attributes(
            &dir.join("hidden.txt"),
            sys::FILE_ATTRIBUTE_HIDDEN
        ));

        let mut cfg = ScanConfig::for_paths(&[&dir]);
        cfg.exclude = vec![dir.join("skipme")];
        cfg.exclude_patterns = vec!["node_modules".into()];
        let p = Progress::new();
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &p);
        assert_eq!(names(&tree), ["a.txt", "b.JPG", "c.jpg"]);
        assert_eq!(tree.bytes, 18);
        assert!(tree.errors.is_empty());
        assert_eq!(p.snapshot().items_done, 3);

        // Without the size filter the empty file shows up; hidden ones on request.
        cfg.skip_hidden = false;
        let tree = walk_with(&cfg, WalkOptions::ANY_SIZE, &Guard::none(), &p);
        assert_eq!(
            names(&tree),
            ["a.txt", "b.JPG", "c.jpg", "empty.txt", "hidden.txt"]
        );

        cfg.only_extensions = vec!["jpg".into()];
        cfg.min_size = 5;
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &p);
        assert_eq!(names(&tree), ["b.JPG"]);

        cfg.recursive = false;
        cfg.only_extensions.clear();
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &p);
        assert_eq!(names(&tree), ["a.txt", "hidden.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nested_and_repeated_roots_are_walked_once() {
        let dir = temp("roots");
        fs::create_dir_all(dir.join("in")).unwrap();
        fs::write(dir.join("in\\a.txt"), b"1").unwrap();
        let mut cfg = ScanConfig::for_paths(&[dir.clone(), dir.join("in")]);
        cfg.include.push(PathBuf::from(
            dir.to_string_lossy().to_uppercase().replace('\\', "/"),
        ));
        cfg.reference = vec![dir.join("in")];
        assert_eq!(roots(&cfg).len(), 1);
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        assert_eq!(tree.files.len(), 1);
        // A reference folder outside the include list is scanned as well.
        let other = temp("roots-ref");
        fs::write(other.join("r.txt"), b"1").unwrap();
        cfg.reference = vec![other.clone()];
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        assert_eq!(names(&tree), ["a.txt", "r.txt"]);
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&other).unwrap();
    }

    #[test]
    fn junctions_are_not_followed() {
        let dir = temp("junction");
        let outside = temp("junction-outside");
        fs::write(outside.join("secret.txt"), b"12345").unwrap();
        fs::create_dir_all(dir.join("real")).unwrap();
        fs::write(dir.join("real\\a.txt"), b"12345").unwrap();
        assert!(sys::make_junction(&dir.join("link"), &outside));
        assert!(dir.join("link\\secret.txt").exists(), "the junction works");

        let cfg = ScanConfig::for_paths(&[&dir]);
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        assert_eq!(names(&tree), ["a.txt"]);
        assert_eq!(tree.dirs[0].skipped_dirs, 1);
        assert!(!is_empty_tree(&dir.join("link")));

        // A junction given as the scan root itself is refused, with a note.
        let cfg = ScanConfig::for_paths(&[dir.join("link")]);
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        assert!(tree.files.is_empty());
        assert_eq!(tree.errors.len(), 1);

        // Removing the junction leaves the target alone.
        fs::remove_dir(dir.join("link")).unwrap();
        assert!(outside.join("secret.txt").exists());
        fs::remove_dir_all(&dir).unwrap();
        fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn protected_locations_are_skipped() {
        let dir = temp("protected");
        fs::create_dir_all(dir.join("system")).unwrap();
        fs::create_dir_all(dir.join("mine")).unwrap();
        fs::write(dir.join("system\\a.dll"), b"12345").unwrap();
        fs::write(dir.join("mine\\b.txt"), b"12345").unwrap();
        let guard = Guard::with_roots(&[dir.join("system")]);
        let mut cfg = ScanConfig::for_paths(&[&dir]);
        let tree = walk_with(&cfg, WalkOptions::ALL, &guard, &Progress::new());
        assert_eq!(names(&tree), ["b.txt"]);
        cfg.skip_system = false;
        let tree = walk_with(&cfg, WalkOptions::ALL, &guard, &Progress::new());
        assert_eq!(names(&tree), ["a.dll", "b.txt"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn folder_counts_are_raw_and_cancel_stops() {
        let dir = temp("counts");
        fs::create_dir_all(dir.join("empty\\inner")).unwrap();
        fs::create_dir_all(dir.join("onlyhidden")).unwrap();
        fs::write(dir.join("onlyhidden\\h.txt"), b"1").unwrap();
        sys::set_attributes(&dir.join("onlyhidden\\h.txt"), sys::FILE_ATTRIBUTE_HIDDEN);
        let cfg = ScanConfig::for_paths(&[&dir]);
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &Progress::new());
        let find = |n: &str| tree.dirs.iter().find(|d| file_name(&d.path) == n).unwrap();
        assert_eq!(find("onlyhidden").files, 1);
        assert_eq!(find("empty").files, 0);
        assert!(tree.files.is_empty());
        assert!(is_empty_tree(&dir.join("empty")));
        assert!(!is_empty_tree(&dir.join("onlyhidden")));
        assert!(!is_empty_tree(&dir.join("missing")));

        let p = Progress::new();
        p.cancel();
        let tree = walk_with(&cfg, WalkOptions::ALL, &Guard::none(), &p);
        assert_eq!(tree.dirs.len(), 1, "only the root was registered");
        fs::remove_dir_all(&dir).unwrap();
    }
}
