//! The egui user interface.
mod app;
mod bidi;
mod pages;
pub mod results_view;
mod scan_view;
pub mod theme;
pub mod widgets;

pub use app::{run, Options};
