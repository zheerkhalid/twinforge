//! Smart selection rules.
//!
//! The one rule that is never broken: in a group of two or more files, at
//! least one stays unselected, and files in reference folders are never
//! selected. (Groups with a single item are rows of a flat list, such as
//! empty files; there the "one must stay" rule does not apply.)
//!
//! Every change of the selection in the window goes through this module, and
//! the action engine checks the same rule again on its own.

use crate::model::{Group, Item};
use crate::util::within;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// Which file of each group to keep; everything else in the group gets selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rule {
    KeepNewest,
    KeepOldest,
    KeepLargest,
    KeepSmallest,
    KeepHighestResolution,
    /// Keep the track with the highest bitrate (then the larger file).
    KeepBestAudio,
    KeepShortestPath,
    KeepLongestPath,
    /// Keep the file whose name does not look like a copy ("photo (1).jpg",
    /// "report - Copy.docx", "Copy of budget.xlsx").
    KeepOriginalName,
    /// Keep the file in the folder that comes first in this list.
    KeepByFolderPriority(Vec<PathBuf>),
}

impl Rule {
    pub fn title(&self) -> &'static str {
        match self {
            Rule::KeepNewest => "Keep the newest",
            Rule::KeepOldest => "Keep the oldest",
            Rule::KeepLargest => "Keep the largest",
            Rule::KeepSmallest => "Keep the smallest",
            Rule::KeepHighestResolution => "Keep the highest resolution",
            Rule::KeepBestAudio => "Keep the best sound quality",
            Rule::KeepShortestPath => "Keep the shortest path",
            Rule::KeepLongestPath => "Keep the longest path",
            Rule::KeepOriginalName => "Keep the original name",
            Rule::KeepByFolderPriority(_) => "Keep by folder order",
        }
    }
}

fn bitrate(i: &Item) -> u32 {
    match i.detail {
        crate::model::Detail::Audio { bitrate_kbps, .. } => bitrate_kbps,
        _ => 0,
    }
}

/// How much a file name looks like a copy made by Windows, a browser or a
/// sync tool: 0 for a plain name, more for every copy mark found.
/// `photo.jpg` = 0, `photo (1).jpg` = 1, `photo - Copy (2).jpg` = 2.
pub fn copy_marks(path: &Path) -> u32 {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut rest = stem.trim();
    let mut marks = 0;
    loop {
        let before = rest;
        // "name (2)", "name(2)"
        if let Some(open) = rest.rfind('(') {
            if let Some(number) = rest[open + 1..].strip_suffix(')') {
                if !number.is_empty()
                    && number.len() <= 2
                    && number.chars().all(|c| c.is_ascii_digit())
                    && !rest[..open].trim_end().is_empty()
                {
                    rest = rest[..open].trim_end();
                    marks += 1;
                }
            }
        }
        // "name - Copy", "name copy", "name_copy", "name-copy"
        if let Some(head) = rest.strip_suffix("copy") {
            let trimmed = head.trim_end_matches([' ', '-', '_']);
            if trimmed.len() < head.len() && !trimmed.is_empty() {
                rest = trimmed;
                marks += 1;
            }
        }
        // "Copy of name"
        if let Some(tail) = rest.strip_prefix("copy of ") {
            if !tail.trim().is_empty() {
                rest = tail.trim_start();
                marks += 1;
            }
        }
        if rest == before {
            return marks;
        }
    }
}

fn path_len(i: &Item) -> usize {
    i.file.path.as_os_str().len()
}

/// Orders two items so that the one to keep comes first (`Less`).
fn prefer(a: &Item, b: &Item, rule: &Rule) -> Ordering {
    let by_rule = match rule {
        Rule::KeepNewest => b.file.modified.cmp(&a.file.modified),
        Rule::KeepOldest => a.file.modified.cmp(&b.file.modified),
        Rule::KeepLargest => b.file.size.cmp(&a.file.size),
        Rule::KeepSmallest => a.file.size.cmp(&b.file.size),
        Rule::KeepHighestResolution => b
            .pixels()
            .cmp(&a.pixels())
            .then(b.file.size.cmp(&a.file.size)),
        Rule::KeepBestAudio => bitrate(b)
            .cmp(&bitrate(a))
            .then(b.file.size.cmp(&a.file.size)),
        Rule::KeepShortestPath => path_len(a).cmp(&path_len(b)),
        Rule::KeepLongestPath => path_len(b).cmp(&path_len(a)),
        // Fewer copy marks first; among equals the file that was there first.
        Rule::KeepOriginalName => copy_marks(&a.file.path)
            .cmp(&copy_marks(&b.file.path))
            .then(a.file.modified.cmp(&b.file.modified)),
        Rule::KeepByFolderPriority(folders) => {
            let rank = |i: &Item| {
                folders
                    .iter()
                    .position(|f| within(&i.file.path, f))
                    .unwrap_or(usize::MAX)
            };
            rank(a).cmp(&rank(b))
        }
    };
    // Ties: the shorter path, then alphabetical order, so the result never
    // depends on the order the files were listed in.
    by_rule
        .then(path_len(a).cmp(&path_len(b)))
        .then_with(|| a.file.path.cmp(&b.file.path))
}

/// The index of the item a rule keeps, among the non-reference items.
fn keeper(group: &Group, rule: &Rule) -> Option<usize> {
    (0..group.items.len())
        .filter(|&i| !group.items[i].reference)
        .min_by(|&a, &b| prefer(&group.items[a], &group.items[b], rule))
}

/// Applies a rule to every group (replacing the current selection). In a
/// group with a reference file, the reference is the copy that stays and
/// every other file is selected.
pub fn apply(groups: &mut [Group], rule: &Rule) {
    for group in groups.iter_mut() {
        if group.items.len() < 2 {
            for item in &mut group.items {
                item.selected = false;
            }
            continue;
        }
        let has_reference = group.items.iter().any(|i| i.reference);
        let keep = if has_reference {
            None
        } else {
            keeper(group, rule)
        };
        for (i, item) in group.items.iter_mut().enumerate() {
            item.selected = !item.reference && Some(i) != keep;
        }
    }
    enforce(groups);
}

/// Ticks everything except the first file of each group (reference files
/// count as the first). Flat lists are ticked completely.
pub fn select_all_but_first(groups: &mut [Group]) {
    for group in groups.iter_mut() {
        let has_reference = group.items.iter().any(|i| i.reference);
        let single = group.items.len() < 2;
        for (i, item) in group.items.iter_mut().enumerate() {
            item.selected = !item.reference && (single || has_reference || i > 0);
        }
    }
    enforce(groups);
}

/// Repairs any selection so the invariant holds. Returns how many items were unticked.
pub fn enforce(groups: &mut [Group]) -> usize {
    let mut unticked = 0;
    for group in groups.iter_mut() {
        for item in &mut group.items {
            if item.reference && item.selected {
                item.selected = false;
                unticked += 1;
            }
        }
        if group.items.len() >= 2 && group.items.iter().all(|i| i.selected) {
            // Everything is ticked: the first file stays.
            group.items[0].selected = false;
            unticked += 1;
        }
    }
    unticked
}

/// Ticks or unticks one item. Returns false (and changes nothing) when that
/// would select a reference item or the last unselected item of the group.
pub fn set_selected(group: &mut Group, index: usize, on: bool) -> bool {
    let Some(item) = group.items.get(index) else {
        return false;
    };
    if !on {
        group.items[index].selected = false;
        return true;
    }
    if item.reference {
        return false;
    }
    if group.items.len() >= 2 {
        let others_kept = group
            .items
            .iter()
            .enumerate()
            .any(|(i, it)| i != index && !it.selected);
        if !others_kept {
            return false;
        }
    }
    group.items[index].selected = true;
    true
}

/// Keeps exactly this item of the group and ticks all the others (reference
/// items stay unticked as always). Returns false for a reference item's
/// group-mates only when nothing could be ticked.
pub fn keep_only(group: &mut Group, index: usize) -> bool {
    if index >= group.items.len() || group.items.len() < 2 {
        return false;
    }
    for (i, item) in group.items.iter_mut().enumerate() {
        item.selected = i != index && !item.reference;
    }
    true
}

/// Inverts the selection in every group, then repairs it.
pub fn invert(groups: &mut [Group]) {
    for group in groups.iter_mut() {
        for item in &mut group.items {
            item.selected = !item.selected && !item.reference;
        }
    }
    enforce(groups);
}

/// Unticks everything.
pub fn clear(groups: &mut [Group]) {
    for group in groups.iter_mut() {
        for item in &mut group.items {
            item.selected = false;
        }
    }
}

/// Ticks every item inside `folder` (as far as the invariant allows: when a
/// whole group lies in the folder, its first file stays). Returns how many
/// were newly ticked.
pub fn select_in_folder(groups: &mut [Group], folder: &Path) -> usize {
    let mut ticked = 0;
    for group in groups.iter_mut() {
        // From the end, so that if one file must stay it is the first.
        for i in (0..group.items.len()).rev() {
            let item = &group.items[i];
            if !item.selected && within(&item.file.path, folder) && set_selected(group, i, true) {
                ticked += 1;
            }
        }
    }
    ticked
}

/// In every group that has a copy inside `folder`, keeps the copies there and
/// ticks all the others. Groups without a file in the folder, and groups that
/// lie in it completely, are left as they are. Returns how many groups changed.
pub fn keep_in_folder(groups: &mut [Group], folder: &Path) -> usize {
    let mut changed = 0;
    for group in groups.iter_mut() {
        if group.items.len() < 2 {
            continue;
        }
        let inside: Vec<bool> = group
            .items
            .iter()
            .map(|i| within(&i.file.path, folder))
            .collect();
        let any_in = inside.iter().any(|x| *x);
        let any_out = group
            .items
            .iter()
            .zip(&inside)
            .any(|(i, within)| !within && !i.reference);
        if !any_in || !any_out {
            continue;
        }
        for (item, within) in group.items.iter_mut().zip(&inside) {
            item.selected = !within && !item.reference;
        }
        changed += 1;
    }
    enforce(groups);
    changed
}

/// Does the invariant hold for these groups?
pub fn is_valid(groups: &[Group]) -> bool {
    groups.iter().all(|g| {
        g.items.iter().all(|i| !(i.reference && i.selected))
            && (g.items.len() < 2 || g.items.iter().any(|i| !i.selected))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Detail, FileEntry};

    fn item(path: &str, size: u64, modified: i64) -> Item {
        Item::new(FileEntry {
            path: PathBuf::from(path),
            size,
            modified,
            ..Default::default()
        })
    }

    fn picture(path: &str, size: u64, w: u32, h: u32) -> Item {
        let mut i = item(path, size, 1);
        i.detail = Detail::Image {
            width: w,
            height: h,
            similarity: 100,
        };
        i
    }

    fn selected(g: &Group) -> Vec<bool> {
        g.items.iter().map(|i| i.selected).collect()
    }

    fn sample() -> Vec<Group> {
        vec![Group::new(
            vec![
                item("X:\\a\\long folder name\\one.bin", 30, 300),
                item("X:\\b\\two.bin", 10, 100),
                item("X:\\c\\deeper\\three.bin", 20, 200),
            ],
            "",
        )]
    }

    #[test]
    fn every_rule_keeps_the_right_file() {
        let cases: Vec<(Rule, usize)> = vec![
            (Rule::KeepNewest, 0),
            (Rule::KeepOldest, 1),
            (Rule::KeepLargest, 0),
            (Rule::KeepSmallest, 1),
            (Rule::KeepShortestPath, 1),
            (Rule::KeepLongestPath, 0),
            (
                Rule::KeepByFolderPriority(vec!["X:\\c".into(), "X:\\a".into()]),
                2,
            ),
            (Rule::KeepByFolderPriority(vec!["X:\\nowhere".into()]), 1),
        ];
        for (rule, keep) in cases {
            let mut groups = sample();
            apply(&mut groups, &rule);
            let expect: Vec<bool> = (0..3).map(|i| i != keep).collect();
            assert_eq!(selected(&groups[0]), expect, "{rule:?}");
            assert!(is_valid(&groups));
            assert!(!rule.title().is_empty());
        }
    }

    #[test]
    fn resolution_rule_and_ties() {
        let mut groups = vec![Group::new(
            vec![
                picture("X:\\p\\small.jpg", 900, 800, 600),
                picture("X:\\p\\big-b.jpg", 500, 4000, 3000),
                picture("X:\\p\\big-a.png", 700, 4000, 3000),
            ],
            "",
        )];
        apply(&mut groups, &Rule::KeepHighestResolution);
        // Same pixel count: the larger file wins.
        assert_eq!(selected(&groups[0]), [true, true, false]);
        // No pictures: falls back to the largest file.
        let mut plain = sample();
        apply(&mut plain, &Rule::KeepHighestResolution);
        assert_eq!(selected(&plain[0]), [false, true, true]);
        // Music: the highest bitrate stays; without tags, the largest file.
        let track = |path: &str, size: u64, kbps: u32| {
            let mut i = item(path, size, 1);
            i.detail = Detail::Audio {
                artist: String::new(),
                title: String::new(),
                album: String::new(),
                duration_secs: 200,
                bitrate_kbps: kbps,
            };
            i
        };
        let mut tracks = vec![Group::new(
            vec![
                track("X:\\m\\a.mp3", 900, 192),
                track("X:\\m\\a.flac", 800, 990),
                track("X:\\m\\a.ogg", 700, 160),
            ],
            "",
        )];
        apply(&mut tracks, &Rule::KeepBestAudio);
        assert_eq!(selected(&tracks[0]), [true, false, true]);
        let mut plain = sample();
        apply(&mut plain, &Rule::KeepBestAudio);
        assert_eq!(selected(&plain[0]), [false, true, true]);
        // A complete tie is settled by path, whatever the listing order.
        let mut tie = vec![Group::new(
            vec![item("X:\\t\\b.bin", 5, 5), item("X:\\t\\a.bin", 5, 5)],
            "",
        )];
        apply(&mut tie, &Rule::KeepNewest);
        assert_eq!(selected(&tie[0]), [true, false]);
    }

    #[test]
    fn reference_files_are_the_keepers() {
        let mut groups = sample();
        groups[0].items[1].reference = true;
        for rule in [Rule::KeepNewest, Rule::KeepSmallest, Rule::KeepLongestPath] {
            apply(&mut groups, &rule);
            assert_eq!(selected(&groups[0]), [true, false, true], "{rule:?}");
        }
        assert!(
            !set_selected(&mut groups[0], 1, true),
            "a reference cannot be ticked"
        );
        invert(&mut groups);
        assert_eq!(selected(&groups[0]), [false, false, false]);
        select_all_but_first(&mut groups);
        assert_eq!(selected(&groups[0]), [true, false, true]);
        // A group made only of references can never have a selection.
        let mut refs = sample();
        for i in &mut refs[0].items {
            i.reference = true;
            i.selected = true;
        }
        assert_eq!(enforce(&mut refs), 3);
        apply(&mut refs, &Rule::KeepNewest);
        assert_eq!(selected(&refs[0]), [false, false, false]);
    }

    #[test]
    fn the_last_file_cannot_be_ticked() {
        let mut groups = sample();
        assert!(set_selected(&mut groups[0], 0, true));
        assert!(set_selected(&mut groups[0], 1, true));
        assert!(!set_selected(&mut groups[0], 2, true), "the last one stays");
        assert_eq!(selected(&groups[0]), [true, true, false]);
        assert!(set_selected(&mut groups[0], 0, false));
        assert!(set_selected(&mut groups[0], 2, true));
        assert!(!set_selected(&mut groups[0], 9, true));
        assert!(is_valid(&groups));

        // A state forced from outside is repaired.
        for i in &mut groups[0].items {
            i.selected = true;
        }
        assert!(!is_valid(&groups));
        assert_eq!(enforce(&mut groups), 1);
        assert_eq!(selected(&groups[0]), [false, true, true]);
        assert_eq!(enforce(&mut groups), 0);

        assert!(keep_only(&mut groups[0], 1));
        assert_eq!(selected(&groups[0]), [true, false, true]);
        clear(&mut groups);
        assert_eq!(selected(&groups[0]), [false, false, false]);
        invert(&mut groups);
        assert_eq!(selected(&groups[0]), [false, true, true]);
    }

    #[test]
    fn flat_lists_may_be_ticked_completely() {
        let mut flat = vec![
            Group::new(vec![item("X:\\e\\zero-1", 0, 1)], ""),
            Group::new(vec![item("X:\\e\\zero-2", 0, 1)], ""),
        ];
        select_all_but_first(&mut flat);
        assert!(flat.iter().all(|g| g.items[0].selected));
        assert_eq!(enforce(&mut flat), 0);
        assert!(is_valid(&flat));
        assert!(set_selected(&mut flat[0], 0, false));
        assert!(set_selected(&mut flat[0], 0, true));
        invert(&mut flat);
        assert!(flat.iter().all(|g| !g.items[0].selected));
        // Keep-rules mean nothing for a single item: nothing gets ticked.
        apply(&mut flat, &Rule::KeepNewest);
        assert!(flat.iter().all(|g| !g.items[0].selected));
        assert!(!keep_only(&mut flat[0], 0));
    }

    #[test]
    fn selecting_by_folder() {
        let mut groups = vec![
            Group::new(
                vec![
                    item("X:\\keep\\a.bin", 1, 1),
                    item("X:\\dump\\a.bin", 1, 1),
                    item("X:\\dump\\sub\\a.bin", 1, 1),
                ],
                "",
            ),
            // This whole group lies in the folder: one must stay.
            Group::new(
                vec![
                    item("X:\\dump\\b.bin", 1, 1),
                    item("X:\\DUMP\\b2.bin", 1, 1),
                ],
                "",
            ),
            Group::new(
                vec![
                    item("X:\\keep\\c.bin", 1, 1),
                    item("X:\\dumpster\\c.bin", 1, 1),
                ],
                "",
            ),
        ];
        assert_eq!(select_in_folder(&mut groups, Path::new("x:\\dump")), 3);
        assert_eq!(selected(&groups[0]), [false, true, true]);
        assert_eq!(selected(&groups[1]), [false, true]);
        assert_eq!(selected(&groups[2]), [false, false]);
        assert!(is_valid(&groups));
        assert_eq!(select_in_folder(&mut groups, Path::new("x:\\dump")), 0);
    }

    #[test]
    fn the_invariant_survives_random_operations() {
        // A tiny deterministic generator: no test should depend on luck.
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let rules = [
            Rule::KeepNewest,
            Rule::KeepOldest,
            Rule::KeepLargest,
            Rule::KeepSmallest,
            Rule::KeepHighestResolution,
            Rule::KeepBestAudio,
            Rule::KeepShortestPath,
            Rule::KeepLongestPath,
            Rule::KeepByFolderPriority(vec!["X:\\f1".into(), "X:\\f3".into()]),
        ];
        for round in 0..300 {
            let mut groups: Vec<Group> = (0..1 + next(6))
                .map(|g| {
                    let items = (0..1 + next(5))
                        .map(|i| {
                            let mut it = item(
                                &format!("X:\\f{}\\g{g}-{i}.bin", next(4)),
                                next(1000),
                                next(1000) as i64,
                            );
                            it.reference = next(6) == 0;
                            it.selected = next(2) == 0;
                            it
                        })
                        .collect();
                    Group::new(items, "")
                })
                .collect();
            enforce(&mut groups);
            assert!(is_valid(&groups), "round {round}: enforce");
            for step in 0..12 {
                match next(7) {
                    0 => apply(&mut groups, &rules[next(rules.len() as u64) as usize]),
                    1 => invert(&mut groups),
                    2 => select_all_but_first(&mut groups),
                    3 => {
                        select_in_folder(&mut groups, Path::new(&format!("X:\\f{}", next(4))));
                    }
                    4 => {
                        let g = next(groups.len() as u64) as usize;
                        let i = next(groups[g].items.len() as u64) as usize;
                        let on = next(2) == 0;
                        set_selected(&mut groups[g], i, on);
                    }
                    5 => {
                        let g = next(groups.len() as u64) as usize;
                        let i = next(groups[g].items.len() as u64) as usize;
                        keep_only(&mut groups[g], i);
                    }
                    _ => clear(&mut groups),
                }
                assert!(is_valid(&groups), "round {round}, step {step}");
            }
            // After a keep-rule, every group of 2+ keeps exactly one file
            // (or only its references).
            apply(&mut groups, &Rule::KeepNewest);
            for g in groups.iter().filter(|g| g.items.len() >= 2) {
                let kept = g.items.iter().filter(|i| !i.selected).count();
                let refs = g.items.iter().filter(|i| i.reference).count();
                assert_eq!(kept, refs.max(1), "round {round}");
            }
        }
    }

    #[test]
    fn copy_marks_are_recognised() {
        for (name, marks) in [
            ("photo.jpg", 0),
            ("photo (1).jpg", 1),
            ("photo(12).jpg", 1),
            ("report - Copy.docx", 1),
            ("report - Copy (2).docx", 2),
            ("report copy.docx", 1),
            ("report_copy.docx", 1),
            ("Copy of budget.xlsx", 1),
            ("Copy of budget (3).xlsx", 2),
            // Not copy marks: part of the real name.
            ("copy.txt", 0),
            ("photocopy.pdf", 0),
            ("Symphony No. 5 (1808).flac", 0),
            ("(1).jpg", 0),
            ("Holiday (Lisbon).jpg", 0),
            ("\u{648}\u{6ce}\u{646}\u{6d5} (1).jpg", 1),
        ] {
            assert_eq!(copy_marks(Path::new(name)), marks, "{name}");
        }
    }

    #[test]
    fn original_name_rule_prefers_the_plain_name() {
        let mut groups = vec![Group::new(
            vec![
                item("X:\\Downloads\\Harbour walk (1).mp4", 10, 100),
                item("X:\\Videos\\Harbour walk.mp4", 10, 300),
                item("X:\\Backup\\Harbour walk - Copy.mp4", 10, 200),
            ],
            "",
        )];
        apply(&mut groups, &Rule::KeepOriginalName);
        assert_eq!(selected(&groups[0]), [true, false, true]);
        // No name looks like a copy: the oldest file is the original.
        let mut plain = sample();
        apply(&mut plain, &Rule::KeepOriginalName);
        assert_eq!(selected(&plain[0]), [true, false, true]);
        assert!(is_valid(&plain));
    }

    #[test]
    fn keep_in_folder_keeps_the_copies_there() {
        let mut groups = vec![
            Group::new(
                vec![
                    item("X:\\Library\\a.bin", 1, 1),
                    item("X:\\Downloads\\a.bin", 1, 1),
                    item("X:\\Temp\\a.bin", 1, 1),
                ],
                "",
            ),
            // Not in the folder at all: untouched.
            Group::new(
                vec![
                    item("X:\\Downloads\\b.bin", 1, 1),
                    item("X:\\Temp\\b.bin", 1, 1),
                ],
                "",
            ),
            // Completely in the folder: untouched.
            Group::new(
                vec![
                    item("X:\\Library\\c.bin", 1, 1),
                    item("X:\\Library\\sub\\c.bin", 1, 1),
                ],
                "",
            ),
        ];
        groups[1].items[1].selected = true;
        assert_eq!(keep_in_folder(&mut groups, Path::new("X:\\Library")), 1);
        assert_eq!(selected(&groups[0]), [false, true, true]);
        assert_eq!(selected(&groups[1]), [false, true]);
        assert_eq!(selected(&groups[2]), [false, false]);
        assert!(is_valid(&groups));
        // A reference copy outside the folder is never ticked.
        groups[0].items[2].reference = true;
        keep_in_folder(&mut groups, Path::new("X:\\Library"));
        assert_eq!(selected(&groups[0]), [false, true, false]);
    }
}
