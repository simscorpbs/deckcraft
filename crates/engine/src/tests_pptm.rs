//! Saving to a macro-enabled `.pptm`: a PowerPoint macro-enabled package, never native bytes, and
//! a warning that macros are not kept.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Read;

use serde_json::json;

use crate::Session;
use crate::cmd::file::{MACROS_DROPPED, format_for_path, save_bytes, writable_format_for_path};

fn main_content_type(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip package");
    let mut types = String::new();
    zip.by_name("[Content_Types].xml").unwrap().read_to_string(&mut types).unwrap();
    let at = types.find("/ppt/presentation.xml").expect("the main part is listed");
    let open = types[..at].rfind("<Override").unwrap();
    let close = types[at..].find("/>").unwrap() + at;
    types[open..close].to_string()
}

#[test]
fn pptm_is_a_writable_format_of_its_own() {
    assert_eq!(format_for_path("Deck.pptm"), "pptm");
    assert_eq!(format_for_path("dir/DECK.PPTM"), "pptm");
    assert_eq!(writable_format_for_path("Deck.pptm"), Some("pptm"));
    assert_eq!(format_for_path("Deck.pptx"), "pptx", "the others are as they were");
}

#[test]
fn pptm_bytes_are_a_macro_enabled_package_not_native_bytes() {
    let s = Session::with_new();
    let doc = &s.doc().unwrap().doc;
    let pptm = save_bytes(doc, "pptm").unwrap();
    assert!(deckcraft_pptx::sniff(&pptm), "pptx structure");
    assert!(!deckcraft_format::sniff(&pptm), "not DeckCraft's native bytes");
    assert!(main_content_type(&pptm).contains("application/vnd.ms-powerpoint.presentation.macroEnabled.main+xml"));
    let pptx = save_bytes(doc, "pptx").unwrap();
    assert!(main_content_type(&pptx).contains("presentationml.presentation.main+xml"), "a pptx keeps its own type");
    // The package reads back.
    let back = deckcraft_pptx::import(&pptm).unwrap();
    assert_eq!(back.slides.len(), doc.slides.len());
}

#[test]
fn saving_to_a_pptm_writes_the_package_and_warns_that_macros_are_dropped() {
    let dir = std::env::temp_dir().join(format!("deckcraft-pptm-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Macros.pptm").to_string_lossy().to_string();

    let mut s = Session::with_new();
    s.execute("slide.new", &json!({})).unwrap();
    let r = s.execute("file.save", &json!({"path": path})).unwrap();
    assert_eq!(r["saved"], true);
    assert_eq!(r["macrosDropped"], true);
    assert_eq!(r["warning"], MACROS_DROPPED);
    let bytes = std::fs::read(&path).unwrap();
    assert!(deckcraft_pptx::sniff(&bytes) && !deckcraft_format::sniff(&bytes));
    assert!(main_content_type(&bytes).contains("macroEnabled.main+xml"));

    // It opens again and saves again.
    let mut t = Session::new();
    t.execute("file.open", &json!({"path": path})).unwrap();
    assert_eq!(t.doc().unwrap().doc.slides.len(), 2);
    assert_eq!(t.execute("file.save", &json!({})).unwrap()["macrosDropped"], true);

    // A pptx save carries no warning; export to a pptm does.
    let pptx = dir.join("Plain.pptx").to_string_lossy().to_string();
    let r = s.execute("file.saveAs", &json!({"path": pptx})).unwrap();
    assert!(r.get("warning").is_none() && r.get("macrosDropped").is_none());
    let out = dir.join("Export.pptm").to_string_lossy().to_string();
    let r = s.execute("file.export", &json!({"path": out})).unwrap();
    assert_eq!(r["macrosDropped"], true);
    assert!(main_content_type(&std::fs::read(&out).unwrap()).contains("macroEnabled"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn autosave_and_the_version_history_still_leave_a_pptm_alone() {
    // A deck opened from a macro-enabled file has lost its macros already; AutoSave writing it again
    // and again would hide that, so only an explicit Save (which warns) writes a .pptm.
    assert!(!crate::autosave::keeps_everything("Macros.pptm"));
}
