//! Runs one finder from start to finish and tidies its result the same way
//! for every finder: reference flags, no groups that offer nothing to do, a
//! stable order.

use crate::model::{Finder, Group, Progress, ScanConfig, ScanResult};
use crate::{dupes, finders, images, music, select};
use std::time::Instant;

/// Runs `finder` over the folders in `cfg`. Read-only.
pub fn run(finder: Finder, cfg: &ScanConfig, progress: &Progress) -> ScanResult {
    let started = Instant::now();
    let mut result = match finder {
        Finder::Duplicates => dupes::scan(cfg, progress),
        Finder::SimilarImages => images::scan(cfg, progress),
        Finder::SimilarMusic => music::scan(cfg, progress),
        other => finders::scan(other, cfg, progress),
    };
    result.finder = finder;
    tidy(&mut result, cfg);
    result.cancelled |= progress.cancelled();
    result.elapsed_ms = started.elapsed().as_millis() as u64;
    result
}

/// Marks reference items, clears any selection, drops groups with nothing to
/// decide and sorts the rest (most wasted space first; flat lists keep the
/// finder's own order).
pub fn tidy(result: &mut ScanResult, cfg: &ScanConfig) {
    for group in &mut result.groups {
        for item in &mut group.items {
            item.reference = cfg.is_reference(&item.file.path);
            item.selected = false;
        }
    }
    if result.finder.is_grouped() {
        result
            .groups
            .retain(|g| g.items.len() >= 2 && g.items.iter().any(|i| !i.reference));
        // Reference items first, so the "original" leads each group.
        for group in &mut result.groups {
            group.items.sort_by_key(|i| !i.reference);
        }
        result.groups.sort_by(|a, b| {
            b.wasted_bytes()
                .cmp(&a.wasted_bytes())
                .then_with(|| first_path(a).cmp(&first_path(b)))
        });
        select::enforce(&mut result.groups);
    } else {
        // Items in reference folders stay in a flat list (they are still the
        // biggest or the emptiest), locked: they can be seen, never selected.
        result.groups.retain(|g| g.items.len() == 1);
    }
}

fn first_path(g: &Group) -> String {
    g.items
        .first()
        .map(|i| i.file.path.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Item};
    use std::path::PathBuf;

    fn item(path: &str, size: u64) -> Item {
        Item::new(FileEntry {
            path: PathBuf::from(path),
            size,
            modified: 1,
            ..Default::default()
        })
    }

    #[test]
    fn tidy_marks_references_and_drops_pointless_groups() {
        let cfg = ScanConfig {
            reference: vec!["X:\\Masters".into()],
            ..Default::default()
        };
        let mut r = ScanResult::empty(Finder::Duplicates);
        r.groups = vec![
            Group::new(
                vec![item("X:\\a\\1", 10), item("X:\\Masters\\1", 10)],
                "small",
            ),
            Group::new(
                vec![item("X:\\Masters\\2", 5), item("X:\\Masters\\sub\\2", 5)],
                "refs only",
            ),
            Group::new(vec![item("X:\\a\\3", 99)], "single"),
            Group::new(vec![item("X:\\a\\4", 500), item("X:\\b\\4", 500)], "big"),
        ];
        r.groups[3].items[0].selected = true;
        tidy(&mut r, &cfg);
        let labels: Vec<&str> = r.groups.iter().map(|g| g.label.as_str()).collect();
        assert_eq!(labels, ["big", "small"]);
        assert!(r.groups[1].items[0].reference, "reference item leads");
        assert_eq!(r.selected_count(), 0);

        let mut flat = ScanResult::empty(Finder::EmptyFiles);
        flat.groups = vec![
            Group::new(vec![item("X:\\a\\z", 0)], ""),
            Group::new(vec![item("X:\\Masters\\z", 0)], ""),
        ];
        tidy(&mut flat, &cfg);
        assert_eq!(flat.groups.len(), 2);
        assert!(flat.groups[1].items[0].reference && !flat.groups[0].items[0].reference);
    }

    #[test]
    fn run_on_nothing_is_empty_and_quick() {
        let cfg = ScanConfig::default();
        for finder in Finder::ALL {
            let r = run(finder, &cfg, &Progress::new());
            assert_eq!(r.finder, finder);
            assert!(r.groups.is_empty());
            assert!(!r.cancelled);
        }
    }
}
