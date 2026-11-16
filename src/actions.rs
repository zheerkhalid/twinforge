//! The guarded action engine: the only code in TwinForge that removes, moves
//! or replaces files.
//!
//! Whatever the window asks for, this module decides for itself, right before
//! each change, whether the change is safe:
//!
//! * a group never loses its last copy: at least one file that is **not**
//!   marked for removal must still exist (and, for exact duplicates, be
//!   unchanged since the scan);
//! * a file is only touched when its size and time stamp are what the scan
//!   saw, and (optionally) when its bytes still equal the kept copy;
//! * the "kept copy" must be a different file on disk, not another name of
//!   the same file;
//! * protected system locations and reference folders are never touched;
//! * junctions and symbolic links are never followed or removed;
//! * folders are only ever removed with `remove_dir`, which fails unless the
//!   folder is empty.
//!
//! Everything that is refused is reported with the reason.

use crate::model::{ActionKind, Finder, Progress, ScanResult};
use crate::sys::{self, Guard};
use crate::util::norm_path;
use crate::walk::is_empty_tree;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

/// How the selected files are dealt with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Recycle,
    Delete,
    /// Move into this folder, keeping the original folder structure below it.
    MoveTo(PathBuf),
    HardLink,
    SymLink,
}

impl Method {
    pub fn kind(&self) -> ActionKind {
        match self {
            Method::Recycle => ActionKind::Recycle,
            Method::Delete => ActionKind::Delete,
            Method::MoveTo(_) => ActionKind::Move,
            Method::HardLink => ActionKind::HardLink,
            Method::SymLink => ActionKind::SymLink,
        }
    }
}

/// One file of a group as the scan saw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanItem {
    pub path: PathBuf,
    pub size: u64,
    pub modified: i64,
    pub is_dir: bool,
    /// Marked for removal. Items that are not marked are the candidates for "kept copy".
    pub remove: bool,
    pub reference: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlanGroup {
    pub items: Vec<PlanItem>,
}

/// Everything one action is going to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub finder: Finder,
    pub method: Method,
    pub groups: Vec<PlanGroup>,
    /// Compare each exact duplicate with its kept copy byte by byte before removing it.
    pub verify_content: bool,
}

impl Plan {
    /// Builds the plan from the ticked items of a result. Groups without a
    /// ticked item are left out.
    pub fn from_result(result: &ScanResult, method: Method, verify_content: bool) -> Plan {
        let groups = result
            .groups
            .iter()
            .filter(|g| g.items.iter().any(|i| i.selected))
            .map(|g| PlanGroup {
                items: g
                    .items
                    .iter()
                    .map(|i| PlanItem {
                        path: i.file.path.clone(),
                        size: i.file.size,
                        modified: i.file.modified,
                        is_dir: i.file.is_dir,
                        remove: i.selected,
                        reference: i.reference,
                    })
                    .collect(),
            })
            .collect();
        Plan {
            finder: result.finder,
            method,
            groups,
            verify_content,
        }
    }

    /// How many items are marked for removal.
    pub fn count(&self) -> usize {
        self.groups
            .iter()
            .flat_map(|g| g.items.iter())
            .filter(|i| i.remove)
            .count()
    }

    /// Total size of the items marked for removal.
    pub fn bytes(&self) -> u64 {
        self.groups
            .iter()
            .flat_map(|g| g.items.iter())
            .filter(|i| i.remove)
            .map(|i| i.size)
            .sum()
    }

    /// How many groups would lose every copy if the plan ran as written. The
    /// engine refuses those groups; the window uses this to warn up front.
    pub fn groups_without_keeper(&self) -> usize {
        if !self.finder.is_grouped() {
            return 0;
        }
        self.groups
            .iter()
            .filter(|g| g.items.iter().any(|i| i.remove) && g.items.iter().all(|i| i.remove))
            .count()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    /// The change was made.
    Done,
    /// The file was already gone; nothing to do.
    Gone,
    /// A safety rule said no. Nothing was changed.
    Refused(String),
    /// Windows said no. Nothing was changed.
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub path: PathBuf,
    pub size: u64,
    pub status: Status,
    /// Where the file went (move action).
    pub moved_to: Option<PathBuf>,
    /// The copy that was kept in its place (grouped finders).
    pub kept: Option<PathBuf>,
}

/// What happened, item by item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub finder: Finder,
    pub action: ActionKind,
    pub outcomes: Vec<Outcome>,
    pub cancelled: bool,
    /// Items the plan contained (the rest was not attempted when cancelled).
    pub planned: usize,
}

impl Report {
    pub fn done(&self) -> usize {
        self.count(|s| matches!(s, Status::Done))
    }
    pub fn refused(&self) -> usize {
        self.count(|s| matches!(s, Status::Refused(_)))
    }
    pub fn failed(&self) -> usize {
        self.count(|s| matches!(s, Status::Failed(_)))
    }
    fn count(&self, f: impl Fn(&Status) -> bool) -> usize {
        self.outcomes.iter().filter(|o| f(&o.status)).count()
    }

    /// Bytes no longer taken up at the original places.
    pub fn bytes_done(&self) -> u64 {
        self.outcomes
            .iter()
            .filter(|o| o.status == Status::Done)
            .map(|o| o.size)
            .sum()
    }

    /// Paths that are no longer where the results list shows them.
    pub fn gone_paths(&self) -> HashSet<PathBuf> {
        self.outcomes
            .iter()
            .filter(|o| matches!(o.status, Status::Done | Status::Gone))
            .map(|o| o.path.clone())
            .collect()
    }

    /// Completed moves as `(from, to)`.
    pub fn moves(&self) -> Vec<(PathBuf, PathBuf)> {
        self.outcomes
            .iter()
            .filter(|o| o.status == Status::Done)
            .filter_map(|o| o.moved_to.clone().map(|to| (o.path.clone(), to)))
            .collect()
    }
}

// --------------------------------------------------------------------------------------
// Checks
// --------------------------------------------------------------------------------------

fn explain_io(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(5) => "Access denied. The file may be in use or need administrator rights.".into(),
        Some(32) => "It is in use by another program.".into(),
        Some(145) => "The folder is not empty.".into(),
        Some(17) => "The two places are on different drives.".into(),
        Some(1314) => {
            "Windows only lets administrators, or PCs with Developer Mode switched on, create symbolic links.".into()
        }
        Some(1) | Some(50) => "This drive does not support that kind of link.".into(),
        _ => e.to_string(),
    }
}

/// A file that exists right now as a plain file (not a link, not a folder),
/// with its size and time stamp.
fn plain_file(path: &Path) -> Option<(u64, i64)> {
    use std::os::windows::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    let attrs = meta.file_attributes();
    if attrs & (sys::FILE_ATTRIBUTE_DIRECTORY | sys::FILE_ATTRIBUTE_REPARSE_POINT) != 0
        || meta.file_type().is_symlink()
    {
        return None;
    }
    Some((meta.len(), sys::unix_secs(meta.modified())))
}

/// Are the two files equal byte for byte?
pub fn same_content(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    if fa.metadata()?.len() != fb.metadata()?.len() {
        return Ok(false);
    }
    let mut ba = vec![0u8; 256 * 1024];
    let mut bb = vec![0u8; 256 * 1024];
    loop {
        let n = read_full(&mut fa, &mut ba)?;
        let m = read_full(&mut fb, &mut bb)?;
        if n != m || ba[..n] != bb[..m] {
            return Ok(false);
        }
        if n == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match f.read(&mut buf[filled..])? {
            0 => break,
            n => filled += n,
        }
    }
    Ok(filled)
}

/// The checks every item must pass before anything is done to it. Returns
/// `Ok(false)` when the item is already gone.
fn check_item(item: &PlanItem, guard: &Guard) -> Result<bool, String> {
    if item.reference {
        return Err("It is in a reference folder, which is never changed.".into());
    }
    if guard.is_protected(&item.path) {
        return Err("It is in a protected system location.".into());
    }
    let Ok(meta) = std::fs::symlink_metadata(&item.path) else {
        return Ok(false);
    };
    use std::os::windows::fs::MetadataExt;
    if meta.file_attributes() & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0
        || meta.file_type().is_symlink()
    {
        return Err("It is a link to another place; links are left alone.".into());
    }
    if sys::passes_through_link(&item.path) {
        return Err("It is reached through a folder link; links are never followed.".into());
    }
    if item.is_dir != meta.is_dir() {
        return Err("It is no longer what the scan saw.".into());
    }
    if item.is_dir {
        if guard.contains_protected(&item.path) {
            return Err("A protected system location lies inside it.".into());
        }
        if !is_empty_tree(&item.path) {
            return Err("The folder is no longer empty.".into());
        }
    } else if meta.len() != item.size || sys::unix_secs(meta.modified()) != item.modified {
        return Err("It has changed since the scan. Scan again to be sure.".into());
    }
    Ok(true)
}

/// Picks the copy that stays for `item`: a live keeper of the group that is a
/// different file on disk (and, when asked, has the same bytes).
fn pick_keeper(
    item: &PlanItem,
    keepers: &[&PlanItem],
    exact: bool,
    verify: bool,
) -> Result<PathBuf, String> {
    let Some(id) = sys::file_id(&item.path) else {
        return Err("Its identity on disk could not be read.".into());
    };
    let mut reason = "No other copy of this file would remain.".to_string();
    for k in keepers {
        let Some(kid) = sys::file_id(&k.path) else {
            continue;
        };
        if kid.same_file(&id) {
            reason = "The copy that would remain is the same file on disk (a hard link or a second path to it), not a real copy.".into();
            continue;
        }
        if exact && verify {
            match same_content(&item.path, &k.path) {
                Ok(true) => {}
                Ok(false) => {
                    reason = "Its contents no longer match the copy that would remain.".into();
                    continue;
                }
                Err(e) => {
                    reason = format!("It could not be compared with the kept copy: {e}");
                    continue;
                }
            }
        }
        return Ok(k.path.clone());
    }
    Err(reason)
}

// --------------------------------------------------------------------------------------
// The individual changes
// --------------------------------------------------------------------------------------

fn clear_readonly(path: &Path) {
    if let Some(attrs) = sys::attributes(path) {
        if attrs & sys::FILE_ATTRIBUTE_READONLY != 0 {
            sys::set_attributes(path, attrs & !sys::FILE_ATTRIBUTE_READONLY);
        }
    }
}

fn delete_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(_) => {
            clear_readonly(path);
            std::fs::remove_file(path).map_err(|e| explain_io(&e))
        }
    }
}

/// Removes a folder that holds nothing but empty folders, bottom-up, using
/// only `remove_dir` (which cannot remove a folder with anything in it).
fn remove_empty_tree(dir: &Path) -> Result<(), String> {
    if sys::is_reparse_point(dir) {
        return Err("It is a link to another place; links are left alone.".into());
    }
    let reader = std::fs::read_dir(dir).map_err(|e| explain_io(&e))?;
    for entry in reader {
        let entry = entry.map_err(|e| explain_io(&e))?;
        let meta = entry.metadata().map_err(|e| explain_io(&e))?;
        if !meta.is_dir() {
            return Err("The folder is no longer empty.".into());
        }
        remove_empty_tree(&entry.path())?;
    }
    std::fs::remove_dir(dir).map_err(|e| explain_io(&e))
}

fn recycle(path: &Path) -> Result<(), String> {
    sys::recycle(&[path.to_path_buf()])?;
    if std::fs::symlink_metadata(path).is_ok() {
        return Err("It is still there after the Recycle Bin took it (it may be in use).".into());
    }
    Ok(())
}

/// Where `path` lands below `dest`: `D:\Photos\a.jpg` becomes `<dest>\D\Photos\a.jpg`.
/// If that name is taken, ` (2)`, ` (3)`, ... is added.
pub fn move_target(path: &Path, dest: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let text = text.strip_prefix("\\\\?\\").unwrap_or(&text);
    let mut target = dest.to_path_buf();
    for part in text.split(['\\', '/']).filter(|p| !p.is_empty()) {
        target.push(part.trim_end_matches(':'));
    }
    if std::fs::symlink_metadata(&target).is_err() {
        return target;
    }
    let stem = target
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = target
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    for n in 2..10_000 {
        let candidate = target.with_file_name(format!("{stem} ({n}){ext}"));
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
    target
}

/// Moves a file or an empty folder. Across drives a file is copied, the copy
/// is checked, and only then is the original removed.
fn move_path(from: &Path, to: &Path, is_dir: bool) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| explain_io(&e))?;
    }
    if std::fs::symlink_metadata(to).is_ok() {
        return Err("Something with that name is already at the destination.".into());
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    if is_dir {
        // An empty folder: recreate it over there, then remove it here.
        std::fs::create_dir_all(to).map_err(|e| explain_io(&e))?;
        return remove_empty_tree(from);
    }
    std::fs::copy(from, to).map_err(|e| explain_io(&e))?;
    match same_content(from, to) {
        Ok(true) => {}
        _ => {
            let _ = std::fs::remove_file(to);
            return Err("The copy at the destination did not match; the original was kept.".into());
        }
    }
    delete_file(from).inspect_err(|_| {
        let _ = std::fs::remove_file(to);
    })
}

/// Replaces `path` with a link to `keeper`. The original is set aside under a
/// temporary name first and only removed once the link exists; if anything
/// fails it is put back.
fn replace_with_link(path: &Path, keeper: &Path, symbolic: bool) -> Result<(), String> {
    let mut aside = None;
    for n in 0..1000 {
        let mut name = path
            .file_name()
            .map(|s| s.to_os_string())
            .unwrap_or_default();
        name.push(format!(".twinforge-{n}.tmp"));
        let candidate = path.with_file_name(name);
        if std::fs::symlink_metadata(&candidate).is_err() {
            aside = Some(candidate);
            break;
        }
    }
    let aside = aside.ok_or("No free temporary name next to the file.")?;
    std::fs::rename(path, &aside).map_err(|e| explain_io(&e))?;
    let linked = if symbolic {
        std::os::windows::fs::symlink_file(keeper, path)
    } else {
        std::fs::hard_link(keeper, path)
    };
    match linked {
        Ok(()) => {
            // The link is in place; the set-aside original is now redundant.
            if delete_file(&aside).is_err() {
                return Err(format!(
                    "The link was created, but the old copy could not be removed. It is now called {}.",
                    aside.to_string_lossy()
                ));
            }
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(path);
            std::fs::rename(&aside, path).map_err(|e2| {
                format!(
                    "The link failed and the original could not be renamed back from {}: {e2}",
                    aside.to_string_lossy()
                )
            })?;
            Err(explain_io(&e))
        }
    }
}

fn perform(
    item: &PlanItem,
    method: &Method,
    keeper: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    sys::guard_write()?;
    match method {
        Method::Recycle => recycle(&item.path).map(|_| None),
        Method::Delete => {
            if item.is_dir {
                remove_empty_tree(&item.path).map(|_| None)
            } else {
                delete_file(&item.path).map(|_| None)
            }
        }
        Method::MoveTo(dest) => {
            let target = move_target(&item.path, dest);
            move_path(&item.path, &target, item.is_dir).map(|_| Some(target))
        }
        Method::HardLink | Method::SymLink => {
            let keeper = keeper.ok_or("There is no kept copy to link to.")?;
            replace_with_link(&item.path, keeper, *method == Method::SymLink).map(|_| None)
        }
    }
}

// --------------------------------------------------------------------------------------
// Running a plan
// --------------------------------------------------------------------------------------

/// Carries out a plan. Never panics on file errors; every item gets an outcome
/// unless the run is cancelled first.
pub fn execute(plan: &Plan, guard: &Guard, progress: &Progress) -> Report {
    let mut report = Report {
        finder: plan.finder,
        action: plan.method.kind(),
        outcomes: Vec::new(),
        cancelled: false,
        planned: plan.count(),
    };
    progress.set_stage(
        plan.method.kind().title(),
        plan.count() as u64,
        plan.bytes(),
    );

    let refuse_all = |report: &mut Report, reason: &str| {
        for item in plan.groups.iter().flat_map(|g| g.items.iter()) {
            if item.remove {
                report.outcomes.push(Outcome {
                    path: item.path.clone(),
                    size: item.size,
                    status: Status::Refused(reason.to_string()),
                    moved_to: None,
                    kept: None,
                });
            }
        }
    };

    // Whole-plan rules.
    if let Err(e) = sys::guard_write() {
        refuse_all(&mut report, &e);
        return report;
    }
    let kind = plan.method.kind();
    if !kind.available_for(plan.finder) {
        refuse_all(&mut report, "Links can only replace exact duplicates.");
        return report;
    }
    if let Method::MoveTo(dest) = &plan.method {
        let problem = if dest.as_os_str().is_empty() || !dest.is_absolute() {
            Some("The destination folder is not a full path.")
        } else if guard.is_protected(dest) {
            Some("The destination is a protected system location.")
        } else if sys::passes_through_link(dest) {
            Some("The destination is reached through a folder link.")
        } else {
            None
        };
        if let Some(p) = problem {
            refuse_all(&mut report, p);
            return report;
        }
    }

    // A file that some group wants removed can never serve as a kept copy.
    let doomed: HashSet<String> = plan
        .groups
        .iter()
        .flat_map(|g| g.items.iter())
        .filter(|i| i.remove)
        .map(|i| norm_path(&i.path.to_string_lossy()))
        .collect();
    let grouped = plan.finder.is_grouped();
    let exact = plan.finder == Finder::Duplicates;
    let linking = matches!(plan.method, Method::HardLink | Method::SymLink);

    'groups: for group in &plan.groups {
        // The copies that will stay: not marked, not doomed elsewhere, still
        // there as plain files and (for exact duplicates) unchanged.
        let keepers: Vec<&PlanItem> = if grouped {
            group
                .items
                .iter()
                .filter(|k| !k.remove && !doomed.contains(&norm_path(&k.path.to_string_lossy())))
                .filter(|k| match plain_file(&k.path) {
                    Some((size, modified)) => !exact || (size == k.size && modified == k.modified),
                    None => false,
                })
                .collect()
        } else {
            Vec::new()
        };

        for item in group.items.iter().filter(|i| i.remove) {
            if progress.cancelled() {
                report.cancelled = true;
                break 'groups;
            }
            progress.set_current(&item.path);
            let mut kept = None;
            let status = match check_item(item, guard) {
                Err(reason) => Status::Refused(reason),
                Ok(false) => Status::Gone,
                Ok(true) => {
                    let keeper = if grouped {
                        if item.is_dir {
                            Err("Folders are not part of duplicate groups.".to_string())
                        } else if keepers.is_empty() {
                            Err("No other copy of this file would remain, so nothing in this group was touched.".to_string())
                        } else {
                            // Links must point at identical bytes, always.
                            pick_keeper(item, &keepers, exact, plan.verify_content || linking)
                                .map(Some)
                        }
                    } else {
                        Ok(None)
                    };
                    match keeper {
                        Err(reason) => Status::Refused(reason),
                        Ok(keeper) => {
                            kept = keeper.clone();
                            match perform(item, &plan.method, keeper.as_deref()) {
                                Ok(moved_to) => {
                                    report.outcomes.push(Outcome {
                                        path: item.path.clone(),
                                        size: item.size,
                                        status: Status::Done,
                                        moved_to,
                                        kept: kept.clone(),
                                    });
                                    progress.add_item();
                                    progress.add_bytes(item.size);
                                    continue;
                                }
                                Err(e) => Status::Failed(e),
                            }
                        }
                    }
                }
            };
            report.outcomes.push(Outcome {
                path: item.path.clone(),
                size: item.size,
                status,
                moved_to: None,
                kept,
            });
            progress.add_item();
            progress.add_bytes(item.size);
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Group, Item};
    use std::fs;

    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        fn new(name: &str) -> Sandbox {
            let dir = std::env::temp_dir()
                .join(format!("twinforge-actions-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Sandbox { dir }
        }
        fn file(&self, rel: &str, content: &[u8]) -> PathBuf {
            let p = self.dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, content).unwrap();
            p
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn item(path: &Path, remove: bool) -> PlanItem {
        let meta = fs::symlink_metadata(path).unwrap();
        PlanItem {
            path: path.to_path_buf(),
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: sys::unix_secs(meta.modified()),
            is_dir: meta.is_dir(),
            remove,
            reference: false,
        }
    }

    fn plan(finder: Finder, method: Method, groups: Vec<Vec<PlanItem>>) -> Plan {
        Plan {
            finder,
            method,
            groups: groups
                .into_iter()
                .map(|items| PlanGroup { items })
                .collect(),
            verify_content: true,
        }
    }

    fn run(p: &Plan) -> Report {
        execute(p, &Guard::none(), &Progress::new())
    }

    fn refused(report: &Report, index: usize) -> &str {
        match &report.outcomes[index].status {
            Status::Refused(r) => r,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn deletes_selected_duplicates_and_keeps_one() {
        let s = Sandbox::new("basic");
        let a = s.file("a.bin", b"same content");
        let b = s.file("x\\b.bin", b"same content");
        let c = s.file("y\\c.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true), item(&c, true)]],
        );
        assert_eq!((p.count(), p.bytes()), (2, 24));
        let r = run(&p);
        assert_eq!((r.done(), r.refused(), r.failed()), (2, 0, 0));
        assert_eq!(r.bytes_done(), 24);
        assert!(a.exists() && !b.exists() && !c.exists());
        assert_eq!(r.outcomes[0].kept.as_deref(), Some(a.as_path()));
        assert_eq!(r.gone_paths().len(), 2);
    }

    #[test]
    fn refuses_to_remove_the_last_copy() {
        let s = Sandbox::new("last");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same content");
        // Every file of the group is marked: the engine must not obey.
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, true), item(&b, true)]],
        );
        assert_eq!(p.groups_without_keeper(), 1);
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (0, 2));
        assert!(refused(&r, 0).contains("No other copy"));
        assert!(a.exists() && b.exists());

        // The same holds for the other grouped finders.
        for finder in [
            Finder::SimilarImages,
            Finder::SimilarMusic,
            Finder::SameName,
        ] {
            let p = plan(
                finder,
                Method::Delete,
                vec![vec![item(&a, true), item(&b, true)]],
            );
            assert_eq!(run(&p).refused(), 2, "{finder:?}");
            assert!(a.exists() && b.exists());
        }
    }

    #[test]
    fn refuses_when_the_kept_copy_is_gone_or_changed() {
        let s = Sandbox::new("keeper");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        // The keeper disappears between scan and action.
        fs::remove_file(&a).unwrap();
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (0, 1));
        assert!(b.exists());

        // The keeper is edited between scan and action (same length, new time).
        let a = s.file("a.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let f = fs::OpenOptions::new().write(true).open(&a).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(5000))
            .unwrap();
        drop(f);
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (0, 1));
        assert!(b.exists());

        // A keeper that another group wants removed is no keeper at all.
        let a = s.file("a.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![
                vec![item(&a, false), item(&b, true)],
                vec![item(&a, true), item(&b, false)],
            ],
        );
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (0, 2));
        assert!(a.exists() && b.exists());
    }

    #[test]
    fn refuses_files_that_changed_since_the_scan() {
        let s = Sandbox::new("changed");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same content");
        let c = s.file("c.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true), item(&c, true)]],
        );
        // b grows; c keeps its size but gets a new time stamp.
        fs::write(&b, b"same content plus more").unwrap();
        let f = fs::OpenOptions::new().write(true).open(&c).unwrap();
        f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(9000))
            .unwrap();
        drop(f);
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (0, 2));
        assert!(refused(&r, 0).contains("changed since the scan"));
        assert!(b.exists() && c.exists());
    }

    #[test]
    fn verifies_contents_against_the_kept_copy() {
        let s = Sandbox::new("verify");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same c0ntent"); // same size, different bytes
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        for p in [&a, &b] {
            let f = fs::OpenOptions::new().write(true).open(p).unwrap();
            f.set_modified(when).unwrap();
        }
        let mut p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let r = run(&p);
        assert!(refused(&r, 0).contains("no longer match"));
        assert!(b.exists());
        assert!(!same_content(&a, &b).unwrap());
        assert!(same_content(&a, &a).unwrap());
        // Links insist on identical bytes even when verification is switched off.
        p.verify_content = false;
        p.method = Method::HardLink;
        assert_eq!(run(&p).refused(), 1);
        assert_eq!(fs::read(&b).unwrap(), b"same c0ntent");
        // Similar pictures are expected to differ: no byte comparison there.
        p.finder = Finder::SimilarImages;
        p.method = Method::Delete;
        p.verify_content = true;
        assert_eq!(run(&p).done(), 1);
        assert!(a.exists() && !b.exists());
    }

    #[test]
    fn refuses_protected_locations_and_reference_folders() {
        let s = Sandbox::new("protected");
        let a = s.file("mine\\a.bin", b"same content");
        let b = s.file("system\\b.bin", b"same content");
        let c = s.file("masters\\c.bin", b"same content");
        let mut reference = item(&c, true);
        reference.reference = true;
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true), reference]],
        );
        let guard = Guard::with_roots(&[s.dir.join("system")]);
        let r = execute(&p, &guard, &Progress::new());
        assert_eq!((r.done(), r.refused()), (0, 2));
        assert!(refused(&r, 0).contains("protected"));
        assert!(refused(&r, 1).contains("reference"));
        assert!(b.exists() && c.exists());

        // The real guard refuses anything under the Windows folder outright,
        // before looking at the disk (the path is invented; nothing is touched).
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let fake = PlanItem {
            path: PathBuf::from(format!(
                "{root}\\System32\\twinforge-test-not-a-real-file.dll"
            )),
            size: 12,
            modified: 1,
            is_dir: false,
            remove: true,
            reference: false,
        };
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), fake]],
        );
        let r = execute(&p, &Guard::real(), &Progress::new());
        assert!(refused(&r, 0).contains("protected"));

        // Moving into a protected place is refused as a whole.
        let p = plan(
            Finder::Duplicates,
            Method::MoveTo(s.dir.join("system\\out")),
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let r = execute(&p, &guard, &Progress::new());
        assert!(refused(&r, 0).contains("destination"));
    }

    #[test]
    fn never_follows_junctions_or_links() {
        let s = Sandbox::new("links");
        let a = s.file("real\\a.bin", b"same content");
        let b = s.file("target\\b.bin", b"same content");
        assert!(sys::make_junction(
            &s.dir.join("link"),
            &s.dir.join("target")
        ));
        let via_link = s.dir.join("link\\b.bin");
        assert!(via_link.exists());
        // The file reached through the junction is refused...
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&via_link, true)]],
        );
        let r = run(&p);
        assert!(refused(&r, 0).contains("link"));
        assert!(b.exists());
        // ...and so is the junction itself when presented as an empty folder.
        let mut j = item(&s.dir.join("link"), true);
        j.is_dir = true;
        let p = plan(Finder::EmptyFolders, Method::Delete, vec![vec![j]]);
        let r = run(&p);
        assert!(refused(&r, 0).contains("link"));
        assert!(b.exists() && s.dir.join("link").exists());
        fs::remove_dir(s.dir.join("link")).unwrap();
    }

    #[test]
    fn hard_links_of_the_same_file_are_not_copies() {
        let s = Sandbox::new("hardlink-same");
        let a = s.file("a.bin", b"same content");
        let b = s.dir.join("b.bin");
        fs::hard_link(&a, &b).unwrap();
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let r = run(&p);
        assert!(refused(&r, 0).contains("same file on disk"));
        assert!(a.exists() && b.exists());
        // The same path spelled differently is not a copy either.
        let alias = PathBuf::from(a.to_string_lossy().to_uppercase());
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&alias, true)]],
        );
        assert_eq!(run(&p).refused(), 1);
        assert!(a.exists());
    }

    #[test]
    fn replaces_duplicates_with_links() {
        let s = Sandbox::new("linking");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same content");
        let c = s.file("c.bin", b"same content");
        let p = plan(
            Finder::Duplicates,
            Method::HardLink,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let r = run(&p);
        assert_eq!(r.done(), 1, "{:?}", r.outcomes);
        assert_eq!(fs::read(&b).unwrap(), b"same content");
        assert!(sys::file_id(&a)
            .unwrap()
            .same_file(&sys::file_id(&b).unwrap()));
        assert_eq!(
            fs::read_dir(&s.dir).unwrap().count(),
            3,
            "no temp file left"
        );

        // Symbolic links need a privilege; either way nothing may be lost.
        let p = plan(
            Finder::Duplicates,
            Method::SymLink,
            vec![vec![item(&a, false), item(&c, true)]],
        );
        let r = run(&p);
        assert_eq!(fs::read(&c).unwrap(), b"same content");
        if r.done() == 1 {
            assert!(fs::symlink_metadata(&c).unwrap().file_type().is_symlink());
        } else {
            assert_eq!(r.failed(), 1);
            assert!(!fs::symlink_metadata(&c).unwrap().file_type().is_symlink());
        }
        assert_eq!(fs::read_dir(&s.dir).unwrap().count(), 3);

        // Links are refused for anything but exact duplicates.
        let d = s.file("d.bin", b"same content");
        let p = plan(
            Finder::SimilarImages,
            Method::HardLink,
            vec![vec![item(&a, false), item(&d, true)]],
        );
        assert_eq!(run(&p).refused(), 1);
    }

    #[test]
    fn moves_files_and_reports_where_they_went() {
        let s = Sandbox::new("move");
        let a = s.file("in\\a.bin", b"same content");
        let b = s.file("in\\sub\\b.bin", b"same content");
        let c = s.file("in\\sub2\\b.bin", b"same content");
        let dest = s.dir.join("out");
        let p = plan(
            Finder::Duplicates,
            Method::MoveTo(dest.clone()),
            vec![vec![item(&a, false), item(&b, true), item(&c, true)]],
        );
        let r = run(&p);
        assert_eq!(r.done(), 2, "{:?}", r.outcomes);
        let moves = r.moves();
        assert_eq!(moves.len(), 2);
        for (from, to) in &moves {
            assert!(!from.exists() && to.exists());
            assert!(crate::util::within(to, &dest));
            assert_eq!(fs::read(to).unwrap(), b"same content");
        }
        assert_ne!(moves[0].1, moves[1].1);
        // A taken name gets a number instead of overwriting.
        let again = s.file("in\\sub\\b.bin", b"same content");
        let t = move_target(&again, &dest);
        assert!(t.to_string_lossy().ends_with("b (2).bin"), "{t:?}");
    }

    #[test]
    fn flat_finders_and_empty_folders() {
        let s = Sandbox::new("flat");
        let empty = s.file("zero.txt", b"");
        let p = plan(
            Finder::EmptyFiles,
            Method::Delete,
            vec![vec![item(&empty, true)]],
        );
        assert_eq!(p.groups_without_keeper(), 0);
        assert_eq!(run(&p).done(), 1);
        assert!(!empty.exists());

        fs::create_dir_all(s.dir.join("hollow\\inner\\deep")).unwrap();
        fs::create_dir_all(s.dir.join("filled")).unwrap();
        let hollow = item(&s.dir.join("hollow"), true);
        let filled = item(&s.dir.join("filled"), true);
        // A file arrives after the scan: the folder must stay.
        s.file("filled\\late.txt", b"x");
        let p = plan(
            Finder::EmptyFolders,
            Method::Delete,
            vec![vec![hollow], vec![filled]],
        );
        let r = run(&p);
        assert_eq!((r.done(), r.refused()), (1, 1));
        assert!(!s.dir.join("hollow").exists());
        assert!(s.dir.join("filled\\late.txt").exists());
        assert!(refused(&r, 1).contains("no longer empty"));

        // An item that vanished on its own is reported as gone, not as an error.
        let ghost = s.file("ghost.tmp", b"x");
        let p = plan(
            Finder::TempFiles,
            Method::Delete,
            vec![vec![item(&ghost, true)]],
        );
        fs::remove_file(&ghost).unwrap();
        let r = run(&p);
        assert_eq!(r.outcomes[0].status, Status::Gone);
        assert_eq!(r.gone_paths().len(), 1);
    }

    #[test]
    fn read_only_files_and_cancelling() {
        let s = Sandbox::new("misc");
        let a = s.file("a.bin", b"same content");
        let b = s.file("b.bin", b"same content");
        sys::set_attributes(&b, sys::FILE_ATTRIBUTE_READONLY);
        let p = plan(
            Finder::Duplicates,
            Method::Delete,
            vec![vec![item(&a, false), item(&b, true)]],
        );
        let progress = Progress::new();
        progress.cancel();
        let r = execute(&p, &Guard::none(), &progress);
        assert!(r.cancelled && r.outcomes.is_empty() && b.exists());
        let r = run(&p);
        assert_eq!(r.done(), 1, "{:?}", r.outcomes);
        assert!(!b.exists());
    }

    #[test]
    fn plan_from_result_takes_only_groups_with_a_selection() {
        let file = |p: &str| FileEntry {
            path: PathBuf::from(p),
            size: 10,
            modified: 1,
            ..Default::default()
        };
        let mut r = ScanResult::empty(Finder::Duplicates);
        let mut g1 = Group::new(vec![Item::new(file("X:\\a")), Item::new(file("X:\\b"))], "");
        g1.items[1].selected = true;
        g1.items[0].reference = true;
        let g2 = Group::new(vec![Item::new(file("X:\\c")), Item::new(file("X:\\d"))], "");
        r.groups = vec![g1, g2];
        let p = Plan::from_result(&r, Method::Recycle, true);
        assert_eq!(p.groups.len(), 1);
        assert_eq!((p.count(), p.bytes()), (1, 10));
        assert!(p.groups[0].items[0].reference && !p.groups[0].items[0].remove);
        assert_eq!(p.method.kind(), ActionKind::Recycle);
    }
}
