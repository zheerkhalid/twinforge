//! The start page of every finder (where to look, filters, options) and the
//! page shown while a scan runs.

use super::app::App;
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::model::{Finder, HashAlgo, ImageHash, Progress, ScanConfig};
use crate::settings::Settings;
use crate::sys;
use crate::util::{
    fmt_count, fmt_date, fmt_duration, fmt_size, parse_date, parse_extensions, parse_size,
};
use eframe::egui::{self, vec2, Align, CornerRadius, Layout, RichText, Sense, Stroke, Ui};
use std::path::PathBuf;

/// The filter boxes as the user typed them. They are parsed when a scan starts.
#[derive(Clone, Debug, Default)]
pub struct SetupText {
    pub min_size: String,
    pub max_size: String,
    pub only_ext: String,
    pub skip_ext: String,
    pub after: String,
    pub before: String,
    pub patterns: String,
}

/// A size as the filter boxes show it: exact where possible (`16 KB`), so
/// that reading it back gives the same number of bytes.
fn size_text(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1 << 30, "GB"), (1 << 20, "MB"), (1 << 10, "KB")];
    for (unit, name) in UNITS {
        if bytes >= unit && bytes.is_multiple_of(unit) {
            return format!("{} {name}", bytes / unit);
        }
    }
    format!("{bytes} B")
}

impl SetupText {
    pub fn from_config(cfg: &ScanConfig) -> SetupText {
        SetupText {
            min_size: if cfg.min_size <= 1 {
                String::new()
            } else {
                size_text(cfg.min_size)
            },
            max_size: cfg.max_size.map(size_text).unwrap_or_default(),
            only_ext: cfg.only_extensions.join(", "),
            skip_ext: cfg.skip_extensions.join(", "),
            after: cfg
                .modified_after
                .map(|t| fmt_date(t, 0))
                .unwrap_or_default(),
            before: cfg
                .modified_before
                .map(|t| fmt_date(t - 86_400, 0))
                .unwrap_or_default(),
            patterns: cfg.exclude_patterns.join(", "),
        }
    }

    /// Parses the boxes into the configuration, or says which box is wrong.
    pub fn apply(&self, cfg: &mut ScanConfig) -> Result<(), String> {
        let size = |text: &str, what: &str| -> Result<Option<u64>, String> {
            if text.trim().is_empty() {
                Ok(None)
            } else {
                parse_size(text).map(Some).ok_or(format!(
                    "\"{}\" is not a size. Write the {what} size like 500 KB or 2 GB.",
                    text.trim()
                ))
            }
        };
        let date = |text: &str| -> Result<Option<i64>, String> {
            if text.trim().is_empty() {
                Ok(None)
            } else {
                parse_date(text).map(Some).ok_or(format!(
                    "\"{}\" is not a date. Write dates like 2025-03-31.",
                    text.trim()
                ))
            }
        };
        let min = size(&self.min_size, "smallest")?;
        let max = size(&self.max_size, "largest")?;
        if let (Some(a), Some(b)) = (min, max) {
            if a > b {
                return Err("The smallest size is larger than the largest size.".into());
            }
        }
        let after = date(&self.after)?;
        // "Until" includes the whole day that was typed.
        let before = date(&self.before)?.map(|t| t + 86_400);
        if let (Some(a), Some(b)) = (after, before) {
            if a >= b {
                return Err("The \"from\" date is after the \"until\" date.".into());
            }
        }
        // Zero-byte files have their own finder; everything else starts at one byte.
        cfg.min_size = min.unwrap_or(1).max(1);
        cfg.max_size = max;
        cfg.only_extensions = parse_extensions(&self.only_ext);
        cfg.skip_extensions = parse_extensions(&self.skip_ext);
        cfg.modified_after = after;
        cfg.modified_before = before;
        cfg.exclude_patterns = self
            .patterns
            .split([',', ';', '\n'])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        Ok(())
    }
}

/// Invented folders for demo mode.
pub fn demo_settings() -> Settings {
    let mut s = Settings::default();
    s.scan.include = vec![
        PathBuf::from("D:\\Photos"),
        PathBuf::from("D:\\Music"),
        PathBuf::from("C:\\Users\\Alex\\Documents"),
        PathBuf::from("C:\\Users\\Alex\\Downloads"),
    ];
    s.scan.reference = vec![PathBuf::from("E:\\Photo archive")];
    s.scan.exclude = vec![PathBuf::from("D:\\Photos\\Lightroom catalog")];
    s.scan.exclude_patterns = vec!["node_modules".into(), ".git".into()];
    s.scan.min_size = 16 * 1024;
    s
}

/// A scan frozen half-way, for the demo screenshot.
pub fn demo_progress() -> Progress {
    let p = Progress::new();
    p.set_stage_count(4);
    p.set_stage("Listing files", 0, 0);
    p.set_stage("Comparing sizes", 0, 0);
    p.set_stage("Reading the first and last blocks", 0, 0);
    p.set_stage("Reading whole files", 6_184, 48_700_000_000);
    p.add_items(3_907);
    p.add_bytes(30_240_000_000);
    p.set_head_start(236);
    p.set_current(std::path::Path::new(
        "D:\\Photos\\2023\\Lisbon\\IMG_4471.CR3",
    ));
    p
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Scan,
    Reference,
    Exclude,
}

fn text_box(ui: &mut Ui, label: &str, value: &mut String, hint: &str, width: f32) {
    let p = pal(ui);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        ui.label(RichText::new(label).size(12.5).color(p.muted));
        ui.add(
            egui::TextEdit::singleline(value)
                .hint_text(RichText::new(hint).color(p.faint))
                .desired_width(width)
                .margin(egui::Margin::symmetric(8, 6)),
        );
    });
}

impl App {
    pub(super) fn setup_ui(&mut self, ui: &mut Ui, finder: Finder) {
        let p = pal(ui);
        let other_scan = self.running.as_ref().map(|r| r.finder);
        let mut start = false;
        Self::page_header(ui, finder.title(), finder.blurb(), |ui| {
            let label = Icon::PLAY.with("Start scan");
            if widgets::primary_button(ui, &label, other_scan.is_none())
                .on_hover_text("Start scanning (F5)")
                .clicked()
            {
                start = true;
            }
            if let Some(f) = other_scan {
                ui.label(
                    RichText::new(format!("{} is being scanned", f.title()))
                        .size(12.5)
                        .color(p.muted),
                );
            }
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let total = ui.available_width() - 8.0;
                let left = (total * 0.56).floor();
                let right = total - left - 14.0;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    ui.allocate_ui_with_layout(
                        vec2(left, 10.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(left);
                            self.locations_card(ui);
                            ui.add_space(6.0);
                            self.safety_card(ui, finder);
                        },
                    );
                    ui.allocate_ui_with_layout(
                        vec2(right, 10.0),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_width(right);
                            self.options_card(ui, finder);
                            self.filters_card(ui, finder);
                        },
                    );
                });
                ui.add_space(12.0);
            });

        if start {
            let ctx = ui.ctx().clone();
            self.start_scan(&ctx, finder);
        }
    }

    fn locations_card(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 34.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    widgets::section(ui, "Where to look");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let drives = if self.demo {
                            vec![
                                PathBuf::from("C:\\"),
                                PathBuf::from("D:\\"),
                                PathBuf::from("E:\\"),
                            ]
                        } else {
                            sys::drives()
                        };
                        ui.menu_button(Icon::DRIVE.with("Add drive"), |ui| {
                            for d in drives {
                                if ui.button(d.to_string_lossy()).clicked() {
                                    self.add_location(d);
                                    ui.close_menu();
                                }
                            }
                        });
                        if widgets::ghost_button(ui, &Icon::NEW_FOLDER.with("Add folder")).clicked()
                            && !self.demo
                        {
                            if let Some(dirs) = rfd::FileDialog::new()
                                .set_title("Choose folders to scan")
                                .pick_folders()
                            {
                                for d in dirs {
                                    self.add_location(d);
                                }
                            }
                        }
                    });
                },
            );

            let cfg = &self.settings.scan;
            let mut rows: Vec<(PathBuf, Role)> = Vec::new();
            rows.extend(cfg.include.iter().map(|p| (p.clone(), Role::Scan)));
            rows.extend(cfg.reference.iter().map(|p| (p.clone(), Role::Reference)));
            rows.extend(cfg.exclude.iter().map(|p| (p.clone(), Role::Exclude)));

            if rows.is_empty() {
                ui.add_space(14.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(Icon::FOLDER_OPEN.glyph())
                            .size(30.0)
                            .color(p.faint),
                    );
                    ui.label(RichText::new("No folders yet").font(semibold(15.0)));
                    ui.label(
                        RichText::new("Add the folders or drives you want to tidy up.")
                            .color(p.muted),
                    );
                });
                ui.add_space(14.0);
                return;
            }

            let mut change: Option<(usize, Option<Role>)> = None;
            for (i, (path, role)) in rows.iter().enumerate() {
                let (rect, _) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
                ui.painter().rect(
                    rect,
                    CornerRadius::same(9),
                    p.bg,
                    Stroke::new(1.0_f32, p.border),
                    egui::StrokeKind::Inside,
                );
                let mut row = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(vec2(10.0, 4.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                );
                let (icon, color) = match role {
                    Role::Scan => (Icon::FOLDER, p.accent),
                    Role::Reference => (Icon::LOCK, p.good),
                    Role::Exclude => (Icon::REMOVE, p.faint),
                };
                row.label(RichText::new(icon.glyph()).size(16.0).color(color));
                row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::CLOSE, "Remove from the list").clicked() {
                        change = Some((i, None));
                    }
                    let current = match role {
                        Role::Scan => 0,
                        Role::Reference => 1,
                        Role::Exclude => 2,
                    };
                    if let Some(pick) =
                        widgets::segmented(ui, &["Scan", "Reference", "Skip"], current)
                    {
                        change =
                            Some((i, Some([Role::Scan, Role::Reference, Role::Exclude][pick])));
                    }
                    let rest = ui.available_rect_before_wrap();
                    widgets::text_in(
                        ui,
                        rest.shrink2(vec2(4.0, 0.0)),
                        &path.to_string_lossy(),
                        regular(14.0),
                        if *role == Role::Exclude {
                            p.muted
                        } else {
                            p.text
                        },
                    );
                });
            }
            if let Some((i, new_role)) = change {
                let (path, _) = rows.remove(i);
                let cfg = &mut self.settings.scan;
                cfg.include.retain(|p| p != &path);
                cfg.reference.retain(|p| p != &path);
                cfg.exclude.retain(|p| p != &path);
                match new_role {
                    Some(Role::Scan) => cfg.include.push(path),
                    Some(Role::Reference) => cfg.reference.push(path),
                    Some(Role::Exclude) => cfg.exclude.push(path),
                    None => {}
                }
            }
            ui.add_space(2.0);
            ui.label(
                RichText::new("Reference folders are compared but never changed: their files are the originals. Skipped folders are not looked at.")
                    .size(12.5)
                    .color(p.muted),
            );
        });
    }

    pub(super) fn add_location(&mut self, path: PathBuf) {
        let cfg = &mut self.settings.scan;
        let known = cfg
            .include
            .iter()
            .chain(cfg.reference.iter())
            .chain(cfg.exclude.iter())
            .any(|p| {
                crate::util::norm_path(&p.to_string_lossy())
                    == crate::util::norm_path(&path.to_string_lossy())
            });
        if !known {
            cfg.include.push(path);
        }
    }

    fn safety_card(&mut self, ui: &mut Ui, finder: Finder) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(Icon::SHIELD.glyph()).size(16.0).color(p.good));
                widgets::section(ui, "Scanning changes nothing");
            });
            let lines: &[&str] = if finder.is_grouped() {
                &[
                    "A scan only reads. You decide afterwards what happens to each file.",
                    "At least one file of every group is always kept. TwinForge refuses to remove the last copy.",
                    "Windows, Program Files and other system locations are skipped and can never be changed.",
                    "Folder links (junctions) and symbolic links are not followed.",
                ]
            } else {
                &[
                    "A scan only reads. You decide afterwards what happens to each item.",
                    "Each item is checked again right before it is removed; anything that changed is left alone.",
                    "Windows, Program Files and other system locations are skipped and can never be changed.",
                    "Folder links (junctions) and symbolic links are not followed.",
                ]
            };
            for line in lines {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(Icon::CHECK.glyph()).size(12.0).color(p.good));
                    ui.add(egui::Label::new(RichText::new(*line).color(p.muted)).wrap());
                });
            }
        });
    }

    fn options_card(&mut self, ui: &mut Ui, finder: Finder) {
        let p = pal(ui);
        let cfg = &mut self.settings.scan;
        let note = |ui: &mut Ui, text: &str| {
            ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.muted)).wrap());
        };
        if matches!(
            finder,
            Finder::EmptyFiles | Finder::EmptyFolders | Finder::BrokenShortcuts
        ) {
            return;
        }
        widgets::card(ui, |ui| {
            widgets::section(ui, "How to compare");
            match finder {
                Finder::Duplicates => {
                    ui.horizontal(|ui| {
                        ui.label("Fingerprint");
                        let current = usize::from(cfg.hash == HashAlgo::Xxh3);
                        if let Some(i) = widgets::segmented(ui, &["BLAKE3", "XXH3-128"], current) {
                            cfg.hash = [HashAlgo::Blake3, HashAlgo::Xxh3][i];
                        }
                    });
                    widgets::checkbox(
                        ui,
                        &mut cfg.byte_verify,
                        "Also compare byte by byte (slower)",
                    );
                    note(ui, "Files are compared by size first, then by their first and last blocks, then completely. Names and dates do not matter. Hard links to the same file are not counted as copies.");
                }
                Finder::SimilarImages => {
                    ui.horizontal(|ui| {
                        ui.label("Similarity");
                        ui.spacing_mut().slider_width = 150.0;
                        ui.add(
                            egui::Slider::new(&mut cfg.similarity, 60..=100)
                                .suffix("%")
                                .trailing_fill(true),
                        );
                        let word = match cfg.similarity {
                            98..=100 => "near identical",
                            90..=97 => "very similar",
                            80..=89 => "similar",
                            _ => "loosely similar",
                        };
                        ui.label(RichText::new(word).color(p.muted));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Method");
                        let current = usize::from(cfg.image_hash == ImageHash::Gradient);
                        if let Some(i) =
                            widgets::segmented(ui, &["Perceptual", "Gradient"], current)
                        {
                            cfg.image_hash = [ImageHash::Perceptual, ImageHash::Gradient][i];
                        }
                    });
                    widgets::checkbox(
                        ui,
                        &mut cfg.match_rotated,
                        "Also match rotated and mirrored copies",
                    );
                    note(ui, "Finds resized, recompressed and lightly edited copies in JPEG, PNG, GIF, BMP, WebP and TIFF files, and in HEIC, AVIF and camera RAW files when Windows has the codec for them.");
                }
                Finder::SimilarMusic => {
                    ui.label("These tags must agree");
                    ui.horizontal_wrapped(|ui| {
                        widgets::checkbox(ui, &mut cfg.music.title, "Title");
                        widgets::checkbox(ui, &mut cfg.music.artist, "Artist");
                        widgets::checkbox(ui, &mut cfg.music.album, "Album");
                        widgets::checkbox(ui, &mut cfg.music.duration, "Length");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Text match");
                        ui.spacing_mut().slider_width = 150.0;
                        ui.add(
                            egui::Slider::new(&mut cfg.music.fuzziness, 60..=100)
                                .suffix("%")
                                .trailing_fill(true),
                        );
                    });
                    note(ui, "Reads the tags of MP3, FLAC, M4A, OGG, Opus, WAV and WMA files. Spelling, capitals and extras like \"(Remastered)\" are forgiven below 100%.");
                }
                Finder::SameName => {
                    widgets::checkbox(
                        ui,
                        &mut cfg.name_ignore_extension,
                        "Ignore the file type (report.docx = report.pdf)",
                    );
                    note(ui, "Files with the same name can have different contents. Compare before removing anything.");
                }
                Finder::BigFiles => {
                    ui.horizontal(|ui| {
                        ui.label("Show the largest");
                        let options = [50usize, 100, 250, 500];
                        let current = options
                            .iter()
                            .position(|n| *n == cfg.big_count)
                            .unwrap_or(1);
                        if let Some(i) =
                            widgets::segmented(ui, &["50", "100", "250", "500"], current)
                        {
                            cfg.big_count = options[i];
                        }
                    });
                }
                Finder::TempFiles => {
                    note(ui, "Looks for leftovers such as *.tmp, *.temp, *.bak, *.old, ~ editor backups, Office lock files (~$...), Thumbs.db, .DS_Store, crash dumps and partial downloads.");
                }
                _ => {}
            }
        });
        ui.add_space(6.0);
    }

    fn filters_card(&mut self, ui: &mut Ui, finder: Finder) {
        let p = pal(ui);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Filters");
            let w = ((ui.available_width() - 14.0) / 2.0 - 18.0).max(90.0);
            let sizes = !matches!(
                finder,
                Finder::EmptyFiles | Finder::EmptyFolders | Finder::BrokenShortcuts
            );
            if sizes {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    text_box(ui, "Smallest size", &mut self.setup.min_size, "any", w);
                    text_box(ui, "Largest size", &mut self.setup.max_size, "any", w);
                });
            }
            if finder != Finder::EmptyFolders {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    text_box(
                        ui,
                        "Only these types",
                        &mut self.setup.only_ext,
                        "all, or jpg, png",
                        w,
                    );
                    text_box(ui, "Skip these types", &mut self.setup.skip_ext, "none", w);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    text_box(ui, "Changed from", &mut self.setup.after, "YYYY-MM-DD", w);
                    text_box(ui, "Changed until", &mut self.setup.before, "YYYY-MM-DD", w);
                });
            }
            text_box(
                ui,
                "Skip names (wildcards)",
                &mut self.setup.patterns,
                "node_modules, *.iso",
                ui.available_width() - 18.0,
            );
            ui.add_space(4.0);
            let cfg = &mut self.settings.scan;
            widgets::checkbox(ui, &mut cfg.recursive, "Look inside subfolders");
            widgets::checkbox(ui, &mut cfg.skip_hidden, "Skip hidden files and folders");
            widgets::checkbox(
                ui,
                &mut cfg.skip_system,
                "Skip system files and system folders",
            );
            if !cfg.skip_system {
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(Icon::INFO.glyph()).size(12.0).color(p.info));
                    ui.add(
                        egui::Label::new(
                            RichText::new("System locations will be listed, but TwinForge still refuses to change anything inside them.")
                                .size(12.5)
                                .color(p.muted),
                        )
                        .wrap(),
                    );
                });
            }
        });
    }

    // ----------------------------------------------------------------------------------
    // While a scan runs
    // ----------------------------------------------------------------------------------

    pub(super) fn scanning_ui(&mut self, ui: &mut Ui, finder: Finder) {
        let p = pal(ui);
        let Some(running) = &self.running else { return };
        let snap = running.progress.snapshot();
        let stopping = running.progress.cancelled();
        let mut stop = false;
        Self::page_header(ui, finder.title(), finder.blurb(), |ui| {
            if widgets::ghost_button_enabled(ui, &Icon::STOP.with("Stop"), !stopping).clicked() {
                stop = true;
            }
        });
        ui.add_space((ui.available_height() * 0.16).clamp(10.0, 110.0));
        ui.vertical_centered(|ui| {
            ui.set_max_width(640.0);
            widgets::card(ui, |ui| {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Spinner::new().size(34.0).color(p.accent));
                    ui.add_space(8.0);
                    let title = if stopping {
                        "Stopping…".to_string()
                    } else if snap.stage.is_empty() {
                        "Starting…".to_string()
                    } else {
                        snap.stage.clone()
                    };
                    ui.label(RichText::new(title).font(semibold(19.0)));
                    if snap.stage_count > 0 && snap.stage_no > 0 {
                        ui.label(
                            RichText::new(format!(
                                "Step {} of {}",
                                snap.stage_no.min(snap.stage_count),
                                snap.stage_count
                            ))
                            .color(p.muted),
                        );
                    }
                    ui.add_space(10.0);
                    widgets::progress_bar(ui, snap.fraction(), 520.0);
                    ui.add_space(8.0);

                    let mut parts: Vec<String> = Vec::new();
                    if snap.items_total > 0 {
                        parts.push(format!(
                            "{} of {} files",
                            fmt_count(snap.items_done),
                            fmt_count(snap.items_total)
                        ));
                    } else if snap.items_done > 0 {
                        parts.push(format!("{} files", fmt_count(snap.items_done)));
                    }
                    if snap.bytes_total > 0 {
                        parts.push(format!(
                            "{} of {}",
                            fmt_size(snap.bytes_done),
                            fmt_size(snap.bytes_total)
                        ));
                    }
                    if let Some(speed) = snap.speed() {
                        parts.push(format!("{}/s", fmt_size(speed as u64)));
                    }
                    ui.label(RichText::new(parts.join("  ·  ")).font(regular(14.5)));
                    let eta = match snap.eta_secs() {
                        Some(s) if !stopping => format!(
                            "About {} left  ·  {} so far",
                            fmt_duration(s),
                            fmt_duration(snap.elapsed_secs as u64)
                        ),
                        _ => format!("{} so far", fmt_duration(snap.elapsed_secs as u64)),
                    };
                    ui.label(RichText::new(eta).color(p.muted));
                    ui.add_space(6.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(560.0, 18.0), Sense::hover());
                    widgets::text_in_center(ui, rect, &snap.current, regular(12.5), p.faint);
                });
                ui.add_space(6.0);
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new("Nothing is changed while scanning. You can keep using other tabs.")
                    .size(12.5)
                    .color(p.faint),
            );
        });
        if stop {
            self.stop_scan();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_boxes_round_trip() {
        let mut cfg = ScanConfig::default();
        let text = SetupText {
            min_size: "16 KB".into(),
            max_size: "2 GB".into(),
            only_ext: "JPG, .png".into(),
            skip_ext: "tmp".into(),
            after: "2024-01-01".into(),
            before: "2024-12-31".into(),
            patterns: "node_modules, *.iso".into(),
        };
        text.apply(&mut cfg).unwrap();
        assert_eq!(cfg.min_size, 16 * 1024);
        assert_eq!(cfg.max_size, Some(2 * 1024 * 1024 * 1024));
        assert_eq!(cfg.only_extensions, ["jpg", "png"]);
        assert_eq!(cfg.exclude_patterns, ["node_modules", "*.iso"]);
        assert_eq!(cfg.modified_after, parse_date("2024-01-01"));
        assert_eq!(cfg.modified_before, parse_date("2025-01-01"));
        let back = SetupText::from_config(&cfg);
        assert_eq!(back.min_size, "16 KB");
        assert_eq!(size_text(1500), "1500 B");
        assert_eq!(size_text(3 << 30), "3 GB");
        assert_eq!(back.before, "2024-12-31");
        assert_eq!(back.only_ext, "jpg, png");
        let mut again = ScanConfig::default();
        back.apply(&mut again).unwrap();
        assert_eq!(again, cfg);
    }

    #[test]
    fn filter_boxes_report_mistakes() {
        let mut cfg = ScanConfig::default();
        let bad = |f: fn(&mut SetupText)| {
            let mut t = SetupText::default();
            f(&mut t);
            t.apply(&mut ScanConfig::default()).unwrap_err()
        };
        assert!(bad(|t| t.min_size = "huge".into()).contains("not a size"));
        assert!(bad(|t| t.after = "last week".into()).contains("not a date"));
        assert!(bad(|t| {
            t.min_size = "2 GB".into();
            t.max_size = "1 GB".into();
        })
        .contains("larger"));
        assert!(bad(|t| {
            t.after = "2025-01-02".into();
            t.before = "2025-01-01".into();
        })
        .contains("after"));
        // Empty boxes mean "no limit"; the minimum never drops below one byte.
        SetupText::default().apply(&mut cfg).unwrap();
        assert_eq!((cfg.min_size, cfg.max_size), (1, None));
        assert_eq!(demo_settings().scan.include.len(), 4);
        assert_eq!(demo_progress().snapshot().stage_no, 4);
    }
}
