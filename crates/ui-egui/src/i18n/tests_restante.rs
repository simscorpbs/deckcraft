//! What the earlier passes left in English: animation effects and their options, the Bevel menu,
//! starter templates, proofing-language names, engine and file errors, and the date samples.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use crate::ribbon::animations::animation_option_label;
use crate::ribbon::galleries::bevel_name;

fn pt() -> Lang {
    Lang::from_code("pt-br").expect("the pt-br catalogs are embedded (i18n/locales/pt-br)")
}

/// Every English name the animation galleries and the Effect Options menu can show.
fn animation_names() -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for a in deckcraft_model::anim::ANIMATIONS {
        out.push(a.1.to_string());
        out.extend(a.5.iter().map(|o| animation_option_label(a.0, o)));
    }
    out.extend(["Entrance", "Emphasis", "Exit", "Motion Paths", "Motion", "By Paragraph", "As One Object"].map(String::from));
    out
}

const BEVEL_IDS: [&str; 13] = ["none", "circle", "relaxedInset", "cross", "coolSlant", "angle", "softRound", "convex", "slope", "divot", "riblet", "hardEdge", "artDeco"];

fn bevel_names() -> Vec<String> {
    BEVEL_IDS.iter().map(|id| bevel_name(id).to_string()).collect()
}

fn template_names() -> Vec<String> {
    deckcraft_engine::templates::starters().iter().flat_map(|s| [s.name.to_string(), s.summary.to_string()]).collect()
}

fn language_names() -> Vec<String> {
    let mut out: Vec<String> = crate::squiggles::LANGS.iter().map(|(n, _)| n.to_string()).collect();
    out.extend(["English (United States)", "English (United Kingdom)", "French (France)", "Japanese", "Portuguese (Brazil)", "Swedish"].map(String::from));
    out
}

fn error_keys() -> Vec<String> {
    errors::TEMPLATES.iter().chain(errors::OS_REASONS).map(|s| s.to_string()).collect()
}

/// The names with no row: in `ctx` (then the plain row), or the plain row when `ctx` is empty.
/// The command palette's entries: every engine command it can run with no arguments, and the UI's.
fn palette_labels() -> Vec<String> {
    let session = deckcraft_engine::Session::with_new();
    let mut out: Vec<String> = session
        .commands()
        .into_iter()
        .filter(|c| !deckcraft_engine::cmd::needs_arguments(c.params))
        .map(|c| c.label.to_string())
        .collect();
    out.extend(crate::UI_COMMANDS.iter().filter(|c| !deckcraft_engine::cmd::needs_arguments(c.3)).map(|c| c.1.to_string()));
    out
}

fn missing(ctx: &str, names: Vec<String>) -> Vec<String> {
    let pt = pt();
    let mut out: Vec<String> = names.into_iter().filter(|n| !(has(pt, n) || !ctx.is_empty() && pt.i18n().get(pt.lang, ctx, n).is_some())).collect();
    out.sort();
    out.dedup();
    out
}

/// (area, context its rows may carry, English names).
fn areas() -> Vec<(&'static str, &'static str, Vec<String>)> {
    vec![
        ("animations", "animation", animation_names()),
        ("bevel", "bevel", bevel_names()),
        ("templates", "", template_names()),
        ("languages", "", language_names()),
        ("errors", "", error_keys()),
        ("palette", "", palette_labels()),
    ]
}

#[test]
#[ignore = "lists the names that have no Portuguese row yet; run with --ignored --nocapture"]
fn list_missing_rows() {
    for (area, ctx, names) in areas() {
        let m = missing(ctx, names);
        println!("AREA {area} {}", m.len());
        for n in m {
            println!("ROW\t{n}");
        }
    }
}

#[test]
fn restante_areas_have_rows() {
    for (area, ctx, names) in areas() {
        let m = missing(ctx, names);
        assert!(m.is_empty(), "{area}: no Portuguese row for {m:?}");
    }
}

#[test]
fn animation_names_and_options_are_portuguese() {
    let pt = pt();
    for (en, want) in [
        ("Appear", "Aparecer"),
        ("Fade", "Esmaecer"),
        ("Float In", "Flutuar para Dentro"),
        ("Split", "Dividir"),
        ("Wipe", "Revelar"),
        ("Shape", "Forma"),
        ("Wheel", "Roda"),
        ("Random Bars", "Barras Aleatórias"),
        ("Grow & Turn", "Crescer e Girar"),
        ("Zoom", "Zoom"),
        ("Spin", "Girar"),
        ("Bounce", "Saltar"),
        ("From Bottom", "Da Parte Inferior"),
        ("From Left", "Da Esquerda"),
        ("From Right", "Da Direita"),
        ("From Top", "Da Parte Superior"),
        ("Horizontal", "Horizontal"),
        ("Vertical", "Vertical"),
        ("As One Object", "Como Um Objeto"),
        ("By Paragraph", "Por Parágrafo"),
        ("Effect Options", "Opções de Efeito"),
    ] {
        assert_eq!(trc(pt, "animation", en), want, "{en}");
    }
    // The label is built in English from the effect and the option, then looked up whole.
    assert_eq!(trc(pt, "animation", &animation_option_label("fly", "b")), "Da Parte Inferior");
    assert_eq!(trc(pt, "animation", &animation_option_label("flyOut", "l")), "Para a Esquerda");
    assert_eq!(trc(pt, "animation", &animation_option_label("wheel", "4")), "4 raios");
    assert_eq!(trc(pt, "animation", &animation_option_label("zoom", "in")), "Zoom para Dentro");
}

#[test]
fn bevel_menu_shows_names_not_ids() {
    let pt = pt();
    for (id, en, want) in [
        ("circle", "Circle", "Círculo"),
        ("relaxedInset", "Relaxed Inset", "Inserção Relaxada"),
        ("cross", "Cross", "Cruz"),
        ("coolSlant", "Cool Slant", "Inclinação Suave"),
        ("angle", "Angle", "Ângulo"),
        ("softRound", "Soft Round", "Arredondamento Suave"),
        ("convex", "Convex", "Convexo"),
        ("slope", "Slope", "Declive"),
        ("divot", "Divot", "Divot"),
        ("riblet", "Riblet", "Nervura"),
        ("hardEdge", "Hard Edge", "Borda Rígida"),
        ("artDeco", "Art Deco", "Art Déco"),
    ] {
        assert_eq!(bevel_name(id), en);
        assert_eq!(trc(pt, "bevel", bevel_name(id)), want, "{id}");
    }
    assert_eq!(bevel_name("unknownId"), "unknownId");
    // No id leaks into what the menu draws, in either language.
    for lang in [Lang::EN, pt] {
        for id in BEVEL_IDS {
            let shown = trc(lang, "bevel", bevel_name(id));
            assert_ne!(shown, id, "the raw id reached the menu");
        }
    }
}

#[test]
fn templates_and_languages_are_portuguese() {
    let pt = pt();
    assert_eq!(tr(pt, "English (United States)"), "Inglês (Estados Unidos)");
    assert_eq!(tr(pt, "Español (España)"), "Espanhol (Espanha)");
    assert_eq!(tr(pt, "日本語"), "Japonês");
    assert_eq!(tr(pt, "Português (Brasil)"), "Português (Brasil)");
    assert_eq!(tr(pt, "Field Notes"), "Notas de Campo");
}

#[test]
fn engine_errors_are_worded_at_the_display_point() {
    set_current(pt());
    let e = |s: &str| user_error(s).into_owned();
    let no_file = e("/tmp/a.pptx: No such file or directory (os error 2)");
    assert!(no_file.starts_with("/tmp/a.pptx: ") && !no_file.contains("No such file"), "{no_file}");
    let denied = e("Couldn't save a.pptx: Permission denied (os error 13)");
    assert!(!denied.contains("Couldn't") && !denied.contains("Permission denied") && denied.contains("a.pptx"), "{denied}");
    let unreadable = e("deck.pptx: not a presentation DeckCraft can read");
    assert!(unreadable.contains("deck.pptx") && !unreadable.contains("not a presentation"), "{unreadable}");
    let unsupported = e("unsupported: tables in tables");
    assert!(!unsupported.contains("unsupported") && unsupported.contains("tables in tables"), "{unsupported}");
    let read_only = "This is a previous version. Restore it or save a copy to make changes.";
    assert_ne!(e(read_only), read_only);
    // Anything unmapped passes through untouched.
    assert_eq!(e("something nobody mapped"), "something nobody mapped");
    set_current(Lang::EN);
    assert_eq!(user_error("/tmp/x.pptx: Permission denied (os error 13)"), "/tmp/x.pptx: Permission denied (os error 13)");
}

#[test]
fn date_samples_follow_the_language() {
    let probe = deckcraft_text::datetime::DateTime::new(2026, 11, 8, 20, 5, 7);
    let f = |lang: &str, kind: &str| deckcraft_text::datetime::field_text(kind, Some(lang), probe);
    assert_eq!(f("pt-BR", "datetime2").as_deref(), Some("domingo, 8 de novembro de 2026"));
    assert_eq!(f("en-US", "datetime4").as_deref(), Some("November 8, 2026"));
    // The date lists take the language: Portuguese rows carry the Portuguese month names.
    let rows = crate::dialogs::date_sample_rows("pt-BR");
    assert!(rows.iter().any(|(_, s)| s.contains(" de ")), "{rows:?}");
    let rows = crate::dialogs::date_sample_rows("en-US");
    assert!(!rows.iter().any(|(_, s)| s.contains(" de ")), "{rows:?}");
}

/// Draw one state of the app and collect the texts it shows. What a state shows identically in
/// English and in Portuguese is a leftover (or a name that is the same in both, a font, document text).
fn drawn_texts(language: &str, setup: impl FnOnce(&mut crate::SlideApp)) -> std::collections::BTreeSet<String> {
    use crate::SlideApp;
    use egui_kittest::kittest::NodeT;
    use crate::services::Services;
    use deckcraft_engine::Session;
    let mut app = SlideApp::new(Session::with_new(), Services::default());
    app.ui.language = language.into();
    setup(&mut app);
    let mut h = egui_kittest::Harness::builder().with_size(egui::vec2(1500.0, 900.0)).with_accessibility_check(false).build_ui_state(
        |ui, app: &mut SlideApp| {
            app.logic(ui.ctx());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(6);
    let mut out = std::collections::BTreeSet::new();
    for node in h.root().children_recursive() {
        for text in [node.accesskit_node().label(), node.accesskit_node().value()].into_iter().flatten() {
            let text = text.trim();
            if text.chars().filter(|c| c.is_alphabetic()).count() >= 3 {
                out.insert(text.to_string());
            }
        }
    }
    set_current(Lang::EN);
    out
}

#[test]
#[ignore = "lists texts drawn identically in English and Portuguese; run with --ignored --nocapture"]
fn list_drawn_english() {
    let tabs = ["Home", "Insert", "Draw", "Design", "Transitions", "Animations", "Slide Show", "Record", "Review", "View"];
    let panes = ["animation", "background", "clipboard", "comments", "designer", "format", "selection", "smartart", "versions"];
    let dialogs = [
        "table", "slideSize", "sizePosition", "headerFooter", "hyperlink", "setupShow", "zoom", "comment", "altText", "outline", "renameSection",
        "reuseSlides", "renameShape", "renameLayout", "paragraph", "tabs", "font", "about", "preferences", "autocorrect", "gridOptions", "columns",
        "export", "symbol", "photoAlbum", "replaceFonts", "equation", "dateTime", "spelling", "pasteSpecial", "find", "accessibility", "language",
        "customShow", "chartData", "trim", "smartart", "bullets", "action", "saveConflict", "saveToCloud", "themeColors", "themeFonts",
    ];
    type Setup = Box<dyn Fn(&mut crate::SlideApp)>;
    let mut states: Vec<(String, Setup)> = vec![];
    for t in tabs {
        states.push((format!("tab {t}"), Box::new(move |a| a.ui.tab = t.into())));
    }
    for p in panes {
        states.push((format!("pane {p}"), Box::new(move |a| a.ui.pane = Some(p.into()))));
    }
    for d in dialogs {
        states.push((format!("dialog {d}"), Box::new(move |a| a.dialog = Some(crate::dialogs::Dialog::new(d)))));
    }
    let mut seen = std::collections::BTreeMap::<String, String>::new();
    for (what, setup) in &states {
        // A state that cannot be drawn on this machine (a missing font aborts the frame) is reported, not fatal.
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (drawn_texts("en", |a| setup(a)), drawn_texts("pt-br", |a| setup(a)))));
        match drawn {
            Ok((en, pt)) => en.intersection(&pt).for_each(|s| {
                seen.entry(s.clone()).or_insert_with(|| what.clone());
            }),
            Err(_) => println!("SKIPPED	{what}"),
        }
    }
    println!("DRAWN {}", seen.len());
    for (s, w) in seen {
        println!("DRAWN	{w}	{s}");
    }
}
