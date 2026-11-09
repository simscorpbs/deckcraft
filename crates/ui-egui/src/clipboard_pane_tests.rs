//! The Clipboard pane in the window: the Home › Clipboard launcher opens and closes it, copies and
//! cuts fill it, an item pastes again, Paste All, Clear All, Delete, and Portuguese labels.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(not(target_arch = "wasm32"))]

use deckcraft_engine::Session;
use egui::accesskit::Role;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use crate::i18n::Lang;
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

/// An app whose clipboard holds a copy of each of `texts` (a rectangle with that text).
fn app_with_copies(texts: &[&str]) -> SlideApp {
    crate::i18n::set_current(Lang::EN);
    let mut session = Session::with_new();
    for t in texts {
        let r = session.execute("shape.insert", &json!({"preset": "rect", "rect": [10, 10, 80, 40], "text": t})).unwrap();
        session.execute("selection.select", &json!({"id": r["id"]})).unwrap();
        session.execute("edit.copy", &json!({})).unwrap();
    }
    SlideApp::new(session, Services::default())
}

fn shapes(h: &Harness<'_, SlideApp>) -> usize {
    h.state().session.active().unwrap().current_slide().unwrap().shapes.len()
}

fn open_pane(h: &mut Harness<'_, SlideApp>) {
    h.state_mut().run("view.pane", json!({"pane": "clipboard"})).unwrap();
    h.run_steps(3);
}

#[test]
fn the_launcher_opens_and_closes_the_pane() {
    let mut h = harness(app_with_copies(&["one"]));
    assert!(h.query_by_label("Paste All").is_none(), "closed at first");
    h.get_by_role_and_label(Role::Button, "Clipboard").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.pane.as_deref(), Some("clipboard"));
    for label in ["Paste All", "Clear All", "Delete", "Click an item to paste it."] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    assert!(h.query_by_label("Paste one").is_some(), "the item, named for assistive technology");
    // The launcher toggles it.
    h.get_by_role_and_label(Role::Button, "Clipboard").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.pane, None);
    assert!(h.query_by_label("Paste All").is_none());
}

#[test]
fn an_empty_pane_says_so_and_has_nothing_to_paste() {
    let mut h = harness(app_with_copies(&[]));
    open_pane(&mut h);
    assert!(h.query_by_label("Nothing collected yet. Items you copy or cut appear here.").is_some());
    assert!(h.query_by_label("Delete").is_none());
}

#[test]
fn clicking_an_item_pastes_it_and_puts_it_on_the_clipboard() {
    let mut h = harness(app_with_copies(&["alpha", "beta"]));
    open_pane(&mut h);
    let before = shapes(&h);
    h.get_by_label("Paste alpha").click();
    h.run_steps(3);
    assert_eq!(shapes(&h), before + 1);
    assert_eq!(h.state().session.clipboard.plain, "alpha");
    assert_eq!(h.state().session.clip_history.len(), 2, "pasting adds nothing to the list");
    assert!(h.state().session.active().unwrap().current_slide().unwrap().shapes.last().unwrap().text.as_ref().unwrap().text() == "alpha");
}

#[test]
fn paste_all_clear_all_and_delete() {
    let mut h = harness(app_with_copies(&["one", "two", "three"]));
    open_pane(&mut h);
    let before = shapes(&h);
    h.get_by_label("Paste All").click();
    h.run_steps(3);
    assert_eq!(shapes(&h), before + 3);

    assert_eq!(h.query_all_by_label("Delete").count(), 3);
    h.query_all_by_label("Delete").next().unwrap().click();
    h.run_steps(3);
    assert_eq!(h.state().session.clip_history.len(), 2);
    assert!(h.query_by_label("Paste three").is_none(), "the newest was deleted");

    h.get_by_label("Clear All").click();
    h.run_steps(3);
    assert!(h.state().session.clip_history.is_empty());
    assert!(h.query_by_label("Nothing collected yet. Items you copy or cut appear here.").is_some());
}

#[test]
fn a_new_copy_shows_up_while_the_pane_is_open() {
    let mut h = harness(app_with_copies(&["first"]));
    open_pane(&mut h);
    let r = h.state_mut().session.execute("shape.insert", &json!({"preset": "rect", "rect": [20, 20, 80, 40], "text": "second"})).unwrap();
    h.state_mut().session.execute("selection.select", &json!({"id": r["id"]})).unwrap();
    h.state_mut().run("edit.copy", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Paste second").is_some() && h.query_by_label("Paste first").is_some());
}

#[test]
fn the_labels_are_in_brazilian_portuguese() {
    let mut app = app_with_copies(&["alpha"]);
    app.ui.language = "pt-br".into();
    app.ui.pane = Some("clipboard".into());
    let mut h = harness(app);
    assert_eq!(h.query_all_by_label("Área de Transferência").count(), 2, "the launcher and the pane's title");
    for label in ["Colar Tudo", "Limpar Tudo", "Excluir", "Clique em um item para colá-lo.", "Colar alpha"] {
        assert!(h.query_by_label(label).is_some(), "{label}");
    }
    h.state_mut().session.clip_history = Default::default();
    h.run_steps(3);
    assert!(h.query_by_label("Nada coletado ainda. Os itens que você copiar ou recortar aparecem aqui.").is_some());
    crate::i18n::set_current(Lang::EN);
}
