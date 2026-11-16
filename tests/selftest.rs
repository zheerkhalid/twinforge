//! End-to-end test: the same run as `TwinForge.exe --selftest`.
//!
//! Everything it touches is created by the test itself inside one folder in
//! the temp directory, which is removed again.

#[test]
fn generated_tree_scan_select_remove_verify() {
    match twinforge::selftest::run(false) {
        Ok(log) => {
            for line in &log {
                println!("{line}");
            }
            assert!(log.len() >= 12, "every stage ran: {log:#?}");
        }
        Err(e) => panic!("{e}"),
    }
    // Nothing may be left behind.
    let leftover = std::env::temp_dir().join(format!("twinforge-selftest-{}", std::process::id()));
    assert!(!leftover.exists());
}
