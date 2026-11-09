//! The Clipboard task pane (Home › Clipboard): the last 24 copies and cuts made in the app, newest
//! first, as thumbnails or text previews. Clicking (or Enter on) an item pastes it at the caret or
//! on the slide; Paste All pastes every item in the order they were copied; Clear All and each
//! item's Delete empty the list. The engine keeps the history (`edit.clipboardItems` and friends).

use std::collections::HashMap;

use deckcraft_engine::cmd::clipboard_pane::{ClipItem, ClipKind};
use egui::{CornerRadius, Sense, Stroke, TextureHandle, Ui, pos2, vec2};
use serde_json::json;

use super::{SlideApp, Tokens, run, theme};

/// Pictures of the collected items, by item id, kept between frames.
#[derive(Default)]
pub struct Thumbs {
    textures: HashMap<u64, TextureHandle>,
}

/// Tallest a thumbnail is drawn.
const THUMB_H: f32 = 64.0;

/// What an item without text says it is.
fn kind_label(item: &ClipItem) -> String {
    let (n, one, many) = match item.kind() {
        ClipKind::Slides => (item.clip.slides.len(), "Slide", "Slides"),
        ClipKind::Shapes => (item.clip.shapes.len(), "Object", "Objects"),
        ClipKind::Text => (0, "Text", "Text"),
    };
    if n > 1 { format!("{n} {}", crate::tl!(many)) } else { crate::tl!(one).to_string() }
}

pub(super) fn clipboard(app: &mut SlideApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let count = app.session.clip_history.len();
    ui.horizontal(|ui| {
        if ui.add_enabled(count > 0, egui::Button::new(crate::tl!("Paste All"))).clicked() {
            run(app, "edit.pasteAllClipboard", json!({}));
        }
        if ui.add_enabled(count > 0, egui::Button::new(crate::i18n::tc("clipboard", "Clear All"))).clicked() {
            run(app, "edit.clearClipboard", json!({}));
        }
    });
    ui.add_space(4.0);
    let hint = if count == 0 { "Nothing collected yet. Items you copy or cut appear here." } else { "Click an item to paste it." };
    ui.label(egui::RichText::new(crate::tl!(hint)).size(11.5).color(t.text_dim));
    ui.separator();

    // Drop pictures of items that are gone; build the ones that are new.
    let live: Vec<u64> = app.session.clip_history.items().iter().map(|i| i.id).collect();
    app.clip_thumbs.textures.retain(|id, _| live.contains(id));
    for item in app.session.clip_history.items() {
        if let (Some(img), false) = (&item.thumbnail, app.clip_thumbs.textures.contains_key(&item.id)) {
            let tex = crate::textures::upload(ui.ctx(), format!("clip-{}", item.id), crate::textures::to_color_image(img, false));
            app.clip_thumbs.textures.insert(item.id, tex);
        }
    }

    // What to do is decided after the loop: the loop borrows the history.
    let mut paste: Option<usize> = None;
    let mut delete: Option<usize> = None;
    for (i, item) in app.session.clip_history.items().iter().enumerate() {
        let preview = item.preview();
        let label = if preview.is_empty() { kind_label(item) } else { preview.clone() };
        egui::Frame::NONE.fill(t.chrome).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(
            ui,
            |ui| {
                ui.set_width(ui.available_width());
                let tex = app.clip_thumbs.textures.get(&item.id);
                let height = if tex.is_some() { THUMB_H } else { 36.0 };
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::click());
                let name = crate::i18n::fmt(crate::tl!("Paste {item}"), &[("item", label.as_str())]);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, name.clone()));
                let resp = resp.on_hover_text(crate::tl!("Paste"));
                if resp.hovered() || resp.has_focus() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), t.hover);
                }
                if resp.has_focus() {
                    ui.painter().rect_stroke(rect, CornerRadius::same(4), Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
                }
                let painter = ui.painter_at(rect);
                match tex {
                    Some(tex) => {
                        let [w, h] = tex.size();
                        let scale = (rect.width() / w.max(1) as f32).min(rect.height() / h.max(1) as f32);
                        let size = vec2(w as f32 * scale, h as f32 * scale);
                        let at = egui::Rect::from_min_size(pos2(rect.min.x, rect.center().y - size.y / 2.0), size);
                        painter.rect_filled(at, CornerRadius::same(2), egui::Color32::WHITE);
                        painter.image(tex.id(), at, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), egui::Color32::WHITE);
                        painter.rect_stroke(at, CornerRadius::same(2), Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                    }
                    None => {
                        let mut job = egui::text::LayoutJob::single_section(
                            label.clone(),
                            egui::TextFormat { font_id: theme::font(12.5), color: t.text, ..Default::default() },
                        );
                        job.wrap.max_width = rect.width();
                        job.wrap.max_rows = 2;
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        painter.galley(rect.left_top(), galley, t.text);
                    }
                }
                if resp.clicked() {
                    paste = Some(i);
                }
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(kind_label(item)).size(11.0).color(t.text_faint));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let del = ui.small_button(crate::tl!("Delete"));
                        let del = del.on_hover_text(crate::i18n::fmt(crate::tl!("Delete {item}"), &[("item", label.as_str())]));
                        if del.clicked() {
                            delete = Some(i);
                        }
                    });
                });
            },
        );
        ui.add_space(6.0);
    }
    if let Some(i) = paste {
        run(app, "edit.pasteClipboardItem", json!({"index": i}));
    } else if let Some(i) = delete {
        run(app, "edit.deleteClipboardItem", json!({"index": i}));
    }
}
