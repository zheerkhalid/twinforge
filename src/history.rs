//! The action log: one JSON file per action in `<app dir>\history`, and the
//! undo for move actions (files are moved back to where they came from).

use crate::actions::{Report, Status};
use crate::model::{ActionKind, Finder};
use crate::sys::{self, Guard};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// At most this many per-file lines are stored per action.
const MAX_LINES: usize = 20_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub path: PathBuf,
    pub size: u64,
    /// Empty when the change was made; otherwise why it was not.
    pub note: String,
    /// Where the file went (move actions).
    pub moved_to: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Also the file name (without `.json`).
    pub id: String,
    /// `2026-01-31 14:05:09`, local time.
    pub when: String,
    pub finder: Finder,
    pub action: ActionKind,
    pub done: usize,
    pub refused: usize,
    pub failed: usize,
    pub bytes: u64,
    pub lines: Vec<Line>,
    /// The moves of this entry were undone.
    #[serde(default)]
    pub undone: bool,
}

impl Entry {
    /// Builds the log entry for a finished action.
    pub fn from_report(report: &Report) -> Entry {
        let when = sys::timestamp();
        let id = format!(
            "{}-{}",
            when.replace([' ', ':'], "-"),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_millis())
                .unwrap_or(0)
        );
        Entry {
            id,
            when,
            finder: report.finder,
            action: report.action,
            done: report.done(),
            refused: report.refused(),
            failed: report.failed(),
            bytes: report.bytes_done(),
            lines: report
                .outcomes
                .iter()
                .take(MAX_LINES)
                .map(|o| Line {
                    path: o.path.clone(),
                    size: o.size,
                    note: match &o.status {
                        Status::Done => String::new(),
                        Status::Gone => "Already gone.".into(),
                        Status::Refused(r) => format!("Refused: {r}"),
                        Status::Failed(r) => format!("Failed: {r}"),
                    },
                    moved_to: o.moved_to.clone(),
                })
                .collect(),
            undone: false,
        }
    }

    /// Moves that can still be undone.
    pub fn undoable(&self) -> usize {
        if self.undone || self.action != ActionKind::Move {
            return 0;
        }
        self.lines
            .iter()
            .filter(|l| l.note.is_empty() && l.moved_to.is_some())
            .count()
    }
}

fn dir(app_dir: &Path) -> PathBuf {
    app_dir.join("history")
}

pub fn save(app_dir: &Path, entry: &Entry) -> Result<(), String> {
    sys::guard_write()?;
    let dir = dir(app_dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(entry).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", entry.id)), json)
        .map_err(|e| format!("Could not write the action log: {e}"))
}

/// All log entries, newest first. Unreadable files are skipped.
pub fn load_all(app_dir: &Path) -> Vec<Entry> {
    let mut entries: Vec<Entry> = std::fs::read_dir(dir(app_dir))
        .map(|reader| {
            reader
                .flatten()
                .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|t| serde_json::from_str::<Entry>(&t).ok())
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by(|a, b| b.id.cmp(&a.id));
    entries
}

/// Removes one entry from the log (the files it mentions are not touched).
pub fn forget(app_dir: &Path, id: &str) -> Result<(), String> {
    sys::guard_write()?;
    if id.is_empty() || id.contains(['\\', '/', ':', '.']) {
        return Err("Not a log entry.".into());
    }
    std::fs::remove_file(dir(app_dir).join(format!("{id}.json"))).map_err(|e| e.to_string())
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UndoReport {
    pub restored: usize,
    /// `(path, reason)` for every file that could not be put back.
    pub problems: Vec<(PathBuf, String)>,
}

/// Moves the files of a move action back to where they came from. A file is
/// only put back when it is still at the destination and nothing has taken
/// its old place. Marks the entry as undone and saves it.
pub fn undo_moves(app_dir: &Path, entry: &mut Entry, guard: &Guard) -> Result<UndoReport, String> {
    sys::guard_write()?;
    if entry.action != ActionKind::Move {
        return Err("Only move actions can be undone here. Files sent to the Recycle Bin can be restored from the Recycle Bin.".into());
    }
    if entry.undone {
        return Err("This action was already undone.".into());
    }
    let mut report = UndoReport::default();
    for line in &entry.lines {
        let (Some(to), true) = (&line.moved_to, line.note.is_empty()) else {
            continue;
        };
        let from = &line.path;
        let problem = if guard.is_protected(from) {
            Some("The original place is a protected system location.".to_string())
        } else if std::fs::symlink_metadata(to).is_err() {
            Some("It is no longer at the place it was moved to.".to_string())
        } else if sys::is_reparse_point(to) {
            Some("The moved item was replaced by a link.".to_string())
        } else if std::fs::symlink_metadata(from).is_ok() {
            Some("Something else now has its old name.".to_string())
        } else {
            let result = from
                .parent()
                .map(std::fs::create_dir_all)
                .unwrap_or(Ok(()))
                .and_then(|_| match std::fs::rename(to, from) {
                    Ok(()) => Ok(()),
                    // Across drives: copy, then remove the moved copy.
                    Err(_) if to.is_file() => {
                        std::fs::copy(to, from).and_then(|_| std::fs::remove_file(to))
                    }
                    Err(e) => Err(e),
                });
            result.err().map(|e| e.to_string())
        };
        match problem {
            None => report.restored += 1,
            Some(p) => report.problems.push((from.clone(), p)),
        }
    }
    entry.undone = true;
    save(app_dir, entry)?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{execute, Method, Plan, PlanGroup, PlanItem};
    use crate::model::Progress;
    use std::fs;

    #[test]
    fn move_log_undo_round_trip() {
        let base = std::env::temp_dir().join(format!("twinforge-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (data, app, out) = (base.join("data"), base.join("app"), base.join("out"));
        fs::create_dir_all(data.join("sub")).unwrap();
        let keep = data.join("keep.bin");
        let a = data.join("sub\\a.bin");
        let b = data.join("sub\\b.bin");
        for p in [&keep, &a, &b] {
            fs::write(p, b"twin").unwrap();
        }
        let item = |p: &PathBuf, remove: bool| {
            let m = fs::metadata(p).unwrap();
            PlanItem {
                path: p.clone(),
                size: m.len(),
                modified: sys::unix_secs(m.modified()),
                is_dir: false,
                remove,
                reference: false,
            }
        };
        let plan = Plan {
            finder: Finder::Duplicates,
            method: Method::MoveTo(out.clone()),
            groups: vec![PlanGroup {
                items: vec![item(&keep, false), item(&a, true), item(&b, true)],
            }],
            verify_content: true,
        };
        let report = execute(&plan, &Guard::none(), &Progress::new());
        assert_eq!(report.done(), 2);
        let mut entry = Entry::from_report(&report);
        assert_eq!((entry.done, entry.bytes, entry.undoable()), (2, 8, 2));
        save(&app, &entry).unwrap();
        assert_eq!(load_all(&app), vec![entry.clone()]);

        // Something else takes a's old name: that one cannot be put back.
        fs::write(&a, b"newcomer").unwrap();
        let undo = undo_moves(&app, &mut entry, &Guard::none()).unwrap();
        assert_eq!(undo.restored, 1);
        assert_eq!(undo.problems.len(), 1);
        assert_eq!(fs::read(&b).unwrap(), b"twin");
        assert_eq!(fs::read(&a).unwrap(), b"newcomer");
        assert!(entry.undone && entry.undoable() == 0);
        assert!(undo_moves(&app, &mut entry, &Guard::none()).is_err());
        assert!(load_all(&app)[0].undone);

        assert!(forget(&app, "..\\..\\evil").is_err());
        forget(&app, &entry.id).unwrap();
        assert!(load_all(&app).is_empty());
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn only_moves_are_undoable() {
        let report = Report {
            finder: Finder::Duplicates,
            action: ActionKind::Delete,
            outcomes: vec![],
            cancelled: false,
            planned: 0,
        };
        let mut entry = Entry::from_report(&report);
        assert_eq!(entry.undoable(), 0);
        let app = std::env::temp_dir().join("twinforge-history-unused");
        assert!(undo_moves(&app, &mut entry, &Guard::none()).is_err());
    }
}
