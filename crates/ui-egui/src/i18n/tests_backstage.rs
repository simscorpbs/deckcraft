//! Backstage, slide show, presenter, media bar and notices in Portuguese.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn pt() -> Lang {
    Lang::from_code("pt-br").expect("the pt-br catalogs are embedded (i18n/locales/pt-br)")
}

#[test]
fn backstage_wording_is_translated() {
    let pt = pt();
    assert_eq!(tr(pt, "Save As"), "Salvar como");
    assert_eq!(trc(pt, "tab", "Home"), "Página Inicial");
    assert_eq!(tr(pt, "Recover Unsaved Presentations"), "Recuperar Apresentações Não Salvas");
    assert_eq!(tr(pt, "Handouts (3 slides per page)"), "Folhetos (3 slides por página)");
    assert_eq!(fmt(tr(pt, "No recent presentations match “{query}”."), &[("query", "abc")]), "Nenhuma apresentação recente corresponde a “abc”.");
}

#[test]
fn show_presenter_and_notice_wording_is_translated() {
    let pt = pt();
    assert_eq!(tr(pt, "End Show"), "Encerrar Apresentação");
    assert_eq!(tr(pt, "End of slide show, click to exit."), "Fim da apresentação de slides. Clique para sair.");
    assert_eq!(tr(pt, "Play or pause media"), "Reproduzir ou pausar mídia");
    assert_eq!(fmt(tr(pt, "Slide {n} of {total}"), &[("n", "2"), ("total", "9")]), "Slide 2 de 9");
}

#[test]
fn every_backstage_string_has_a_portuguese_row() {
    let pt = pt();
    for s in [
        "Back (Esc)",
        "Blank Presentation",
        "Remove from Recent",
        "PowerPoint Presentation (*.pptx)",
        "Opens in PowerPoint and other presentation apps",
        "Add a title",
        "Check for Issues",
        "Print All Slides",
        "Full Page Slides",
        "Letter (8.5 × 11 in)",
        "Create PDF Document",
        "Opening {name}…",
        "Opened the presentation as a PDF to print",
        "{names} aren't installed on this computer. Text that uses them is shown in a similar font.",
        "Presenter View — DeckCraft",
        "No sound: {error}",
        "Media control",
    ] {
        assert!(has(pt, s), "no pt-br row for {s:?}");
    }
}
