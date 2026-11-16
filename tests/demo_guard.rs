//! Demo mode must make every writing operation refuse. This lives in its own
//! test program because the demo switch is global to the process.

use std::path::PathBuf;
use twinforge::actions::{execute, Method, Plan, PlanGroup, PlanItem, Status};
use twinforge::model::{Finder, Progress, ScanResult};
use twinforge::settings::Settings;
use twinforge::sys::{self, Guard};
use twinforge::{export, history};

#[test]
fn nothing_is_written_or_removed_in_demo_mode() {
    let dir = std::env::temp_dir().join(format!("twinforge-demo-guard-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (a, b) = (dir.join("a.bin"), dir.join("b.bin"));
    std::fs::write(&a, b"twin").unwrap();
    std::fs::write(&b, b"twin").unwrap();
    let item = |p: &PathBuf, remove: bool| {
        let m = std::fs::metadata(p).unwrap();
        PlanItem {
            path: p.clone(),
            size: m.len(),
            modified: sys::unix_secs(m.modified()),
            is_dir: false,
            remove,
            reference: false,
        }
    };
    let plan = |method: Method| Plan {
        finder: Finder::Duplicates,
        method,
        groups: vec![PlanGroup {
            items: vec![item(&a, false), item(&b, true)],
        }],
        verify_content: true,
    };

    sys::set_demo(true);
    assert!(sys::is_demo());
    for method in [
        Method::Delete,
        Method::Recycle,
        Method::MoveTo(dir.join("out")),
        Method::HardLink,
        Method::SymLink,
    ] {
        let report = execute(&plan(method.clone()), &Guard::none(), &Progress::new());
        assert_eq!(report.done(), 0, "{method:?}");
        assert!(
            matches!(&report.outcomes[0].status, Status::Refused(r) if r.contains("Demo mode")),
            "{method:?}: {:?}",
            report.outcomes
        );
    }
    assert!(a.exists() && b.exists());
    assert!(!dir.join("out").exists());
    assert!(sys::recycle(std::slice::from_ref(&b)).is_err());
    assert!(Settings::default().save(&dir.join("app")).is_err());
    assert!(export::write(
        &ScanResult::empty(Finder::Duplicates),
        &dir.join("list.csv"),
        0
    )
    .is_err());
    assert!(history::forget(&dir.join("app"), "2026-01-01-00-00-00-000").is_err());
    assert!(twinforge::selftest::run(false).is_err());
    assert!(!dir.join("app").exists() && !dir.join("list.csv").exists());
    assert!(sys::open_path("C:\\invented\\file.txt").is_err());

    // Outside demo mode the same plan works.
    sys::set_demo(false);
    let report = execute(&plan(Method::Delete), &Guard::none(), &Progress::new());
    assert_eq!(report.done(), 1);
    assert!(a.exists() && !b.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
