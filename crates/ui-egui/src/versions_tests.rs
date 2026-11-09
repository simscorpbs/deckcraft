//! Version History in the window: the pane (days, rows, empty states), the title bar's file menu,
//! a version opened read only beside the presentation with its Previous version bar, Compare,
//! Restore, Save a copy, Close, New, Open and Quit while a version is shown, and Portuguese labels.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use deckcraft_engine::Session;
use deckcraft_engine::versions::{READ_ONLY, VersionInfo};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::i18n::Lang;
use crate::versions_pane::{LocalTime, day_heading, time_of_day};
use crate::{Services, SlideApp};

fn harness(app: SlideApp) -> Harness<'static, SlideApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("deckcraft-versions-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Desktop services whose Save dialog answers with `dir`/<suggested name>.
fn saving_into(dir: &Path) -> Services {
    let dir = dir.to_path_buf();
    Services { pick_save: Some(Box::new(move |name: &str| Some(dir.join(name).to_string_lossy().to_string()))), ..Default::default() }
}

fn slides(app: &SlideApp) -> usize {
    app.session.active().unwrap().doc.slides.len()
}

fn wait_for(app: &mut SlideApp, n: usize) -> Vec<VersionInfo> {
    assert!(app.session.version_wait_idle(std::time::Duration::from_secs(30)), "the worker did not finish");
    app.invalidate_history();
    let owner = app.session.active_index().and_then(|i| app.session.history_owner(i)).unwrap();
    let list = app.session.version_infos(owner);
    assert!(list.len() >= n, "{} versions, wanted {n}", list.len());
    list
}

/// `Quarterly.pptx` saved with one slide, then two; a third slide is not saved yet. Returns the
/// app, its folder and the ids of the oldest and the newest version.
fn app_with_history(name: &str) -> (SlideApp, PathBuf, String, String) {
    crate::i18n::set_current(Lang::EN);
    let d = dir(name);
    let mut session = Session::new();
    session.prefs.author = "Tester Two".into();
    session.enable_versions(d.join("versions"), Arc::new(|| {})).unwrap();
    session.execute("file.new", &json!({})).unwrap();
    let mut app = SlideApp::new(session, saving_into(&d));
    let file = d.join("Quarterly.pptx");
    app.run("file.saveAs", json!({"path": file.to_string_lossy()})).unwrap();
    app.run("slide.new", json!({"layout": "blank"})).unwrap();
    app.run("file.save", json!({})).unwrap();
    app.run("slide.new", json!({"layout": "blank"})).unwrap();
    let list = wait_for(&mut app, 2);
    (app, d, list[1].id.clone(), list[0].id.clone())
}

fn with_pane(app: &mut SlideApp) {
    app.run("view.pane", json!({"pane": "versions"})).unwrap();
}

fn status(app: &mut SlideApp) -> Option<String> {
    app.fresh_status()
}

#[test]
fn days_are_today_yesterday_or_a_date_in_the_interface_language() {
    let at = |s: &str| LocalTime::parse(s).unwrap();
    let today = at("2026-03-01 09:30");
    crate::i18n::set_current(Lang::EN);
    assert_eq!(day_heading(at("2026-03-01 00:00"), today), "Today");
    assert_eq!(day_heading(at("2026-02-28 23:59"), today), "Yesterday", "across a month end");
    assert_eq!(day_heading(at("2026-02-27 08:00"), today), "February 27, 2026");
    assert_eq!(day_heading(at("2025-12-31 08:00"), at("2026-01-01 08:00")), "Yesterday", "across a year end");
    assert_eq!(time_of_day(at("2026-02-27 15:05")), "3:05 PM");
    assert_eq!(time_of_day(at("2026-02-27 00:07")), "12:07 AM");
    let pt = Lang::from_code("pt-br").unwrap();
    crate::i18n::set_current(pt);
    assert_eq!(day_heading(at("2026-03-01 00:00"), today), "Hoje");
    assert_eq!(day_heading(at("2026-02-28 00:00"), today), "Ontem");
    assert_eq!(day_heading(at("2026-02-27 08:00"), today), "27 de fevereiro de 2026");
    assert_eq!(day_heading(at("2026-03-02 08:00"), today), "2 de março de 2026", "a clock set back is a date, not a day");
    assert_eq!(time_of_day(at("2026-02-27 15:05")), "15:05");
    crate::i18n::set_current(Lang::EN);
    for bad in ["", "garbage", "2026-13-01 00:00", "2026-02-30 00:00", "2026-02-27 24:00"] {
        assert!(LocalTime::parse(bad).is_none(), "{bad:?}");
    }
}

#[test]
fn the_pane_lists_versions_newest_first_under_today_with_time_author_and_open_version() {
    let (mut app, d, _, newest) = app_with_history("pane");
    with_pane(&mut app);
    let mut h = harness(app);
    assert!(h.query_by_label("Version History").is_some(), "the pane's title");
    assert_eq!(h.query_all_by_label("Today").count(), 1, "both were saved today: one heading");
    assert_eq!(h.query_all_by_label("Open version").count(), 2, "every row can be opened");
    assert_eq!(h.query_all_by_label("Tester Two").count(), 2, "and names its author");
    let shown = h.state_mut().session.version_infos(0);
    assert_eq!(shown[0].id, newest, "newest first");
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_older_day_gets_its_own_heading() {
    crate::i18n::set_current(Lang::EN);
    let d = dir("yesterday");
    let mut session = Session::new();
    session.enable_versions(d.join("versions"), Arc::new(|| {})).unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    session.set_version_clock(Arc::new(move || now - 86_400));
    session.execute("file.new", &json!({})).unwrap();
    let mut app = SlideApp::new(session, saving_into(&d));
    app.run("file.saveAs", json!({"path": d.join("Old.pptx").to_string_lossy()})).unwrap();
    assert!(app.session.version_wait_idle(std::time::Duration::from_secs(30)));
    app.session.set_version_clock(Arc::new(move || now));
    app.run("slide.new", json!({})).unwrap();
    app.run("file.save", json!({})).unwrap();
    wait_for(&mut app, 2);
    with_pane(&mut app);
    let h = harness(app);
    assert_eq!(h.query_all_by_label("Today").count(), 1);
    assert_eq!(h.query_all_by_label("Yesterday").count(), 1);
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_pane_says_why_it_is_empty() {
    crate::i18n::set_current(Lang::EN);
    // Never saved.
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    with_pane(&mut app);
    let h = harness(app);
    assert!(h.query_by_label("Save the presentation to start its version history.").is_some());
    drop(h);
    // A file the history has never seen.
    let d = dir("empty");
    let mut other = Session::with_new();
    let file = d.join("Elsewhere.pptx");
    other.execute("file.saveAs", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut session = Session::new();
    session.enable_versions(d.join("versions"), Arc::new(|| {})).unwrap();
    session.execute("file.open", &json!({"path": file.to_string_lossy()})).unwrap();
    let mut app = SlideApp::new(session, Services::default());
    with_pane(&mut app);
    let h = harness(app);
    assert!(h.query_by_label("No versions yet.").is_some());
    drop(h);
    // The browser keeps no history.
    let host = Services { host_save: Some(Box::new(|_: &str, _: &[u8], _: bool| Ok(()))), ..Default::default() };
    let mut app = SlideApp::new(Session::with_new(), host);
    with_pane(&mut app);
    let h = harness(app);
    assert!(h.query_by_label("Version history is kept by the desktop app.").is_some());
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_info_page_opens_the_pane() {
    let (mut app, d, _, _) = app_with_history("info");
    app.run("app.backstage", json!({"page": "info"})).unwrap();
    let mut h = harness(app);
    h.get_by_role_and_label(Role::Button, "Version History").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.pane.as_deref(), Some("versions"));
    assert!(!h.state().backstage_open(), "the File tab gives way to the pane");
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_file_menu_renames_the_file_on_enter_and_opens_version_history() {
    let (app, d, _, _) = app_with_history("menu");
    let mut h = harness(app);
    let before = h.state().session.active().unwrap().path.clone().unwrap();
    // The title is dirty ("Edited"), so its text carries the suffix.
    h.get_by_role_and_label(Role::Button, "Quarterly — Edited").click();
    h.run_steps(4);
    assert!(h.query_by_label("File name").is_some() && h.query_by_label("Folder").is_some());
    assert!(h.query_by_label(d.to_string_lossy().as_ref()).is_some(), "the folder");
    let field = h.get_by_role(Role::TextInput);
    field.click();
    h.run_steps(2);
    h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    h.get_by_role(Role::TextInput).type_text("Budget");
    h.run_steps(2);
    h.key_press(egui::Key::Enter);
    h.run_steps(4);
    let after = h.state().session.active().unwrap().path.clone().unwrap();
    assert_ne!(before, after);
    assert!(after.ends_with("Budget.pptx"), "{after}");
    assert!(Path::new(&after).exists() && !Path::new(&before).exists(), "the file was renamed on disk");
    assert_eq!(h.state().session.documents().len(), 1);
    // The history followed it.
    assert!(h.state_mut().session.version_infos(0).len() >= 2);
    // Version History from the same menu.
    h.get_by_role_and_label(Role::Button, "Budget — Edited").click();
    h.run_steps(4);
    h.get_by_role_and_label(Role::Button, "Version History").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.pane.as_deref(), Some("versions"));
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_bad_name_is_refused_and_the_file_stays() {
    let (mut app, d, _, _) = app_with_history("badname");
    let path = app.session.active().unwrap().path.clone().unwrap();
    assert!(app.rename_file("a/b").is_err());
    assert!(app.rename_file("").is_err());
    assert_eq!(app.session.active().unwrap().path.as_deref(), Some(path.as_str()));
    assert!(Path::new(&path).exists());
    assert!(status(&mut app).is_some(), "the reason is on the status line");
    let _ = std::fs::remove_dir_all(d);
}

/// Open the oldest version from the pane's first link that belongs to it.
fn open_oldest(h: &mut Harness<'static, SlideApp>, oldest: &str) {
    open_oldest_as(h, oldest, "Open version");
}

/// [`open_oldest`] with the link's label in another interface language.
fn open_oldest_as(h: &mut Harness<'static, SlideApp>, oldest: &str, label: &str) {
    let row = h.state_mut().session.version_infos(0).iter().position(|v| v.id == oldest).unwrap();
    h.query_all_by_label(label).nth(row).unwrap().click();
    h.run_steps(4);
}

#[test]
fn an_opened_version_is_read_only_highlighted_and_closes_back_exactly() {
    let (mut app, d, oldest, _) = app_with_history("open");
    // The unsaved third slide is written by AutoSave before the version opens: it joins the history.
    assert!(app.session.active().unwrap().is_dirty());
    let uid = app.session.active().unwrap().uid;
    with_pane(&mut app);
    let mut h = harness(app);
    open_oldest(&mut h, &oldest);
    let list = wait_for(h.state_mut(), 3);
    assert_eq!(list.len(), 3, "AutoSave wrote the unsaved slide first");
    assert!(!h.state().session.documents()[0].is_dirty(), "and the presentation is saved");
    h.run_steps(3);
    // A presentation of its own beside the first, read only.
    assert_eq!(h.state().session.documents().len(), 2);
    assert_eq!(slides(h.state()), 1);
    assert!(h.state().session.active_previous_version().is_some());
    // The title bar and the bar name it.
    assert!(h.query_by_label_contains("— Previous version").is_some(), "the title says so");
    assert!(h.query_by_label("Previous version").is_some(), "and so does the bar");
    for button in ["Compare", "Restore", "Save a copy"] {
        assert!(h.query_by_label(button).is_some(), "{button}");
    }
    assert_eq!(h.query_all_by_label("Close").count(), 1, "the bar's Close (the pane's is an icon)");
    assert_eq!(h.query_all_by_label("Open version").count(), 2, "the open version's row is highlighted and has no link");
    // Editing is refused, with the reason on the status line; looking around is not.
    assert!(h.state_mut().run("slide.new", json!({})).is_err());
    assert_eq!(status(h.state_mut()).as_deref(), Some(READ_ONLY));
    assert_eq!(slides(h.state()), 1);
    assert!(h.state_mut().run("edit.undo", json!({})).is_err());
    assert!(h.state_mut().run("slide.go", json!({"index": 0})).is_ok());
    assert!(h.state_mut().run("view.zoomIn", json!({})).is_ok());
    // Close (the bar's, not the pane's): the presentation as it was.
    let doc = h.state().session.documents()[0].doc.clone();
    h.get_by_label("Close").click();
    h.run_steps(4);
    assert_eq!(h.state().session.documents().len(), 1);
    assert_eq!(h.state().session.active().unwrap().uid, uid);
    assert!(Arc::ptr_eq(&h.state().session.active().unwrap().doc, &doc));
    assert!(h.query_by_label("Previous version").is_none());
    assert_eq!(h.state().ui.pane.as_deref(), Some("versions"), "the pane stays");
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn restore_from_the_bar_is_one_undo_step() {
    let (mut app, d, oldest, _) = app_with_history("restore");
    with_pane(&mut app);
    let mut h = harness(app);
    open_oldest(&mut h, &oldest);
    h.get_by_label("Restore").click();
    h.run_steps(4);
    assert_eq!(h.state().session.documents().len(), 1, "back at the presentation");
    assert_eq!(slides(h.state()), 1, "which now holds the oldest version");
    h.state_mut().run("edit.undo", json!({})).unwrap();
    assert_eq!(slides(h.state()), 3, "one Undo undoes the Restore");
    // The row's menu restores too.
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn save_a_copy_writes_a_new_file_and_returns() {
    let (mut app, d, oldest, _) = app_with_history("copy");
    with_pane(&mut app);
    let mut h = harness(app);
    open_oldest(&mut h, &oldest);
    h.get_by_label("Save a copy").click();
    h.run_steps(4);
    assert_eq!(h.state().session.documents().len(), 1, "back at the presentation");
    assert_eq!(slides(h.state()), 3);
    let copies: Vec<_> = std::fs::read_dir(&d)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("Quarterly (version ") && n.ends_with(".pptx"))
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert!(!copies[0].contains(':'), "a file name cannot hold the clock's colon");
    let copy = deckcraft_engine::cmd::file::open_presentation("x.pptx", &std::fs::read(d.join(&copies[0])).unwrap()).unwrap();
    assert_eq!(copy.slides.len(), 1, "the version, not the presentation");
    assert!(status(h.state_mut()).is_some_and(|m| m.starts_with("Saved a copy of the previous version as ")));
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn compare_shows_the_differences_and_pressing_it_again_shows_the_version() {
    let (mut app, d, oldest, _) = app_with_history("compare");
    with_pane(&mut app);
    let mut h = harness(app);
    open_oldest(&mut h, &oldest);
    h.get_by_label("Compare").click();
    h.run_steps(4);
    assert_eq!(h.state().ui.pane.as_deref(), Some("revisions"));
    assert!(h.state().session.active().unwrap().compare.is_some());
    assert!(h.query_by_label_contains("Compared with Quarterly").is_some());
    assert!(h.query_by_label("Accept").is_none() && h.query_by_label("Reject").is_none(), "a version is compared read only");
    assert_eq!(h.state().session.active().unwrap().doc.slides.len(), 1);
    h.get_by_label("Compare").click();
    h.run_steps(4);
    assert!(h.state().session.active().unwrap().compare.is_none());
    assert_eq!(h.state().ui.pane.as_deref(), Some("versions"), "the history is back");
    drop(h);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn close_new_open_and_quit_go_back_to_the_presentation_first() {
    let (mut app, d, oldest, _) = app_with_history("switch");
    let uid = app.session.active().unwrap().uid;
    let file = app.session.active().unwrap().path.clone().unwrap();
    // File Close closes only the version.
    app.run("file.openVersion", json!({"id": oldest})).unwrap();
    app.close_presentation();
    assert_eq!(app.session.documents().len(), 1);
    assert_eq!(app.session.active().unwrap().uid, uid);
    // New: the version closes; the presentation and the new one stay open.
    app.run("file.openVersion", json!({"id": oldest})).unwrap();
    app.run("file.new", json!({})).unwrap();
    assert!(app.session.documents().iter().all(|d| d.previous.is_none()));
    assert_eq!(app.session.documents().len(), 2);
    // Open.
    let i = app.session.index_of_uid(uid).unwrap();
    app.session.set_active(i);
    app.run("file.openVersion", json!({"id": oldest})).unwrap();
    app.run("file.open", json!({"path": file})).unwrap();
    assert!(app.session.documents().iter().all(|d| d.previous.is_none()));
    // Quit: the version goes first, the presentation is in front again.
    let i = app.session.index_of_uid(uid).unwrap();
    app.session.set_active(i);
    app.run("file.openVersion", json!({"id": oldest})).unwrap();
    app.return_from_versions();
    assert!(app.session.documents().iter().all(|d| d.previous.is_none()));
    assert_eq!(app.session.active().unwrap().uid, uid);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_labels_are_in_brazilian_portuguese() {
    let (mut app, d, oldest, _) = app_with_history("pt");
    app.ui.language = "pt-br".into();
    with_pane(&mut app);
    let mut h = harness(app);
    assert!(h.query_by_label("Histórico de Versões").is_some());
    assert!(h.query_by_label("Hoje").is_some());
    assert_eq!(h.query_all_by_label("Abrir versão").count(), 2);
    open_oldest_as(&mut h, &oldest, "Abrir versão");
    for label in ["Versão anterior", "Comparar", "Restaurar", "Salvar uma cópia"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert_eq!(h.query_all_by_label("Fechar").count(), 1);
    assert!(h.query_by_label_contains("— Versão anterior").is_some());
    // The refusal is worded in Portuguese too.
    let _ = h.state_mut().run("slide.new", json!({}));
    assert_eq!(status(h.state_mut()).as_deref(), Some("Esta é uma versão anterior. Restaure-a ou salve uma cópia para fazer alterações."));
    drop(h);
    crate::i18n::set_current(Lang::EN);
    let _ = std::fs::remove_dir_all(d);
}
