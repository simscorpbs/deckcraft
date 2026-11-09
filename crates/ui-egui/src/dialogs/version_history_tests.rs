//! Preferences › Version History: the folders apply on Done, Cancel leaves them, and each Delete
//! button asks before it deletes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use deckcraft_engine::Session;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::{Services, SlideApp};

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("deckcraft-versions-ui-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An app whose history is kept in `root/versions`, with a presentation saved as `root/talk.pptx`.
fn app_in(root: &std::path::Path) -> SlideApp {
    let mut session = Session::with_new();
    session.enable_versions(root.join("versions"), std::sync::Arc::new(|| {})).unwrap();
    session.execute("file.saveAs", &json!({"path": root.join("talk.pptx").to_string_lossy()})).unwrap();
    assert!(session.version_wait_idle(std::time::Duration::from_secs(30)));
    SlideApp::new(session, Services::default())
}

fn harness(app: SlideApp) -> Harness<'static, SlideApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 1000.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    h
}

/// Click the folder field and type into it, as a person does.
fn type_folder(h: &mut Harness<'_, SlideApp>, text: &str) {
    h.get_by_role_and_label(Role::TextInput, "Version history folder:").click();
    h.run_steps(2);
    h.get_by_role_and_label(Role::TextInput, "Version history folder:").type_text(text);
    h.run_steps(2);
}

fn open_preferences(h: &mut Harness<'_, SlideApp>) {
    h.state_mut().run("app.preferences", json!({})).unwrap();
    h.run_steps(3);
}

#[test]
fn the_folder_applies_on_done_and_cancel_leaves_it() {
    let root = dir("folders");
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let mut h = harness(app_in(&root));
    open_preferences(&mut h);
    type_folder(&mut h, &elsewhere.to_string_lossy());
    h.get_by_label("Mirror folder").click();
    h.run_steps(3);
    assert_eq!(h.state().session.prefs.versions_folder, None, "nothing applies before Done");
    h.get_by_label("Cancel").click();
    h.run_steps(3);
    assert_eq!(h.state().session.prefs.versions_folder, None);
    assert!(!h.state().session.prefs.versions_mirror);
    assert_eq!(h.state().session.version_folders().0, vec![root.join("versions")]);

    open_preferences(&mut h);
    type_folder(&mut h, &elsewhere.to_string_lossy());
    h.get_by_label("Mirror folder").click();
    h.run_steps(3);
    h.get_by_label("Done").click();
    h.run_steps(3);
    let prefs = &h.state().session.prefs;
    assert_eq!(prefs.versions_folder.as_deref(), Some(elsewhere.to_string_lossy().as_ref()));
    assert!(prefs.versions_mirror);
    assert_eq!(h.state().session.version_folders().0, vec![root.join("versions"), elsewhere.clone()]);
    assert!(h.state().dialog.is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_folder_that_is_not_a_full_path_keeps_the_dialog_open() {
    let root = dir("relative");
    let mut h = harness(app_in(&root));
    open_preferences(&mut h);
    type_folder(&mut h, "relative/folder");
    h.get_by_label("Done").click();
    h.run_steps(3);
    assert!(h.state().dialog.is_some(), "refused, so the dialog stays");
    assert_eq!(h.state().session.prefs.versions_folder, None);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn each_delete_button_asks_first() {
    let root = dir("delete");
    let mut h = harness(app_in(&root));
    assert_eq!(h.state_mut().session.version_infos(0).len(), 1);
    open_preferences(&mut h);
    h.get_by_label("Delete this document's history").click();
    h.run_steps(3);
    assert!(h.query_by_label("Delete the version history of this document? This can't be undone.").is_some());
    h.get_by_label("Don't delete").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().session.version_infos(0).len(), 1, "Declining the question deletes nothing");
    assert!(h.state().dialog.is_some(), "and the dialog stays");
    h.get_by_label("Delete this document's history").click();
    h.run_steps(3);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    assert_eq!(h.state_mut().session.version_infos(0).len(), 0);
    assert!(h.query_by_label("This document's version history was deleted.").is_some());
    h.get_by_label("Delete all history").click();
    h.run_steps(3);
    assert!(h.query_by_label("Delete the version history of every document? This can't be undone.").is_some());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn the_labels_have_portuguese() {
    let pt = crate::i18n::Lang::from_code("pt-br").expect("pt-br is embedded");
    for (en, want) in [
        ("Version History", "Histórico de Versões"),
        ("Version history folder:", "Pasta do histórico de versões:"),
        ("Mirror folder", "Pasta espelho"),
        ("Delete this document's history", "Excluir o histórico deste documento"),
        ("Delete all history", "Excluir todo o histórico"),
    ] {
        assert_eq!(crate::i18n::tr(pt, en), want, "{en}");
    }
}
