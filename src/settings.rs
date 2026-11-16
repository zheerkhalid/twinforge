//! User settings, stored as JSON in `%LOCALAPPDATA%\TwinForge\settings.json`.

use crate::model::{ActionKind, ScanConfig};
use crate::sys::guard_write;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeChoice {
    Dark,
    Light,
    System,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeChoice,
    /// The folders, filters and finder options used last time.
    pub scan: ScanConfig,
    /// Remember file hashes between runs so unchanged files are not read again.
    pub use_cache: bool,
    /// What the main action button does.
    pub default_action: ActionKind,
    /// Compare each file with the copy that is kept, byte by byte, right
    /// before it is removed (exact duplicates only; slower, safest).
    pub verify_before_remove: bool,
    /// Show picture thumbnails in the results.
    pub thumbnails: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: ThemeChoice::Dark,
            scan: ScanConfig::default(),
            use_cache: true,
            default_action: ActionKind::Recycle,
            verify_before_remove: true,
            thumbnails: true,
        }
    }
}

impl Settings {
    pub fn load(app_dir: &Path) -> Settings {
        std::fs::read_to_string(app_dir.join("settings.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, app_dir: &Path) -> Result<(), String> {
        guard_write()?;
        std::fs::create_dir_all(app_dir).map_err(|e| e.to_string())?;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(app_dir.join("settings.json"), json)
            .map_err(|e| format!("Could not save settings: {e}"))
    }

    /// The scan configuration with the cache path filled in (or cleared).
    pub fn scan_config(&self, app_dir: &Path) -> ScanConfig {
        let mut cfg = self.scan.clone();
        cfg.cache_path = self.use_cache.then(|| app_dir.join("hash-cache.bin"));
        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tolerant_loading() {
        let dir =
            std::env::temp_dir().join(format!("twinforge-settings-test-{}", std::process::id()));
        assert_eq!(Settings::load(&dir), Settings::default());
        let mut s = Settings {
            theme: ThemeChoice::Light,
            default_action: ActionKind::Move,
            ..Default::default()
        };
        s.scan.include.push("X:\\Sample".into());
        s.scan.similarity = 80;
        s.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), s);
        assert_eq!(
            s.scan_config(&dir).cache_path,
            Some(dir.join("hash-cache.bin"))
        );
        s.use_cache = false;
        assert_eq!(s.scan_config(&dir).cache_path, None);
        // Partial files keep defaults for the rest; broken files fall back entirely.
        std::fs::write(
            dir.join("settings.json"),
            r#"{"theme":"System","future_option":true,"scan":{"min_size":77}}"#,
        )
        .unwrap();
        let loaded = Settings::load(&dir);
        assert_eq!(loaded.theme, ThemeChoice::System);
        assert_eq!(loaded.scan.min_size, 77);
        assert!(loaded.scan.recursive);
        std::fs::write(dir.join("settings.json"), "{{{").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
