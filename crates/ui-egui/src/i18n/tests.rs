//! The interface-language system: lookup and fallback, the `language` setting (default, saved,
//! hostile), the shipped catalogs, and the ribbon drawn in Portuguese.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use deckcraft_engine::Session;
use egui_kittest::kittest::Queryable;
use serde_json::json;

use super::*;
use crate::{Services, SlideApp, UiState};

fn pt() -> Lang {
    Lang::from_code("pt-br").expect("the pt-br catalogs are embedded (i18n/locales/pt-br)")
}

#[test]
fn english_is_the_source_and_the_default_in_tests() {
    set_current(Lang::EN);
    assert_eq!(t("Paste"), "Paste");
    assert_eq!(t("A string nobody translated"), "A string nobody translated");
    assert_eq!(system_lang(), Lang::EN, "tests drive the UI by its English labels");
    assert_eq!(Lang::from_pref(AUTO), Lang::EN);
    assert_eq!(Lang::from_pref("xx"), Lang::EN, "an unknown code follows the system");
}

#[test]
fn languages_are_english_then_portuguese_brazil() {
    let codes: Vec<_> = Lang::all().map(Lang::code).collect();
    assert_eq!(codes, ["en", "pt-br"]);
    assert_eq!(pt().name(), "Português (Brasil)");
    assert_eq!(normalize_pref("PT-BR"), Some("pt-br"));
    assert_eq!(normalize_pref("Auto"), Some(AUTO));
    assert_eq!(normalize_pref("klingon"), None);
}

#[test]
fn portuguese_lookups_use_the_catalogs_with_the_fallbacks() {
    // Ribbon tab names live under the context `tab`, so the File ▸ Home page can keep its own word.
    assert_eq!(trc(pt(), "tab", "Home"), "Página Inicial");
    assert_eq!(trc(pt(), "tab", "Slide Show"), "Apresentação de Slides");
    assert_eq!(tr(pt(), "Paste"), "Colar");
    // A string in neither catalog stays English.
    assert_eq!(tr(pt(), "A string nobody translated"), "A string nobody translated");
    assert!(has(pt(), "Paste"));
    assert!(!has(Lang::EN, "Paste"));
    // pt-BR system tags resolve to it.
    assert_eq!(lang_from_tag("pt_BR.UTF-8"), Some(pt()));
    assert_eq!(lang_from_tag("pt-BR"), Some(pt()));
    assert_eq!(lang_from_tag("C"), Some(Lang::EN));
    assert_eq!(first_supported(["fr-FR", "pt-BR", "en-US"]), Some(pt()));
    assert_eq!(first_supported(["fr-FR", "ja"]), None);
    // Placeholders keep their names and may be reordered by a translation.
    set_current(pt());
    let slide = fmt(t("Slide {n} of {total}"), &[("n", "3"), ("total", "9")]);
    assert_eq!(fmt_plain(t("Slide {n} of {total}"), &[("n", "3"), ("total", "9")]), "Slide 3 de 9", "{slide:?}");
    set_current(Lang::EN);
}

#[test]
fn two_line_labels_fall_back_to_the_one_line_row() {
    set_current(pt());
    assert_eq!(t_lines("New\nSlide"), "Novo Slide");
    assert_eq!(t_lines("Nothing\nTranslated"), "Nothing\nTranslated");
    set_current(Lang::EN);
    assert_eq!(t_lines("New\nSlide"), "New\nSlide");
}

/// Every row of the DeckCraft catalog parses: three columns, matching placeholders, no outer
/// space in the English (it would never match a lookup), no duplicates.
#[test]
fn deckcraft_rows_parse() {
    if !catalogs_embedded() {
        return;
    }
    assert!(I18N.check_all().is_empty(), "catalog problems: {:?}", I18N.check_all());
    for (code, _, app) in CRAFT_I18N.iter() {
        let mut seen = std::collections::BTreeSet::new();
        let mut rows = 0;
        for (n, line) in app.lines().enumerate() {
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            assert_eq!(cols.len(), 3, "{code}/deckcraft.tsv line {}: want context TAB English TAB translation: {line:?}", n + 1);
            let (ctx, en, tr) = (cols[0], cols[1], cols[2]);
            assert!(!en.is_empty() && !tr.is_empty(), "{code}/deckcraft.tsv line {}: empty column", n + 1);
            assert_eq!(en, en.trim(), "{code}/deckcraft.tsv line {}: the English key has outer spaces", n + 1);
            assert!(seen.insert((ctx, en)), "{code}/deckcraft.tsv line {}: duplicate row for {en:?}", n + 1);
            let holes = |s: &str| -> std::collections::BTreeSet<String> {
                s.split('{').skip(1).filter_map(|r| r.split_once('}')).map(|(name, _)| name.to_string()).collect()
            };
            if !ctx.starts_with('@') {
                assert_eq!(holes(en), holes(tr), "{code}/deckcraft.tsv line {}: placeholders differ: {en:?}", n + 1);
            }
            rows += 1;
        }
        assert!(rows > 100, "{code}/deckcraft.tsv has only {rows} rows");
    }
}

#[test]
fn language_setting_defaults_to_auto_and_old_files_get_it() {
    assert_eq!(UiState::default().language, AUTO);
    let old: UiState = serde_json::from_value(json!({"tab": "Insert"})).unwrap();
    assert_eq!(old.language, AUTO);
}

#[test]
fn language_setting_round_trips_through_ui_json() {
    let d = std::env::temp_dir().join(format!("deckcraft-lang-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    let ui = UiState { language: "pt-br".into(), ..Default::default() };
    crate::settings::save(&d, &ui, &deckcraft_engine::Prefs::default());
    let text = std::fs::read_to_string(d.join(crate::settings::UI_FILE)).unwrap();
    assert!(text.contains(r#""language": "pt-br""#), "{text}");
    assert_eq!(crate::settings::load(&d).0.unwrap().language, "pt-br");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hostile_language_values_read_as_auto_and_keep_the_rest_of_the_file() {
    let d = std::env::temp_dir().join(format!("deckcraft-lang-hostile-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let long = "x".repeat(100_000);
    for bad in [long.as_str(), "", "../../etc/passwd", "pt br", "pt-br\u{0}", "<script>", "\u{202e}pt"] {
        std::fs::write(d.join(crate::settings::UI_FILE), json!({"language": bad, "tab": "Insert"}).to_string()).unwrap();
        let ui = crate::settings::load(&d).0.unwrap_or_else(|| panic!("{bad:?} should not discard ui.json"));
        assert_eq!((ui.language.as_str(), ui.tab.as_str()), (AUTO, "Insert"), "{bad:?}");
    }
    // A well-formed code this build has no catalog for is kept in the file but shows the system language.
    std::fs::write(d.join(crate::settings::UI_FILE), r#"{"language": "zz-top"}"#).unwrap();
    let ui = crate::settings::load(&d).0.unwrap();
    assert_eq!(ui.language, "zz-top");
    assert_eq!(Lang::from_pref(&ui.language), Lang::EN);
    // A wrong type is a damaged file, as for every other setting.
    std::fs::write(d.join(crate::settings::UI_FILE), r#"{"language": 7}"#).unwrap();
    assert!(crate::settings::load(&d).0.is_none());
    let _ = std::fs::remove_dir_all(&d);
}

fn harness(language: &str) -> egui_kittest::Harness<'static, SlideApp> {
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.ui.language = language.into();
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
fn the_ribbon_is_drawn_in_portuguese_when_asked_and_in_english_otherwise() {
    let h = harness("pt-br");
    for label in ["Arquivo", "Página Inicial", "Inserir", "Desenhar", "Transições", "Animações", "Apresentação de Slides", "Gravar", "Revisão", "Exibir"] {
        assert!(h.query_all_by_label(label).next().is_some(), "the ribbon shows {label:?}");
    }
    assert!(h.query_all_by_label("Home").next().is_none(), "no English tab left");
    // The tab's key stays English: state, the control channel and tests keep using it.
    assert_eq!(h.state().ui.tab, "Home");
    set_current(Lang::EN);

    let h = harness("auto");
    assert!(h.query_all_by_label("Home").next().is_some());
    assert!(h.query_all_by_label("Página Inicial").next().is_none());
}

#[test]
fn ui_language_command_switches_validates_and_cancel_restores() {
    let mut h = harness("auto");
    assert!(h.state_mut().run("ui.language", json!({"value": "klingon"})).is_err());
    assert_eq!(h.state().ui.language, AUTO);
    let r = h.state_mut().run("ui.language", json!({"value": "PT-BR"})).unwrap();
    assert_eq!(r["language"], "pt-br");
    h.run_steps(3);
    assert!(h.query_all_by_label("Inserir").next().is_some());

    // Preferences: the choice applies as it is made and Cancel puts the old one back.
    let mut h = harness("auto");
    h.state_mut().run("app.preferences", json!({})).unwrap();
    h.run_steps(3);
    h.state_mut().run("ui.language", json!({"value": "pt-br"})).unwrap();
    h.run_steps(2);
    h.query_by_label("Cancelar").expect("Cancelar is there").click();
    h.run_steps(3);
    assert_eq!(h.state().ui.language, AUTO);
    set_current(Lang::EN);
}

#[test]
fn a_fixture_catalog_drives_lookups_without_the_shipped_wording() {
    let l = fixture_lang("pt-br", "\tPaste\tCOLAR\n");
    assert_eq!(tr(l, "Paste"), "COLAR");
    assert_eq!(tr(l, "Cut"), "Cut");
    set_current(l);
    assert_eq!(t("Paste"), "COLAR");
    set_current(Lang::EN);
}
