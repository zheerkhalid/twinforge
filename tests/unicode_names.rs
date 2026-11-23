//! Kurdish and Arabic file and folder names must work from end to end: found
//! by the walker, grouped by the finders, selected by the rules, hidden by the
//! ignore list and removed by the action engine.
//!
//! Everything lives in one generated folder in the temp directory, which is
//! removed again.

use std::path::{Path, PathBuf};
use twinforge::actions::{execute, Method, Plan};
use twinforge::ignore::IgnoreList;
use twinforge::model::{Finder, Progress, ScanConfig, ScanResult};
use twinforge::select::{self, Rule};
use twinforge::sys::Guard;
use twinforge::{dupes, engine, export, finders};

const KURDISH_FOLDER: &str = "بەڵگەنامەکان";
const KURDISH_FILE: &str = "ڕاپۆرتی ساڵانە.txt";
const KURDISH_COPY: &str = "ڕاپۆرتی ساڵانە (1).txt";
const ARABIC_FILE: &str = "التقرير السنوي.txt";

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn names(result: &ScanResult) -> Vec<Vec<String>> {
    result
        .groups
        .iter()
        .map(|g| {
            let mut n: Vec<String> = g
                .items
                .iter()
                .map(|i| {
                    i.file
                        .path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect();
            n.sort();
            n
        })
        .collect()
}

#[test]
fn right_to_left_names_work_from_scan_to_removal() {
    let root = std::env::temp_dir().join(format!("twinforge-unicode-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let original = root.join(KURDISH_FOLDER).join(KURDISH_FILE);
    let copy = root.join("Downloads").join(KURDISH_COPY);
    let arabic_a = root.join(KURDISH_FOLDER).join(ARABIC_FILE);
    let arabic_b = root.join("نسخة احتياطية").join(ARABIC_FILE);
    write(&original, b"the yearly report, the same bytes twice");
    write(&copy, b"the yearly report, the same bytes twice");
    write(&arabic_a, b"first version of another report");
    write(&arabic_b, b"second version, different contents!!");

    let cfg = ScanConfig::for_paths(&[&root]);
    let guard = Guard::none();

    // Exact duplicates: the two Kurdish files, nothing else.
    let mut found = dupes::scan_with(&cfg, &guard, &Progress::new());
    engine::tidy(&mut found, &cfg);
    assert!(found.errors.is_empty(), "{:?}", found.errors);
    let mut expected = vec![KURDISH_FILE.to_string(), KURDISH_COPY.to_string()];
    expected.sort();
    assert_eq!(names(&found), vec![expected]);

    // Same name: the two Arabic files share a name, with different contents.
    let mut same = finders::scan_with(Finder::SameName, &cfg, &guard, &Progress::new());
    same.finder = Finder::SameName;
    engine::tidy(&mut same, &cfg);
    assert_eq!(
        names(&same),
        vec![vec![ARABIC_FILE.to_string(), ARABIC_FILE.to_string()]]
    );

    // The names survive every export format unharmed.
    for text in [
        export::to_csv(&found, 0),
        export::to_json(&found),
        export::to_html(&found, 0),
    ] {
        assert!(text.contains("ڕاپۆرتی ساڵانە"), "{text}");
    }

    // "Keep the original name" keeps the file without the "(1)".
    select::apply(&mut found.groups, &Rule::KeepOriginalName);
    let selected: Vec<PathBuf> = found.groups[0]
        .items
        .iter()
        .filter(|i| i.selected)
        .map(|i| i.file.path.clone())
        .collect();
    assert_eq!(selected.len(), 1);
    assert!(selected[0].ends_with(KURDISH_COPY), "{selected:?}");

    // A hidden group stays hidden, also after the list was saved and loaded.
    let app_dir = root.join("app data");
    let mut ignore = IgnoreList::default();
    let paths: Vec<PathBuf> = same.groups[0]
        .items
        .iter()
        .map(|i| i.file.path.clone())
        .collect();
    assert!(ignore.add(Finder::SameName, &paths));
    ignore.save(&app_dir).unwrap();
    let ignore = IgnoreList::load(&app_dir);
    let mut again = same.clone();
    assert_eq!(ignore.apply(&mut again), 1);
    assert!(again.groups.is_empty());
    let mut untouched = found.clone();
    assert_eq!(ignore.apply(&mut untouched), 0);

    // Removing the selected copy works and the original stays.
    let plan = Plan::from_result(&found, Method::Delete, true);
    let report = execute(&plan, &guard, &Progress::new());
    assert_eq!(
        (report.done(), report.refused(), report.failed()),
        (1, 0, 0),
        "{:?}",
        report.outcomes
    );
    assert!(original.exists() && !copy.exists());
    assert!(arabic_a.exists() && arabic_b.exists());

    std::fs::remove_dir_all(&root).unwrap();
}
