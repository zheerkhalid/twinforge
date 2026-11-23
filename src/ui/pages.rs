//! The History and Settings tabs and all dialogs (confirm, progress, report,
//! about, keyboard shortcuts).

use super::app::App;
use super::theme::{self, pal, pal_ctx, regular, semibold, Icon};
use super::widgets;
use crate::actions::{Method, Outcome, Plan, Report, Status};
use crate::history::{self, Entry, Line};
use crate::model::{ActionKind, Finder};
use crate::settings::ThemeChoice;
use crate::sys::{self, Guard};
use crate::util::{fmt_count, fmt_size};
use eframe::egui::{
    self, vec2, Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke,
    Ui,
};
use std::path::PathBuf;

fn plural(n: usize, one: &str, many: &str) -> String {
    format!(
        "{} {}",
        fmt_count(n as u64),
        if n == 1 { one } else { many }
    )
}

fn item_word(finder: Finder, n: usize) -> String {
    if finder == Finder::EmptyFolders {
        plural(n, "folder", "folders")
    } else {
        plural(n, "file", "files")
    }
}

/// Invented log entries for demo mode.
pub fn demo_history() -> Vec<Entry> {
    let line = |path: &str, size: u64, note: &str, to: Option<&str>| Line {
        path: PathBuf::from(path),
        size,
        note: note.to_string(),
        moved_to: to.map(PathBuf::from),
    };
    vec![
        Entry {
            id: "2026-03-14-18-42-07-120".into(),
            when: "2026-03-14 18:42:07".into(),
            finder: Finder::SimilarImages,
            action: ActionKind::Move,
            done: 5,
            refused: 0,
            failed: 0,
            bytes: 21_934_080,
            lines: vec![
                line(
                    "D:\\Photos\\2023\\Lisbon\\IMG_4471 (1).jpg",
                    5_210_112,
                    "",
                    Some("E:\\Sorted out\\D\\Photos\\2023\\Lisbon\\IMG_4471 (1).jpg"),
                ),
                line(
                    "D:\\Photos\\2023\\Lisbon\\IMG_4471-small.jpg",
                    812_544,
                    "",
                    Some("E:\\Sorted out\\D\\Photos\\2023\\Lisbon\\IMG_4471-small.jpg"),
                ),
                line(
                    "D:\\Photos\\Phone import\\20230917_101502.jpg",
                    4_934_656,
                    "",
                    Some("E:\\Sorted out\\D\\Photos\\Phone import\\20230917_101502.jpg"),
                ),
                line(
                    "D:\\Photos\\Phone import\\20230917_101503.jpg",
                    5_012_480,
                    "",
                    Some("E:\\Sorted out\\D\\Photos\\Phone import\\20230917_101503.jpg"),
                ),
                line(
                    "C:\\Users\\Alex\\Downloads\\harbour-sunset.png",
                    5_964_288,
                    "",
                    Some("E:\\Sorted out\\C\\Users\\Alex\\Downloads\\harbour-sunset.png"),
                ),
            ],
            undone: false,
        },
        Entry {
            id: "2026-03-14-18-31-55-004".into(),
            when: "2026-03-14 18:31:55".into(),
            finder: Finder::Duplicates,
            action: ActionKind::Recycle,
            done: 42,
            refused: 1,
            failed: 1,
            bytes: 3_489_660_928,
            lines: vec![
                line(
                    "C:\\Users\\Alex\\Downloads\\holiday-video-final.mp4",
                    1_288_490_188,
                    "",
                    None,
                ),
                line(
                    "C:\\Users\\Alex\\Downloads\\setup-photo-suite (2).exe",
                    412_316_860,
                    "",
                    None,
                ),
                line(
                    "D:\\Music\\Unsorted\\Northern Lights - Aurora.flac",
                    38_797_312,
                    "",
                    None,
                ),
                line(
                    "C:\\Users\\Alex\\Documents\\Taxes\\2024\\statement.pdf",
                    1_048_576,
                    "Refused: It has changed since the scan. Scan again to be sure.",
                    None,
                ),
                line(
                    "C:\\Users\\Alex\\Documents\\Budget.xlsx",
                    96_256,
                    "Failed: It is in use by another program.",
                    None,
                ),
            ],
            undone: false,
        },
        Entry {
            id: "2026-03-09-09-12-30-871".into(),
            when: "2026-03-09 09:12:30".into(),
            finder: Finder::EmptyFolders,
            action: ActionKind::Delete,
            done: 17,
            refused: 0,
            failed: 0,
            bytes: 0,
            lines: vec![
                line("D:\\Photos\\2019\\New folder", 0, "", None),
                line("D:\\Photos\\2019\\New folder (2)", 0, "", None),
                line("D:\\Music\\Playlists\\old", 0, "", None),
            ],
            undone: false,
        },
        Entry {
            id: "2026-03-02-21-05-11-433".into(),
            when: "2026-03-02 21:05:11".into(),
            finder: Finder::Duplicates,
            action: ActionKind::Move,
            done: 8,
            refused: 0,
            failed: 0,
            bytes: 734_003_200,
            lines: vec![line(
                "D:\\Music\\Unsorted\\Tidewater - Low Sun.mp3",
                9_122_816,
                "",
                Some("E:\\Sorted out\\D\\Music\\Unsorted\\Tidewater - Low Sun.mp3"),
            )],
            undone: true,
        },
    ]
}

/// An invented report for the demo screenshot.
pub fn demo_report(plan: &Plan) -> Report {
    let mut outcomes: Vec<Outcome> = plan
        .groups
        .iter()
        .flat_map(|g| g.items.iter())
        .filter(|i| i.remove)
        .map(|i| Outcome {
            path: i.path.clone(),
            size: i.size,
            status: Status::Done,
            moved_to: None,
            kept: None,
        })
        .collect();
    let n = outcomes.len();
    if n >= 3 {
        outcomes[n - 1].status =
            Status::Refused("It has changed since the scan. Scan again to be sure.".into());
        outcomes[n - 2].status = Status::Failed("It is in use by another program.".into());
    }
    Report {
        finder: plan.finder,
        action: plan.method.kind(),
        planned: n,
        outcomes,
        cancelled: false,
    }
}

fn setting_row(ui: &mut Ui, title: &str, text: &str, control: impl FnOnce(&mut Ui)) {
    let p = pal(ui);
    ui.allocate_ui_with_layout(
        vec2(ui.available_width(), 46.0),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(title).font(regular(14.5)));
                ui.label(RichText::new(text).size(12.5).color(p.muted));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), control);
        },
    );
}

impl App {
    // ----------------------------------------------------------------------------------
    // History
    // ----------------------------------------------------------------------------------

    pub(super) fn history_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let needle = self.search.trim().to_lowercase();
        let total_bytes: u64 = self.history.iter().map(|e| e.bytes).sum();
        let subtitle = if self.history.is_empty() {
            "Every action is written down here.".to_string()
        } else {
            format!(
                "{} · {} dealt with in total",
                plural(self.history.len(), "action", "actions"),
                fmt_size(total_bytes)
            )
        };
        Self::page_header(ui, "History", &subtitle, |_| {});
        ui.label(
            RichText::new("Moves can be undone here. Files sent to the Recycle Bin can be restored from the Recycle Bin.")
                .color(p.muted),
        );
        ui.add_space(4.0);

        let visible: Vec<usize> = (0..self.history.len())
            .filter(|i| {
                let e = &self.history[*i];
                needle.is_empty()
                    || e.action.title().to_lowercase().contains(&needle)
                    || e.finder.title().to_lowercase().contains(&needle)
                    || e.lines
                        .iter()
                        .any(|l| l.path.to_string_lossy().to_lowercase().contains(&needle))
            })
            .collect();
        if visible.is_empty() {
            if needle.is_empty() {
                widgets::empty_state(
                    ui,
                    Icon::HISTORY,
                    p.accent,
                    "Nothing has been changed yet",
                    "When you remove, move or link files, the details appear here.",
                );
            } else {
                widgets::empty_state(
                    ui,
                    Icon::SEARCH,
                    p.muted,
                    "No match",
                    "No past action mentions that text.",
                );
            }
            return;
        }

        let mut undo: Option<usize> = None;
        let mut forget: Option<usize> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width() - 8.0);
                for i in visible {
                    let open = self.history_open == Some(i);
                    let e = &self.history[i];
                    widgets::card(ui, |ui| {
                        let (rect, resp) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                        let mut row = ui.new_child(
                            egui::UiBuilder::new()
                                .max_rect(rect)
                                .layout(Layout::left_to_right(Align::Center)),
                        );
                        let chevron = if open {
                            Icon::CHEVRON_DOWN
                        } else {
                            Icon::CHEVRON_RIGHT
                        };
                        row.label(RichText::new(chevron.glyph()).size(12.0).color(p.faint));
                        let (icon, color) = match e.action {
                            ActionKind::Recycle => (Icon::DELETE, p.accent),
                            ActionKind::Delete => (Icon::DELETE, p.danger),
                            ActionKind::Move => (Icon::MOVE, p.info),
                            _ => (Icon::LINK, p.good),
                        };
                        row.label(RichText::new(icon.glyph()).size(17.0).color(color));
                        row.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            ui.label(
                                RichText::new(format!(
                                    "{} · {}",
                                    e.action.title(),
                                    e.finder.title()
                                ))
                                .font(semibold(14.5)),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {}",
                                    e.when,
                                    item_word(e.finder, e.done),
                                    fmt_size(e.bytes)
                                ))
                                .size(12.5)
                                .color(p.muted),
                            );
                        });
                        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::CLOSE, "Remove from the log")
                                .clicked()
                            {
                                forget = Some(i);
                            }
                            if e.undoable() > 0
                                && widgets::ghost_button(ui, &Icon::UNDO.with("Undo move"))
                                    .on_hover_text("Move the files back to where they were")
                                    .clicked()
                            {
                                undo = Some(i);
                            }
                            if e.undone {
                                widgets::badge(ui, "UNDONE", p.muted);
                            }
                            if e.failed > 0 {
                                widgets::badge(ui, &format!("{} failed", e.failed), p.danger);
                            }
                            if e.refused > 0 {
                                widgets::badge(ui, &format!("{} refused", e.refused), p.warn);
                            }
                        });
                        if resp.clicked() {
                            self.history_open = if open { None } else { Some(i) };
                        }
                        if open {
                            ui.add_space(2.0);
                            widgets::divider(ui);
                            for l in e.lines.iter().take(300) {
                                let (rect, _) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 22.0),
                                    Sense::hover(),
                                );
                                let size_rect = Rect::from_min_max(
                                    egui::pos2(rect.right() - 80.0, rect.top()),
                                    rect.max,
                                );
                                widgets::text_in_right(
                                    ui,
                                    size_rect,
                                    &fmt_size(l.size),
                                    regular(12.5),
                                    p.faint,
                                );
                                let ok = l.note.is_empty();
                                let text = match (&l.moved_to, ok) {
                                    (Some(to), true) => format!(
                                        "{}   →   {}",
                                        l.path.to_string_lossy(),
                                        to.to_string_lossy()
                                    ),
                                    (_, true) => l.path.to_string_lossy().into_owned(),
                                    (_, false) => {
                                        format!("{}   ·   {}", l.path.to_string_lossy(), l.note)
                                    }
                                };
                                let text_rect = Rect::from_min_max(
                                    rect.min + vec2(22.0, 0.0),
                                    egui::pos2(rect.right() - 90.0, rect.bottom()),
                                );
                                ui.painter().text(
                                    rect.left_center() + vec2(6.0, 0.0),
                                    egui::Align2::CENTER_CENTER,
                                    if ok {
                                        Icon::CHECK.glyph()
                                    } else {
                                        Icon::WARNING.glyph()
                                    },
                                    regular(11.0),
                                    if ok { p.good } else { p.warn },
                                );
                                widgets::text_in(
                                    ui,
                                    text_rect,
                                    &text,
                                    regular(13.0),
                                    if ok { p.text } else { p.muted },
                                );
                            }
                            if e.lines.len() > 300 {
                                ui.label(
                                    RichText::new(format!(
                                        "…and {} more",
                                        fmt_count((e.lines.len() - 300) as u64)
                                    ))
                                    .color(p.faint),
                                );
                            }
                        }
                    });
                    ui.add_space(2.0);
                }
            });

        if let Some(i) = undo {
            if self.demo {
                self.toast("Demo mode: nothing is changed on this PC.", false);
            } else {
                let app_dir = self.app_dir.clone();
                match history::undo_moves(&app_dir, &mut self.history[i], &Guard::real()) {
                    Ok(r) if r.problems.is_empty() => {
                        self.toast(
                            format!("{} moved back.", plural(r.restored, "file", "files")),
                            false,
                        );
                    }
                    Ok(r) => {
                        let first = r
                            .problems
                            .first()
                            .map(|(p, why)| format!("{}: {why}", p.to_string_lossy()))
                            .unwrap_or_default();
                        self.toast(
                            format!(
                                "{} moved back, {} could not be. {first}",
                                r.restored,
                                r.problems.len()
                            ),
                            true,
                        );
                    }
                    Err(e) => self.toast(e, true),
                }
            }
        }
        if let Some(i) = forget {
            if self.demo {
                self.history.remove(i);
            } else {
                match history::forget(&self.app_dir, &self.history[i].id) {
                    Ok(()) => {
                        self.history.remove(i);
                    }
                    Err(e) => self.toast(e, true),
                }
            }
            self.history_open = None;
        }
    }

    // ----------------------------------------------------------------------------------
    // Settings
    // ----------------------------------------------------------------------------------

    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        Self::page_header(ui, "Settings", "Saved automatically.", |_| {});
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(760.0);
                widgets::card(ui, |ui| {
                    widgets::section(ui, "Appearance");
                    setting_row(ui, "Theme", "Dark, light, or whatever Windows uses.", |ui| {
                        let current = match self.settings.theme {
                            ThemeChoice::Dark => 0,
                            ThemeChoice::Light => 1,
                            ThemeChoice::System => 2,
                        };
                        if let Some(i) = widgets::segmented(ui, &["Dark", "Light", "System"], current)
                        {
                            self.settings.theme =
                                [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System][i];
                            theme::apply_choice(ui.ctx(), self.settings.theme);
                        }
                    });
                    setting_row(
                        ui,
                        "Picture thumbnails",
                        "Show small previews next to pictures in the results.",
                        |ui| {
                            if widgets::toggle(ui, self.settings.thumbnails, true).clicked() {
                                self.settings.thumbnails = !self.settings.thumbnails;
                            }
                        },
                    );
                });
                ui.add_space(4.0);
                widgets::card(ui, |ui| {
                    widgets::section(ui, "Removing files");
                    setting_row(
                        ui,
                        "Main action",
                        "What the big button and the Delete key do.",
                        |ui| {
                            let options = [ActionKind::Recycle, ActionKind::Move, ActionKind::Delete];
                            let current = options
                                .iter()
                                .position(|a| *a == self.settings.default_action)
                                .unwrap_or(0);
                            if let Some(i) = widgets::segmented(
                                ui,
                                &["Recycle Bin", "Move to a folder", "Delete for good"],
                                current,
                            ) {
                                self.settings.default_action = options[i];
                            }
                        },
                    );
                    setting_row(
                        ui,
                        "Compare once more before removing",
                        "Exact duplicates are compared byte by byte with the copy that stays, right before they go.",
                        |ui| {
                            if widgets::toggle(ui, self.settings.verify_before_remove, true).clicked()
                            {
                                self.settings.verify_before_remove =
                                    !self.settings.verify_before_remove;
                            }
                        },
                    );
                });
                ui.add_space(4.0);
                widgets::card(ui, |ui| {
                    widgets::section(ui, "Scanning");
                    setting_row(
                        ui,
                        "Remember fingerprints",
                        "Files that have not changed since the last scan are not read again.",
                        |ui| {
                            if widgets::toggle(ui, self.settings.use_cache, true).clicked() {
                                self.settings.use_cache = !self.settings.use_cache;
                            }
                        },
                    );
                    let cache = self.app_dir.join("hash-cache.bin");
                    let size = if self.demo {
                        2_311_168
                    } else {
                        std::fs::metadata(&cache).map(|m| m.len()).unwrap_or(0)
                    };
                    setting_row(
                        ui,
                        "Fingerprint cache",
                        &format!("Currently {} on disk.", fmt_size(size)),
                        |ui| {
                            if widgets::ghost_button_enabled(ui, "Clear cache", size > 0).clicked() {
                                if self.demo {
                                    self.toast("Demo mode: nothing is changed on this PC.", false);
                                } else {
                                    match std::fs::remove_file(&cache) {
                                        Ok(()) => self.toast("Cache cleared.", false),
                                        Err(e) => self.toast(e.to_string(), true),
                                    }
                                }
                            }
                        },
                    );
                    let hidden = if self.demo { 3 } else { self.ignore.len() };
                    setting_row(
                        ui,
                        "Hidden groups",
                        &match hidden {
                            0 => "Groups you mark as \"not a match\" are left out of later scans.".to_string(),
                            1 => "1 group you marked as \"not a match\" is left out of scans.".to_string(),
                            n => format!(
                                "{n} groups you marked as \"not a match\" are left out of scans."
                            ),
                        },
                        |ui| {
                            if widgets::ghost_button_enabled(ui, "Show them again", hidden > 0)
                                .clicked()
                            {
                                if self.demo {
                                    self.toast("Demo mode: nothing is changed on this PC.", false);
                                } else {
                                    let before = self.ignore.clone();
                                    self.ignore.clear();
                                    match self.ignore.save(&self.app_dir) {
                                        Ok(()) => self.toast(
                                            "Hidden groups will be listed again at the next scan.",
                                            false,
                                        ),
                                        Err(e) => {
                                            self.ignore = before;
                                            self.toast(e, true);
                                        }
                                    }
                                }
                            }
                        },
                    );
                });
                ui.add_space(4.0);
                widgets::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(Icon::SHIELD.glyph()).size(16.0).color(p.good));
                        widgets::section(ui, "Safety rules that cannot be switched off");
                    });
                    for line in [
                        "At least one file of every group of copies always stays.",
                        "Every file is checked again right before it is touched: same size, same date, and the kept copy must still be there.",
                        "Windows, Program Files, ProgramData and other system locations are never changed.",
                        "Junctions and symbolic links are never followed or removed.",
                        "Files in reference folders are never selected or changed.",
                    ] {
                        ui.horizontal_top(|ui| {
                            ui.label(RichText::new(Icon::CHECK.glyph()).size(12.0).color(p.good));
                            ui.add(egui::Label::new(RichText::new(line).color(p.muted)).wrap());
                        });
                    }
                });
                ui.add_space(4.0);
                widgets::card(ui, |ui| {
                    setting_row(
                        ui,
                        "TwinForge",
                        &format!(
                            "Version {} · Developed by Zheer Khalid (zheer.dev)",
                            env!("CARGO_PKG_VERSION")
                        ),
                        |ui| {
                            if widgets::ghost_button(ui, "About").clicked() {
                                self.about_open = true;
                            }
                            if widgets::ghost_button(ui, "Keyboard shortcuts").clicked() {
                                self.keys_open = true;
                            }
                        },
                    );
                });
                ui.add_space(12.0);
            });
        self.persist_settings();
    }

    // ----------------------------------------------------------------------------------
    // Dialogs
    // ----------------------------------------------------------------------------------

    pub(super) fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else { return };
        let plan = &confirm.plan;
        let p = pal_ctx(ctx);
        let (count, bytes) = (plan.count(), plan.bytes());
        let what = item_word(plan.finder, count);
        let kind = plan.method.kind();
        let (title, intro, button) = match &plan.method {
            Method::Recycle => (
                format!("Move {what} to the Recycle Bin?"),
                "You can restore them from the Recycle Bin until you empty it.".to_string(),
                format!("Move {what} to the Recycle Bin"),
            ),
            Method::Delete => (
                format!("Delete {what} permanently?"),
                "This cannot be undone. The files do not go to the Recycle Bin.".to_string(),
                format!("Delete {what} for good"),
            ),
            Method::MoveTo(dest) => (
                format!("Move {what}?"),
                format!(
                    "They go to {} with their folder structure. You can undo this from History.",
                    dest.to_string_lossy()
                ),
                format!("Move {what}"),
            ),
            Method::HardLink => (
                format!("Replace {what} with hard links?"),
                "Each file stays where it is but shares its storage with the kept copy. Editing one then changes both.".to_string(),
                format!("Replace {what}"),
            ),
            Method::SymLink => (
                format!("Replace {what} with symbolic links?"),
                "Each file becomes a pointer to the kept copy. If the kept copy is moved or deleted later, the links stop working.".to_string(),
                format!("Replace {what}"),
            ),
        };
        let orphans = plan.groups_without_keeper();
        let mut decision: Option<bool> = None;
        let modal = egui::Modal::new(egui::Id::new("confirm-action"))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(600.0);
                ui.label(RichText::new(&title).font(semibold(19.0)));
                ui.add_space(2.0);
                ui.add(egui::Label::new(RichText::new(&intro).color(p.muted)).wrap());
                ui.add_space(8.0);

                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 46.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 28.0;
                        widgets::stat(ui, &fmt_count(count as u64), if plan.finder == Finder::EmptyFolders { "folders" } else { "files" }, p.text);
                        widgets::stat(ui, &fmt_size(bytes), "in total", p.accent);
                        if plan.finder.is_grouped() {
                            widgets::stat(
                                ui,
                                &fmt_count(plan.groups.len() as u64),
                                "groups, each keeps a copy",
                                p.good,
                            );
                        }
                    },
                );
                ui.add_space(6.0);
                if orphans > 0 {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new(Icon::WARNING.glyph()).color(p.warn));
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "In {} every file is selected. TwinForge will not touch those groups: one copy must stay.",
                                    plural(orphans, "group", "groups")
                                ))
                                .color(p.warn),
                            )
                            .wrap(),
                        );
                    });
                }
                Frame::new()
                    .fill(p.bg)
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        egui::ScrollArea::vertical()
                            .max_height(230.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                let items = plan
                                    .groups
                                    .iter()
                                    .flat_map(|g| g.items.iter())
                                    .filter(|i| i.remove);
                                for item in items.take(400) {
                                    let (rect, _) = ui.allocate_exact_size(
                                        vec2(ui.available_width() - 6.0, 21.0),
                                        Sense::hover(),
                                    );
                                    widgets::text_in_right(
                                        ui,
                                        Rect::from_min_max(
                                            egui::pos2(rect.right() - 76.0, rect.top()),
                                            rect.max,
                                        ),
                                        &fmt_size(item.size),
                                        regular(12.5),
                                        p.faint,
                                    );
                                    widgets::text_in(
                                        ui,
                                        Rect::from_min_max(
                                            rect.min,
                                            egui::pos2(rect.right() - 84.0, rect.bottom()),
                                        ),
                                        &item.path.to_string_lossy(),
                                        regular(13.0),
                                        p.text,
                                    );
                                }
                                if count > 400 {
                                    ui.label(
                                        RichText::new(format!(
                                            "…and {} more",
                                            fmt_count((count - 400) as u64)
                                        ))
                                        .color(p.faint),
                                    );
                                }
                            });
                    });
                ui.add_space(6.0);
                ui.horizontal_top(|ui| {
                    ui.label(RichText::new(Icon::SHIELD.glyph()).size(13.0).color(p.good));
                    let text = if plan.finder == Finder::Duplicates && plan.verify_content {
                        "Each file is compared with its kept copy once more, byte by byte, right before it goes. Anything that changed is left alone."
                    } else {
                        "Each file is checked once more right before it goes. Anything that changed since the scan is left alone."
                    };
                    ui.add(
                        egui::Label::new(RichText::new(text).size(12.5).color(p.muted)).wrap(),
                    );
                });
                ui.add_space(10.0);
                widgets::right_row(ui, 36.0, |ui| {
                    let clicked = if kind == ActionKind::Delete {
                        widgets::danger_button(ui, &button, true).clicked()
                    } else {
                        widgets::primary_button(ui, &button, true).clicked()
                    };
                    if clicked {
                        decision = Some(true);
                    }
                    if widgets::ghost_button(ui, "Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        match decision {
            Some(true) => self.confirmed(ctx),
            Some(false) => self.confirm = None,
            None => {}
        }
    }

    pub(super) fn job_dialog(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let p = pal_ctx(ctx);
        let snap = job.progress.snapshot();
        let stopping = job.progress.cancelled();
        let mut stop = false;
        egui::Modal::new(egui::Id::new("job-progress"))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(
                    RichText::new(match job.action {
                        ActionKind::Recycle => "Moving to the Recycle Bin…",
                        ActionKind::Delete => "Deleting…",
                        ActionKind::Move => "Moving…",
                        _ => "Creating links…",
                    })
                    .font(semibold(19.0)),
                );
                ui.add_space(8.0);
                widgets::progress_bar(ui, snap.fraction(), 460.0);
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} of {} · {}",
                        fmt_count(snap.items_done),
                        fmt_count(snap.items_total),
                        fmt_size(snap.bytes_done)
                    ))
                    .color(p.muted),
                );
                let (rect, _) = ui.allocate_exact_size(vec2(460.0, 18.0), Sense::hover());
                widgets::text_in(ui, rect, &snap.current, regular(12.5), p.faint);
                ui.add_space(8.0);
                widgets::right_row(ui, 36.0, |ui| {
                    if widgets::ghost_button_enabled(
                        ui,
                        if stopping { "Stopping…" } else { "Stop" },
                        !stopping,
                    )
                    .clicked()
                    {
                        stop = true;
                    }
                });
            });
        if stop {
            job.progress.cancel();
        }
    }

    pub(super) fn report_dialog(&mut self, ctx: &egui::Context) {
        let Some(report) = &self.report else { return };
        let p = pal_ctx(ctx);
        let (done, refused, failed) = (report.done(), report.refused(), report.failed());
        let title = if report.cancelled {
            "Stopped"
        } else if refused + failed == 0 {
            "All done"
        } else if done == 0 {
            "Nothing was changed"
        } else {
            "Done, with exceptions"
        };
        let verb = match report.action {
            ActionKind::Recycle => "moved to the Recycle Bin",
            ActionKind::Delete => "deleted",
            ActionKind::Move => "moved",
            _ => "replaced with links",
        };
        let can_undo = report.action == ActionKind::Move
            && self
                .history
                .first()
                .map(|e| e.undoable() > 0)
                .unwrap_or(false);
        let mut close = false;
        let mut undo = false;
        let modal = egui::Modal::new(egui::Id::new("action-report"))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(600.0);
                ui.horizontal(|ui| {
                    let (icon, color) = if refused + failed == 0 && !report.cancelled {
                        (Icon::DONE, p.good)
                    } else {
                        (Icon::INFO, p.warn)
                    };
                    ui.label(RichText::new(icon.glyph()).size(22.0).color(color));
                    ui.label(RichText::new(title).font(semibold(19.0)));
                });
                ui.add_space(6.0);
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 46.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 28.0;
                        widgets::stat(ui, &fmt_count(done as u64), verb, p.good);
                        widgets::stat(ui, &fmt_size(report.bytes_done()), "dealt with", p.accent);
                        if refused > 0 {
                            widgets::stat(
                                ui,
                                &fmt_count(refused as u64),
                                "left alone for safety",
                                p.warn,
                            );
                        }
                        if failed > 0 {
                            widgets::stat(ui, &fmt_count(failed as u64), "failed", p.danger);
                        }
                    },
                );
                if report.cancelled {
                    ui.label(
                        RichText::new(format!(
                            "Stopped after {} of {}. The rest was not touched.",
                            fmt_count(report.outcomes.len() as u64),
                            fmt_count(report.planned as u64)
                        ))
                        .color(p.muted),
                    );
                }
                let problems: Vec<&Outcome> = report
                    .outcomes
                    .iter()
                    .filter(|o| matches!(o.status, Status::Refused(_) | Status::Failed(_)))
                    .collect();
                if !problems.is_empty() {
                    ui.add_space(6.0);
                    Frame::new()
                        .fill(p.bg)
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .max_height(240.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for o in problems.iter().take(300) {
                                        let (why, color) = match &o.status {
                                            Status::Refused(r) => (r.as_str(), p.warn),
                                            Status::Failed(r) => (r.as_str(), p.danger),
                                            _ => ("", p.muted),
                                        };
                                        let (rect, _) = ui.allocate_exact_size(
                                            vec2(ui.available_width() - 6.0, 40.0),
                                            Sense::hover(),
                                        );
                                        let (top, bottom) = rect.split_top_bottom_at_fraction(0.5);
                                        widgets::text_in(
                                            ui,
                                            top,
                                            &o.path.to_string_lossy(),
                                            regular(13.0),
                                            p.text,
                                        );
                                        widgets::text_in(ui, bottom, why, regular(12.5), color);
                                    }
                                });
                        });
                }
                ui.add_space(10.0);
                widgets::right_row(ui, 36.0, |ui| {
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                    if can_undo
                        && widgets::ghost_button(ui, &Icon::UNDO.with("Undo move")).clicked()
                    {
                        undo = true;
                    }
                    ui.label(
                        RichText::new("The details are kept in History.")
                            .size(12.5)
                            .color(p.faint),
                    );
                });
            });
        if undo {
            let app_dir = self.app_dir.clone();
            if let Some(entry) = self.history.first_mut() {
                match history::undo_moves(&app_dir, entry, &Guard::real()) {
                    Ok(r) => {
                        let n = r.problems.len();
                        self.toast(
                            if n == 0 {
                                format!(
                                    "{} moved back. Scan again to see them.",
                                    plural(r.restored, "file", "files")
                                )
                            } else {
                                format!("{} moved back, {n} could not be.", r.restored)
                            },
                            n > 0,
                        );
                    }
                    Err(e) => self.toast(e, true),
                }
            }
            close = true;
        }
        if close || modal.should_close() {
            self.report = None;
        }
    }

    pub(super) fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("about"))
            .frame(
                Frame::new()
                    .fill(p.card)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(16))
                    .inner_margin(Margin::same(28)),
            )
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("TwinForge").font(semibold(26.0)));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                            .color(p.muted),
                    );
                    ui.add_space(10.0);
                    ui.label("Finds duplicate files, similar pictures and other clutter,");
                    ui.label("and never removes the last copy of anything.");
                    ui.add_space(10.0);
                    ui.label(RichText::new("Developed by Zheer Khalid").font(semibold(15.0)));
                    ui.add_space(2.0);
                    if ui
                        .link(RichText::new("zheer.dev").color(p.accent))
                        .clicked()
                    {
                        let _ = sys::open_path("https://zheer.dev");
                    }
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Free and open source under the MIT License.")
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.label(
                        RichText::new("Built with Rust, egui, BLAKE3, xxHash, image and lofty.")
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.add_space(14.0);
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.about_open = false;
        }
    }

    pub(super) fn keys_dialog(&mut self, ctx: &egui::Context) {
        if !self.keys_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("keys"))
            .frame(widgets::modal_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Keyboard shortcuts").font(semibold(19.0)));
                ui.add_space(6.0);
                for (keys, what) in [
                    ("Ctrl+1 … Ctrl+9", "Switch between the finders"),
                    ("F5", "Start the scan, or scan again"),
                    ("Ctrl+F", "Filter the results"),
                    ("Ctrl+A", "Select all but one file in every group"),
                    ("Ctrl+I", "Invert the selection"),
                    ("Ctrl+D", "Clear the selection"),
                    ("Up / Down", "Move through the files"),
                    ("Left / Right", "Close or open the highlighted group"),
                    ("Space", "Tick or untick the highlighted file"),
                    ("Enter", "Open the highlighted file"),
                    ("Delete", "Run the main action on the selection"),
                    ("Shift+Delete", "Delete the selection permanently"),
                    ("Ctrl+E", "Export the list"),
                    ("Esc", "Close a dialog or clear the filter"),
                    ("F1", "Show this list"),
                ] {
                    let (rect, _) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                    ui.painter().text(
                        rect.left_center(),
                        egui::Align2::LEFT_CENTER,
                        keys,
                        semibold(13.5),
                        p.accent,
                    );
                    ui.painter().text(
                        rect.left_center() + vec2(150.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        what,
                        regular(14.0),
                        p.text,
                    );
                }
                ui.add_space(8.0);
                widgets::right_row(ui, 36.0, |ui| {
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.keys_open = false;
        }
    }
}
