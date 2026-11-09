//! Tests of the previous version shown from Version History: it opens read only beside the
//! presentation, refuses what would change it, compares, restores, saves a copy and closes.

#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use super::{READ_ONLY, VersionInfo};
use crate::tools::{Mods, PointerEvent, PointerKind};
use crate::{Session, ToolKind};

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("deckcraft-version-view-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn listed(s: &mut Session) -> Vec<VersionInfo> {
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)), "the worker did not finish");
    let owner = s.active_index().and_then(|i| s.history_owner(i)).unwrap();
    s.version_infos(owner)
}

fn slides(s: &Session) -> usize {
    s.active().unwrap().doc.slides.len()
}

/// A presentation saved twice (one slide, then two) with a third slide not yet saved. Returns the
/// session, its folder, the file and the ids of the oldest and the newest saved version.
fn with_history(name: &str) -> (Session, PathBuf, PathBuf, String, String) {
    let root = temp(name);
    let mut s = Session::new();
    s.prefs.author = "Tester Two".into();
    s.enable_versions(root.join("versions"), Arc::new(|| {})).unwrap();
    s.execute("file.new", &json!({})).unwrap();
    let file = root.join("Quarterly.pptx");
    let saved = s.execute("file.save", &json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!(saved["saved"], true, "{saved}");
    s.execute("slide.new", &json!({"layout": "blank"})).unwrap();
    s.execute("file.save", &json!({})).unwrap();
    s.execute("slide.new", &json!({"layout": "blank"})).unwrap();
    let list = listed(&mut s);
    assert_eq!(list.len(), 2, "two saves, two versions");
    let (newest, oldest) = (list[0].id.clone(), list[1].id.clone());
    (s, root, file, oldest, newest)
}

fn open(s: &mut Session, id: &str) -> serde_json::Value {
    s.execute("file.openVersion", &json!({"id": id})).unwrap()
}

#[test]
fn a_version_opens_read_only_beside_the_presentation() {
    let (mut s, root, _, oldest, _) = with_history("open");
    let current = s.active().unwrap().uid;
    let r = open(&mut s, &oldest);
    assert_eq!(s.documents().len(), 2, "beside the presentation, not in its place");
    assert_eq!(r["document"], 1);
    let shown = s.active().unwrap();
    assert!(shown.previous.is_some() && shown.path.is_none() && !shown.is_dirty());
    assert!(shown.title().starts_with("Quarterly (version 20"), "{}", shown.title());
    assert_eq!(slides(&s), 1, "the oldest version has one slide");
    assert_eq!(s.documents()[0].doc.slides.len(), 3, "the presentation is untouched");
    assert_eq!(s.active_previous_version().map(|v| v.source), Some(current));
    assert_eq!(s.active_previous_version().map(|v| v.info.author.as_str()), Some("Tester Two"));
    // The history shown is the presentation's, from either one.
    assert_eq!(listed(&mut s).len(), 2);
    assert_eq!(s.execute("file.versions", &json!({})).unwrap().as_array().map(Vec::len), Some(2));
    // Opening another version replaces the one shown.
    let list = listed(&mut s);
    open(&mut s, &list[0].id);
    assert_eq!(s.documents().len(), 2);
    assert_eq!(slides(&s), 2);
    // The viewer can look around.
    s.execute("slide.go", &json!({"index": 1})).unwrap();
    s.execute("view.slideMaster", &json!({})).ok();
    assert!(s.execute("file.info", &json!({})).is_ok());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn what_would_change_a_version_is_refused_and_looking_still_works() {
    let (mut s, root, _, oldest, _) = with_history("refuse");
    open(&mut s, &oldest);
    s.execute("shape.insert", &json!({"preset": "rect", "rect": [10.0, 10.0, 50.0, 50.0]})).expect_err("a shape");
    for (id, p) in [
        ("slide.new", json!({})),
        ("edit.undo", json!({})),
        ("edit.redo", json!({})),
        ("edit.paste", json!({})),
        ("text.insert", json!({"text": "x"})),
        ("file.properties", json!({"title": "x"})),
        ("file.revert", json!({})),
        ("review.compare", json!({"data": "AAAA"})),
    ] {
        let e = s.execute(id, &p).expect_err(id).to_string();
        assert_eq!(e, READ_ONLY, "{id}");
    }
    assert_eq!(slides(&s), 1);
    assert!(!s.active().unwrap().is_dirty());
    // Moving around, selecting, copying and the view are not edits.
    s.execute("edit.selectAll", &json!({})).unwrap();
    s.execute("edit.copy", &json!({})).unwrap();
    s.execute("edit.deselect", &json!({})).unwrap();
    s.execute("slide.first", &json!({})).unwrap();
    s.execute("view.zoomIn", &json!({})).ok();
    assert!(s.execute("shape.inspect", &json!({"id": 1})).is_ok() || s.execute("slide.inspect", &json!({})).is_ok());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_pointer_gesture_cannot_move_or_draw_in_a_version() {
    let root = temp("pointer");
    let mut s = Session::new();
    s.enable_versions(root.join("versions"), Arc::new(|| {})).unwrap();
    s.execute("file.new", &json!({})).unwrap();
    s.execute("slide.new", &json!({"layout": "blank"})).unwrap();
    let id = s.execute("shape.insert", &json!({"preset": "rect", "rect": [100.0, 100.0, 100.0, 80.0]})).unwrap()["id"].as_u64().unwrap();
    s.prefs.snap_to_grid = false;
    s.prefs.smart_guides = false;
    let file = root.join("Deck.pptx");
    s.execute("file.save", &json!({"path": file.to_string_lossy()})).unwrap();
    let version = listed(&mut s)[0].id.clone();
    open(&mut s, &version);
    let before = s.active().unwrap().doc.clone();
    let ev = |kind, x, y| PointerEvent { kind, x, y, mods: Mods::default(), tol: 3.0, pressure: None };
    s.pointer(ev(PointerKind::Down, 150.0, 140.0)).ok();
    let moved = s.pointer(ev(PointerKind::Drag, 190.0, 170.0));
    s.pointer(ev(PointerKind::Up, 190.0, 170.0)).ok();
    assert!(moved.is_err(), "dragging the shape is refused");
    assert!(Arc::ptr_eq(&s.active().unwrap().doc, &before), "and nothing moved");
    assert!(s.active().unwrap().history.undo.is_empty());
    // Clicking selects, as it does in any presentation.
    assert_eq!(s.active().unwrap().selection.shapes, vec![crate::model::ShapeId(u32::try_from(id).unwrap())]);
    // Drawing is refused too.
    s.set_tool(ToolKind::Shape { preset: "rect".into() });
    s.pointer(ev(PointerKind::Down, 10.0, 10.0)).ok();
    s.pointer(ev(PointerKind::Drag, 60.0, 60.0)).ok();
    s.pointer(ev(PointerKind::Up, 60.0, 60.0)).ok();
    assert!(Arc::ptr_eq(&s.active().unwrap().doc, &before));
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn close_returns_to_the_presentation_exactly_as_it_was() {
    let (mut s, root, _, oldest, _) = with_history("close");
    let (uid, doc, dirty, steps, slide) = {
        let d = s.active().unwrap();
        (d.uid, d.doc.clone(), d.is_dirty(), d.history.undo.len(), d.selection.slide)
    };
    assert!(dirty, "the third slide was never saved");
    open(&mut s, &oldest);
    s.execute("file.closeVersion", &json!({})).unwrap();
    assert_eq!(s.documents().len(), 1);
    let d = s.active().unwrap();
    assert_eq!(d.uid, uid);
    assert!(Arc::ptr_eq(&d.doc, &doc), "the very same presentation, unsaved changes and all");
    assert_eq!((d.is_dirty(), d.history.undo.len(), d.selection.slide), (dirty, steps, slide));
    assert!(d.previous.is_none() && d.path.is_some());
    // No version is shown: closing one is not available.
    assert!(s.execute("file.closeVersion", &json!({})).is_err());
    // File Close does the same and stops there.
    open(&mut s, &oldest);
    s.execute("file.close", &json!({})).unwrap();
    assert_eq!(s.documents().len(), 1);
    assert_eq!(s.active().unwrap().uid, uid);
    // Closing the presentation closes the version shown for it.
    open(&mut s, &oldest);
    s.set_active(0);
    s.execute("file.close", &json!({})).unwrap();
    assert!(s.documents().is_empty());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn restore_from_a_version_is_one_undo_step() {
    let (mut s, root, _, oldest, _) = with_history("restore");
    let uid = s.active().unwrap().uid;
    open(&mut s, &oldest);
    let r = s.execute("file.versions", &json!({"restore": oldest})).unwrap();
    assert_eq!(r["restored"], json!(oldest));
    assert_eq!(s.documents().len(), 1, "the version is closed first");
    assert_eq!(s.active().unwrap().uid, uid);
    assert_eq!(slides(&s), 1);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(slides(&s), 3, "one Undo brings back the presentation with its unsaved slide");
    let list = listed(&mut s);
    assert!(list.iter().any(|v| v.kind.id() == "restore"), "the restore is recorded at once");
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn save_a_copy_writes_a_new_file_and_goes_back() {
    let (mut s, root, file, oldest, _) = with_history("copy");
    let uid = s.active().unwrap().uid;
    let original_id = s.version_document_id(0).unwrap();
    open(&mut s, &oldest);
    let copy = root.join("Quarterly (copy).pptx");
    let r = s.execute("file.saveAs", &json!({"path": copy.to_string_lossy()})).unwrap();
    assert_eq!(r["saved"], true, "{r}");
    assert_eq!(r["copyOf"], json!(oldest));
    assert!(copy.exists());
    assert_eq!(s.documents().len(), 1, "back to the presentation");
    assert_eq!(s.active().unwrap().uid, uid);
    assert_eq!(s.active().unwrap().path.as_deref(), Some(file.to_string_lossy().as_ref()));
    assert_eq!(slides(&s), 3);
    // The copy has a document id of its own and says where it came from.
    assert_eq!(listed(&mut s).len(), 2, "the presentation's own history did not get the copy");
    let mut t = Session::new();
    t.enable_versions(root.join("versions"), Arc::new(|| {})).unwrap();
    t.execute("file.open", &json!({"path": copy.to_string_lossy()})).unwrap();
    assert_eq!(slides(&t), 1);
    let copy_id = t.version_document_id(0).unwrap();
    assert_ne!(copy_id, original_id);
    let list = listed(&mut t);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind.id(), "saveAs");
    let origin = list[0].details.as_ref().and_then(|d| d.origin.clone()).expect("the origin is recorded");
    assert_eq!(origin.document_id.as_deref(), Some(original_id.as_str()));
    assert_eq!(origin.version_id.as_deref(), Some(oldest.as_str()));
    drop((s, t));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compare_shows_the_version_as_the_original_and_again_shows_the_version() {
    let (mut s, root, _, oldest, _) = with_history("compare");
    open(&mut s, &oldest);
    let before = s.active().unwrap().doc.clone();
    let r = s.execute("file.compareVersion", &json!({})).unwrap();
    assert_eq!(r["comparing"], true);
    let all = s.execute("review.revisions", &json!({})).unwrap();
    let listed_changes = all["changes"].as_array().map(Vec::len).unwrap_or(0);
    assert!(listed_changes >= 2, "the slides added since are differences: {all}");
    assert_eq!(r["changes"].as_u64(), Some(listed_changes as u64));
    assert!(all["changes"].as_array().unwrap().iter().any(|c| c["kind"] == "inserted"), "slides only the presentation has are inserted: {all}");
    let d = s.active().unwrap();
    assert!(d.compare.is_some() && d.previous.is_some());
    assert!(Arc::ptr_eq(&d.doc, &before), "the version stays the original");
    assert_eq!(d.compare.as_ref().map(|c| c.revised.slides.len()), Some(3), "the presentation is the revised one");
    assert_eq!(s.ui_requests.last().map(|r| format!("{r:?}")).as_deref().map(|t| t.contains("revisions")), Some(true));
    let revisions = s.execute("review.revisions", &json!({})).unwrap();
    assert_eq!(revisions["changes"].as_array().map(Vec::len), Some(listed_changes));
    // Accepting or rejecting would change the version.
    assert_eq!(s.execute("review.accept", &json!({"all": true})).expect_err("accept").to_string(), READ_ONLY);
    assert_eq!(s.execute("review.reject", &json!({"all": true})).expect_err("reject").to_string(), READ_ONLY);
    let again = s.execute("file.compareVersion", &json!({})).unwrap();
    assert_eq!(again["comparing"], false);
    assert!(s.active().unwrap().compare.is_none());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn new_and_open_go_back_to_the_presentation_first() {
    let (mut s, root, file, oldest, _) = with_history("newopen");
    let uid = s.active().unwrap().uid;
    open(&mut s, &oldest);
    s.execute("file.new", &json!({})).unwrap();
    assert_eq!(s.documents().len(), 2, "the version closed; the presentation and the new one are open");
    assert!(s.documents().iter().all(|d| d.previous.is_none()));
    assert!(s.documents().iter().any(|d| d.uid == uid && d.is_dirty()), "its unsaved changes are still there");
    s.set_active(s.index_of_uid(uid).unwrap());
    open(&mut s, &oldest);
    s.execute("file.open", &json!({"path": file.to_string_lossy()})).unwrap();
    assert!(s.documents().iter().all(|d| d.previous.is_none()));
    // Quitting closes what is shown first and leaves the presentation in front.
    s.set_active(s.index_of_uid(uid).unwrap());
    open(&mut s, &oldest);
    s.close_previous_versions();
    assert!(s.documents().iter().all(|d| d.previous.is_none()));
    assert_eq!(s.active().unwrap().uid, uid);
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn renaming_the_file_keeps_the_history_and_refuses_bad_names() {
    let (mut s, root, file, _, _) = with_history("rename");
    let id = s.version_document_id(0).unwrap();
    let r = s.execute("file.rename", &json!({"name": "Budget"})).unwrap();
    let renamed = root.join("Budget.pptx");
    assert_eq!(r["renamed"], true);
    assert_eq!(s.active().unwrap().path.as_deref(), Some(renamed.to_string_lossy().as_ref()));
    assert!(renamed.exists() && !file.exists());
    assert_eq!(s.active().unwrap().title(), "Budget");
    assert_eq!(s.version_document_id(0).as_deref(), Some(id.as_str()));
    assert_eq!(listed(&mut s).len(), 2, "the history is the file's, not the name's");
    // The same name again is nothing to do; a taken name, a bad name and a version are refused.
    assert_eq!(s.execute("file.rename", &json!({"name": "Budget.pptx"})).unwrap()["renamed"], false);
    std::fs::write(root.join("Taken.pptx"), b"x").unwrap();
    assert!(s.execute("file.rename", &json!({"name": "Taken"})).unwrap_err().to_string().contains("already exists"));
    assert!(root.join("Taken.pptx").exists() && renamed.exists());
    for bad in ["", "a/b", "what?"] {
        assert!(s.execute("file.rename", &json!({"name": bad})).is_err(), "{bad:?}");
    }
    // A save after the rename goes to the new name, and the history grows.
    s.execute("file.save", &json!({})).unwrap();
    assert_eq!(listed(&mut s).len(), 3);
    let version = listed(&mut s)[2].id.clone();
    open(&mut s, &version);
    assert!(s.execute("file.rename", &json!({"name": "Nope"})).is_err(), "a version has no file");
    assert!(Path::new(&renamed).exists());
    drop(s);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_presentation_with_no_file_renames_nothing() {
    let mut s = Session::with_new();
    assert!(s.execute("file.rename", &json!({"name": "Anything"})).unwrap_err().to_string().contains("save the presentation"));
}
