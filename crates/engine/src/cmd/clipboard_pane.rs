//! The Clipboard task pane (Home › Clipboard): the last 24 copies and cuts made in the app, kept
//! in memory for the session, newest first, each with the app's own rich payload so that pasting
//! an old item is as faithful as a normal paste.

use deckcraft_render::{Image, RenderOpts};
use serde_json::{Value, json};

use super::{CommandSpec, always, bad, cmd, has_doc};
use crate::{Clipboard, DocState, Result, Session};

/// Most items the pane keeps.
pub const CLIP_MAX_ITEMS: usize = 24;
/// A copy bigger than this (estimated) is not collected (it still reaches the clipboard).
const CLIP_ITEM_BYTES: usize = 32 << 20;
/// All collected items together stay under this; the oldest go first.
const CLIP_TOTAL_BYTES: usize = 128 << 20;
/// Characters of preview per item.
const CLIP_PREVIEW_CHARS: usize = 120;
/// Copied shapes are drawn into their thumbnail at this scale (pixels per point).
const THUMB_SCALE: f64 = 0.4;
/// A thumbnail of a copied slide is this wide, in pixels.
const THUMB_SLIDE_WIDTH: f64 = 192.0;
/// More shapes than this are not drawn into a thumbnail.
const THUMB_MAX_SHAPES: usize = 200;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "edit.clipboardItems",
            "Clipboard Items",
            ["Home", "Clipboard", "Clipboard Pane"],
            None,
            "→ [{index, kind, preview, chars, shapes, slides}] (0 = newest)",
            always,
            |s, _| Ok(s.clip_history.describe())
        ),
        cmd!(
            noundo "edit.deleteClipboardItem",
            "Delete Clipboard Item",
            ["Home", "Clipboard", "Clipboard Pane"],
            None,
            "{index: n (0 = newest)}",
            has_clips,
            |s, p| {
                let i = clip_index(s, p, "edit.deleteClipboardItem")?;
                s.clip_history.items.remove(i);
                Ok(s.clip_history.describe())
            }
        ),
        cmd!(
            noundo "edit.clearClipboard",
            "Clear All",
            ["Home", "Clipboard", "Clipboard Pane"],
            None,
            "{}",
            always,
            |s, _| {
                s.clip_history.items.clear();
                Ok(s.clip_history.describe())
            }
        ),
        cmd!(
            "edit.pasteClipboardItem",
            "Paste Clipboard Item",
            ["Home", "Clipboard", "Clipboard Pane"],
            None,
            "{index: n (0 = newest)} → {text, html}: the item becomes the clipboard",
            has_pasteable,
            paste_item
        ),
        cmd!("edit.pasteAllClipboard", "Paste All", ["Home", "Clipboard", "Clipboard Pane"], None, "{}", has_pasteable, paste_all),
    ]
}

fn has_clips(s: &Session) -> std::result::Result<(), String> {
    if s.clip_history.is_empty() { Err("the Clipboard pane is empty".into()) } else { Ok(()) }
}

fn has_pasteable(s: &Session) -> std::result::Result<(), String> {
    has_doc(s)?;
    has_clips(s)
}

fn clip_index(s: &Session, p: &Value, cmd: &str) -> Result<usize> {
    let i = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "`index` (number, 0 = newest) is required"))?;
    usize::try_from(i)
        .ok()
        .filter(|i| *i < s.clip_history.len())
        .ok_or_else(|| bad(cmd, format!("no clipboard item {i} (there are {})", s.clip_history.len())))
}

/// Paste one collected item through the normal paste path and make it the clipboard, so the
/// next Ctrl+V repeats it. Its paste counter (the offset of repeated pastes) is the item's own.
fn paste_item(s: &mut Session, p: &Value) -> Result<Value> {
    let i = clip_index(s, p, "edit.pasteClipboardItem")?;
    s.clipboard = s.clip_history.items[i].clip.clone();
    let r = s.execute("edit.paste", &json!({}));
    s.clip_history.items[i].clip.pastes = s.clipboard.pastes;
    r?;
    Ok(json!({"text": s.clipboard.plain, "html": super::edit::copied_html(s)}))
}

/// Every collected item, in the order they were copied. One undo step; the clipboard stays as it was.
fn paste_all(s: &mut Session, _: &Value) -> Result<Value> {
    let saved = s.clipboard.clone();
    let mut last = Ok(Value::Null);
    for i in (0..s.clip_history.len()).rev() {
        s.clipboard = s.clip_history.items[i].clip.clone();
        last = s.execute("edit.paste", &json!({}));
        s.clip_history.items[i].clip.pastes = s.clipboard.pastes;
        if last.is_err() {
            break;
        }
    }
    s.clipboard = saved;
    last
}

/// What one item holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipKind {
    Text,
    Shapes,
    Slides,
}

/// One item collected by Copy or Cut.
#[derive(Clone, Debug)]
pub struct ClipItem {
    /// Never reused in a session: a window can key a picture of the item by it.
    pub id: u64,
    pub clip: Clipboard,
    /// A small picture of the copied shapes or of the first copied slide.
    pub thumbnail: Option<Image>,
    /// Rough memory use.
    bytes: usize,
}

impl ClipItem {
    pub fn kind(&self) -> ClipKind {
        if !self.clip.slides.is_empty() {
            ClipKind::Slides
        } else if !self.clip.shapes.is_empty() {
            ClipKind::Shapes
        } else {
            ClipKind::Text
        }
    }

    /// One line of the item's text for a list: whitespace runs become one space.
    pub fn preview(&self) -> String {
        let mut out = String::new();
        let mut n = 0;
        for w in self.clip.plain.split_whitespace() {
            if !out.is_empty() {
                out.push(' ');
                n += 1;
            }
            for c in w.chars() {
                if n >= CLIP_PREVIEW_CHARS {
                    out.push('…');
                    return out;
                }
                out.push(c);
                n += 1;
            }
        }
        out
    }
}

/// Items collected by Copy and Cut during the session, newest first. At most [`CLIP_MAX_ITEMS`];
/// oversized copies are skipped. Not saved.
#[derive(Clone, Debug, Default)]
pub struct ClipHistory {
    items: Vec<ClipItem>,
    next_id: u64,
}

impl ClipHistory {
    pub fn items(&self) -> &[ClipItem] {
        &self.items
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add what was just copied (`clip`), unless it is empty, too big, or the item on top.
    pub(crate) fn push(&mut self, clip: &Clipboard, thumbnail: Option<Image>) {
        if clip.shapes.is_empty() && clip.slides.is_empty() && clip.text.is_none() && clip.plain.is_empty() {
            return;
        }
        let bytes = estimate(clip).saturating_add(thumbnail.as_ref().map_or(0, |t| t.pixels.len()));
        if bytes > CLIP_ITEM_BYTES {
            return;
        }
        // Copying the same thing again doesn't add a second item.
        if self.items.first().is_some_and(|i| same_content(&i.clip, clip)) {
            return;
        }
        let mut clip = clip.clone();
        clip.pastes = 0;
        self.next_id += 1;
        self.items.insert(0, ClipItem { id: self.next_id, clip, thumbnail, bytes });
        self.items.truncate(CLIP_MAX_ITEMS);
        while self.items.len() > 1 && self.items.iter().map(|i| i.bytes).sum::<usize>() > CLIP_TOTAL_BYTES {
            self.items.pop();
        }
    }

    /// The items for agents, newest first.
    fn describe(&self) -> Value {
        Value::Array(
            self.items
                .iter()
                .enumerate()
                .map(|(i, it)| {
                    let kind = match it.kind() {
                        ClipKind::Text => "text",
                        ClipKind::Shapes => "shapes",
                        ClipKind::Slides => "slides",
                    };
                    json!({
                        "index": i,
                        "kind": kind,
                        "preview": it.preview(),
                        "chars": it.clip.plain.chars().count(),
                        "shapes": it.clip.shapes.len(),
                        "slides": it.clip.slides.len(),
                    })
                })
                .collect(),
        )
    }
}

fn same_content(a: &Clipboard, b: &Clipboard) -> bool {
    a.plain == b.plain && a.text == b.text && a.shapes == b.shapes && a.slides == b.slides && a.media == b.media && a.placeholders == b.placeholders
}

/// Text plus per-object overhead and the media bytes (shared with the document, but kept alive).
fn estimate(c: &Clipboard) -> usize {
    let objects = c.shapes.len() + c.placeholders.len() + c.slides.iter().map(|s| s.shapes.len() + 1).sum::<usize>();
    let paras = c.text.as_ref().map_or(0, |t| t.paragraphs.len());
    let media: usize = c.media.iter().map(|m| m.data.len()).sum();
    c.plain.len().saturating_mul(2).saturating_add(objects.saturating_mul(2048)).saturating_add(paras.saturating_mul(256)).saturating_add(media)
}

/// A small picture of what `clip` holds, drawn from the document it was copied from.
fn thumbnail(st: &DocState, clip: &Clipboard) -> Option<Image> {
    if let Some(first) = clip.slides.first() {
        let index = st.doc.slides.iter().position(|s| s.id == first.id)?;
        let w = st.doc.slide_size.width.max(1.0);
        return Some(deckcraft_render::render_slide(&st.doc, index, &RenderOpts { scale: THUMB_SLIDE_WIDTH / w, ..Default::default() }));
    }
    if clip.shapes.is_empty() || clip.shapes.len() > THUMB_MAX_SHAPES || st.selection.target != crate::Target::Slides {
        return None;
    }
    let ids: Vec<_> = clip.shapes.iter().map(|x| x.id).collect();
    deckcraft_render::render_shapes(&st.doc, st.current_slide()?, &ids, THUMB_SCALE)
}

/// Collect the copy or cut that has just filled `s.clipboard` (called by Copy and Cut).
pub(crate) fn collect(s: &mut Session) {
    let thumb = s.doc().ok().and_then(|st| thumbnail(st, &s.clipboard));
    let clip = std::mem::take(&mut s.clipboard);
    s.clip_history.push(&clip, thumb);
    s.clipboard = clip;
}
