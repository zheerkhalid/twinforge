//! Application state, background work plumbing and the window frame
//! (header, navigation rail, dialogs, toasts).

use super::results_view::{ResultsCtx, ResultsRequest, ResultsState};
use super::scan_view::SetupText;
use super::theme::{self, pal, pal_ctx, regular, semibold, Icon};
use super::widgets;
use crate::actions::{self, Method, Plan, Report};
use crate::history::{self, Entry};
use crate::model::{ActionKind, ExportFormat, Finder, Progress, ScanResult};
use crate::settings::Settings;
use crate::sys::{self, Guard};
use crate::util::{fmt_count, fmt_size};
use crate::{demo, engine, export};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Rect, RichText,
    Sense, Stroke, TextureHandle, Ui,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// How the app was started.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Show invented data and never change anything.
    pub demo: bool,
    /// Render one frame of the chosen view to this PNG, then exit.
    pub screenshot: Option<PathBuf>,
    /// Which tab / dialog to open first: a finder key (`duplicates`, `pictures`,
    /// `music`, `names`, `empty-files`, `empty-folders`, `shortcuts`, `big`,
    /// `temp`) or `setup`, `scanning`, `confirm`, `report`, `history`,
    /// `settings`, `about`, `keys`.
    pub view: Option<String>,
    /// A named state of the results view (`compare`, `selected`, ...).
    pub pose: Option<String>,
    /// Force the light theme (for screenshots).
    pub light: bool,
    /// Folders given on the command line: open with these instead of the remembered ones.
    pub paths: Vec<PathBuf>,
    /// Self-test: keep settings, cache and history in this folder instead of
    /// the real one.
    pub app_dir: Option<PathBuf>,
    /// Self-test: scan the first of `paths` for duplicates, select with a
    /// rule, delete, and write what happened to this shared log, without any
    /// clicks. Closes the window when done.
    pub autopilot: Option<Arc<Mutex<Vec<String>>>>,
}

/// Drives the window through one complete scan-select-delete round.
pub struct Autopilot {
    log: Arc<Mutex<Vec<String>>>,
    step: u8,
    frames: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Finder(Finder),
    History,
    Settings,
}

pub enum Msg {
    ScanDone(u64, Finder, Box<ScanResult>),
    ActionDone(Finder, Box<Report>),
}

/// A scan that is running in the background.
pub struct Running {
    pub finder: Finder,
    pub progress: Arc<Progress>,
    generation: u64,
}

/// An action (delete, move, link) that is running in the background.
pub struct Job {
    pub action: ActionKind,
    pub progress: Arc<Progress>,
}

/// An action waiting for the user's "yes".
pub struct Confirm {
    pub plan: Plan,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub born: Instant,
}

struct Shot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

pub struct App {
    pub(super) app_dir: PathBuf,
    pub(super) demo: bool,
    pub(super) settings: Settings,
    pub(super) saved_settings: Settings,
    pub(super) setup: SetupText,
    pub(super) tab: Tab,
    pub(super) tz_offset: i64,

    pub(super) results: HashMap<Finder, ResultsState>,
    pub(super) running: Option<Running>,
    generation: u64,
    pub(super) job: Option<Job>,
    pub(super) confirm: Option<Confirm>,
    pub(super) report: Option<Report>,
    pub(super) history: Vec<Entry>,
    pub(super) history_open: Option<usize>,

    pub(super) search: String,
    focus_search: bool,
    pub(super) about_open: bool,
    pub(super) keys_open: bool,
    pub(super) toasts: Vec<Toast>,

    pub(super) tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub(super) logo: Option<TextureHandle>,
    shot: Option<Shot>,
    auto: Option<Autopilot>,
}

/// Starts the window.
pub fn run(options: Options, icon: egui::IconData) -> eframe::Result<()> {
    sys::set_demo(options.demo);
    let viewport = egui::ViewportBuilder::default()
        .with_title(if options.demo {
            "TwinForge (demo)"
        } else {
            "TwinForge"
        })
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1000.0, 640.0])
        .with_icon(icon.clone());
    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "TwinForge",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, options, icon)))),
    )
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, options: Options, icon: egui::IconData) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let app_dir = if options.demo {
            // Nothing is read from or written to this folder in demo mode; an
            // invented path keeps real names out of screenshots.
            PathBuf::from("C:\\Users\\Alex\\AppData\\Local\\TwinForge")
        } else {
            options.app_dir.clone().unwrap_or_else(sys::app_dir)
        };
        let mut settings = if options.demo {
            super::scan_view::demo_settings()
        } else {
            Settings::load(&app_dir)
        };
        let saved_settings = settings.clone();
        if options.light {
            settings.theme = crate::settings::ThemeChoice::Light;
        }
        if !options.paths.is_empty() {
            settings.scan.include = options.paths.clone();
        }
        theme::apply_choice(ctx, settings.theme);

        let logo = (icon.width > 0).then(|| {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ctx.load_texture("logo", image, egui::TextureOptions::LINEAR)
        });
        let (tx, rx) = channel();
        let mut app = App {
            history: if options.demo {
                super::pages::demo_history()
            } else {
                history::load_all(&app_dir)
            },
            app_dir,
            demo: options.demo,
            setup: SetupText::from_config(&settings.scan),
            settings,
            saved_settings,
            tab: Tab::Finder(Finder::Duplicates),
            tz_offset: if options.demo {
                0
            } else {
                sys::local_offset_secs()
            },
            results: HashMap::new(),
            running: None,
            generation: 0,
            job: None,
            confirm: None,
            report: None,
            history_open: None,
            search: String::new(),
            focus_search: false,
            about_open: false,
            keys_open: false,
            toasts: Vec::new(),
            tx,
            rx,
            logo,
            shot: options.screenshot.clone().map(|path| Shot {
                path,
                frames: 0,
                requested: false,
            }),
            auto: options.autopilot.clone().map(|log| Autopilot {
                log,
                step: 0,
                frames: 0,
            }),
        };
        if app.demo {
            for finder in Finder::ALL {
                app.results
                    .insert(finder, ResultsState::new(demo::result(finder)));
            }
        }
        app.apply_view(
            options.view.as_deref().unwrap_or("duplicates"),
            options.pose.as_deref(),
        );
        app
    }

    /// Opens the tab / dialog named on the command line.
    fn apply_view(&mut self, view: &str, pose: Option<&str>) {
        if let Some(finder) = Finder::from_key(view) {
            self.tab = Tab::Finder(finder);
        }
        match view {
            "history" => {
                self.tab = Tab::History;
                self.history_open = Some(0);
            }
            "settings" => self.tab = Tab::Settings,
            "about" => self.about_open = true,
            "keys" => self.keys_open = true,
            "setup" => {
                self.results.remove(&Finder::Duplicates);
            }
            "setup-pictures" => {
                self.tab = Tab::Finder(Finder::SimilarImages);
                self.results.remove(&Finder::SimilarImages);
            }
            "scanning" if self.demo => {
                self.results.remove(&Finder::Duplicates);
                self.running = Some(Running {
                    finder: Finder::Duplicates,
                    progress: Arc::new(super::scan_view::demo_progress()),
                    generation: 0,
                });
            }
            "confirm" | "report" if self.demo => {
                if let Some(state) = self.results.get_mut(&Finder::Duplicates) {
                    state.pose("selected");
                    let plan = Plan::from_result(&state.result, Method::Recycle, true);
                    if view == "confirm" {
                        self.confirm = Some(Confirm { plan });
                    } else {
                        self.report = Some(super::pages::demo_report(&plan));
                    }
                }
            }
            _ => {}
        }
        if let (Some(pose), Tab::Finder(finder)) = (pose, self.tab) {
            if let Some(state) = self.results.get_mut(&finder) {
                state.pose(pose);
            }
        }
    }

    // ----------------------------------------------------------------------------------
    // Scans
    // ----------------------------------------------------------------------------------

    pub(super) fn start_scan(&mut self, ctx: &egui::Context, finder: Finder) {
        if let Some(r) = &self.running {
            self.toast(
                format!(
                    "The {} scan is still running. Stop it first.",
                    r.finder.title().to_lowercase()
                ),
                true,
            );
            return;
        }
        if self.job.is_some() {
            return;
        }
        if let Err(problem) = self.setup.apply(&mut self.settings.scan) {
            self.toast(problem, true);
            return;
        }
        if self.demo {
            self.results
                .insert(finder, ResultsState::new(demo::result(finder)));
            return;
        }
        if self.settings.scan.include.is_empty() && self.settings.scan.reference.is_empty() {
            self.toast("Add at least one folder or drive to scan.", true);
            return;
        }
        self.persist_settings();
        self.generation += 1;
        let generation = self.generation;
        let progress = Arc::new(Progress::new());
        let cfg = self.settings.scan_config(&self.app_dir);
        let (tx, p, c) = (self.tx.clone(), progress.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = engine::run(finder, &cfg, &p);
            let _ = tx.send(Msg::ScanDone(generation, finder, Box::new(result)));
            c.request_repaint();
        });
        self.results.remove(&finder);
        self.running = Some(Running {
            finder,
            progress,
            generation,
        });
    }

    pub(super) fn stop_scan(&mut self) {
        if let Some(r) = &self.running {
            r.progress.cancel();
            if self.demo {
                self.running = None;
            }
        }
    }

    /// Writes the settings when they differ from what is on disk.
    pub(super) fn persist_settings(&mut self) {
        if self.demo || self.settings == self.saved_settings {
            return;
        }
        match self.settings.save(&self.app_dir) {
            Ok(()) => self.saved_settings = self.settings.clone(),
            Err(e) => self.toast(e, true),
        }
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            born: Instant::now(),
        });
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::ScanDone(generation, finder, result) => {
                    if self.running.as_ref().map(|r| r.generation) != Some(generation) {
                        continue;
                    }
                    self.running = None;
                    let text = if result.cancelled {
                        "Scan stopped. The list shows what was found so far.".to_string()
                    } else if result.groups.is_empty() {
                        format!("{}: nothing found.", finder.title())
                    } else {
                        format!(
                            "{}: {} found, {} could be freed.",
                            finder.title(),
                            fmt_count(result.item_count() as u64),
                            fmt_size(result.wasted_bytes())
                        )
                    };
                    self.toast(text, false);
                    self.results.insert(finder, ResultsState::new(*result));
                }
                Msg::ActionDone(finder, report) => {
                    self.job = None;
                    let entry = Entry::from_report(&report);
                    if let Err(e) = history::save(&self.app_dir, &entry) {
                        self.toast(e, true);
                    }
                    self.history.insert(0, entry);
                    if let Some(state) = self.results.get_mut(&finder) {
                        state.remove_paths(&report.gone_paths());
                    }
                    self.report = Some(*report);
                }
            }
        }
    }

    // ----------------------------------------------------------------------------------
    // Actions
    // ----------------------------------------------------------------------------------

    fn handle_request(&mut self, ctx: &egui::Context, finder: Finder, request: ResultsRequest) {
        match request {
            ResultsRequest::NewScan => {
                self.results.remove(&finder);
            }
            ResultsRequest::Rescan => self.start_scan(ctx, finder),
            ResultsRequest::Export(format) => self.export(finder, format),
            ResultsRequest::Run(kind) => self.prepare_action(finder, kind),
        }
    }

    fn export(&mut self, finder: Finder, format: ExportFormat) {
        let Some(state) = self.results.get(&finder) else {
            return;
        };
        if self.demo {
            self.toast("Demo mode: nothing is written.", false);
            return;
        }
        let name = format!("TwinForge {}.{}", finder.title(), format.extension());
        let Some(path) = rfd::FileDialog::new()
            .set_title("Export the list")
            .set_file_name(name)
            .add_filter(format.extension().to_uppercase(), &[format.extension()])
            .save_file()
        else {
            return;
        };
        match export::write(&state.result, &path, self.tz_offset) {
            Ok(_) => self.toast(format!("List saved to {}", path.to_string_lossy()), false),
            Err(e) => self.toast(e, true),
        }
    }

    pub(super) fn prepare_action(&mut self, finder: Finder, kind: ActionKind) {
        if self.job.is_some() || self.confirm.is_some() {
            return;
        }
        let Some(state) = self.results.get(&finder) else {
            return;
        };
        if state.result.selected_count() == 0 {
            self.toast(
                "Nothing is selected. Tick the files to act on first.",
                false,
            );
            return;
        }
        if !kind.available_for(finder) {
            self.toast("Links can only replace exact duplicates.", true);
            return;
        }
        let method = match kind {
            ActionKind::Recycle => Method::Recycle,
            ActionKind::Delete => Method::Delete,
            ActionKind::HardLink => Method::HardLink,
            ActionKind::SymLink => Method::SymLink,
            ActionKind::Move => {
                if self.demo {
                    Method::MoveTo(PathBuf::from("E:\\Sorted out"))
                } else {
                    match rfd::FileDialog::new()
                        .set_title("Move the selected files to...")
                        .pick_folder()
                    {
                        Some(dir) => Method::MoveTo(dir),
                        None => return,
                    }
                }
            }
        };
        let plan = Plan::from_result(&state.result, method, self.settings.verify_before_remove);
        self.confirm = Some(Confirm { plan });
    }

    fn run_plan(&mut self, ctx: &egui::Context, plan: Plan) {
        if self.demo {
            self.toast("Demo mode: nothing is changed on this PC.", false);
            return;
        }
        let progress = Arc::new(Progress::new());
        self.job = Some(Job {
            action: plan.method.kind(),
            progress: progress.clone(),
        });
        let (tx, c) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let report = actions::execute(&plan, &Guard::real(), &progress);
            let _ = tx.send(Msg::ActionDone(plan.finder, Box::new(report)));
            c.request_repaint();
        });
    }

    // ----------------------------------------------------------------------------------
    // Keyboard and screenshots
    // ----------------------------------------------------------------------------------

    fn dialog_open(&self) -> bool {
        self.confirm.is_some()
            || self.report.is_some()
            || self.job.is_some()
            || self.about_open
            || self.keys_open
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (find, refresh, escape, delete, help, export, digit) = ctx.input(|i| {
            let digit = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
                Key::Num7,
                Key::Num8,
                Key::Num9,
            ]
            .iter()
            .position(|k| i.modifiers.command && i.key_pressed(*k));
            (
                i.modifiers.command && i.key_pressed(Key::F),
                i.key_pressed(Key::F5),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Delete),
                i.key_pressed(Key::F1),
                i.modifiers.command && i.key_pressed(Key::E),
                digit,
            )
        });
        if escape {
            if self.job.is_some() {
                // a running action is only stopped with its own button
            } else if self.confirm.is_some() {
                self.confirm = None;
            } else if self.report.is_some() {
                self.report = None;
            } else if self.about_open || self.keys_open {
                self.about_open = false;
                self.keys_open = false;
            } else if !self.search.is_empty() {
                self.search.clear();
            }
            return;
        }
        if self.dialog_open() {
            return;
        }
        if help {
            self.keys_open = true;
        }
        if find {
            self.focus_search = true;
        }
        if let Some(n) = digit {
            self.tab = Tab::Finder(Finder::ALL[n]);
        }
        let Tab::Finder(finder) = self.tab else {
            return;
        };
        if refresh {
            self.start_scan(ctx, finder);
        }
        if export && self.results.contains_key(&finder) {
            self.export(finder, ExportFormat::Csv);
        }
        if delete && !ctx.wants_keyboard_input() && self.results.contains_key(&finder) {
            let shift = ctx.input(|i| i.modifiers.shift);
            self.prepare_action(
                finder,
                if shift {
                    ActionKind::Delete
                } else {
                    self.settings.default_action
                },
            );
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        shot.frames += 1;
        if shot.frames == 1 {
            ctx.set_pixels_per_point(1.0);
        }
        if shot.frames == 3 {
            // Now that one point is one pixel, this gives an exact 1280 x 800 image.
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(1280.0, 800.0)));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            if let Some(parent) = shot.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let result = image::save_buffer(
                &shot.path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            );
            if let Err(e) = result {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            return;
        }
        if !shot.requested && shot.frames > 60 {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }

    // ----------------------------------------------------------------------------------
    // Frame: header and navigation
    // ----------------------------------------------------------------------------------

    fn header(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        egui::TopBottomPanel::top("header")
            .exact_height(64.0)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(18, 0)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(
                    full.x_range().expand(18.0),
                    full.bottom(),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.horizontal_centered(|ui| {
                    // Brand
                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new("TwinForge").font(semibold(19.0)));
                    if self.demo {
                        widgets::badge(ui, "DEMO", p.info)
                            .on_hover_text("Invented data. Nothing on this PC is changed.");
                    }
                    ui.add_space(196.0 - ui.min_rect().width().min(196.0));

                    // Search
                    let search_width = (ui.available_width() - 300.0).clamp(220.0, 460.0);
                    let enabled = matches!(self.tab, Tab::Finder(_) | Tab::History);
                    Frame::new()
                        .fill(if p.dark { p.card } else { p.bg })
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.set_width(search_width);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(Icon::SEARCH.glyph())
                                        .size(14.0)
                                        .color(p.faint),
                                );
                                let hint = match self.tab {
                                    Tab::History => "Search past actions",
                                    _ => "Filter results by name or folder",
                                };
                                let edit = egui::TextEdit::singleline(&mut self.search)
                                    .hint_text(RichText::new(hint).color(p.faint))
                                    .frame(false)
                                    .desired_width(ui.available_width() - 30.0)
                                    .font(regular(15.0));
                                let resp = ui.add_enabled(enabled, edit);
                                if self.focus_search {
                                    resp.request_focus();
                                    self.focus_search = false;
                                }
                                if !self.search.is_empty() {
                                    if widgets::icon_button(ui, Icon::CLOSE, "Clear (Esc)")
                                        .clicked()
                                    {
                                        self.search.clear();
                                    }
                                } else {
                                    ui.label(RichText::new("Ctrl+F").size(11.5).color(p.faint));
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::INFO, "About TwinForge").clicked() {
                            self.about_open = true;
                        }
                        if widgets::icon_button(ui, Icon::KEYBOARD, "Keyboard shortcuts (F1)")
                            .clicked()
                        {
                            self.keys_open = true;
                        }
                        ui.add_space(6.0);
                        widgets::badge(ui, &Icon::SHIELD.with("Safe mode"), p.good).on_hover_text(
                            "TwinForge always keeps at least one copy of every group, re-checks each file right before it is touched, and never changes system folders.",
                        );
                    });
                });
            });
    }

    fn finder_icon(finder: Finder) -> Icon {
        match finder {
            Finder::Duplicates => Icon::COPY,
            Finder::SimilarImages => Icon::PICTURE,
            Finder::SimilarMusic => Icon::MUSIC,
            Finder::SameName => Icon::TAG,
            Finder::EmptyFiles => Icon::EMPTY_PAGE,
            Finder::EmptyFolders => Icon::FOLDER,
            Finder::BrokenShortcuts => Icon::BROKEN_LINK,
            Finder::BigFiles => Icon::BIG,
            Finder::TempFiles => Icon::BROOM,
        }
    }

    fn rail(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        let locked = self.job.is_some();
        egui::SidePanel::left("rail")
            .exact_width(214.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(10, 12)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(
                    full.right() + 10.0,
                    full.y_range().expand(12.0),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.spacing_mut().item_spacing.y = 3.0;
                let mut items: Vec<(Tab, Icon, &str, Option<usize>, bool)> = Vec::new();
                for finder in Finder::ALL {
                    let count = self.results.get(&finder).map(|s| {
                        if finder.is_grouped() {
                            s.result.groups.len()
                        } else {
                            s.result.item_count()
                        }
                    });
                    let busy = self.running.as_ref().map(|r| r.finder) == Some(finder);
                    items.push((
                        Tab::Finder(finder),
                        Self::finder_icon(finder),
                        finder.title(),
                        count,
                        busy,
                    ));
                }
                items.push((
                    Tab::History,
                    Icon::HISTORY,
                    "History",
                    Some(self.history.len()),
                    false,
                ));
                items.push((Tab::Settings, Icon::SETTINGS, "Settings", None, false));

                let section = |ui: &mut Ui, text: &str| {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new(text).font(semibold(11.0)).color(p.faint));
                    });
                    ui.add_space(2.0);
                };
                for (i, (tab, icon, label, count, busy)) in items.into_iter().enumerate() {
                    match i {
                        0 => {
                            ui.horizontal(|ui| {
                                ui.add_space(12.0);
                                ui.label(
                                    RichText::new("FIND COPIES")
                                        .font(semibold(11.0))
                                        .color(p.faint),
                                );
                            });
                            ui.add_space(2.0);
                        }
                        3 => section(ui, "CLEAN UP"),
                        9 => {
                            ui.add_space(8.0);
                            widgets::divider(ui);
                            ui.add_space(8.0);
                        }
                        _ => {}
                    }
                    let active = self.tab == tab;
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                    if active {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.selected);
                        let bar = Rect::from_min_size(
                            rect.left_center() - vec2(0.0, 9.0),
                            vec2(3.0, 18.0),
                        );
                        ui.painter()
                            .rect_filled(bar, CornerRadius::same(2), p.accent);
                    } else if resp.hovered() && !locked {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.hover);
                    }
                    let fg = if locked {
                        p.faint
                    } else if active {
                        p.text
                    } else {
                        p.muted
                    };
                    ui.painter().text(
                        rect.left_center() + vec2(24.0, 0.0),
                        Align2::CENTER_CENTER,
                        icon.glyph(),
                        regular(16.0),
                        if active { p.accent } else { fg },
                    );
                    ui.painter().text(
                        rect.left_center() + vec2(46.0, -1.0),
                        Align2::LEFT_CENTER,
                        label,
                        if active {
                            semibold(14.5)
                        } else {
                            regular(14.5)
                        },
                        fg,
                    );
                    if busy {
                        let c = rect.right_center() - vec2(18.0, 0.0);
                        egui::Spinner::new()
                            .size(14.0)
                            .color(p.accent)
                            .paint_at(ui, Rect::from_center_size(c, vec2(14.0, 14.0)));
                    } else if let Some(n) = count {
                        ui.painter().text(
                            rect.right_center() - vec2(12.0, 1.0),
                            Align2::RIGHT_CENTER,
                            fmt_count(n as u64),
                            regular(12.5),
                            if n > 0 && matches!(tab, Tab::Finder(_)) {
                                p.accent
                            } else {
                                p.faint
                            },
                        );
                    }
                    if resp.clicked() && !locked {
                        self.tab = tab;
                        self.search.clear();
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                });
            });
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < if t.error { 8.0 } else { 4.0 });
        if self.toasts.is_empty() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let p = pal_ctx(ctx);
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    for t in self.toasts.iter().rev().take(4) {
                        let color = if t.error { p.danger } else { p.accent };
                        Frame::new()
                            .fill(p.card)
                            .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.horizontal(|ui| {
                                    let icon = if t.error { Icon::ERROR } else { Icon::DONE };
                                    ui.label(RichText::new(icon.glyph()).color(color));
                                    ui.add(egui::Label::new(t.text.as_str()).wrap());
                                });
                            });
                    }
                });
            });
    }

    /// The standard page header: title, subtitle and right-aligned actions.
    pub(super) fn page_header(
        ui: &mut Ui,
        title: &str,
        subtitle: &str,
        actions: impl FnOnce(&mut Ui),
    ) {
        let p = pal(ui);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 52.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(title).font(semibold(24.0)));
                    ui.label(RichText::new(subtitle).color(p.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), actions);
            },
        );
        ui.add_space(6.0);
    }

    fn finder_page(&mut self, ui: &mut Ui, finder: Finder) {
        if self.running.as_ref().map(|r| r.finder) == Some(finder) {
            self.scanning_ui(ui, finder);
            return;
        }
        if !self.results.contains_key(&finder) {
            self.setup_ui(ui, finder);
            return;
        }
        let mut folders = self.settings.scan.include.clone();
        folders.extend(self.settings.scan.reference.iter().cloned());
        let cx = ResultsCtx {
            search: &self.search,
            settings: &self.settings,
            demo: self.demo,
            busy: self.job.is_some() || self.running.is_some(),
            folders: &folders,
            tz_offset: self.tz_offset,
        };
        let request = self
            .results
            .get_mut(&finder)
            .and_then(|state| state.ui(ui, &cx));
        if let Some(request) = request {
            let ctx = ui.ctx().clone();
            self.handle_request(&ctx, finder, request);
        }
    }

    /// Self-test: one scan-select-delete round without any clicks.
    fn autopilot(&mut self, ctx: &egui::Context) {
        let Some(mut auto) = self.auto.take() else {
            return;
        };
        ctx.request_repaint();
        auto.frames += 1;
        self.autopilot_step(ctx, &mut auto);
        if auto.step < 10 {
            self.auto = Some(auto);
        }
    }

    fn autopilot_step(&mut self, ctx: &egui::Context, auto: &mut Autopilot) {
        let say = |auto: &Autopilot, text: String| {
            if let Ok(mut log) = auto.log.lock() {
                log.push(text);
            }
        };
        let finder = Finder::Duplicates;
        match auto.step {
            0 => {
                auto.step = 1;
                self.tab = Tab::Finder(finder);
                self.start_scan(ctx, finder);
            }
            1 => {
                if self.running.is_some() {
                    return;
                }
                let Some(state) = self.results.get_mut(&finder) else {
                    say(auto, "UI: the scan did not produce a result".into());
                    auto.step = 9;
                    return;
                };
                crate::select::apply(
                    &mut state.result.groups,
                    &crate::select::Rule::KeepShortestPath,
                );
                say(
                    auto,
                    format!(
                        "UI: scan finished with {} groups; the rule selected {} files",
                        state.result.groups.len(),
                        state.result.selected_count()
                    ),
                );
                auto.step = 2;
                self.prepare_action(finder, ActionKind::Delete);
            }
            2 => {
                let Some(confirm) = &self.confirm else {
                    say(auto, "UI: no confirmation was asked for".into());
                    auto.step = 9;
                    return;
                };
                say(
                    auto,
                    format!(
                        "UI: confirmation shows {} files, {} bytes",
                        confirm.plan.count(),
                        confirm.plan.bytes()
                    ),
                );
                auto.step = 3;
                self.confirmed(ctx);
            }
            3 => {
                if self.job.is_some() {
                    return;
                }
                let Some(report) = &self.report else {
                    say(auto, "UI: the action did not report back".into());
                    auto.step = 9;
                    return;
                };
                say(
                    auto,
                    format!(
                        "UI: report: {} deleted, {} refused, {} failed",
                        report.done(),
                        report.refused(),
                        report.failed()
                    ),
                );
                let left = self
                    .results
                    .get(&finder)
                    .map(|s| s.result.groups.len())
                    .unwrap_or(usize::MAX);
                say(auto, format!("UI: {left} groups left in the list"));
                say(auto, format!("UI: {} history entries", self.history.len()));
                self.report = None;
                auto.step = 9;
            }
            _ => {
                auto.step = 10;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Called by the confirm dialog.
    pub(super) fn confirmed(&mut self, ctx: &egui::Context) {
        if let Some(c) = self.confirm.take() {
            self.run_plan(ctx, c.plan);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages();
        self.handle_keys(ctx);
        self.handle_screenshot(ctx);
        self.autopilot(ctx);
        if self.running.is_some() || self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        self.header(ctx);
        self.rail(ctx);

        let p = pal_ctx(ctx);
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(24, 18)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Finder(finder) => self.finder_page(ui, finder),
                Tab::History => self.history_ui(ui),
                Tab::Settings => self.settings_ui(ui),
            });

        self.confirm_dialog(ctx);
        self.job_dialog(ctx);
        self.report_dialog(ctx);
        self.about_dialog(ctx);
        self.keys_dialog(ctx);
        self.toasts_ui(ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Screenshots must not depend on where the mouse happens to be:
        // no hover highlights, no tooltips.
        if self.shot.is_some() {
            raw_input.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            });
            raw_input.events.push(egui::Event::PointerGone);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(r) = &self.running {
            r.progress.cancel();
        }
        if let Some(j) = &self.job {
            j.progress.cancel();
        }
        if self.setup.apply(&mut self.settings.scan).is_ok() {
            self.persist_settings();
        }
    }
}
