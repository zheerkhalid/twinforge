//! The results of a scan: a virtualised list of groups and files, smart
//! selection, a preview / compare panel and the action bar.
//!
//! This view never removes anything. It changes `selected` flags (always
//! through [`crate::select`]) and hands requests back to the application.

use super::theme::{pal, regular, semibold, Icon, Palette};
use super::widgets;
use crate::model::{ActionKind, Detail, ExportFormat, Finder, Item, ScanResult, Thumb};
use crate::select::{self, Rule};
use crate::settings::Settings;
use crate::util::{
    extension, file_name, fmt_count, fmt_date, fmt_datetime, fmt_duration, fmt_size, fmt_track,
    parent_text, within,
};
use crate::{demo, images, sys};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Rect,
    RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

/// What the results view needs from the application each frame.
pub struct ResultsCtx<'a> {
    /// Text typed into the search box in the header.
    pub search: &'a str,
    pub settings: &'a Settings,
    /// Invented data: use `demo::thumbnail` instead of reading files.
    pub demo: bool,
    /// An action is running: disable the action buttons.
    pub busy: bool,
    /// The scanned folders in priority order (include list, then reference list).
    pub folders: &'a [PathBuf],
    /// Seconds to add to UTC for display (`sys::local_offset_secs()`).
    pub tz_offset: i64,
}

/// What the user asked for; the application carries it out.
#[derive(Clone, Debug, PartialEq)]
pub enum ResultsRequest {
    /// Run this action on the selected items (the application confirms first).
    Run(ActionKind),
    Export(ExportFormat),
    /// Go back to the start page of this finder.
    NewScan,
    /// Scan again with the same settings.
    Rescan,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    /// Space that could be freed (groups) or file size (flat lists).
    Space,
    Size,
    Count,
    Name,
    Folder,
    Date,
}

impl SortKey {
    fn title(self, grouped: bool) -> &'static str {
        match self {
            SortKey::Space if grouped => "Space to free",
            SortKey::Space | SortKey::Size => "File size",
            SortKey::Count => "Number of files",
            SortKey::Name => "Name",
            SortKey::Folder => "Folder",
            SortKey::Date => "Date changed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Show {
    All,
    Selected,
    Unselected,
}

/// One line of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Header(usize),
    Item(usize, usize),
}

const HEADER_H: f32 = 38.0;
const ITEM_H: f32 = 50.0;
const PREVIEW_W: f32 = 348.0;
const COMPARE_W: f32 = 412.0;

// --------------------------------------------------------------------------------------
// Thumbnails
// --------------------------------------------------------------------------------------

type ThumbKey = (PathBuf, u32);

struct ThumbQueue {
    jobs: Mutex<(Vec<(ThumbKey, bool)>, bool)>,
    wake: Condvar,
}

/// Decodes pictures on background threads and keeps a bounded set of textures.
struct Thumbs {
    textures: HashMap<ThumbKey, Option<TextureHandle>>,
    recent: VecDeque<ThumbKey>,
    pending: HashSet<ThumbKey>,
    queue: Arc<ThumbQueue>,
    tx: Sender<(ThumbKey, Option<Thumb>)>,
    rx: Receiver<(ThumbKey, Option<Thumb>)>,
    workers: usize,
}

const MAX_TEXTURES: usize = 400;

impl Thumbs {
    fn new() -> Thumbs {
        let (tx, rx) = channel();
        Thumbs {
            textures: HashMap::new(),
            recent: VecDeque::new(),
            pending: HashSet::new(),
            queue: Arc::new(ThumbQueue {
                jobs: Mutex::new((Vec::new(), false)),
                wake: Condvar::new(),
            }),
            tx,
            rx,
            workers: 0,
        }
    }

    fn spawn_worker(&mut self, ctx: &egui::Context) {
        let (queue, tx, ctx) = (self.queue.clone(), self.tx.clone(), ctx.clone());
        self.workers += 1;
        std::thread::spawn(move || loop {
            let job = {
                let Ok(mut guard) = queue.jobs.lock() else {
                    return;
                };
                loop {
                    if guard.1 {
                        return;
                    }
                    // Newest request first: that is what is on screen now.
                    if let Some(job) = guard.0.pop() {
                        break job;
                    }
                    guard = match queue.wake.wait(guard) {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                }
            };
            let ((path, side), is_demo) = job;
            let thumb = if is_demo {
                demo::thumbnail(&path, side)
            } else {
                images::load_thumbnail(&path, side)
            };
            if tx.send(((path, side), thumb)).is_err() {
                return;
            }
            ctx.request_repaint();
        });
    }

    /// Takes finished pictures and turns them into textures.
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok((key, thumb)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let texture = thumb.map(|t| {
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [t.width as usize, t.height as usize],
                    &t.rgba,
                );
                ctx.load_texture(
                    format!("thumb:{}:{}", key.0.to_string_lossy(), key.1),
                    image,
                    egui::TextureOptions::LINEAR,
                )
            });
            self.textures.insert(key.clone(), texture);
            self.recent.push_back(key);
            while self.recent.len() > MAX_TEXTURES {
                if let Some(old) = self.recent.pop_front() {
                    self.textures.remove(&old);
                }
            }
        }
    }

    /// The texture for a picture, requesting it in the background if needed.
    /// `None` while loading or when the file is not a readable picture.
    fn get(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        side: u32,
        is_demo: bool,
    ) -> Option<&TextureHandle> {
        let key = (path.to_path_buf(), side);
        if !self.textures.contains_key(&key) && !self.pending.contains(&key) {
            if self.workers < 2 {
                self.spawn_worker(ctx);
            }
            if let Ok(mut guard) = self.queue.jobs.lock() {
                // Requests for rows that scrolled away long ago are dropped.
                if guard.0.len() > 200 {
                    let dropped: Vec<(ThumbKey, bool)> = guard.0.drain(..100).collect();
                    for (k, _) in dropped {
                        self.pending.remove(&k);
                    }
                }
                guard.0.push((key.clone(), is_demo));
                self.pending.insert(key.clone());
                self.queue.wake.notify_one();
            }
        }
        self.textures.get(&key).and_then(|t| t.as_ref())
    }

    fn loading(&self, path: &Path, side: u32) -> bool {
        self.pending.contains(&(path.to_path_buf(), side))
    }
}

impl Drop for Thumbs {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.queue.jobs.lock() {
            guard.1 = true;
        }
        self.queue.wake.notify_all();
    }
}

// --------------------------------------------------------------------------------------
// State
// --------------------------------------------------------------------------------------

/// The results of one finder plus everything the view remembers about them.
pub struct ResultsState {
    pub result: ScanResult,
    sort: SortKey,
    descending: bool,
    show: Show,
    collapsed: HashSet<usize>,
    rows: Vec<Row>,
    tops: Vec<f32>,
    total_height: f32,
    dirty: bool,
    last_search: String,
    focus: Option<(usize, usize)>,
    preview_open: bool,
    scroll_to_focus: bool,
    note: Option<(String, Instant)>,
    errors_open: bool,
    folders: Vec<(PathBuf, usize)>,
    thumbs: Thumbs,
}

enum Event {
    Toggle(usize, usize),
    Focus(usize, usize),
    ToggleGroup(usize),
    Open(PathBuf),
    Reveal(PathBuf),
    Copy(PathBuf),
    SelectFolder(PathBuf),
    KeepOnly(usize, usize),
}

fn group_matches(result: &ScanResult, g: usize, needle: &str, show: Show) -> bool {
    let group = &result.groups[g];
    let shown = match show {
        Show::All => true,
        Show::Selected => group.items.iter().any(|i| i.selected),
        Show::Unselected => group.items.iter().all(|i| !i.selected),
    };
    shown
        && (needle.is_empty()
            || group.items.iter().any(|i| {
                i.file
                    .path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(needle)
            }))
}

/// The order in which groups are shown.
pub fn sorted_groups(result: &ScanResult, key: SortKey, descending: bool) -> Vec<usize> {
    let grouped = result.finder.is_grouped();
    let first = |g: usize| &result.groups[g].items[0];
    let mut order: Vec<usize> = (0..result.groups.len())
        .filter(|g| !result.groups[*g].items.is_empty())
        .collect();
    order.sort_by(|&a, &b| {
        let (ga, gb) = (&result.groups[a], &result.groups[b]);
        let ord = match key {
            SortKey::Space if grouped => ga.wasted_bytes().cmp(&gb.wasted_bytes()),
            SortKey::Space | SortKey::Size => {
                let max = |g: &crate::model::Group| g.items.iter().map(|i| i.file.size).max();
                max(ga).cmp(&max(gb))
            }
            SortKey::Count => ga.items.len().cmp(&gb.items.len()),
            SortKey::Name => file_name(&first(a).file.path)
                .to_lowercase()
                .cmp(&file_name(&first(b).file.path).to_lowercase()),
            SortKey::Folder => parent_text(&first(a).file.path)
                .to_lowercase()
                .cmp(&parent_text(&first(b).file.path).to_lowercase()),
            SortKey::Date => {
                let newest =
                    |g: &crate::model::Group| g.items.iter().map(|i| i.file.modified).max();
                newest(ga).cmp(&newest(gb))
            }
        };
        let ord = if descending { ord.reverse() } else { ord };
        // Equal keys keep the scan's order, whichever direction is chosen.
        ord.then(a.cmp(&b))
    });
    order
}

/// Builds the flat list of rows from the sorted groups, the filter, the
/// search text and the collapsed groups.
pub fn build_rows(
    result: &ScanResult,
    order: &[usize],
    search: &str,
    show: Show,
    collapsed: &HashSet<usize>,
) -> Vec<Row> {
    let needle = search.trim().to_lowercase();
    let grouped = result.finder.is_grouped();
    let mut rows = Vec::new();
    for &g in order {
        if !group_matches(result, g, &needle, show) {
            continue;
        }
        if grouped {
            rows.push(Row::Header(g));
            if collapsed.contains(&g) {
                continue;
            }
        }
        for i in 0..result.groups[g].items.len() {
            rows.push(Row::Item(g, i));
        }
    }
    rows
}

impl ResultsState {
    pub fn new(result: ScanResult) -> ResultsState {
        let grouped = result.finder.is_grouped();
        let mut state = ResultsState {
            sort: if grouped
                || result.finder == Finder::BigFiles
                || result.finder == Finder::TempFiles
            {
                SortKey::Space
            } else {
                SortKey::Folder
            },
            descending: grouped || matches!(result.finder, Finder::BigFiles | Finder::TempFiles),
            result,
            show: Show::All,
            collapsed: HashSet::new(),
            rows: Vec::new(),
            tops: Vec::new(),
            total_height: 0.0,
            dirty: true,
            last_search: String::new(),
            focus: None,
            preview_open: false,
            scroll_to_focus: false,
            note: None,
            errors_open: false,
            folders: Vec::new(),
            thumbs: Thumbs::new(),
        };
        select::enforce(&mut state.result.groups);
        state.rebuild("");
        state
    }

    fn rebuild(&mut self, search: &str) {
        let order = sorted_groups(&self.result, self.sort, self.descending);
        self.rows = build_rows(&self.result, &order, search, self.show, &self.collapsed);
        self.tops.clear();
        let mut y = 0.0;
        for row in &self.rows {
            self.tops.push(y);
            y += match row {
                Row::Header(_) => HEADER_H,
                Row::Item(..) => ITEM_H,
            };
        }
        self.total_height = y;
        self.last_search = search.to_string();
        self.dirty = false;

        // The folders that hold the most listed files, for "select by folder".
        let mut counts: HashMap<PathBuf, usize> = HashMap::new();
        for item in self.result.groups.iter().flat_map(|g| g.items.iter()) {
            if item.reference {
                continue;
            }
            if let Some(parent) = item.file.path.parent() {
                *counts.entry(parent.to_path_buf()).or_default() += 1;
            }
        }
        let mut folders: Vec<(PathBuf, usize)> = counts.into_iter().collect();
        folders.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        folders.truncate(14);
        self.folders = folders;
    }

    /// The rows currently shown (for tests).
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Called after an action: these paths are gone (or no longer duplicates).
    /// Groups of a grouped finder that fall below two items disappear.
    pub fn remove_paths(&mut self, gone: &HashSet<PathBuf>) {
        if gone.is_empty() {
            return;
        }
        let focused = self
            .focus
            .and_then(|(g, i)| self.result.groups.get(g)?.items.get(i))
            .map(|i| i.file.path.clone());
        let grouped = self.result.finder.is_grouped();
        for group in &mut self.result.groups {
            group.items.retain(|i| !gone.contains(&i.file.path));
        }
        self.result
            .groups
            .retain(|g| g.items.len() >= if grouped { 2 } else { 1 });
        select::enforce(&mut self.result.groups);
        // Group numbers changed: forget what referred to them.
        self.collapsed.clear();
        self.focus = focused.and_then(|path| {
            self.result
                .groups
                .iter()
                .enumerate()
                .find_map(|(g, group)| {
                    group
                        .items
                        .iter()
                        .position(|i| i.file.path == path)
                        .map(|i| (g, i))
                })
        });
        if self.focus.is_none() {
            self.preview_open = false;
        }
        let search = self.last_search.clone();
        self.rebuild(&search);
    }

    /// Puts the view into a named state for a screenshot (`compare`, `selected`, ...).
    pub fn pose(&mut self, pose: &str) {
        let grouped = self.result.finder.is_grouped();
        let select_some = |state: &mut ResultsState| {
            if grouped {
                let rule = match state.result.finder {
                    Finder::SimilarImages => Rule::KeepHighestResolution,
                    Finder::SimilarMusic => Rule::KeepBestAudio,
                    _ => Rule::KeepNewest,
                };
                select::apply(&mut state.result.groups, &rule);
            } else {
                for row in state.rows.clone().iter().skip(1).step_by(2).take(5) {
                    if let Row::Item(g, i) = *row {
                        select::set_selected(&mut state.result.groups[g], i, true);
                    }
                }
            }
        };
        match pose {
            "selected" => select_some(self),
            "compare" => {
                select_some(self);
                let target = self.rows.iter().find_map(|r| match r {
                    Row::Item(g, i) if !grouped || *i == 1 => Some((*g, *i)),
                    _ => None,
                });
                if let Some(target) = target {
                    self.focus = Some(target);
                    self.preview_open = true;
                }
            }
            "collapsed" => {
                self.collapsed = (0..self.result.groups.len()).skip(1).collect();
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// The compare view shows two pictures side by side and needs more room.
    fn preview_width(&self) -> f32 {
        let comparing = self.result.finder.is_grouped()
            && self
                .focus
                .and_then(|(g, i)| self.result.groups.get(g)?.items.get(i))
                .map(|i| images::is_picture(&i.file.path))
                .unwrap_or(false);
        if comparing {
            COMPARE_W
        } else {
            PREVIEW_W
        }
    }

    fn say(&mut self, text: &str) {
        self.note = Some((text.to_string(), Instant::now()));
    }

    fn toggle(&mut self, g: usize, i: usize) {
        let Some(group) = self.result.groups.get_mut(g) else {
            return;
        };
        let Some(item) = group.items.get(i) else {
            return;
        };
        let (on, reference) = (!item.selected, item.reference);
        if !select::set_selected(group, i, on) {
            self.say(if reference {
                "Files in a reference folder are never selected."
            } else {
                "One file of every group always stays. Untick another one first."
            });
        }
    }

    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Toggle(g, i) => self.toggle(g, i),
            Event::Focus(g, i) => {
                self.focus = Some((g, i));
                self.preview_open = true;
            }
            Event::ToggleGroup(g) => {
                if !self.collapsed.remove(&g) {
                    self.collapsed.insert(g);
                }
                self.dirty = true;
            }
            Event::Open(path) => {
                if let Err(e) = sys::open_path(&path.to_string_lossy()) {
                    self.say(&e);
                }
            }
            Event::Reveal(path) => {
                if let Err(e) = sys::reveal_in_explorer(&path) {
                    self.say(&e);
                }
            }
            // The clipboard is filled where the event is collected.
            Event::Copy(_) => {}
            Event::SelectFolder(folder) => {
                let n = if self.result.finder.is_grouped() {
                    select::select_in_folder(&mut self.result.groups, &folder)
                } else {
                    let mut n = 0;
                    for item in self
                        .result
                        .groups
                        .iter_mut()
                        .flat_map(|g| g.items.iter_mut())
                    {
                        if !item.selected && !item.reference && within(&item.file.path, &folder) {
                            item.selected = true;
                            n += 1;
                        }
                    }
                    n
                };
                self.say(&format!(
                    "{} more selected in that folder.",
                    fmt_count(n as u64)
                ));
            }
            Event::KeepOnly(g, i) => {
                if let Some(group) = self.result.groups.get_mut(g) {
                    select::keep_only(group, i);
                }
            }
        }
    }

    // ----------------------------------------------------------------------------------
    // Drawing
    // ----------------------------------------------------------------------------------

    /// Draws the whole page into the central panel.
    pub fn ui(&mut self, ui: &mut Ui, cx: &ResultsCtx<'_>) -> Option<ResultsRequest> {
        let p = pal(ui);
        let ctx = ui.ctx().clone();
        self.thumbs.receive(&ctx);
        if self.dirty || self.last_search != cx.search {
            self.rebuild(cx.search);
        }
        let mut request = None;
        let grouped = self.result.finder.is_grouped();

        self.header(ui, &mut request);
        if self.result.groups.is_empty() {
            let (title, text) = empty_words(self.result.finder);
            widgets::empty_state(ui, Icon::DONE, p.good, title, text);
            ui.vertical_centered(|ui| {
                if widgets::primary_button(ui, "New scan", true).clicked() {
                    request = Some(ResultsRequest::NewScan);
                }
            });
            self.errors_note(ui);
            return request;
        }
        self.toolbar(ui, cx);
        self.keys(ui, grouped);

        egui::TopBottomPanel::bottom("results-actions")
            .exact_height(58.0)
            .frame(Frame::new().inner_margin(Margin {
                left: 0,
                right: 0,
                top: 10,
                bottom: 0,
            }))
            .show_separator_line(false)
            .show_inside(ui, |ui| self.action_bar(ui, cx, &mut request));

        let mut events: Vec<Event> = Vec::new();
        if self.preview_open && self.focus.is_some() {
            egui::SidePanel::right("results-preview")
                .exact_width(self.preview_width())
                .resizable(false)
                .frame(Frame::new().inner_margin(Margin {
                    left: 14,
                    right: 0,
                    top: 0,
                    bottom: 0,
                }))
                .show_separator_line(false)
                .show_inside(ui, |ui| self.preview(ui, cx, &mut events));
        }

        egui::CentralPanel::default()
            .frame(Frame::new())
            .show_inside(ui, |ui| self.list(ui, cx, &mut events));

        for event in events {
            if let Event::Copy(path) = &event {
                ctx.copy_text(path.to_string_lossy().into_owned());
                self.say("Path copied.");
            }
            self.apply_event(event);
        }
        if self.note.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        request
    }

    fn header(&mut self, ui: &mut Ui, request: &mut Option<ResultsRequest>) {
        let p = pal(ui);
        let r = &self.result;
        let grouped = r.finder.is_grouped();
        let secs = r.elapsed_ms / 1000;
        let took = if r.elapsed_ms < 1000 {
            "under a second".to_string()
        } else {
            fmt_duration(secs)
        };
        let mut subtitle = if r.groups.is_empty() {
            format!("{} files looked at in {took}", fmt_count(r.files_scanned))
        } else if grouped {
            format!(
                "{} groups · {} files · {} could be freed · {} files looked at in {took}",
                fmt_count(r.groups.len() as u64),
                fmt_count(r.item_count() as u64),
                fmt_size(r.wasted_bytes()),
                fmt_count(r.files_scanned),
            )
        } else if r.finder == Finder::EmptyFolders {
            format!(
                "{} empty folders · {} folders looked at in {took}",
                fmt_count(r.item_count() as u64),
                fmt_count(r.files_scanned),
            )
        } else {
            format!(
                "{} found · {} in total · {} files looked at in {took}",
                fmt_count(r.item_count() as u64),
                fmt_size(r.wasted_bytes()),
                fmt_count(r.files_scanned),
            )
        };
        if r.cancelled {
            subtitle.push_str(" · scan stopped early");
        }
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 52.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(r.finder.title()).font(semibold(24.0)));
                    ui.label(RichText::new(subtitle).color(p.muted));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !r.groups.is_empty() {
                        ui.menu_button(Icon::EXPORT.with("Export"), |ui| {
                            for (label, format) in [
                                ("Spreadsheet (CSV)", ExportFormat::Csv),
                                ("Web page (HTML)", ExportFormat::Html),
                                ("Data (JSON)", ExportFormat::Json),
                            ] {
                                if ui.button(label).clicked() {
                                    *request = Some(ResultsRequest::Export(format));
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                    if widgets::ghost_button(ui, &Icon::REFRESH.with("Scan again"))
                        .on_hover_text("Scan again with the same settings (F5)")
                        .clicked()
                    {
                        *request = Some(ResultsRequest::Rescan);
                    }
                    if widgets::ghost_button(ui, &Icon::BACK.with("New scan"))
                        .on_hover_text("Back to folders and options")
                        .clicked()
                    {
                        *request = Some(ResultsRequest::NewScan);
                    }
                });
            },
        );
        ui.add_space(4.0);
    }

    fn errors_note(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        if self.result.errors.is_empty() {
            return;
        }
        ui.vertical_centered(|ui| {
            ui.add_space(6.0);
            let text = format!(
                "{} could not be read",
                if self.result.errors.len() == 1 {
                    "1 place".to_string()
                } else {
                    format!("{} places", self.result.errors.len())
                }
            );
            if ui
                .link(RichText::new(text).size(12.5).color(p.warn))
                .clicked()
            {
                self.errors_open = !self.errors_open;
            }
            if self.errors_open {
                egui::ScrollArea::vertical()
                    .id_salt("results-errors-empty")
                    .max_height(160.0)
                    .show(ui, |ui| {
                        for e in self.result.errors.iter().take(200) {
                            ui.label(RichText::new(e).size(12.5).color(p.muted));
                        }
                    });
            }
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, cx: &ResultsCtx<'_>) {
        let p = pal(ui);
        let grouped = self.result.finder.is_grouped();
        let finder = self.result.finder;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 36.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // Smart selection
                ui.menu_button(Icon::MAGIC.with("Select"), |ui| {
                    ui.set_min_width(230.0);
                    if grouped {
                        let mut rules = vec![
                            Rule::KeepNewest,
                            Rule::KeepOldest,
                            Rule::KeepLargest,
                            Rule::KeepSmallest,
                        ];
                        if finder == Finder::SimilarImages {
                            rules.push(Rule::KeepHighestResolution);
                        }
                        if finder == Finder::SimilarMusic {
                            rules.push(Rule::KeepBestAudio);
                        }
                        rules.push(Rule::KeepShortestPath);
                        rules.push(Rule::KeepLongestPath);
                        if cx.folders.len() > 1 {
                            rules.push(Rule::KeepByFolderPriority(cx.folders.to_vec()));
                        }
                        for rule in rules {
                            let hover = match &rule {
                                Rule::KeepByFolderPriority(_) => {
                                    "Keeps the copy in the folder that comes first in your list of folders."
                                }
                                _ => "Selects every other file of each group.",
                            };
                            if ui.button(rule.title()).on_hover_text(hover).clicked() {
                                select::apply(&mut self.result.groups, &rule);
                                self.dirty = self.show != Show::All;
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.button("All but one in each group     Ctrl+A").clicked() {
                            select::select_all_but_first(&mut self.result.groups);
                            self.dirty = self.show != Show::All;
                            ui.close_menu();
                        }
                    } else if ui.button("Select all     Ctrl+A").clicked() {
                        select::select_all_but_first(&mut self.result.groups);
                        self.dirty = self.show != Show::All;
                        ui.close_menu();
                    }
                    let folders = self.folders.clone();
                    ui.menu_button("Everything in a folder", |ui| {
                        ui.set_min_width(380.0);
                        for (folder, count) in &folders {
                            let label = format!("{}   ({count})", folder.to_string_lossy());
                            if ui.button(label).clicked() {
                                self.apply_event(Event::SelectFolder(folder.clone()));
                                self.dirty = self.show != Show::All;
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Invert     Ctrl+I").clicked() {
                        select::invert(&mut self.result.groups);
                        self.dirty = self.show != Show::All;
                        ui.close_menu();
                    }
                    if ui.button("Clear     Ctrl+D").clicked() {
                        select::clear(&mut self.result.groups);
                        self.dirty = self.show != Show::All;
                        ui.close_menu();
                    }
                });

                // Sorting
                let keys: &[SortKey] = if grouped {
                    &[
                        SortKey::Space,
                        SortKey::Size,
                        SortKey::Count,
                        SortKey::Name,
                        SortKey::Folder,
                        SortKey::Date,
                    ]
                } else if finder == Finder::EmptyFolders || finder == Finder::EmptyFiles {
                    &[SortKey::Folder, SortKey::Name, SortKey::Date]
                } else {
                    &[SortKey::Size, SortKey::Name, SortKey::Folder, SortKey::Date]
                };
                let label = format!("{}: {}", Icon::SORT.with("Sort"), self.sort.title(grouped));
                ui.menu_button(label, |ui| {
                    for key in keys {
                        if ui
                            .selectable_label(self.sort == *key || (self.sort == SortKey::Space && *key == SortKey::Size && !grouped), key.title(grouped))
                            .clicked()
                        {
                            self.sort = *key;
                            self.dirty = true;
                            ui.close_menu();
                        }
                    }
                });
                let arrow = if self.descending {
                    Icon::SORT_DOWN
                } else {
                    Icon::SORT_UP
                };
                if widgets::icon_button(
                    ui,
                    arrow,
                    if self.descending {
                        "Largest / newest / Z first. Click to reverse."
                    } else {
                        "Smallest / oldest / A first. Click to reverse."
                    },
                )
                .clicked()
                {
                    self.descending = !self.descending;
                    self.dirty = true;
                }

                ui.add_space(6.0);
                for (label, show) in [
                    ("All", Show::All),
                    ("With selection", Show::Selected),
                    ("Without selection", Show::Unselected),
                ] {
                    if widgets::chip(ui, label, None, self.show == show).clicked() {
                        self.show = show;
                        self.dirty = true;
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if grouped {
                        if widgets::icon_button(ui, Icon::CHEVRON_RIGHT, "Collapse all groups")
                            .clicked()
                        {
                            self.collapsed = (0..self.result.groups.len()).collect();
                            self.dirty = true;
                        }
                        if widgets::icon_button(ui, Icon::CHEVRON_DOWN, "Expand all groups")
                            .clicked()
                        {
                            self.collapsed.clear();
                            self.dirty = true;
                        }
                    }
                    if !self.result.errors.is_empty() {
                        let text = format!("{} not read", self.result.errors.len());
                        let resp = widgets::badge(ui, &Icon::WARNING.with(&text), p.warn)
                            .on_hover_text("Some places could not be read. Click to see them.")
                            .interact(Sense::click());
                        if resp.clicked() {
                            self.errors_open = !self.errors_open;
                        }
                    }
                });
            },
        );
        if self.errors_open && !self.result.errors.is_empty() {
            Frame::new()
                .fill(p.card)
                .stroke(Stroke::new(1.0_f32, p.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt("results-errors")
                        .max_height(110.0)
                        .show(ui, |ui| {
                            for e in self.result.errors.iter().take(200) {
                                ui.label(RichText::new(e).size(12.5).color(p.muted));
                            }
                        });
                });
        }
        ui.add_space(4.0);
    }

    fn keys(&mut self, ui: &mut Ui, grouped: bool) {
        let ctx = ui.ctx().clone();
        if ctx.wants_keyboard_input() || ctx.memory(|m| m.any_popup_open()) {
            return;
        }
        let (all, invert, clear, up, down, space, enter) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(Key::A),
                i.modifiers.command && i.key_pressed(Key::I),
                i.modifiers.command && i.key_pressed(Key::D),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Space),
                i.key_pressed(Key::Enter),
            )
        });
        if all {
            select::select_all_but_first(&mut self.result.groups);
            self.say(if grouped {
                "Everything but the first file of each group is selected."
            } else {
                "Everything is selected."
            });
        }
        if invert {
            select::invert(&mut self.result.groups);
        }
        if clear {
            select::clear(&mut self.result.groups);
        }
        if up || down {
            let items: Vec<(usize, usize)> = self
                .rows
                .iter()
                .filter_map(|r| match r {
                    Row::Item(g, i) => Some((*g, *i)),
                    _ => None,
                })
                .collect();
            if !items.is_empty() {
                let at = self.focus.and_then(|f| items.iter().position(|x| *x == f));
                let next = match (at, down) {
                    (None, _) => 0,
                    (Some(n), true) => (n + 1).min(items.len() - 1),
                    (Some(n), false) => n.saturating_sub(1),
                };
                self.focus = Some(items[next]);
                self.preview_open = true;
                self.scroll_to_focus = true;
            }
        }
        if let Some((g, i)) = self.focus {
            if space {
                self.toggle(g, i);
            }
            if enter {
                if let Some(item) = self.result.groups.get(g).and_then(|gr| gr.items.get(i)) {
                    let path = item.file.path.clone();
                    self.apply_event(Event::Open(path));
                }
            }
        }
    }

    fn action_bar(
        &mut self,
        ui: &mut Ui,
        cx: &ResultsCtx<'_>,
        request: &mut Option<ResultsRequest>,
    ) {
        let p = pal(ui);
        let finder = self.result.finder;
        let (count, bytes) = (self.result.selected_count(), self.result.selected_bytes());
        let rect = ui.max_rect();
        ui.painter().rect(
            rect,
            CornerRadius::same(12),
            p.card,
            Stroke::new(1.0_f32, p.border),
            StrokeKind::Inside,
        );
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(14.0, 6.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        let ui = &mut inner;
        let what = if finder == Finder::EmptyFolders {
            if count == 1 {
                "folder"
            } else {
                "folders"
            }
        } else if count == 1 {
            "file"
        } else {
            "files"
        };
        if count == 0 {
            ui.label(RichText::new(Icon::INFO.glyph()).size(15.0).color(p.faint));
            ui.label(
                RichText::new(if finder.is_grouped() {
                    "Nothing selected. Tick files, or let Select pick for you; one file of each group always stays."
                } else {
                    "Nothing selected. Tick what you want to remove."
                })
                .color(p.muted),
            );
        } else {
            ui.label(
                RichText::new(format!("{} {what} selected", fmt_count(count as u64)))
                    .font(semibold(15.5)),
            );
            if finder != Finder::EmptyFolders {
                ui.label(
                    RichText::new(format!("· {}", fmt_size(bytes)))
                        .color(p.accent)
                        .font(semibold(15.5)),
                );
            }
        }
        let note = self
            .note
            .as_ref()
            .filter(|(_, at)| at.elapsed().as_secs_f32() < 4.0)
            .map(|(t, _)| t.clone());
        if note.is_none() {
            self.note = None;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let enabled = count > 0 && !cx.busy;
            let main = cx.settings.default_action;
            ui.add_enabled_ui(enabled, |ui| {
                ui.menu_button(Icon::MORE.with("More actions"), |ui| {
                    ui.set_min_width(240.0);
                    for kind in ActionKind::ALL {
                        if kind == main || !kind.available_for(finder) {
                            continue;
                        }
                        if ui.button(kind.title()).clicked() {
                            *request = Some(ResultsRequest::Run(kind));
                            ui.close_menu();
                        }
                    }
                });
            });
            let (icon, label) = match main {
                ActionKind::Recycle => (Icon::DELETE, "Move to Recycle Bin"),
                ActionKind::Delete => (Icon::DELETE, "Delete permanently"),
                ActionKind::Move => (Icon::MOVE, "Move to a folder"),
                ActionKind::HardLink => (Icon::LINK, "Replace with hard links"),
                ActionKind::SymLink => (Icon::LINK, "Replace with symbolic links"),
            };
            let clicked = if main == ActionKind::Delete {
                widgets::danger_button(ui, &icon.with(label), enabled).clicked()
            } else {
                widgets::primary_button(ui, &icon.with(label), enabled).clicked()
            };
            if clicked {
                *request = Some(ResultsRequest::Run(main));
            }
            if let Some(text) = note {
                ui.add_space(6.0);
                let rest = ui.available_rect_before_wrap();
                let galley = widgets::elided(ui, &text, regular(13.0), p.warn, rest.width() - 8.0);
                ui.painter().galley(
                    pos2(
                        rest.right() - galley.size().x,
                        rest.center().y - galley.size().y / 2.0,
                    ),
                    galley,
                    p.warn,
                );
            }
        });
    }

    // ----------------------------------------------------------------------------------
    // The list
    // ----------------------------------------------------------------------------------

    fn list(&mut self, ui: &mut Ui, cx: &ResultsCtx<'_>, events: &mut Vec<Event>) {
        let p = pal(ui);
        if self.rows.is_empty() {
            widgets::empty_state(
                ui,
                Icon::SEARCH,
                p.muted,
                "Nothing matches",
                "No result fits the filter or the search text. Clear them to see everything again.",
            );
            return;
        }
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());
                let total = self.total_height;
                let scroll_target = if self.scroll_to_focus {
                    self.focus
                } else {
                    None
                };
                self.scroll_to_focus = false;
                egui::ScrollArea::vertical()
                    .id_salt("results-list")
                    .auto_shrink([false, false])
                    .show_viewport(ui, |ui, viewport| {
                        ui.set_height(total);
                        let origin = ui.max_rect().min;
                        let width = ui.available_width() - 4.0;
                        let first = self
                            .tops
                            .partition_point(|t| *t + ITEM_H < viewport.min.y)
                            .saturating_sub(1);
                        for n in first..self.rows.len() {
                            let top = self.tops[n];
                            if top > viewport.max.y {
                                break;
                            }
                            let row = self.rows[n];
                            let h = if matches!(row, Row::Header(_)) {
                                HEADER_H
                            } else {
                                ITEM_H
                            };
                            let rect = Rect::from_min_size(origin + vec2(0.0, top), vec2(width, h));
                            match row {
                                Row::Header(g) => self.header_row(ui, rect, g, events),
                                Row::Item(g, i) => self.item_row(ui, rect, g, i, cx, events),
                            }
                        }
                        if let Some(target) = scroll_target {
                            if let Some(n) = self
                                .rows
                                .iter()
                                .position(|r| *r == Row::Item(target.0, target.1))
                            {
                                let rect = Rect::from_min_size(
                                    origin + vec2(0.0, self.tops[n]),
                                    vec2(width, ITEM_H),
                                );
                                ui.scroll_to_rect(rect, None);
                            }
                        }
                    });
            });
    }

    fn header_row(&mut self, ui: &mut Ui, rect: Rect, g: usize, events: &mut Vec<Event>) {
        let p = pal(ui);
        let group = &self.result.groups[g];
        let resp = ui.interact(rect, ui.id().with(("group", g)), Sense::click());
        let inner = rect.shrink2(vec2(2.0, 3.0));
        ui.painter().rect_filled(
            inner,
            CornerRadius::same(8),
            if resp.hovered() { p.hover } else { p.bg },
        );
        let collapsed = self.collapsed.contains(&g);
        ui.painter().text(
            inner.left_center() + vec2(16.0, 0.0),
            Align2::CENTER_CENTER,
            if collapsed {
                Icon::CHEVRON_RIGHT.glyph()
            } else {
                Icon::CHEVRON_DOWN.glyph()
            },
            regular(11.0),
            p.muted,
        );
        let same_size = group
            .items
            .iter()
            .all(|i| i.file.size == group.items[0].file.size);
        let title = if same_size {
            format!(
                "{} files · {} each",
                group.items.len(),
                fmt_size(group.items[0].file.size)
            )
        } else {
            format!(
                "{} files · {} in total",
                group.items.len(),
                fmt_size(group.total_bytes())
            )
        };
        let title_galley = ui.painter().layout_no_wrap(title, semibold(13.5), p.text);
        let mut x = inner.left() + 32.0;
        let y = inner.center().y;
        let w = title_galley.size().x;
        ui.painter().galley(
            pos2(x, y - title_galley.size().y / 2.0),
            title_galley,
            p.text,
        );
        x += w + 12.0;
        let wasted = format!("{} could be freed", fmt_size(group.wasted_bytes()));
        let wasted_galley = ui.painter().layout_no_wrap(wasted, regular(13.0), p.accent);
        let w = wasted_galley.size().x;
        ui.painter().galley(
            pos2(x, y - wasted_galley.size().y / 2.0),
            wasted_galley,
            p.accent,
        );
        x += w + 12.0;

        // Right side: selection count, then the group's label in what is left.
        let selected = group.selected_count();
        let mut right = inner.right() - 12.0;
        if selected > 0 {
            let text = format!("{selected} selected");
            let galley = ui.painter().layout_no_wrap(text, semibold(12.5), p.accent);
            right -= galley.size().x;
            ui.painter()
                .galley(pos2(right, y - galley.size().y / 2.0), galley, p.accent);
            right -= 14.0;
        }
        if right - x > 60.0 && !group.label.is_empty() {
            widgets::text_in_right(
                ui,
                Rect::from_min_max(pos2(x, inner.top()), pos2(right, inner.bottom())),
                &group.label,
                regular(12.5),
                p.faint,
            );
        }
        if resp.clicked() {
            events.push(Event::ToggleGroup(g));
        }
    }

    fn item_row(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        g: usize,
        i: usize,
        cx: &ResultsCtx<'_>,
        events: &mut Vec<Event>,
    ) {
        let p = pal(ui);
        let finder = self.result.finder;
        let grouped = finder.is_grouped();
        let group = &self.result.groups[g];
        let item = &group.items[i];
        let focused = self.focus == Some((g, i));
        let any_selected = group.items.iter().any(|x| x.selected);

        let resp = ui.interact(rect, ui.id().with(("item", g, i)), Sense::click());
        let check_rect = Rect::from_min_size(
            pos2(rect.left() + if grouped { 22.0 } else { 8.0 }, rect.top()),
            vec2(32.0, rect.height()),
        );
        let check = ui.interact(check_rect, ui.id().with(("check", g, i)), Sense::click());

        let back = rect.shrink2(vec2(2.0, 1.0));
        if focused {
            ui.painter()
                .rect_filled(back, CornerRadius::same(8), p.selected);
        } else if resp.hovered() || check.hovered() {
            ui.painter()
                .rect_filled(back, CornerRadius::same(8), p.hover);
        }

        // Checkbox
        if item.reference {
            let c = check_rect.center();
            ui.painter().text(
                c,
                Align2::CENTER_CENTER,
                Icon::LOCK.glyph(),
                regular(14.0),
                p.good,
            );
        } else {
            widgets::paint_checkbox(ui, check_rect, Some(item.selected), check.hovered());
        }

        // Thumbnail or type tile
        let tile = Rect::from_min_size(
            pos2(check_rect.right() + 6.0, rect.center().y - 19.0),
            vec2(38.0, 38.0),
        );
        let is_pic = !item.file.is_dir && images::is_picture(&item.file.path);
        let mut drew_picture = false;
        if is_pic && cx.settings.thumbnails {
            if let Some(tex) = self.thumbs.get(ui.ctx(), &item.file.path, 96, cx.demo) {
                paint_cover(ui, tile, tex, 6);
                drew_picture = true;
            }
        }
        if !drew_picture {
            let (icon, color) = type_tile(&p, item);
            ui.painter()
                .rect_filled(tile, CornerRadius::same(8), color.gamma_multiply(0.16));
            ui.painter().text(
                tile.center(),
                Align2::CENTER_CENTER,
                icon.glyph(),
                regular(17.0),
                color,
            );
        }

        // Right-hand columns
        let narrow = rect.width() < 760.0;
        let mut right = rect.right() - 14.0;
        let dim = item.selected;
        let size_text = if item.file.is_dir {
            "empty".to_string()
        } else {
            fmt_size(item.file.size)
        };
        widgets::text_in_right(
            ui,
            Rect::from_min_max(pos2(right - 78.0, rect.top()), pos2(right, rect.bottom())),
            &size_text,
            regular(13.5),
            if dim { p.muted } else { p.text },
        );
        right -= 78.0 + 12.0;
        if !narrow {
            widgets::text_in_right(
                ui,
                Rect::from_min_max(pos2(right - 88.0, rect.top()), pos2(right, rect.bottom())),
                &fmt_date(item.file.modified, cx.tz_offset),
                regular(13.0),
                p.muted,
            );
            right -= 88.0 + 12.0;
        }
        let (detail_top, detail_bottom) = detail_lines(item);
        if !detail_top.is_empty() {
            let w = if narrow {
                138.0
            } else if rect.width() > 900.0 {
                290.0
            } else {
                220.0
            };
            let col = Rect::from_min_max(pos2(right - w, rect.top()), pos2(right, rect.bottom()));
            if detail_bottom.is_empty() {
                widgets::text_in_right(ui, col, &detail_top, regular(13.0), p.muted);
            } else {
                let (a, b) = col
                    .shrink2(vec2(0.0, 7.0))
                    .split_top_bottom_at_fraction(0.5);
                widgets::text_in_right(ui, a, &detail_top, regular(13.0), p.text);
                widgets::text_in_right(ui, b, &detail_bottom, regular(12.0), p.muted);
            }
            right -= w + 12.0;
        }

        // Badge
        let badge = if item.reference {
            Some(("REFERENCE", p.good))
        } else if grouped && any_selected && !item.selected {
            Some(("KEPT", p.good))
        } else if item.selected {
            Some((
                match cx.settings.default_action {
                    ActionKind::Move => "MOVE",
                    ActionKind::HardLink | ActionKind::SymLink => "LINK",
                    _ => "REMOVE",
                },
                p.danger,
            ))
        } else {
            None
        };
        if let Some((text, color)) = badge {
            let galley = ui
                .painter()
                .layout_no_wrap(text.to_string(), semibold(11.5), color);
            let w = galley.size().x + 14.0;
            let r = Rect::from_min_size(pos2(right - w, rect.center().y - 10.0), vec2(w, 20.0));
            ui.painter()
                .rect_filled(r, CornerRadius::same(10), color.gamma_multiply(0.16));
            ui.painter().galley(
                pos2(
                    r.center().x - galley.size().x / 2.0,
                    r.center().y - galley.size().y / 2.0,
                ),
                galley,
                color,
            );
            right -= w + 10.0;
        }

        // Name and folder
        let text_left = tile.right() + 12.0;
        let text_rect = Rect::from_min_max(
            pos2(text_left, rect.top() + 6.0),
            pos2(right.max(text_left + 40.0), rect.bottom() - 6.0),
        );
        let (top, bottom) = text_rect.split_top_bottom_at_fraction(0.52);
        widgets::text_in(
            ui,
            top,
            &file_name(&item.file.path),
            semibold(14.0),
            if dim { p.muted } else { p.text },
        );
        widgets::text_in(
            ui,
            bottom,
            &parent_text(&item.file.path),
            regular(12.5),
            p.faint,
        );

        // Interaction
        let path = item.file.path.clone();
        let folder = item.file.path.parent().map(|x| x.to_path_buf());
        let reference = item.reference;
        if check.clicked() {
            events.push(Event::Toggle(g, i));
        } else if resp.double_clicked() {
            events.push(Event::Open(path.clone()));
        } else if resp.clicked() {
            events.push(Event::Focus(g, i));
        }
        let group_len = group.items.len();
        resp.context_menu(|ui| {
            ui.set_min_width(230.0);
            if ui.button("Open").clicked() {
                events.push(Event::Open(path.clone()));
                ui.close_menu();
            }
            if ui.button("Show in Explorer").clicked() {
                events.push(Event::Reveal(path.clone()));
                ui.close_menu();
            }
            if ui.button("Copy path").clicked() {
                events.push(Event::Copy(path.clone()));
                ui.close_menu();
            }
            ui.separator();
            if grouped && group_len >= 2 && !reference && ui.button("Keep only this one").clicked()
            {
                events.push(Event::KeepOnly(g, i));
                ui.close_menu();
            }
            if let Some(folder) = &folder {
                if ui.button("Select everything in this folder").clicked() {
                    events.push(Event::SelectFolder(folder.clone()));
                    ui.close_menu();
                }
            }
        });
    }

    // ----------------------------------------------------------------------------------
    // Preview / compare panel
    // ----------------------------------------------------------------------------------

    fn preview(&mut self, ui: &mut Ui, cx: &ResultsCtx<'_>, events: &mut Vec<Event>) {
        let p = pal(ui);
        let Some((g, i)) = self.focus else { return };
        let Some(group) = self.result.groups.get(g) else {
            self.focus = None;
            return;
        };
        let Some(item) = group.items.get(i).cloned() else {
            self.focus = None;
            return;
        };
        let group_items: Vec<Item> = group.items.clone();
        let grouped = self.result.finder.is_grouped();
        // What to compare with: the copy that stays, or simply another member.
        let partner = if grouped {
            (0..group_items.len())
                .filter(|&k| k != i)
                .min_by_key(|&k| (group_items[k].selected, !group_items[k].reference, k))
        } else {
            None
        };
        let is_pic = images::is_picture(&item.file.path);
        let mut close = false;

        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 28.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(
                            RichText::new(if is_pic && partner.is_some() {
                                "Compare"
                            } else {
                                "Details"
                            })
                            .font(semibold(16.0)),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::CLOSE, "Close").clicked() {
                                close = true;
                            }
                        });
                    },
                );
                egui::ScrollArea::vertical()
                    .id_salt("results-preview-scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width() - 6.0);
                        let show_pictures = cx.settings.thumbnails || cx.demo;
                        let comparing = is_pic && partner.is_some();
                        if let (true, Some(k)) = (is_pic, partner) {
                            let other = group_items[k].clone();
                            let title = if other.reference {
                                "Reference copy"
                            } else if other.selected {
                                "Another copy"
                            } else {
                                "The copy that stays"
                            };
                            ui.columns(2, |cols| {
                                cols[0].vertical(|ui| {
                                    self.preview_card(ui, cx, &item, "This file", show_pictures, 132.0);
                                });
                                cols[1].vertical(|ui| {
                                    self.preview_card(ui, cx, &other, title, show_pictures, 132.0);
                                });
                            });
                            ui.add_space(6.0);
                            Frame::new()
                                .fill(p.bg)
                                .corner_radius(CornerRadius::same(8))
                                .inner_margin(Margin::same(10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal_top(|ui| {
                                        ui.label(
                                            RichText::new(Icon::COMPARE.glyph())
                                                .size(13.0)
                                                .color(p.accent),
                                        );
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(images::difference_hint(&item, &other))
                                                    .color(p.text),
                                            )
                                            .wrap(),
                                        );
                                    });
                                });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                if !item.reference
                                    && widgets::ghost_button(ui, "Keep this one").clicked()
                                {
                                    events.push(Event::KeepOnly(g, i));
                                }
                                if !other.reference
                                    && widgets::ghost_button(ui, "Keep the other").clicked()
                                {
                                    events.push(Event::KeepOnly(g, k));
                                }
                            });
                            if group_items.len() > 2 {
                                ui.label(
                                    RichText::new(format!(
                                        "This group has {} pictures. Click another row to compare it.",
                                        group_items.len()
                                    ))
                                    .size(12.5)
                                    .color(p.faint),
                                );
                            }
                        } else {
                            self.preview_card(ui, cx, &item, "This file", is_pic && show_pictures, 190.0);
                        }
                        if comparing {
                            // The two pictures were compared above.
                        } else if grouped {
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Also in this group")
                                    .font(semibold(13.0))
                                    .color(p.muted),
                            );
                            for (k, other) in group_items.iter().enumerate() {
                                if k == i {
                                    continue;
                                }
                                let (rect, resp) = ui.allocate_exact_size(
                                    vec2(ui.available_width(), 40.0),
                                    Sense::click(),
                                );
                                if resp.hovered() {
                                    ui.painter()
                                        .rect_filled(rect, CornerRadius::same(8), p.hover);
                                }
                                let (a, b) = rect
                                    .shrink2(vec2(6.0, 3.0))
                                    .split_top_bottom_at_fraction(0.5);
                                let mark = if other.reference {
                                    "reference"
                                } else if other.selected {
                                    "selected"
                                } else {
                                    "stays"
                                };
                                widgets::text_in(
                                    ui,
                                    a,
                                    &format!("{}  ·  {mark}", file_name(&other.file.path)),
                                    regular(13.0),
                                    p.text,
                                );
                                widgets::text_in(
                                    ui,
                                    b,
                                    &parent_text(&other.file.path),
                                    regular(12.0),
                                    p.faint,
                                );
                                if resp.clicked() {
                                    events.push(Event::Focus(g, k));
                                }
                            }
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if widgets::ghost_button(ui, &Icon::EYE.with("Open")).clicked() {
                                events.push(Event::Open(item.file.path.clone()));
                            }
                            if widgets::ghost_button(
                                ui,
                                &Icon::FOLDER_OPEN.with("Show in Explorer"),
                            )
                            .clicked()
                            {
                                events.push(Event::Reveal(item.file.path.clone()));
                            }
                        });
                    });
            });
        if close {
            self.preview_open = false;
        }
    }

    /// One file in the preview panel: picture (if any), name, folder, facts.
    #[allow(clippy::too_many_arguments)]
    fn preview_card(
        &mut self,
        ui: &mut Ui,
        cx: &ResultsCtx<'_>,
        item: &Item,
        title: &str,
        picture: bool,
        max_h: f32,
    ) {
        let p = pal(ui);
        ui.label(RichText::new(title).font(semibold(12.0)).color(p.muted));
        if picture {
            let width = ui.available_width();
            let (w, h) = match item.detail {
                Detail::Image {
                    width: iw,
                    height: ih,
                    ..
                } if iw > 0 && ih > 0 => (iw as f32, ih as f32),
                _ => (4.0, 3.0),
            };
            let scale = (width / w).min(max_h / h);
            let size = vec2((w * scale).round(), (h * scale).round());
            let (rect, _) = ui.allocate_exact_size(vec2(width, size.y), Sense::hover());
            let frame = Rect::from_center_size(rect.center(), size);
            ui.painter().rect_filled(rect, CornerRadius::same(8), p.bg);
            let loading = self.thumbs.loading(&item.file.path, 512);
            match self.thumbs.get(ui.ctx(), &item.file.path, 512, cx.demo) {
                Some(tex) => {
                    ui.painter().image(
                        tex.id(),
                        frame,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                }
                None => {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        if loading { "Loading…" } else { "No preview" },
                        regular(12.5),
                        p.faint,
                    );
                }
            }
        }
        ui.add(
            egui::Label::new(RichText::new(file_name(&item.file.path)).font(semibold(14.0))).wrap(),
        );
        ui.add_space(-4.0);
        ui.add(
            egui::Label::new(
                RichText::new(parent_text(&item.file.path))
                    .size(12.5)
                    .color(p.muted),
            )
            .wrap(),
        );
        let mut facts: Vec<String> = Vec::new();
        match &item.detail {
            Detail::Image { width, height, .. } => {
                facts.push(format!("{width} x {height}"));
                facts.push(format!(
                    "{:.1} megapixels",
                    *width as f64 * *height as f64 / 1_000_000.0
                ));
            }
            Detail::Audio {
                artist,
                title,
                album,
                duration_secs,
                bitrate_kbps,
            } => {
                for v in [title, artist, album] {
                    if !v.is_empty() {
                        facts.push(v.clone());
                    }
                }
                if *duration_secs > 0 {
                    facts.push(fmt_track(*duration_secs));
                }
                if *bitrate_kbps > 0 {
                    facts.push(format!("{bitrate_kbps} kbps"));
                }
            }
            Detail::Shortcut { target } => facts.push(format!("Points to {target} (missing)")),
            Detail::Note(note) => facts.push(note.clone()),
            Detail::None => {}
        }
        if !item.file.is_dir {
            facts.push(fmt_size(item.file.size));
        }
        facts.push(format!(
            "Changed {}",
            fmt_datetime(item.file.modified, cx.tz_offset)
        ));
        if item.file.created > 0 && !matches!(item.detail, Detail::Image { .. }) {
            facts.push(format!(
                "Created {}",
                fmt_datetime(item.file.created, cx.tz_offset)
            ));
        }
        ui.add(
            egui::Label::new(RichText::new(facts.join("  ·  ")).size(12.5).color(p.text)).wrap(),
        );
        ui.add_space(2.0);
    }
}

/// Paints a texture so that it covers `rect` (cropping the overflow), with rounded corners.
fn paint_cover(ui: &Ui, rect: Rect, tex: &TextureHandle, radius: u8) {
    let [w, h] = tex.size();
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let target = rect.width() / rect.height();
    let source = w / h;
    let uv = if source > target {
        let keep = target / source;
        Rect::from_min_max(pos2((1.0 - keep) / 2.0, 0.0), pos2((1.0 + keep) / 2.0, 1.0))
    } else {
        let keep = source / target;
        Rect::from_min_max(pos2(0.0, (1.0 - keep) / 2.0), pos2(1.0, (1.0 + keep) / 2.0))
    };
    egui::Image::new((tex.id(), rect.size()))
        .uv(uv)
        .corner_radius(CornerRadius::same(radius))
        .paint_at(ui, rect);
}

/// The glyph and colour of the little tile that stands for a file's kind.
fn type_tile(p: &Palette, item: &Item) -> (Icon, Color32) {
    if item.file.is_dir {
        return (Icon::FOLDER_OPEN, p.accent);
    }
    if matches!(item.detail, Detail::Note(_)) {
        return (Icon::BROOM, p.muted);
    }
    match extension(&item.file.path).as_str() {
        "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "gif" | "bmp" | "webp" | "tif" | "tiff"
        | "cr3" | "psb" => (Icon::PICTURE, p.info),
        "mp3" | "flac" | "m4a" | "m4b" | "aac" | "ogg" | "oga" | "opus" | "wav" | "wma"
        | "aiff" | "aif" | "ape" | "wv" | "m3u" => {
            (Icon::MUSIC, Color32::from_rgb(0xb0, 0x7c, 0xf0))
        }
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "wmv" => {
            (Icon::PLAY, Color32::from_rgb(0xf0, 0x6f, 0x9a))
        }
        "lnk" => (Icon::BROKEN_LINK, p.warn),
        "zip" | "7z" | "rar" | "tar" | "gz" | "iso" | "vhdx" => (Icon::SAVE, p.warn),
        "exe" | "msi" | "dll" => (Icon::SETTINGS, p.muted),
        "tmp" | "temp" | "bak" | "old" | "dmp" | "crdownload" | "part" | "partial" | "swp"
        | "db" => (Icon::BROOM, p.muted),
        _ => (Icon::DOCUMENT, p.good),
    }
}

/// Up to two short lines for the detail column.
fn detail_lines(item: &Item) -> (String, String) {
    match &item.detail {
        Detail::None => (String::new(), String::new()),
        Detail::Image {
            width,
            height,
            similarity,
        } => (
            format!("{width} x {height}"),
            format!(
                "{:.1} MP · {similarity}% alike",
                *width as f64 * *height as f64 / 1_000_000.0
            ),
        ),
        Detail::Audio {
            artist,
            title,
            duration_secs,
            bitrate_kbps,
            ..
        } => {
            let top = if artist.is_empty() {
                title.clone()
            } else {
                format!("{artist} - {title}")
            };
            let mut bottom: Vec<String> = Vec::new();
            if *duration_secs > 0 {
                bottom.push(fmt_track(*duration_secs));
            }
            if *bitrate_kbps > 0 {
                bottom.push(format!("{bitrate_kbps} kbps"));
            }
            bottom.push(extension(&item.file.path).to_uppercase());
            (top, bottom.join(" · "))
        }
        Detail::Shortcut { target } => ("Target is missing".to_string(), target.clone()),
        Detail::Note(note) => (note.clone(), String::new()),
    }
}

fn empty_words(finder: Finder) -> (&'static str, &'static str) {
    match finder {
        Finder::Duplicates => (
            "No duplicate files",
            "Every file in the scanned folders is one of a kind.",
        ),
        Finder::SimilarImages => (
            "No similar pictures",
            "No two pictures look alike at this similarity setting. Lower it on the start page to cast a wider net.",
        ),
        Finder::SimilarMusic => (
            "No similar music",
            "No two tracks share the chosen tags.",
        ),
        Finder::SameName => (
            "No shared names",
            "No two files in the scanned folders have the same name.",
        ),
        Finder::EmptyFiles => ("No empty files", "Nothing with a size of zero was found."),
        Finder::EmptyFolders => ("No empty folders", "Every folder holds at least one file."),
        Finder::BrokenShortcuts => (
            "No broken shortcuts",
            "Every shortcut that was found still points somewhere.",
        ),
        Finder::BigFiles => ("No files", "Nothing passed the filters."),
        Finder::TempFiles => (
            "No temporary files",
            "No leftover temporary or backup files were found.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FileEntry, Group};

    fn item(path: &str, size: u64, modified: i64) -> Item {
        Item::new(FileEntry {
            path: PathBuf::from(path),
            size,
            modified,
            ..Default::default()
        })
    }

    fn sample() -> ScanResult {
        let mut r = ScanResult::empty(Finder::Duplicates);
        r.groups = vec![
            Group::new(
                vec![
                    item("X:\\a\\small.txt", 10, 5),
                    item("X:\\b\\small.txt", 10, 9),
                ],
                "one",
            ),
            Group::new(
                vec![
                    item("X:\\a\\Big.bin", 500, 1),
                    item("X:\\b\\big copy.bin", 500, 2),
                    item("X:\\c\\big copy 2.bin", 500, 3),
                ],
                "two",
            ),
            Group::new(
                vec![
                    item("X:\\m\\mid.dat", 100, 7),
                    item("X:\\n\\mid.dat", 100, 8),
                ],
                "three",
            ),
        ];
        r
    }

    #[test]
    fn rows_follow_sort_filter_search_and_collapse() {
        let r = sample();
        let none = HashSet::new();
        let order = sorted_groups(&r, SortKey::Space, true);
        assert_eq!(order, [1, 2, 0]);
        let rows = build_rows(&r, &order, "", Show::All, &none);
        assert_eq!(rows.len(), 3 + 7);
        assert_eq!(rows[0], Row::Header(1));
        assert_eq!(rows[1], Row::Item(1, 0));
        assert_eq!(rows[4], Row::Header(2));

        assert_eq!(sorted_groups(&r, SortKey::Space, false), [0, 2, 1]);
        assert_eq!(sorted_groups(&r, SortKey::Name, false), [1, 2, 0]);
        assert_eq!(sorted_groups(&r, SortKey::Date, true), [0, 2, 1]);
        assert_eq!(sorted_groups(&r, SortKey::Count, true), [1, 0, 2]);
        assert_eq!(sorted_groups(&r, SortKey::Folder, false), [0, 1, 2]);

        // Search keeps whole groups in which any file matches.
        let rows = build_rows(&r, &order, "  COPY 2 ", Show::All, &none);
        assert_eq!(rows.len(), 4);
        assert!(build_rows(&r, &order, "nothing like this", Show::All, &none).is_empty());

        // Collapsed groups keep their header only.
        let collapsed: HashSet<usize> = [1].into_iter().collect();
        let rows = build_rows(&r, &order, "", Show::All, &collapsed);
        assert_eq!(rows.len(), 3 + 4);
        assert_eq!(rows[1], Row::Header(2));

        // Selection filters.
        let mut r = r;
        r.groups[2].items[1].selected = true;
        assert_eq!(
            build_rows(&r, &order, "", Show::Selected, &none),
            [Row::Header(2), Row::Item(2, 0), Row::Item(2, 1)]
        );
        assert_eq!(
            build_rows(&r, &order, "", Show::Unselected, &none).len(),
            2 + 5
        );

        // Flat lists have no headers.
        let mut flat = ScanResult::empty(Finder::BigFiles);
        flat.groups = vec![
            Group::new(vec![item("X:\\a\\s", 1, 1)], ""),
            Group::new(vec![item("X:\\a\\l", 9, 1)], ""),
        ];
        let order = sorted_groups(&flat, SortKey::Space, true);
        assert_eq!(
            build_rows(&flat, &order, "", Show::All, &none),
            [Row::Item(1, 0), Row::Item(0, 0)]
        );
    }

    #[test]
    fn removing_paths_drops_broken_groups_and_keeps_the_rules() {
        let mut state = ResultsState::new(sample());
        assert_eq!(state.rows().len(), 10);
        state.result.groups[1].items[1].selected = true;
        state.result.groups[1].items[2].selected = true;
        state.focus = Some((1, 0));
        state.preview_open = true;
        let gone: HashSet<PathBuf> = [
            PathBuf::from("X:\\b\\big copy.bin"),
            PathBuf::from("X:\\b\\small.txt"),
        ]
        .into_iter()
        .collect();
        state.remove_paths(&gone);
        // The "small" group fell to one file and disappeared.
        assert_eq!(state.result.groups.len(), 2);
        assert_eq!(state.result.groups[0].items.len(), 2);
        assert!(select::is_valid(&state.result.groups));
        assert_eq!(state.focus, Some((0, 0)), "the highlight follows its file");
        assert_eq!(state.rows().len(), 2 + 4);
        // Removing the highlighted file closes the preview.
        let gone: HashSet<PathBuf> = [PathBuf::from("X:\\a\\Big.bin")].into_iter().collect();
        state.remove_paths(&gone);
        assert_eq!(state.focus, None);
        assert!(!state.preview_open);
        assert_eq!(state.result.groups.len(), 1);

        // A flat list simply loses the rows.
        let mut flat = ScanResult::empty(Finder::EmptyFiles);
        flat.groups = vec![
            Group::new(vec![item("X:\\a\\z1", 0, 1)], ""),
            Group::new(vec![item("X:\\a\\z2", 0, 1)], ""),
        ];
        let mut state = ResultsState::new(flat);
        let gone: HashSet<PathBuf> = [PathBuf::from("X:\\a\\z1")].into_iter().collect();
        state.remove_paths(&gone);
        assert_eq!(state.rows(), [Row::Item(0, 0)]);
    }

    #[test]
    fn poses_and_toggles_respect_the_invariant() {
        for finder in Finder::ALL {
            let mut state = ResultsState::new(demo::result(finder));
            state.pose("selected");
            assert!(state.result.selected_count() > 0, "{finder:?}");
            assert!(select::is_valid(&state.result.groups));
            state.pose("compare");
            assert!(state.focus.is_some() && state.preview_open, "{finder:?}");
            state.pose("no such pose");
        }
        let mut state = ResultsState::new(sample());
        state.toggle(0, 0);
        state.toggle(0, 1); // refused: the last one stays
        assert_eq!(state.result.groups[0].selected_count(), 1);
        assert!(state.note.is_some());
        state.toggle(0, 0);
        assert_eq!(state.result.groups[0].selected_count(), 0);
        state.toggle(9, 9); // out of range: ignored
        state.apply_event(Event::SelectFolder(PathBuf::from("X:\\b")));
        assert_eq!(state.result.selected_count(), 2);
        state.apply_event(Event::KeepOnly(1, 2));
        assert_eq!(state.result.groups[1].selected_count(), 2);
        assert!(select::is_valid(&state.result.groups));
        assert_eq!(detail_lines(&state.result.groups[0].items[0]).0, "");
    }
}
