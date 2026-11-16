//! The shared data model: what a scan is asked to do, what it reports while it
//! runs and what it returns. Every engine and every view speaks these types.

use crate::util::{extension, within};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// The tools TwinForge offers. Each one has its own tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Finder {
    Duplicates,
    SimilarImages,
    SimilarMusic,
    SameName,
    EmptyFiles,
    EmptyFolders,
    BrokenShortcuts,
    BigFiles,
    TempFiles,
}

impl Finder {
    pub const ALL: [Finder; 9] = [
        Finder::Duplicates,
        Finder::SimilarImages,
        Finder::SimilarMusic,
        Finder::SameName,
        Finder::EmptyFiles,
        Finder::EmptyFolders,
        Finder::BrokenShortcuts,
        Finder::BigFiles,
        Finder::TempFiles,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Finder::Duplicates => "Duplicate files",
            Finder::SimilarImages => "Similar pictures",
            Finder::SimilarMusic => "Similar music",
            Finder::SameName => "Same name",
            Finder::EmptyFiles => "Empty files",
            Finder::EmptyFolders => "Empty folders",
            Finder::BrokenShortcuts => "Broken shortcuts",
            Finder::BigFiles => "Biggest files",
            Finder::TempFiles => "Temporary files",
        }
    }

    /// One sentence that explains the tool on its start page.
    pub fn blurb(self) -> &'static str {
        match self {
            Finder::Duplicates => {
                "Files whose contents are identical byte for byte, whatever they are called."
            }
            Finder::SimilarImages => {
                "Pictures that look alike: resized, recompressed, rotated or lightly edited copies."
            }
            Finder::SimilarMusic => {
                "Tracks with matching artist, title, album and length, even when the files differ."
            }
            Finder::SameName => {
                "Files that share a name in different folders. Their contents may differ."
            }
            Finder::EmptyFiles => "Files with a size of zero bytes.",
            Finder::EmptyFolders => "Folders that contain no files at all, not even hidden ones.",
            Finder::BrokenShortcuts => "Shortcuts whose target no longer exists.",
            Finder::BigFiles => "The largest files in the chosen locations.",
            Finder::TempFiles => {
                "Leftover temporary, backup and cache files that programs forgot to clean up."
            }
        }
    }

    /// The short word used on the command line and in view names.
    pub fn key(self) -> &'static str {
        match self {
            Finder::Duplicates => "duplicates",
            Finder::SimilarImages => "pictures",
            Finder::SimilarMusic => "music",
            Finder::SameName => "names",
            Finder::EmptyFiles => "empty-files",
            Finder::EmptyFolders => "empty-folders",
            Finder::BrokenShortcuts => "shortcuts",
            Finder::BigFiles => "big",
            Finder::TempFiles => "temp",
        }
    }

    pub fn from_key(key: &str) -> Option<Finder> {
        Finder::ALL.into_iter().find(|f| f.key() == key)
    }

    /// Grouped finders return sets of files that stand in for each other; at
    /// least one file of every group must always be kept. The other finders
    /// return a flat list (one item per group).
    pub fn is_grouped(self) -> bool {
        matches!(
            self,
            Finder::Duplicates | Finder::SimilarImages | Finder::SimilarMusic | Finder::SameName
        )
    }
}

/// One file (or, for the empty-folder finder, one folder) found by the walker.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FileEntry {
    pub path: PathBuf,
    pub size: u64,
    /// Last write time, seconds since 1970-01-01 UTC (0 = unknown).
    pub modified: i64,
    /// Creation time, seconds since 1970-01-01 UTC (0 = unknown).
    pub created: i64,
    pub is_dir: bool,
}

/// Extra facts a finder learned about an item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub enum Detail {
    #[default]
    None,
    /// A picture. `similarity` is 0..=100 compared with the best (largest)
    /// picture of its group, which itself has 100.
    Image {
        width: u32,
        height: u32,
        similarity: u8,
    },
    /// A music track. Empty text and zeros mean "not recorded in the file".
    Audio {
        artist: String,
        title: String,
        album: String,
        duration_secs: u32,
        bitrate_kbps: u32,
    },
    /// A shortcut and the place it points to.
    Shortcut { target: String },
    /// Why a file counts as temporary, e.g. "Editor backup (~ suffix)".
    Note(String),
}

/// One row in the results.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Item {
    pub file: FileEntry,
    /// Lies in a reference folder: shown, compared, but never selected or changed.
    pub reference: bool,
    /// Ticked for the next action (delete, move, link).
    pub selected: bool,
    pub detail: Detail,
}

impl Item {
    pub fn new(file: FileEntry) -> Item {
        Item {
            file,
            ..Default::default()
        }
    }

    pub fn with_detail(file: FileEntry, detail: Detail) -> Item {
        Item {
            file,
            detail,
            ..Default::default()
        }
    }

    /// Pixel count for pictures, 0 otherwise.
    pub fn pixels(&self) -> u64 {
        match self.detail {
            Detail::Image { width, height, .. } => width as u64 * height as u64,
            _ => 0,
        }
    }
}

/// A set of files that are copies of (or stand in for) each other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Group {
    pub items: Vec<Item>,
    /// Short fact about the group, e.g. `BLAKE3 3fa9c1…`, `94% similar`.
    pub label: String,
}

impl Group {
    pub fn new(items: Vec<Item>, label: impl Into<String>) -> Group {
        Group {
            items,
            label: label.into(),
        }
    }

    pub fn total_bytes(&self) -> u64 {
        self.items.iter().map(|i| i.file.size).sum()
    }

    /// Space that would be freed by keeping only the largest file of the group.
    pub fn wasted_bytes(&self) -> u64 {
        let max = self.items.iter().map(|i| i.file.size).max().unwrap_or(0);
        self.total_bytes() - max
    }

    pub fn selected_count(&self) -> usize {
        self.items.iter().filter(|i| i.selected).count()
    }

    pub fn selected_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter(|i| i.selected)
            .map(|i| i.file.size)
            .sum()
    }
}

/// What a finished scan hands back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScanResult {
    pub finder: Finder,
    /// For flat finders every group holds exactly one item.
    pub groups: Vec<Group>,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    /// Bytes actually read from disk while hashing or decoding.
    pub bytes_read: u64,
    /// Files whose hash came from the cache instead of the disk.
    pub cache_hits: u64,
    pub elapsed_ms: u64,
    pub cancelled: bool,
    /// Things that could not be read, as plain sentences.
    pub errors: Vec<String>,
}

impl ScanResult {
    pub fn empty(finder: Finder) -> ScanResult {
        ScanResult {
            finder,
            groups: Vec::new(),
            files_scanned: 0,
            bytes_scanned: 0,
            bytes_read: 0,
            cache_hits: 0,
            elapsed_ms: 0,
            cancelled: false,
            errors: Vec::new(),
        }
    }

    pub fn item_count(&self) -> usize {
        self.groups.iter().map(|g| g.items.len()).sum()
    }

    /// Space that could be freed: per group everything but the largest file
    /// for grouped finders, everything for flat ones.
    pub fn wasted_bytes(&self) -> u64 {
        if self.finder.is_grouped() {
            self.groups.iter().map(Group::wasted_bytes).sum()
        } else {
            self.groups.iter().map(Group::total_bytes).sum()
        }
    }

    pub fn selected_count(&self) -> usize {
        self.groups.iter().map(Group::selected_count).sum()
    }

    pub fn selected_bytes(&self) -> u64 {
        self.groups.iter().map(Group::selected_bytes).sum()
    }
}

/// What can be done with the selected files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionKind {
    /// Move to the Recycle Bin (the default; can be restored from there).
    Recycle,
    /// Delete for good.
    Delete,
    /// Move into a folder the user picks (an undo log is written).
    Move,
    /// Replace each selected file with a hard link to the kept copy.
    HardLink,
    /// Replace each selected file with a symbolic link to the kept copy.
    SymLink,
}

impl ActionKind {
    pub const ALL: [ActionKind; 5] = [
        ActionKind::Recycle,
        ActionKind::Delete,
        ActionKind::Move,
        ActionKind::HardLink,
        ActionKind::SymLink,
    ];

    pub fn title(self) -> &'static str {
        match self {
            ActionKind::Recycle => "Move to Recycle Bin",
            ActionKind::Delete => "Delete permanently",
            ActionKind::Move => "Move to a folder",
            ActionKind::HardLink => "Replace with hard links",
            ActionKind::SymLink => "Replace with symbolic links",
        }
    }

    /// Link actions need a kept copy to point at, so they only make sense for
    /// exact duplicates.
    pub fn available_for(self, finder: Finder) -> bool {
        match self {
            ActionKind::HardLink | ActionKind::SymLink => finder == Finder::Duplicates,
            _ => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportFormat {
    Csv,
    Html,
    Json,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Html => "html",
            ExportFormat::Json => "json",
        }
    }

    /// Picks the format from a file name; `None` when the extension is unknown.
    pub fn from_path(path: &Path) -> Option<ExportFormat> {
        match extension(path).as_str() {
            "csv" => Some(ExportFormat::Csv),
            "html" | "htm" => Some(ExportFormat::Html),
            "json" => Some(ExportFormat::Json),
            _ => None,
        }
    }
}

/// A decoded, scaled-down picture ready to become a texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, RGBA, not premultiplied.
    pub rgba: Vec<u8>,
    /// Size of the original picture.
    pub full_width: u32,
    pub full_height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlgo {
    /// Cryptographic, 256 bit. The default.
    Blake3,
    /// Non-cryptographic, 128 bit, a little faster on fast disks.
    Xxh3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageHash {
    /// DCT based ("pHash"): robust against recompression and resizing.
    Perceptual,
    /// Gradient based ("dHash"): faster, a little stricter.
    Gradient,
}

/// Which tags must agree for two tracks to count as the same music.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MusicMatch {
    pub title: bool,
    pub artist: bool,
    pub album: bool,
    /// Lengths must agree within a few seconds.
    pub duration: bool,
    /// 0..=100: how alike the compared texts must be (100 = equal after tidying).
    pub fuzziness: u8,
}

impl Default for MusicMatch {
    fn default() -> Self {
        MusicMatch {
            title: true,
            artist: true,
            album: false,
            duration: true,
            fuzziness: 85,
        }
    }
}

/// Everything a scan needs to know.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScanConfig {
    /// Folders or drives to scan.
    pub include: Vec<PathBuf>,
    /// Folders that are skipped entirely.
    pub exclude: Vec<PathBuf>,
    /// Reference folders: scanned and compared, never selected or changed.
    /// A reference folder is scanned even when it is not listed in `include`.
    pub reference: Vec<PathBuf>,
    /// Wildcards matched against file and folder names, e.g. `node_modules`, `*.bak`.
    pub exclude_patterns: Vec<String>,
    pub recursive: bool,
    pub skip_hidden: bool,
    /// Skip files and folders with the "system" attribute and protected system
    /// locations (Windows, Program Files, the Recycle Bin, ...).
    pub skip_system: bool,
    /// Smallest file size in bytes that is looked at.
    pub min_size: u64,
    /// Largest file size in bytes that is looked at (`None` = no limit).
    pub max_size: Option<u64>,
    /// Lower-case extensions without dot. Empty = all.
    pub only_extensions: Vec<String>,
    /// Lower-case extensions without dot that are skipped.
    pub skip_extensions: Vec<String>,
    /// Only files modified at or after this time (seconds since 1970 UTC).
    pub modified_after: Option<i64>,
    /// Only files modified before this time (seconds since 1970 UTC).
    pub modified_before: Option<i64>,

    // Exact duplicates
    pub hash: HashAlgo,
    /// After the full hash, compare the files byte by byte as well.
    pub byte_verify: bool,
    /// Where the hash cache lives; `None` switches the cache off.
    pub cache_path: Option<PathBuf>,
    /// Worker threads; 0 = one per CPU core (at most 8).
    pub threads: usize,

    // Similar pictures
    /// 50..=100: how alike two pictures must be. 100 = practically identical.
    pub similarity: u8,
    pub image_hash: ImageHash,
    /// Also match copies that were rotated by 90/180/270 degrees or mirrored.
    pub match_rotated: bool,

    // Similar music
    pub music: MusicMatch,

    // Same name
    /// Compare names without their extension (`song.mp3` = `song.flac`).
    pub name_ignore_extension: bool,

    // Biggest files
    pub big_count: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        ScanConfig {
            include: Vec::new(),
            exclude: Vec::new(),
            reference: Vec::new(),
            exclude_patterns: Vec::new(),
            recursive: true,
            skip_hidden: true,
            skip_system: true,
            min_size: 1,
            max_size: None,
            only_extensions: Vec::new(),
            skip_extensions: Vec::new(),
            modified_after: None,
            modified_before: None,
            hash: HashAlgo::Blake3,
            byte_verify: false,
            cache_path: None,
            threads: 0,
            similarity: 90,
            image_hash: ImageHash::Perceptual,
            match_rotated: true,
            music: MusicMatch::default(),
            name_ignore_extension: false,
            big_count: 100,
        }
    }
}

impl ScanConfig {
    /// A config that scans these folders with default options.
    pub fn for_paths<P: AsRef<Path>>(paths: &[P]) -> ScanConfig {
        ScanConfig {
            include: paths.iter().map(|p| p.as_ref().to_path_buf()).collect(),
            ..Default::default()
        }
    }

    /// Does this path lie in a reference folder?
    pub fn is_reference(&self, path: &Path) -> bool {
        self.reference.iter().any(|r| within(path, r))
    }

    /// Does this path lie in an excluded folder?
    pub fn is_excluded(&self, path: &Path) -> bool {
        self.exclude.iter().any(|r| within(path, r))
    }

    /// The size, extension and date filters for one file.
    pub fn file_passes(&self, file: &FileEntry) -> bool {
        self.size_passes(file.size) && self.extension_passes(&file.path) && self.date_passes(file)
    }

    pub fn size_passes(&self, size: u64) -> bool {
        size >= self.min_size && self.max_size.map(|m| size <= m).unwrap_or(true)
    }

    pub fn extension_passes(&self, path: &Path) -> bool {
        if self.only_extensions.is_empty() && self.skip_extensions.is_empty() {
            return true;
        }
        let ext = extension(path);
        (self.only_extensions.is_empty() || self.only_extensions.contains(&ext))
            && !self.skip_extensions.contains(&ext)
    }

    pub fn date_passes(&self, file: &FileEntry) -> bool {
        self.modified_after
            .map(|t| file.modified >= t)
            .unwrap_or(true)
            && self
                .modified_before
                .map(|t| file.modified < t)
                .unwrap_or(true)
    }

    /// The number of worker threads to use.
    pub fn worker_count(&self) -> usize {
        if self.threads > 0 {
            self.threads.min(32)
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
                .clamp(1, 8)
        }
    }
}

/// Live progress of a scan or an action, shared between the worker and the
/// window. Workers call the `set_*`/`add_*` methods; the window polls
/// [`Progress::snapshot`]. Setting `cancel` asks the worker to stop soon.
pub struct Progress {
    cancel: AtomicBool,
    stage: Mutex<(String, Instant)>,
    stage_no: AtomicU64,
    stage_count: AtomicU64,
    items_done: AtomicU64,
    items_total: AtomicU64,
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    current: Mutex<String>,
    started: Instant,
    /// Seconds added to the clocks (the demo shows a scan that is well under way).
    head_start: AtomicU64,
}

/// A copy of the progress counters at one moment.
#[derive(Clone, Debug, Default)]
pub struct ProgressSnapshot {
    pub stage: String,
    /// 1-based number of the current stage and how many there are (0 = unknown).
    pub stage_no: u64,
    pub stage_count: u64,
    pub items_done: u64,
    pub items_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: String,
    pub elapsed_secs: f64,
    pub stage_secs: f64,
}

impl ProgressSnapshot {
    /// 0.0..=1.0 for the current stage, or `None` while the total is unknown.
    pub fn fraction(&self) -> Option<f32> {
        if self.bytes_total > 0 {
            Some((self.bytes_done as f64 / self.bytes_total as f64).min(1.0) as f32)
        } else if self.items_total > 0 {
            Some((self.items_done as f64 / self.items_total as f64).min(1.0) as f32)
        } else {
            None
        }
    }

    /// Estimated seconds left in the current stage.
    pub fn eta_secs(&self) -> Option<u64> {
        let f = self.fraction()? as f64;
        if f < 0.02 || self.stage_secs < 1.0 {
            return None;
        }
        Some(((self.stage_secs / f) * (1.0 - f)).round() as u64)
    }

    /// Bytes per second in the current stage.
    pub fn speed(&self) -> Option<f64> {
        (self.stage_secs > 0.5 && self.bytes_done > 0)
            .then(|| self.bytes_done as f64 / self.stage_secs)
    }
}

impl Default for Progress {
    fn default() -> Self {
        Self::new()
    }
}

impl Progress {
    pub fn new() -> Progress {
        let now = Instant::now();
        Progress {
            cancel: AtomicBool::new(false),
            stage: Mutex::new((String::new(), now)),
            stage_no: AtomicU64::new(0),
            stage_count: AtomicU64::new(0),
            items_done: AtomicU64::new(0),
            items_total: AtomicU64::new(0),
            bytes_done: AtomicU64::new(0),
            bytes_total: AtomicU64::new(0),
            current: Mutex::new(String::new()),
            started: now,
            head_start: AtomicU64::new(0),
        }
    }

    /// Pretends the job and its current stage began this many seconds earlier.
    pub fn set_head_start(&self, secs: u64) {
        self.head_start.store(secs, Ordering::Relaxed);
    }

    /// Ask the worker to stop.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Workers check this often and return early when it is true.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Tell the window how many stages the job has (call once, optional).
    pub fn set_stage_count(&self, count: u64) {
        self.stage_count.store(count, Ordering::Relaxed);
    }

    /// Starts a new stage and resets the counters. Totals of 0 mean "unknown".
    pub fn set_stage(&self, name: &str, items_total: u64, bytes_total: u64) {
        if let Ok(mut s) = self.stage.lock() {
            *s = (name.to_string(), Instant::now());
        }
        self.stage_no.fetch_add(1, Ordering::Relaxed);
        self.items_done.store(0, Ordering::Relaxed);
        self.bytes_done.store(0, Ordering::Relaxed);
        self.items_total.store(items_total, Ordering::Relaxed);
        self.bytes_total.store(bytes_total, Ordering::Relaxed);
    }

    /// One more item finished.
    pub fn add_item(&self) {
        self.items_done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_items(&self, n: u64) {
        self.items_done.fetch_add(n, Ordering::Relaxed);
    }

    /// More bytes were processed.
    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Ordering::Relaxed);
    }

    /// The path being worked on (call now and then, not for every file).
    pub fn set_current(&self, path: &Path) {
        if let Ok(mut c) = self.current.try_lock() {
            c.clear();
            c.push_str(&path.to_string_lossy());
        }
    }

    pub fn snapshot(&self) -> ProgressSnapshot {
        let (stage, since) = self
            .stage
            .lock()
            .map(|s| (s.0.clone(), s.1))
            .unwrap_or_else(|_| (String::new(), self.started));
        let head = self.head_start.load(Ordering::Relaxed);
        ProgressSnapshot {
            stage,
            stage_no: self.stage_no.load(Ordering::Relaxed),
            stage_count: self.stage_count.load(Ordering::Relaxed),
            items_done: self.items_done.load(Ordering::Relaxed),
            items_total: self.items_total.load(Ordering::Relaxed),
            bytes_done: self.bytes_done.load(Ordering::Relaxed),
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            current: self.current.lock().map(|c| c.clone()).unwrap_or_default(),
            elapsed_secs: self.started.elapsed().as_secs_f64() + head as f64 * 1.4,
            stage_secs: since.elapsed().as_secs_f64() + head as f64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, modified: i64) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            size,
            modified,
            ..Default::default()
        }
    }

    #[test]
    fn filters() {
        let mut cfg = ScanConfig {
            min_size: 10,
            max_size: Some(100),
            ..Default::default()
        };
        assert!(cfg.file_passes(&file("C:\\a\\x.jpg", 10, 5)));
        assert!(!cfg.file_passes(&file("C:\\a\\x.jpg", 9, 5)));
        assert!(!cfg.file_passes(&file("C:\\a\\x.jpg", 101, 5)));
        cfg.only_extensions = vec!["jpg".into(), "png".into()];
        assert!(cfg.file_passes(&file("C:\\a\\x.JPG", 50, 5)));
        assert!(!cfg.file_passes(&file("C:\\a\\x.gif", 50, 5)));
        cfg.only_extensions.clear();
        cfg.skip_extensions = vec!["gif".into()];
        assert!(!cfg.file_passes(&file("C:\\a\\x.gif", 50, 5)));
        assert!(cfg.file_passes(&file("C:\\a\\noext", 50, 5)));
        cfg.modified_after = Some(100);
        cfg.modified_before = Some(200);
        assert!(!cfg.file_passes(&file("C:\\a\\x.jpg", 50, 99)));
        assert!(cfg.file_passes(&file("C:\\a\\x.jpg", 50, 100)));
        assert!(!cfg.file_passes(&file("C:\\a\\x.jpg", 50, 200)));
    }

    #[test]
    fn reference_and_exclude() {
        let cfg = ScanConfig {
            reference: vec!["C:\\Masters".into()],
            exclude: vec!["C:\\Data\\Skip".into()],
            ..Default::default()
        };
        assert!(cfg.is_reference(Path::new("c:\\masters\\a\\b.png")));
        assert!(!cfg.is_reference(Path::new("C:\\Masters2\\b.png")));
        assert!(cfg.is_excluded(Path::new("C:\\Data\\Skip\\x")));
        assert!(!cfg.is_excluded(Path::new("C:\\Data\\Skipper\\x")));
    }

    #[test]
    fn totals() {
        let mut g = Group::new(
            vec![
                Item::new(file("a", 10, 0)),
                Item::new(file("b", 30, 0)),
                Item::new(file("c", 20, 0)),
            ],
            "x",
        );
        assert_eq!(g.total_bytes(), 60);
        assert_eq!(g.wasted_bytes(), 30);
        g.items[0].selected = true;
        g.items[2].selected = true;
        assert_eq!((g.selected_count(), g.selected_bytes()), (2, 30));
        let mut r = ScanResult::empty(Finder::Duplicates);
        r.groups.push(g.clone());
        assert_eq!(r.wasted_bytes(), 30);
        r.finder = Finder::BigFiles;
        assert_eq!(r.wasted_bytes(), 60);
        assert_eq!(r.item_count(), 3);
        assert_eq!(Finder::from_key("pictures"), Some(Finder::SimilarImages));
        assert_eq!(Finder::from_key("nope"), None);
    }

    #[test]
    fn progress_counts_and_estimates() {
        let p = Progress::new();
        p.set_stage_count(3);
        p.set_stage("Hashing", 10, 1000);
        p.add_item();
        p.add_bytes(250);
        let s = p.snapshot();
        assert_eq!(
            (s.stage.as_str(), s.stage_no, s.stage_count),
            ("Hashing", 1, 3)
        );
        assert_eq!(s.fraction(), Some(0.25));
        assert!(!p.cancelled());
        p.cancel();
        assert!(p.cancelled());
        let s = ProgressSnapshot {
            bytes_done: 500,
            bytes_total: 1000,
            stage_secs: 10.0,
            ..Default::default()
        };
        assert_eq!(s.eta_secs(), Some(10));
        assert_eq!(s.speed(), Some(50.0));
    }
}
