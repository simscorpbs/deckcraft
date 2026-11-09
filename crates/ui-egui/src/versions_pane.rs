//! The Version History pane, the Previous version bar and the title bar's file menu.
//!
//! As in PowerPoint with a presentation in OneDrive, File › Info › Version History opens a pane on
//! the right that lists the versions newest first, grouped by day, each with its time, its author
//! and an Open version link. An opened version is a read only presentation of its own, shown
//! beside the one it came from (the engine refuses what would change it, `cmd::version_view`): the
//! bar above it offers Compare, Restore, Save a copy and Close, and its row in the pane is
//! highlighted. Closing it puts the presentation it came from in front, exactly as it was.
//!
//! The engine does the work (`file.versions`, `file.openVersion`, `file.compareVersion`,
//! `file.closeVersion`, `file.rename`); this module decides what to show and runs AutoSave first,
//! so the state a version is opened or restored from is in the history too.

use chrono::NaiveDate;
use deckcraft_engine::autosave::Flush;
use deckcraft_engine::versions::{READ_ONLY, VersionInfo, local_timestamp};
use deckcraft_text::datetime::{DateTime, field_text};
use egui::{Align, CornerRadius, Key, Sense, Stroke, Ui};
use serde_json::{Value, json};

use crate::SlideApp;
use crate::clock::monotonic_secs;
use crate::theme::{self, Tokens};

/// How long a listing of the history is reused: reading it means reading the history folder.
const LIST_TTL_SECS: f64 = 1.0;

/// What the pane and the file menu keep between frames. Not saved.
#[derive(Default)]
pub struct State {
    /// The history of one presentation (its uid), as last listed, and when.
    cache: Option<(u64, f64, Vec<VersionInfo>)>,
    /// The file menu's name field: the presentation (uid) it was filled for, and its text.
    name_draft: Option<(u64, String)>,
    /// Whether the presentation in front was a previous version at the last frame.
    was_previous: bool,
}

// ---------------------------------------------------------------- dates

/// A local date and time as [`local_timestamp`] writes it (`YYYY-MM-DD HH:MM`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
}

impl LocalTime {
    pub(crate) fn parse(local: &str) -> Option<Self> {
        let num = |range: std::ops::Range<usize>| local.get(range).and_then(|s| s.parse::<u32>().ok());
        let year = local.get(0..4).and_then(|s| s.parse::<i32>().ok())?;
        let time = Self { year, month: num(5..7)?, day: num(8..10)?, hour: num(11..13)?, minute: num(14..16)? };
        (time.date().is_some() && time.hour < 24 && time.minute < 60).then_some(time)
    }

    /// The computer's local date and time now (`None` where there is no clock).
    pub(crate) fn now() -> Option<Self> {
        let n = DateTime::now_local()?;
        Some(Self { year: n.year, month: n.month, day: n.day, hour: n.hour, minute: n.minute })
    }

    fn date(self) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(self.year, self.month, self.day)
    }

    fn as_datetime(self) -> DateTime {
        DateTime::new(self.year, self.month, self.day, self.hour, self.minute, 0)
    }
}

/// The interface language's primary subtag (`pt` for Brazilian Portuguese).
fn language() -> &'static str {
    let code = crate::i18n::current().code();
    code.split(['-', '_']).next().unwrap_or(code)
}

/// A date written the way the interface language writes it: English and Portuguese with month
/// names (`November 9, 2026`, `9 de novembro de 2026`), other languages as an ISO date.
fn long_date(time: LocalTime) -> String {
    let named = match language() {
        "en" => field_text("datetime4", Some("en-US"), time.as_datetime()),
        "pt" => field_text("datetime4", Some("pt-BR"), time.as_datetime()),
        _ => None,
    };
    named.unwrap_or_else(|| format!("{:04}-{:02}-{:02}", time.year, time.month, time.day))
}

/// A day heading: Today, Yesterday, or the date.
pub(crate) fn day_heading(day: LocalTime, today: LocalTime) -> String {
    let days_ago = match (today.date(), day.date()) {
        (Some(today), Some(day)) => Some(today.signed_duration_since(day).num_days()),
        _ => None,
    };
    match days_ago {
        Some(0) => crate::tl!("Today").to_string(),
        Some(1) => crate::tl!("Yesterday").to_string(),
        _ => long_date(day),
    }
}

/// A version's time of day in the interface language: `3:05 PM` in English, `15:05` elsewhere.
pub(crate) fn time_of_day(time: LocalTime) -> String {
    if language() == "en" {
        field_text("datetime12", Some("en-US"), time.as_datetime()).unwrap_or_else(|| format!("{:02}:{:02}", time.hour, time.minute))
    } else {
        format!("{:02}:{:02}", time.hour, time.minute)
    }
}

/// A version's day and time for the Previous version bar: `Today, 3:05 PM`.
pub(crate) fn date_and_time(saved_at: &str) -> String {
    let local = local_timestamp(saved_at);
    let (Some(time), Some(today)) = (LocalTime::parse(&local), LocalTime::now()) else { return local };
    format!("{}, {}", day_heading(time, today), time_of_day(time))
}

/// A version's full date and time, for its tooltip: `November 9, 2026 3:05 PM`.
pub(crate) fn full_date_and_time(saved_at: &str) -> String {
    let local = local_timestamp(saved_at);
    match LocalTime::parse(&local) {
        Some(time) => format!("{} {}", long_date(time), time_of_day(time)),
        None => local,
    }
}

// ---------------------------------------------------------------- the commands, with AutoSave first

/// The status line for a failed command: an edit refused in a previous version says why in the
/// interface language.
pub(crate) fn status_text(error: &str) -> String {
    if error == READ_ONLY { crate::tl!(READ_ONLY).to_string() } else { error.to_string() }
}

impl SlideApp {
    /// Before a version is opened or restored, AutoSave writes the presentation if it has unsaved
    /// changes, so that state is in the history too (restoring also keeps it by itself). `true`
    /// when the command can go on: a save conflict stops it and opens its dialog.
    pub(crate) fn versions_before(&mut self, id: &str, params: &Value) -> bool {
        let restoring = id == "file.versions" && params.get("restore").is_some_and(Value::is_string);
        if id != "file.openVersion" && !restoring {
            return true;
        }
        let at = match params.get("document").and_then(Value::as_u64) {
            Some(uid) => self.session.index_of_uid(uid),
            None => self.session.active_index(),
        };
        let Some(owner) = at.and_then(|i| self.session.history_owner(i)) else { return true };
        self.autosave_before_close(owner) != Flush::Conflict
    }

    /// After a command in [`SlideApp::run`]: forget the listing the command made stale, and when
    /// Compare shows the version again, bring back the history pane the comparison replaced.
    pub(crate) fn versions_after(&mut self, id: &str, result: &Result<Value, String>) {
        if id.starts_with("file.") {
            self.versions.cache = None;
        }
        if id == "file.compareVersion"
            && result.as_ref().is_ok_and(|v| v["comparing"] == false)
            && self.ui.pane.as_deref() == Some("revisions")
        {
            self.ui.pane = Some("versions".into());
        }
    }

    /// The history of the presentation in front (or of the one a previous version came from),
    /// newest first. Listed at most once a second.
    pub(crate) fn history(&mut self) -> Vec<VersionInfo> {
        let Some(owner) = self.session.active_index().and_then(|i| self.session.history_owner(i)) else { return Vec::new() };
        let Some(uid) = self.session.documents().get(owner).map(|d| d.uid) else { return Vec::new() };
        let now = monotonic_secs();
        if let Some((cached, at, items)) = &self.versions.cache
            && *cached == uid
            && now - *at < LIST_TTL_SECS
        {
            return items.clone();
        }
        let items = self.session.version_infos(owner);
        self.versions.cache = Some((uid, now, items.clone()));
        items
    }

    /// Forget the listing, so the next frame reads the history again.
    pub fn invalidate_history(&mut self) {
        self.versions.cache = None;
    }

    /// Open version `id` read only beside the presentation (AutoSave writes it first).
    pub fn open_version(&mut self, id: &str) -> Result<Value, String> {
        self.run("file.openVersion", json!({"id": id}))
    }

    /// Restore version `id`: back to the presentation, which the version replaces in one Undo step.
    pub fn restore_version(&mut self, id: &str) -> Result<Value, String> {
        self.run("file.versions", json!({"restore": id}))
    }

    /// Save the previous version in front as a new file (the system Save dialog), then go back to
    /// the presentation it came from.
    pub fn save_version_copy(&mut self) {
        let Some(owner) = self.session.active_index().and_then(|i| self.session.history_owner(i)) else { return };
        let Some(version) = self.session.active_previous_version().map(|v| v.info.clone()) else { return };
        let ext = self
            .session
            .documents()
            .get(owner)
            .and_then(|d| d.path.as_deref())
            .and_then(|p| std::path::Path::new(p).extension())
            .map_or_else(|| "pptx".to_string(), |e| e.to_string_lossy().into_owned());
        let stem = self.session.documents().get(owner).map(|d| d.title()).unwrap_or_else(|| "Presentation".into());
        // A file name cannot hold the clock's colon.
        let when = local_timestamp(&version.saved_at).replace(':', ".");
        let suggested = format!("{stem} (version {when}).{ext}");
        let Some(pick) = self.services.pick_save.as_mut() else {
            self.set_status(crate::tl!("Save a copy needs the desktop app"));
            return;
        };
        let Some(path) = pick(&suggested) else { return };
        if self.run("file.saveAs", json!({"path": path})).is_ok() {
            self.set_status(crate::i18n::fmt(crate::tl!("Saved a copy of the previous version as {path}"), &[("path", path.as_str())]));
        }
    }

    /// The file menu's name field was applied: rename the presentation's file.
    pub fn rename_file(&mut self, name: &str) -> Result<(), String> {
        let old = self.session.active().and_then(|d| d.path.clone());
        let done = self.run("file.rename", json!({"name": name}))?;
        if let (Some(old), Some(new)) = (old, done.get("path").and_then(Value::as_str))
            && done["renamed"] == true
        {
            let was = self.ui.recent.iter().position(|p| *p == old);
            self.ui.recent.retain(|p| *p != old && p != new);
            if let Some(at) = was {
                self.ui.recent.insert(at.min(self.ui.recent.len()), new.to_string());
            }
            self.set_status(crate::i18n::fmt(crate::tl!("Renamed to {name}"), &[("name", &file_name(new))]));
        }
        Ok(())
    }

    /// A shown version must not outlive what it is shown beside: quitting returns to the
    /// presentation first, so a shown version never hides unsaved work (it has none of its own).
    pub(crate) fn return_from_versions(&mut self) {
        if self.session.documents().iter().any(|d| d.previous.is_some()) {
            self.session.close_previous_versions();
        }
    }

    /// Every frame: when the version in front is gone while its comparison (the Revisions pane)
    /// was open, the pane goes back to the history it came from.
    pub(crate) fn sync_versions(&mut self) {
        let previous = self.session.active_previous_version().is_some();
        if self.versions.was_previous && !previous && self.ui.pane.as_deref() == Some("revisions") {
            self.ui.pane = Some("versions".into());
        }
        self.versions.was_previous = previous;
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path).file_name().map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------- the pane

/// Why the pane has no versions to list.
fn empty_message(app: &SlideApp, owner_has_file: bool) -> &'static str {
    let browser = cfg!(target_arch = "wasm32") || app.services.download.is_some() || app.services.host_save.is_some();
    if browser {
        "Version history is kept by the desktop app."
    } else if !owner_has_file {
        "Save the presentation to start its version history."
    } else if !app.session.versions_enabled() {
        "Version history isn't available here."
    } else {
        "No versions yet."
    }
}

/// The Version History pane's contents (the task pane draws its title and Close).
pub(crate) fn pane(app: &mut SlideApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let versions = app.history();
    if versions.is_empty() {
        let has_file = app.session.active_index().and_then(|i| app.session.history_owner(i)).and_then(|i| app.session.documents().get(i)).is_some_and(|d| d.path.is_some());
        ui.label(egui::RichText::new(crate::tl!(empty_message(app, has_file))).font(theme::font(12.5)).color(t.text_dim));
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        return;
    }
    let viewing = app.session.active_previous_version().map(|v| v.info.id.clone());
    let today = LocalTime::now();
    let mut day: Option<(i32, u32, u32)> = None;
    for (n, info) in versions.iter().enumerate() {
        let local = local_timestamp(&info.saved_at);
        let time = LocalTime::parse(&local);
        let this_day = time.map(|t| (t.year, t.month, t.day));
        if n == 0 || this_day != day {
            day = this_day;
            ui.add_space(if n == 0 { 0.0 } else { 8.0 });
            let heading = match (time, today) {
                (Some(time), Some(today)) => day_heading(time, today),
                _ => local.get(..10).unwrap_or(&local).to_string(),
            };
            ui.label(egui::RichText::new(heading).font(theme::bold(12.5)).color(t.text));
            ui.add_space(2.0);
        }
        let shown = time.map_or_else(|| local.clone(), time_of_day);
        version_row(app, ui, info, &shown, viewing.as_deref() == Some(info.id.as_str()), &t);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
}

/// The id of a version's row in the pane.
pub(crate) fn row_id(version: &str) -> egui::Id {
    egui::Id::unique(("version_row", version))
}

/// One version: its time and author, and Open version. The version being viewed is highlighted
/// and has no link (it is already open). Right-click offers Open version and Restore.
fn version_row(app: &mut SlideApp, ui: &mut Ui, info: &VersionInfo, time: &str, viewing: bool, t: &Tokens) {
    let fill = if viewing { t.selected } else { egui::Color32::TRANSPARENT };
    // The row senses clicks under its contents (the link keeps its own), for its menu.
    let row = egui::UiBuilder::new().id_salt(row_id(&info.id)).sense(Sense::click());
    let inner = ui.scope_builder(row, |ui| {
        egui::Frame::NONE.fill(fill).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(time).font(theme::bold(12.5)).color(t.text));
            ui.label(egui::RichText::new(info.author.clone()).font(theme::font(11.5)).color(t.text_dim));
            if !viewing {
                let link = egui::Button::new(egui::RichText::new(crate::tl!("Open version")).font(theme::font(12.0)).color(t.accent)).frame(false);
                if ui.add(link).clicked() {
                    let _ = app.open_version(&info.id);
                }
            }
        });
    });
    let response = inner.response.on_hover_text(full_date_and_time(&info.saved_at));
    response.context_menu(|ui| {
        if !viewing && ui.button(crate::tl!("Open version")).clicked() {
            let _ = app.open_version(&info.id);
            ui.close();
        }
        if ui.button(crate::tl!("Restore")).clicked() {
            let _ = app.restore_version(&info.id);
            ui.close();
        }
    });
    ui.add_space(2.0);
}

// ---------------------------------------------------------------- the bar above a previous version

/// The bar PowerPoint shows above a presentation opened from Version History: which version this
/// is, and Compare, Restore, Save a copy and Close.
pub fn previous_bar(app: &mut SlideApp, ui: &mut Ui) {
    let Some(info) = app.session.active_previous_version().map(|v| v.info.clone()) else { return };
    let comparing = app.session.active().is_some_and(|d| d.compare.is_some());
    let t = Tokens::get(ui.ctx());
    let when = date_and_time(&info.saved_at);
    egui::Panel::top("previous_version_bar")
        .exact_size(36.0)
        .frame(egui::Frame::NONE.fill(t.selected).inner_margin(egui::Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, t.border)))
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(egui::RichText::new(crate::tl!("Previous version")).font(theme::bold(12.5)).color(t.text));
                ui.label(egui::RichText::new(format!("{when} · {}", info.author)).font(theme::font(12.0)).color(t.text_dim));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    if ui.button(crate::tl!("Close")).on_hover_text(crate::tl!("Back to the current version")).clicked() {
                        let _ = app.run("file.closeVersion", json!({}));
                    }
                    if ui.button(crate::tl!("Save a copy")).clicked() {
                        app.save_version_copy();
                    }
                    let restore = egui::Button::new(egui::RichText::new(crate::tl!("Restore")).color(t.accent_text)).fill(t.accent);
                    if ui.add(restore).clicked() {
                        let _ = app.restore_version(&info.id);
                    }
                    let tip = if comparing { crate::tl!("Show the previous version again") } else { crate::tl!("Show what changed since this version") };
                    if ui.add(egui::Button::new(crate::tl!("Compare")).selected(comparing)).on_hover_text(tip).clicked() {
                        let _ = app.run("file.compareVersion", json!({}));
                    }
                });
            });
        });
}

// ---------------------------------------------------------------- the title bar's file menu

/// The title bar text: the presentation's name, `Edited` when it has unsaved changes, and for a
/// version `Name (version <date time>) — Previous version`.
pub(crate) fn title_text(app: &SlideApp) -> String {
    match app.session.active() {
        Some(d) if d.previous.is_some() => format!("{} — {}", d.title(), crate::tl!("Previous version")),
        Some(d) if d.is_dirty() => format!("{} — {}", d.title(), crate::tl!("Edited")),
        Some(d) => d.title(),
        None => "DeckCraft".into(),
    }
}

/// The file menu that opens from the document name in the title bar, as PowerPoint's does for a
/// OneDrive file: the name (changed there, applied with Enter), the folder, and Version History.
pub(crate) fn file_menu(app: &mut SlideApp, response: &egui::Response) {
    let Some((uid, path, title, previous)) = app.session.active().map(|d| (d.uid, d.path.clone(), d.title(), d.previous.is_some())) else { return };
    let mut popup = egui::Popup::menu(response).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).width(300.0);
    // The right button opens it as well as the left.
    if response.secondary_clicked() {
        popup = popup.open_memory(egui::SetOpenCommand::Bool(true));
    }
    let t = Tokens::get(&response.ctx);
    let ext = path.as_deref().and_then(|p| std::path::Path::new(p).extension()).map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    // The name field starts as the file's name each time the menu opens, with the focus in it.
    let (mut draft, fresh) = match app.versions.name_draft.take() {
        Some((u, text)) if u == uid => (text, false),
        _ => (title.clone(), true),
    };
    let mut apply = false;
    let mut history = false;
    let opened = popup.show(|ui| {
        ui.set_min_width(280.0);
        ui.label(egui::RichText::new(crate::tl!("File name")).font(theme::font(11.5)).color(t.text_dim));
        if path.is_some() && !previous {
            let field = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(f32::INFINITY));
            if fresh {
                field.request_focus();
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                apply = true;
            }
            if !ext.is_empty() {
                ui.label(egui::RichText::new(crate::i18n::fmt(crate::tl!("The {ext} extension stays"), &[("ext", &ext)])).font(theme::font(11.0)).color(t.text_faint));
            }
        } else {
            ui.label(egui::RichText::new(&title).font(theme::bold(13.0)).color(t.text));
        }
        ui.add_space(6.0);
        let folder = path.as_deref().and_then(|p| std::path::Path::new(p).parent()).map(|d| d.to_string_lossy().into_owned());
        let folder = if previous { None } else { folder.filter(|f| !f.is_empty()) };
        ui.label(egui::RichText::new(crate::tl!("Folder")).font(theme::font(11.5)).color(t.text_dim));
        ui.label(egui::RichText::new(folder.unwrap_or_else(|| crate::tl!("Not saved").to_string())).font(theme::font(12.5)).color(t.text));
        ui.separator();
        if ui.button(crate::tl!("Version History")).clicked() {
            history = true;
            ui.close();
        }
    });
    // Closed: the next opening starts from the file's name again.
    if opened.is_none() {
        app.versions.name_draft = None;
        return;
    }
    if apply {
        let name = draft.trim().to_string();
        if app.rename_file(&name).is_ok() {
            app.versions.name_draft = None;
            egui::Popup::close_id(&response.ctx, egui::Popup::default_response_id(response));
            return;
        }
    }
    app.versions.name_draft = Some((uid, draft));
    if history {
        app.ui.pane = Some("versions".into());
    }
}
