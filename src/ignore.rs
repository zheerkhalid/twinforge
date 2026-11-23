//! The ignore list: groups the user marked as "not a match".
//!
//! A hidden group is remembered as the set of its paths, per finder, in
//! `ignore.json` next to the settings. After every scan, a group whose files
//! all belong to one remembered set is left out of the results. When a new
//! file joins such a group the group is shown again, because there is
//! something new to decide.
//!
//! Hiding never touches a file; it only changes what is listed.

use crate::model::{Finder, Group, ScanResult};
use crate::sys::guard_write;
use crate::util::norm_path;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The list never grows beyond this; the oldest entries make room.
pub const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// `Finder::key()` of the finder the group came from.
    finder: String,
    /// Normalised paths (see [`norm_path`]), sorted.
    paths: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreList {
    entries: Vec<Entry>,
}

fn keys<P: AsRef<Path>>(paths: &[P]) -> Vec<String> {
    let mut keys: Vec<String> = paths
        .iter()
        .map(|p| norm_path(&p.as_ref().to_string_lossy()))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

impl IgnoreList {
    /// Reads the list; a missing or damaged file gives an empty list.
    pub fn load(app_dir: &Path) -> IgnoreList {
        std::fs::read_to_string(app_dir.join("ignore.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, app_dir: &Path) -> Result<(), String> {
        guard_write()?;
        std::fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(app_dir.join("ignore.json"), json)
            .map_err(|e| format!("Could not save the ignore list: {e}"))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Remembers a group of `finder` as "not a match". Returns false when
    /// there is nothing to remember (fewer than two files, or already known).
    pub fn add(&mut self, finder: Finder, paths: &[PathBuf]) -> bool {
        let paths = keys(paths);
        if paths.len() < 2 || self.covers(finder, &paths) {
            return false;
        }
        // A larger set makes the smaller ones it contains pointless.
        let new: HashSet<&String> = paths.iter().collect();
        self.entries
            .retain(|e| e.finder != finder.key() || !e.paths.iter().all(|p| new.contains(p)));
        self.entries.push(Entry {
            finder: finder.key().to_string(),
            paths,
        });
        if self.entries.len() > MAX_ENTRIES {
            let extra = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(..extra);
        }
        true
    }

    fn covers(&self, finder: Finder, sorted_keys: &[String]) -> bool {
        self.entries.iter().any(|e| {
            e.finder == finder.key()
                && e.paths.len() >= sorted_keys.len()
                && sorted_keys.iter().all(|k| e.paths.binary_search(k).is_ok())
        })
    }

    /// Was this group hidden by the user?
    pub fn is_ignored(&self, finder: Finder, group: &Group) -> bool {
        if self.entries.is_empty() || group.items.len() < 2 {
            return false;
        }
        let paths: Vec<&PathBuf> = group.items.iter().map(|i| &i.file.path).collect();
        self.covers(finder, &keys(&paths))
    }

    /// Drops the hidden groups from a result. Returns how many were dropped.
    pub fn apply(&self, result: &mut ScanResult) -> usize {
        if self.entries.is_empty() || !result.finder.is_grouped() {
            return 0;
        }
        let before = result.groups.len();
        let finder = result.finder;
        result.groups.retain(|g| !self.is_ignored(finder, g));
        before - result.groups.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Item};

    fn group(paths: &[&str]) -> Group {
        Group::new(
            paths
                .iter()
                .map(|p| {
                    Item::new(FileEntry {
                        path: PathBuf::from(p),
                        size: 10,
                        ..Default::default()
                    })
                })
                .collect(),
            "",
        )
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn hidden_groups_stay_hidden_until_something_new_joins() {
        let mut list = IgnoreList::default();
        assert!(list.add(
            Finder::SimilarImages,
            &paths(&["X:\\a\\one.jpg", "X:\\b\\two.jpg", "X:\\c\\three.jpg"])
        ));
        // Any spelling of the same paths, in any order, and any part of the set.
        assert!(list.is_ignored(
            Finder::SimilarImages,
            &group(&["x:/C/three.jpg", "X:\\A\\ONE.JPG"])
        ));
        // A new file in the group: there is something to decide again.
        assert!(!list.is_ignored(
            Finder::SimilarImages,
            &group(&["X:\\a\\one.jpg", "X:\\b\\two.jpg", "X:\\d\\four.jpg"])
        ));
        // Another finder is not affected.
        assert!(!list.is_ignored(
            Finder::Duplicates,
            &group(&["X:\\a\\one.jpg", "X:\\b\\two.jpg"])
        ));
        // Known sets are not stored twice; a bigger set replaces its parts.
        assert!(!list.add(
            Finder::SimilarImages,
            &paths(&["X:\\b\\two.jpg", "X:\\a\\one.jpg"])
        ));
        assert!(list.add(
            Finder::SimilarImages,
            &paths(&[
                "X:\\a\\one.jpg",
                "X:\\b\\two.jpg",
                "X:\\c\\three.jpg",
                "X:\\d\\four.jpg"
            ])
        ));
        assert_eq!(list.len(), 1);
        assert!(!list.add(Finder::SimilarImages, &paths(&["X:\\a\\one.jpg"])));
    }

    #[test]
    fn apply_filters_results_and_the_list_survives_a_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("twinforge-ignore-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(IgnoreList::load(&dir).is_empty());
        let mut list = IgnoreList::default();
        list.add(
            Finder::Duplicates,
            &paths(&["X:\\a\\1.bin", "X:\\b\\1.bin"]),
        );
        list.save(&dir).unwrap();
        let loaded = IgnoreList::load(&dir);
        assert_eq!(loaded, list);

        let mut result = ScanResult::empty(Finder::Duplicates);
        result.groups = vec![
            group(&["X:\\a\\1.bin", "X:\\b\\1.bin"]),
            group(&["X:\\a\\2.bin", "X:\\b\\2.bin"]),
        ];
        assert_eq!(loaded.apply(&mut result), 1);
        assert_eq!(result.groups.len(), 1);
        assert!(result.groups[0].items[0].file.path.ends_with("2.bin"));

        // Flat lists are never filtered.
        let mut flat = ScanResult::empty(Finder::BigFiles);
        flat.groups = vec![group(&["X:\\a\\1.bin", "X:\\b\\1.bin"])];
        assert_eq!(loaded.apply(&mut flat), 0);

        let mut cleared = loaded.clone();
        cleared.clear();
        assert!(cleared.is_empty());
        std::fs::write(dir.join("ignore.json"), "not json").unwrap();
        assert!(IgnoreList::load(&dir).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_list_is_bounded() {
        let mut list = IgnoreList::default();
        for n in 0..(MAX_ENTRIES + 25) {
            list.add(
                Finder::SameName,
                &paths(&[&format!("X:\\a\\{n}.txt"), &format!("X:\\b\\{n}.txt")]),
            );
        }
        assert_eq!(list.len(), MAX_ENTRIES);
        // The oldest entries made room.
        assert!(!list.is_ignored(Finder::SameName, &group(&["X:\\a\\0.txt", "X:\\b\\0.txt"])));
        assert!(list.is_ignored(
            Finder::SameName,
            &group(&["X:\\a\\100.txt", "X:\\b\\100.txt"])
        ));
    }
}
