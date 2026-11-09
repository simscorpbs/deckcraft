//! Panes (Format Shape, Format Background, Animation, Comments, Designer, Text pane, Thesaurus),
//! the canvas overlays and the long-tail gallery names in Portuguese.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;

fn pt() -> Lang {
    Lang::from_code("pt-br").expect("the pt-br catalogs are embedded (i18n/locales/pt-br)")
}

/// The English names the galleries show for the model's ids: shape presets and their categories,
/// SmartArt layouts, themes, slide transitions and their effect options.
fn gallery_names() -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for cat in deckcraft_geom::preset::Category::ALL {
        out.push(cat.label().into());
    }
    out.extend(deckcraft_geom::preset::CATALOG.iter().map(|p| p.label.to_string()));
    out.extend(deckcraft_engine::tools::FREEFORM_TOOLS.iter().map(|f| f.1.to_string()));
    out.extend(deckcraft_engine::cmd::smartart::LAYOUTS.iter().map(|l| l.1.to_string()));
    out.extend(deckcraft_model::theme::builtin_themes().into_iter().map(|t| t.name));
    for (id, label, cat, _, opts) in deckcraft_model::anim::TRANSITIONS {
        out.push(label.to_string());
        out.push(cat.to_string());
        out.extend(opts.iter().map(|o| deckcraft_model::anim::transition_option_label(id, o)));
    }
    out
}

/// "perspectiveHeroicLeftFacing" as Format Shape ▸ 3-D shows it: "Perspective heroic left facing".
fn three_d_names() -> Vec<String> {
    use deckcraft_engine::cmd::three_d::{BEVELS, CAMERAS, LIGHTS, MATERIALS};
    let label = |name: &str| {
        let mut out = String::new();
        for (i, c) in name.chars().enumerate() {
            if i == 0 {
                out.extend(c.to_uppercase());
            } else if c.is_uppercase() {
                out.push(' ');
                out.extend(c.to_lowercase());
            } else {
                out.push(c);
            }
        }
        out
    };
    BEVELS.iter().chain(MATERIALS).chain(LIGHTS).chain(CAMERAS).map(|n| label(n)).chain(std::iter::once("None".to_string())).collect()
}

#[test]
#[ignore = "lists the gallery names that have no Portuguese row yet; run with --ignored --nocapture"]
fn list_missing_gallery_rows() {
    let mut missing: Vec<String> = gallery_names().into_iter().chain(three_d_names()).filter(|n| !has(pt(), n)).collect();
    missing.sort();
    missing.dedup();
    println!("MISSING {}", missing.len());
    for m in missing {
        println!("ROW\t{m}");
    }
}

#[test]
fn panes_and_overlays_are_translated() {
    let pt = pt();
    for (en, want) in [
        ("Format Shape", "Formatar Forma"),
        ("Format Background", "Formatar Plano de Fundo"),
        ("Animation Pane", "Painel de Animação"),
        ("Fill & Line", "Preenchimento e Linha"),
        ("Size & Properties", "Tamanho e Propriedades"),
        ("Rotate with shape", "Girar com a forma"),
        ("Hide background graphics", "Ocultar elementos gráficos de plano de fundo"),
        ("Apply to All", "Aplicar a Todos"),
        ("Pick a design for this slide. Ideas are generated on your device.", "Escolha um design para este slide. As ideias são geradas no seu dispositivo."),
        ("Text ruler", "Régua de texto"),
    ] {
        assert_eq!(tr(pt, en), want, "{en}");
    }
    assert_eq!(fmt(tr(pt, "Slide {n} deleted"), &[("n", "3")]), "Slide 3 excluído");
}

#[test]
fn the_gallery_names_are_translated() {
    let pt = pt();
    let missing: Vec<String> = gallery_names().into_iter().chain(three_d_names()).filter(|n| !has(pt, n)).collect();
    assert!(missing.is_empty(), "no Portuguese row for: {missing:?}");
}
