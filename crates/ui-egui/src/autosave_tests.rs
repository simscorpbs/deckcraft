//! AutoSave through the window: the title bar switch and what it does, closing without a Save
//! Changes prompt, the Save Conflict and Save to Cloud dialogs, the schedule and its status bar
//! error, and the Preferences default.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use deckcraft_engine::Session;
use deckcraft_engine::autosave::{AutoSaveBlock, SETTLE_SECS};
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

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
    let d = std::env::temp_dir().join(format!("deckcraft-autosave-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Desktop services whose Save dialog answers with `dir`/<suggested name>.
fn saving_into(dir: &Path) -> Services {
    let dir = dir.to_path_buf();
    Services { pick_save: Some(Box::new(move |name: &str| Some(dir.join(name).to_string_lossy().to_string()))), ..Default::default() }
}

fn edit(app: &mut SlideApp) {
    app.session.execute("slide.new", &json!({})).unwrap();
}

fn slides(app: &SlideApp) -> usize {
    app.session.active().unwrap().doc.slides.len()
}

fn slides_in(path: &Path) -> usize {
    deckcraft_engine::cmd::file::open_presentation("x.pptx", &std::fs::read(path).unwrap()).unwrap().slides.len()
}

/// An app with one presentation saved as `dir`/talk.pptx.
fn saved_app(dir: &Path) -> (SlideApp, PathBuf) {
    let path = dir.join("talk.pptx");
    let mut app = SlideApp::new(Session::with_new(), saving_into(dir));
    app.session.execute("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    (app, path)
}

/// Another device saves a different deck over `path`.
fn changed_elsewhere(path: &Path) -> Vec<u8> {
    let mut other = Session::with_new();
    for _ in 0..5 {
        other.execute("slide.new", &json!({})).unwrap();
    }
    other.execute("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap();
    std::fs::read(path).unwrap()
}

fn dialog_id(app: &SlideApp) -> Option<&str> {
    app.dialog.as_ref().map(|d| d.id.as_str())
}

fn switch<'h>(h: &'h Harness<'static, SlideApp>) -> egui_kittest::Node<'h> {
    h.get_by_role_and_label(Role::CheckBox, "AutoSave")
}

#[test]
fn the_switch_is_off_with_a_reason_until_the_file_is_saved_then_toggles_and_is_remembered() {
    let d = dir("switch");
    let mut h = harness(SlideApp::new(Session::with_new(), saving_into(&d)));
    // A new presentation: AutoSave can't cover it, and says why.
    assert_eq!(h.state().autosave_block(0), Some(AutoSaveBlock::Unsaved));
    assert!(!h.state().autosave_covers(0));
    assert_eq!(super::app::autosave::block_reason(&AutoSaveBlock::Unsaved), "Save the presentation to turn on AutoSave");
    // Clicking the switch saves it (no cloud folder is known) and turns AutoSave on for the new file.
    switch(&h).click();
    h.run_steps(3);
    assert!(h.state().session.active().unwrap().path.is_some(), "the click asked where to save");
    assert!(h.state().autosave_covers(0));
    // Now it toggles, and the choice is remembered for the file.
    switch(&h).click();
    h.run_steps(3);
    assert!(!h.state().session.active().unwrap().autosave);
    assert_eq!(h.state().session.autosave_remembered(0), Some(false));
    switch(&h).click();
    h.run_steps(3);
    assert!(h.state().autosave_covers(0));
    assert_eq!(h.state().session.autosave_remembered(0), None, "back on forgets the choice");
}

#[test]
fn a_lossy_format_is_greyed_out_with_the_reason_and_the_browser_is_never_covered() {
    let d = dir("formats");
    let mut app = SlideApp::new(Session::with_new(), saving_into(&d));
    edit(&mut app);
    app.session.execute("file.save", &json!({"path": d.join("notes.txt").to_string_lossy()})).unwrap();
    let block = app.autosave_block(0).unwrap();
    assert!(matches!(&block, AutoSaveBlock::Format { name } if name == "notes.txt"), "{block:?}");
    assert!(super::app::autosave::block_reason(&block).contains("notes.txt"));
    // The browser: the page keeps the files, and nothing is covered.
    let host = Services { host_save: Some(Box::new(|_: &str, _: &[u8], _: bool| Ok(()))), ..Default::default() };
    let app = SlideApp::new(Session::with_new(), host);
    assert_eq!(app.autosave_block(0), Some(AutoSaveBlock::Browser));
    assert!(!app.autosave_covers(0));
}

#[test]
fn turning_it_on_for_a_new_presentation_offers_the_cloud_folders_found() {
    use deckcraft_engine::cloud::{CloudRoot, Provider};
    let d = dir("cloud");
    let onedrive = d.join("OneDrive-Work");
    let icloud = d.join("iCloud");
    std::fs::create_dir_all(&onedrive).unwrap();
    std::fs::create_dir_all(&icloud).unwrap();
    let mut h = harness(SlideApp::new(Session::with_new(), saving_into(&d)));
    h.state_mut().session.cloud_roots = vec![
        CloudRoot { provider: Provider::ICloudDrive, path: icloud.clone(), account: None },
        CloudRoot { provider: Provider::OneDrive, path: onedrive.clone(), account: Some("Work".into()) },
    ];
    edit(h.state_mut());
    switch(&h).click();
    h.run_steps(3);
    assert_eq!(dialog_id(h.state()), Some("saveToCloud"));
    assert!(h.query_by_label("OneDrive (Work)").is_some() && h.query_by_label("iCloud Drive").is_some());
    h.get_by_label("OneDrive (Work)").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    let path = PathBuf::from(h.state().session.active().unwrap().path.clone().unwrap());
    assert_eq!(path.parent(), Some(onedrive.as_path()));
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("pptx"));
    assert!(h.state().autosave_covers(0), "AutoSave is on for the file saved to the cloud");
    assert_eq!(h.state().cloud_name(0), Some("OneDrive"));
    assert!(!h.state().session.active().unwrap().is_dirty());
}

#[test]
fn close_writes_the_changes_and_skips_the_save_changes_prompt() {
    let d = dir("close");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    edit(&mut app);
    app.close_presentation();
    assert!(app.dialog.is_none(), "no Save Changes prompt");
    assert!(app.session.documents().is_empty());
    assert_eq!(slides_in(&path), 3, "the pending changes were written");

    // With AutoSave off for the file, the prompt stays.
    let (mut app, _) = saved_app(&dir("close-off"));
    app.session.set_autosave_of(0, false);
    edit(&mut app);
    app.close_presentation();
    assert_eq!(dialog_id(&app), Some("close"));
    assert_eq!(app.session.documents().len(), 1);

    // A new presentation has no file: the prompt stays.
    let mut app = SlideApp::new(Session::with_new(), saving_into(&d));
    edit(&mut app);
    app.close_presentation();
    assert_eq!(dialog_id(&app), Some("close"));
}

#[test]
fn close_over_a_file_changed_elsewhere_keeps_the_presentation_and_asks_what_to_do() {
    let d = dir("close-conflict");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    let theirs = changed_elsewhere(&path);
    app.close_presentation();
    assert_eq!(dialog_id(&app), Some("saveConflict"));
    assert_eq!(app.session.documents().len(), 1, "the presentation stays open");
    assert_eq!(std::fs::read(&path).unwrap(), theirs, "the other version is untouched");
    // Decide later closes the dialog and leaves everything as it was; the switch reopens it.
    let mut h = harness(app);
    h.get_by_label("Decide later").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(h.state().session.active().unwrap().is_dirty());
    assert_eq!(h.state().autosave_block(0), Some(AutoSaveBlock::Conflict { missing: false }));
    switch(&h).click();
    h.run_steps(3);
    assert_eq!(dialog_id(h.state()), Some("saveConflict"));
    // Keep both: this version goes beside the other and AutoSave carries on there.
    h.get_by_label("Keep both").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert_eq!(std::fs::read(&path).unwrap(), theirs);
    let copy = d.join("talk (conflict copy).pptx");
    assert_eq!(slides_in(&copy), 2);
    assert!(h.state().autosave_covers(0));
}

#[test]
fn the_conflict_dialog_offers_what_the_situation_allows() {
    // Changed: four answers.
    let d = dir("answers");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    changed_elsewhere(&path);
    app.session.execute("file.save", &json!({"ifUnchanged": true})).unwrap();
    app.show_save_conflict(app.session.active().unwrap().uid);
    let h = harness(app);
    for label in ["Keep both", "Keep this version", "Open the other version", "Decide later"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Save it here again").is_none());
    drop(h);

    // Moved or deleted: only Save it here again, and Decide later.
    let (mut app, path) = saved_app(&dir("answers-missing"));
    edit(&mut app);
    std::fs::remove_file(&path).unwrap();
    app.session.execute("file.save", &json!({"ifUnchanged": true})).unwrap();
    app.show_save_conflict(app.session.active().unwrap().uid);
    let mut h = harness(app);
    assert!(h.query_by_label("Keep both").is_none() && h.query_by_label("Open the other version").is_none());
    assert!(h.query_by_label("Decide later").is_some());
    h.get_by_label("Save it here again").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_none());
    assert!(path.exists(), "the presentation is written back where it was");
    assert!(!h.state().session.active().unwrap().is_dirty());

    // Open the other version: the file as it is now replaces this one.
    let (mut app, path) = saved_app(&dir("answers-theirs"));
    edit(&mut app);
    changed_elsewhere(&path);
    app.session.execute("file.save", &json!({"ifUnchanged": true})).unwrap();
    app.show_save_conflict(app.session.active().unwrap().uid);
    let mut h = harness(app);
    h.get_by_label("Open the other version").click();
    h.run_steps(3);
    assert_eq!(slides(h.state()), 6);
    assert!(!h.state().session.active().unwrap().is_dirty());
    // Answers outside a dialog are refused.
    assert!(h.state_mut().run("ui.saveConflict", json!({"answer": "both"})).is_err());
}

#[test]
fn quitting_writes_what_autosave_covers_and_asks_only_about_the_rest() {
    let d = dir("quit");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    assert!(!app.autosave_before_quit());
    assert!(app.session.documents().iter().all(|d| !d.is_dirty()), "nothing left to ask about");
    assert_eq!(slides_in(&path), 2);

    // One presentation AutoSave can't cover is left for the prompt; the covered one is saved.
    app.session.execute("file.new", &json!({})).unwrap();
    edit(&mut app);
    app.session.set_active(0);
    edit(&mut app);
    assert!(!app.autosave_before_quit());
    let dirty: Vec<bool> = app.session.documents().iter().map(|d| d.is_dirty()).collect();
    assert_eq!(dirty, [false, true]);

    // A conflict holds the quit back.
    let (mut app, path) = saved_app(&dir("quit-conflict"));
    edit(&mut app);
    changed_elsewhere(&path);
    assert!(app.autosave_before_quit());
    assert_eq!(dialog_id(&app), Some("saveConflict"));
}

#[test]
fn autosave_writes_after_the_presentation_settles_and_shows_errors_until_a_write_succeeds() {
    let d = dir("tick");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    app.autosave_tick(10.0);
    app.autosave_tick(10.0 + SETTLE_SECS - 0.1);
    assert!(app.session.active().unwrap().is_dirty());
    app.autosave_tick(10.0 + SETTLE_SECS);
    assert!(!app.session.active().unwrap().is_dirty());
    assert_eq!(slides_in(&path), 2);
    assert_eq!(app.autosave_failure_text(), None);

    // The file can't be written: the error stays in the status bar while it keeps failing.
    edit(&mut app);
    app.autosave_tick(100.0);
    lock(&path);
    app.autosave_tick(100.0 + SETTLE_SECS);
    let text = app.autosave_failure_text().expect("the failure is shown");
    assert!(text.starts_with("AutoSave couldn't write the file:") && text.ends_with("It will try again shortly."), "{text}");
    assert!(app.session.active().unwrap().autosave, "the switch stays on");
    assert!(app.session.active().unwrap().is_dirty());
    unlock(&path);
    app.autosave_tick(100.0 + SETTLE_SECS + 5.0);
    assert_eq!(app.autosave_failure_text(), None, "the next successful write takes the message away");
    assert_eq!(slides_in(&path), 3);
}

#[test]
fn a_save_changes_prompt_that_is_open_closes_when_autosave_gets_the_file_written() {
    let d = dir("prompt");
    let (mut app, path) = saved_app(&d);
    edit(&mut app);
    // The write fails at close: the prompt stays in charge.
    lock(&path);
    app.close_presentation();
    assert_eq!(dialog_id(&app), Some("close"));
    assert_eq!(app.session.documents().len(), 1);
    // The file is free again; AutoSave's next try writes it and the pending close goes on.
    unlock(&path);
    app.autosave_tick(1000.0);
    app.autosave_tick(1000.0 + SETTLE_SECS);
    assert!(app.dialog.is_none());
    assert!(app.session.documents().is_empty());
    assert_eq!(slides_in(&path), 2);
}

fn lock(path: &Path) {
    if cfg!(windows) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(path, p).unwrap();
    } else {
        set_mode(path.parent().unwrap(), 0o555);
    }
}

fn unlock(path: &Path) {
    if cfg!(windows) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(path, p).unwrap();
    } else {
        set_mode(path.parent().unwrap(), 0o755);
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

#[test]
fn preferences_turn_on_autosave_by_default_and_cancel_puts_it_back() {
    let d = dir("prefs");
    let (mut app, _) = saved_app(&d);
    assert!(app.session.prefs.autosave_by_default);
    app.session.execute("file.new", &json!({})).unwrap();
    let mut h = harness(app);
    h.state_mut().run("app.preferences", json!({})).unwrap();
    h.run_steps(3);
    h.get_by_label("Turn on AutoSave by default").click();
    h.run_steps(3);
    assert!(!h.state().session.prefs.autosave_by_default);
    assert!(!h.state().session.active().unwrap().autosave, "a file with no choice follows the default");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert!(h.state().session.prefs.autosave_by_default, "Cancel puts the default back");
    assert!(h.state().session.active().unwrap().autosave);

    // Done keeps it, and the preference is saved with the others.
    h.state_mut().run("app.preferences", json!({})).unwrap();
    h.run_steps(3);
    h.get_by_label("Turn on AutoSave by default").click();
    h.run_steps(3);
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(!h.state().session.prefs.autosave_by_default);
    let saved = serde_json::to_value(&h.state().session.prefs).unwrap();
    assert_eq!(saved["autosaveByDefault"], false);
}

#[test]
fn the_new_labels_have_portuguese() {
    let pt = crate::i18n::Lang::from_code("pt-br").expect("pt-br is embedded");
    for (en, want) in [
        ("AutoSave", "Salvamento Automático"),
        ("Keep both", "Manter ambos"),
        ("Keep this version", "Manter esta versão"),
        ("Open the other version", "Abrir a outra versão"),
        ("Decide later", "Decidir depois"),
        ("Save it here again", "Salvar aqui de novo"),
        ("Turn on AutoSave by default", "Ativar o Salvamento Automático por padrão"),
        ("Save Conflict", "Conflito ao Salvar"),
        ("Save to Cloud", "Salvar na Nuvem"),
    ] {
        assert_eq!(crate::i18n::tr(pt, en), want, "{en}");
    }
}
