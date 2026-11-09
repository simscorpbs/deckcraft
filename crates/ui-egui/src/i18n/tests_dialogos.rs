//! The dialogs drawn in Portuguese: catalog lookups for the wording that needed a context, and a few
//! dialogs opened with `language = pt-br`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use deckcraft_engine::Session;
use egui_kittest::kittest::Queryable;

use super::*;
use crate::dialogs::Dialog;
use crate::{Services, SlideApp};

fn pt() -> Lang {
    Lang::from_code("pt-br").expect("the pt-br catalogs are embedded (i18n/locales/pt-br)")
}

fn dialog_harness(id: &str, language: &str) -> egui_kittest::Harness<'static, SlideApp> {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.ui.language = language.into();
    app.dialog = Some(Dialog::new(id));
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1500.0, 900.0)).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

#[test]
fn dialog_wording_with_a_context_overrides_the_plain_word() {
    assert_eq!(trc(pt(), "dialog", "Tabs"), "Tabulações");
    assert_eq!(trc(pt(), "hint", "Outline"), "Estrutura de tópicos");
    assert_eq!(trc(pt(), "unit", "in"), "pol.");
    // Untouched wording keeps working through the plain row and English.
    assert_eq!(trc(pt(), "dialog", "Zoom"), "Zoom");
    assert_eq!(trc(Lang::EN, "dialog", "Tabs"), "Tabs");
}

#[test]
fn the_font_and_hyperlink_dialogs_are_drawn_in_portuguese() {
    for (id, labels) in [
        ("font", &["Fonte", "Negrito", "Itálico", "Sublinhado", "Cancelar"][..]),
        ("hyperlink", &["Inserir Hiperlink", "Endereço:", "Dica de Tela:", "Cancelar"][..]),
        ("tabs", &["Tabulações", "Posições de tabulação:", "Limpar tudo", "Cancelar"][..]),
    ] {
        let h = dialog_harness(id, "pt-br");
        for label in labels {
            assert!(h.query_all_by_label(label).next().is_some(), "{id} shows {label:?}");
        }
        set_current(Lang::EN);
    }
}

#[test]
fn dialogs_stay_in_english_when_the_language_is_auto_in_tests() {
    let h = dialog_harness("font", "auto");
    assert!(h.query_all_by_label("Bold").next().is_some());
    assert!(h.query_all_by_label("Negrito").next().is_none());
}
