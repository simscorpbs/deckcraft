//! The Clipboard pane's history: what Copy and Cut collect, and pasting from it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};

use crate::Session;
use crate::cmd::clipboard_pane::CLIP_MAX_ITEMS;

fn session() -> Session {
    Session::with_new()
}

/// A rectangle with `text`, copied.
fn copy_shape(s: &mut Session, text: &str) {
    let r = s.execute("shape.insert", &json!({"preset": "rect", "rect": [10, 10, 80, 40], "text": text})).unwrap();
    s.execute("selection.select", &json!({"id": r["id"]})).unwrap();
    s.execute("edit.copy", &json!({})).unwrap();
}

fn previews(s: &mut Session) -> Vec<String> {
    let items = s.execute("edit.clipboardItems", &json!({})).unwrap();
    items.as_array().unwrap().iter().map(|i| i["preview"].as_str().unwrap().to_string()).collect()
}

fn slide_texts(s: &Session) -> Vec<String> {
    s.doc().unwrap().current_slide().unwrap().shapes.iter().filter_map(|x| x.text.as_ref().map(|t| t.text())).collect()
}

#[test]
fn history_keeps_the_last_24_newest_first() {
    let mut s = session();
    for i in 0..30 {
        copy_shape(&mut s, &format!("item {i}"));
    }
    let p = previews(&mut s);
    assert_eq!(p.len(), CLIP_MAX_ITEMS);
    assert_eq!(p[0], "item 29");
    assert_eq!(p[CLIP_MAX_ITEMS - 1], "item 6", "the oldest drop out");
    // Copying the same selection again adds nothing.
    s.execute("edit.copy", &json!({})).unwrap();
    assert_eq!(s.clip_history.len(), CLIP_MAX_ITEMS);
    assert_eq!(previews(&mut s)[0], "item 29");
}

#[test]
fn a_caret_and_an_empty_selection_collect_nothing() {
    let mut s = session();
    s.execute("selection.clear", &json!({})).ok();
    s.execute("edit.copy", &json!({})).unwrap();
    assert!(s.clip_history.is_empty());
}

#[test]
fn cut_is_collected_too() {
    let mut s = session();
    let r = s.execute("shape.insert", &json!({"preset": "rect", "rect": [10, 10, 80, 40], "text": "cut me"})).unwrap();
    s.execute("selection.select", &json!({"id": r["id"]})).unwrap();
    s.execute("edit.cut", &json!({})).unwrap();
    assert_eq!(previews(&mut s), ["cut me"]);
    assert!(!slide_texts(&s).contains(&"cut me".to_string()));
    s.execute("edit.pasteClipboardItem", &json!({"index": 0})).unwrap();
    assert!(slide_texts(&s).contains(&"cut me".to_string()));
}

#[test]
fn an_old_item_pastes_as_the_normal_paste_would_and_becomes_the_clipboard() {
    let mut s = session();
    copy_shape(&mut s, "alpha");
    copy_shape(&mut s, "beta");
    let before = slide_texts(&s).len();
    let out = s.execute("edit.pasteClipboardItem", &json!({"index": 1})).unwrap();
    assert_eq!(out["text"], "alpha");
    let texts = slide_texts(&s);
    assert_eq!(texts.len(), before + 1);
    assert_eq!(texts.last().unwrap(), "alpha");
    assert_eq!(s.clipboard.plain, "alpha", "the item is now the clipboard");
    // Ctrl+V repeats it.
    s.execute("edit.paste", &json!({})).unwrap();
    assert_eq!(slide_texts(&s).iter().filter(|t| *t == "alpha").count(), 3, "the original and two pastes");
    // The history is unchanged by pasting.
    assert_eq!(previews(&mut s), ["beta", "alpha"]);
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(slide_texts(&s).len(), before);
}

#[test]
fn repeated_pastes_of_one_item_offset_each_time() {
    let mut s = session();
    copy_shape(&mut s, "once");
    let x_of_last = |s: &Session| s.doc().unwrap().current_slide().unwrap().shapes.last().unwrap().xfrm.unwrap().x;
    s.execute("edit.pasteClipboardItem", &json!({"index": 0})).unwrap();
    let first = x_of_last(&s);
    s.execute("edit.pasteClipboardItem", &json!({"index": 0})).unwrap();
    assert!(x_of_last(&s) > first, "a second paste of the same item lands offset from the first");
}

#[test]
fn paste_all_goes_in_copy_order_as_one_undo_step_and_keeps_the_clipboard() {
    let mut s = session();
    copy_shape(&mut s, "one");
    copy_shape(&mut s, "two");
    copy_shape(&mut s, "three");
    let before = slide_texts(&s);
    s.execute("edit.pasteAllClipboard", &json!({})).unwrap();
    let after = slide_texts(&s);
    assert_eq!(after[before.len()..], ["one", "two", "three"]);
    assert_eq!(s.clipboard.plain, "three", "the clipboard is as it was");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(slide_texts(&s), before, "Paste All is one undo step");
}

#[test]
fn delete_one_clear_all_and_bad_indexes() {
    let mut s = session();
    copy_shape(&mut s, "a");
    copy_shape(&mut s, "b");
    copy_shape(&mut s, "c");
    let left = s.execute("edit.deleteClipboardItem", &json!({"index": 1})).unwrap();
    assert_eq!(left.as_array().unwrap().len(), 2);
    assert_eq!(previews(&mut s), ["c", "a"]);

    let before = slide_texts(&s);
    for bad in [json!({"index": 2}), json!({"index": -1}), json!({"index": 1e300}), json!({"index": "x"}), json!({})] {
        assert!(s.execute("edit.pasteClipboardItem", &bad).is_err(), "{bad}");
        assert!(s.execute("edit.deleteClipboardItem", &bad).is_err(), "{bad}");
    }
    assert_eq!(slide_texts(&s), before);
    assert_eq!(s.clip_history.len(), 2);

    s.execute("edit.clearClipboard", &json!({})).unwrap();
    assert!(s.clip_history.is_empty());
    assert!(s.execute("edit.pasteClipboardItem", &json!({"index": 0})).is_err());
    assert!(s.execute("edit.pasteAllClipboard", &json!({})).is_err(), "disabled when empty");
    assert!(s.execute("edit.deleteClipboardItem", &json!({"index": 0})).is_err());
}

#[test]
fn a_copied_slide_is_an_item_with_a_thumbnail_and_pastes_as_a_slide() {
    let mut s = session();
    s.execute("slide.new", &json!({})).unwrap();
    let n = s.doc().unwrap().doc.slides.len();
    s.execute("edit.copy", &json!({"scope": "slides"})).unwrap();
    let items = s.execute("edit.clipboardItems", &json!({})).unwrap();
    assert_eq!(items[0]["kind"], "slides");
    assert_eq!(items[0]["slides"], 1);
    assert!(s.clip_history.items()[0].thumbnail.is_some());
    s.execute("edit.pasteClipboardItem", &json!({"index": 0})).unwrap();
    assert_eq!(s.doc().unwrap().doc.slides.len(), n + 1);
}

#[test]
fn copied_shapes_get_a_thumbnail() {
    let mut s = session();
    copy_shape(&mut s, "pic");
    let t = s.clip_history.items()[0].thumbnail.as_ref().expect("a thumbnail");
    assert!(t.width > 0 && t.height > 0 && t.pixels.len() == (t.width * t.height * 4) as usize);
    let v: Value = s.execute("edit.clipboardItems", &json!({})).unwrap();
    assert_eq!(v[0]["kind"], "shapes");
}
