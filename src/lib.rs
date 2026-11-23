//! TwinForge: a safe finder for duplicate files, similar pictures and other clutter.
//!
//! The logic lives in this library so that it can be unit-tested; `main.rs`
//! only parses the command line and opens the window.
//!
//! Layout:
//! * `model`, `util`, `sys`, `walk`: shared types, helpers, Win32 wrappers, the folder walker
//! * `hashing`, `cache`, `dupes`, `finders`: exact duplicates and the small finders
//! * `phash`, `images`, `fuzzy`, `music`: similar pictures and similar music
//! * `select`: smart selection rules; `actions`, `history`: the guarded action engine and its undo log
//! * `wic`: HEIC, AVIF and camera RAW through the Windows codecs; `ignore`: groups marked "not a match"
//! * `engine`: runs a finder; `export`, `cli`, `selftest`, `demo`
//! * `ui`: the window
pub mod actions;
pub mod cache;
pub mod cli;
pub mod demo;
pub mod dupes;
pub mod engine;
pub mod export;
pub mod finders;
pub mod fuzzy;
pub mod hashing;
pub mod history;
pub mod ignore;
pub mod images;
pub mod lnk;
pub mod model;
pub mod music;
pub mod phash;
pub mod select;
pub mod selftest;
pub mod settings;
pub mod sys;
pub mod ui;
pub mod util;
pub mod walk;
pub mod wic;
