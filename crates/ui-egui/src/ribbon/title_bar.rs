use super::{Align2, Icon, Rect, Sense, SlideApp, Tokens, Ui, icon_toggle, json, pos2, theme, vec2};

pub const TITLE_H: f32 = 38.0;

/// Up to two initials of the author's name for the avatar; `?` when no name is set.
fn initials(author: &str) -> String {
    let letters: String = author.split_whitespace().filter_map(|w| w.chars().next()).take(2).flat_map(char::to_uppercase).collect();
    if letters.is_empty() { "?".into() } else { letters }
}

/// macOS draws the window into the title bar (`with_fullsize_content_view`) and AppKit paints the
/// traffic lights itself, 16 pt from the top of the window on macOS 27. Moving them would take
/// AppKit calls (`unsafe`, forbidden here), so on macOS the title bar is the band they are centred
/// in, with the controls beside them (#71). Up to macOS 15 the lights are smaller and centred 14 pt
/// from the top, about 2 pt above this bar's centre.
pub const MAC_TITLE_BAR: f32 = 32.0;

/// The traffic lights as AppKit lays them out, measured on macOS 27: button diameter,
/// centre-to-centre pitch and the first button's left edge.
pub(super) const MAC_LIGHT: f32 = 14.0;

pub(super) const MAC_LIGHT_PITCH: f32 = 23.0;

pub(super) const MAC_LIGHTS_LEFT: f32 = 9.0;

/// Where title-bar content starts on macOS: the end of the three lights plus the gap the lights
/// keep from the top of the window.
pub const MAC_CONTENT_LEFT: f32 = MAC_LIGHTS_LEFT + 2.0 * MAC_LIGHT_PITCH + MAC_LIGHT + (MAC_TITLE_BAR - MAC_LIGHT) / 2.0;

/// The title bar's height, the left edge of its content and the height of its tallest control.
/// With the integrated title bar (macOS) the bar is shorter, so the controls shrink to keep a
/// margin above and below them.
pub(super) fn title_geometry(integrated: bool) -> (f32, f32, f32) {
    if integrated { (MAC_TITLE_BAR, MAC_CONTENT_LEFT, 22.0) } else { (TITLE_H, 10.0, 26.0) }
}

/// The AutoSave switch, as PowerPoint's: on writes every change to the file. It is greyed out, with
/// the reason as its tooltip, when AutoSave can't cover the presentation (never saved, a format that
/// loses things, a conflict waiting, the browser); clicking it then does what helps. The text is part
/// of the switch.
fn autosave_switch(app: &mut SlideApp, ui: &mut Ui, t: &Tokens) {
    let Some(index) = app.session.active_index() else { return };
    let block = app.autosave_block(index);
    let on = app.autosave_covers(index);
    let color = if block.is_some() { t.text_dim.gamma_multiply(0.6) } else { t.text_dim };
    let label = crate::tl!("AutoSave");
    let text = ui.add(egui::Label::new(egui::RichText::new(label).font(theme::font(11.5)).color(color)).selectable(false).sense(Sense::click()));
    let (r, resp) = ui.allocate_exact_size(vec2(30.0, 16.0), Sense::click());
    let (fill, stroke, knob_fill) = match (&block, on) {
        (Some(_), _) => (t.control.gamma_multiply(0.5), t.border.gamma_multiply(0.7), t.text_dim.gamma_multiply(0.45)),
        (None, true) => (t.accent, t.accent, t.accent_text),
        (None, false) => (t.control, t.border, t.text_dim),
    };
    ui.painter().rect(r, 8.0, fill, egui::Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
    let knob_x = if on { r.max.x - 8.0 } else { r.min.x + 8.0 };
    ui.painter().circle_filled(pos2(knob_x, r.center().y), 5.0, knob_fill);
    let tip = match &block {
        Some(b) => crate::app::autosave::block_reason(b),
        None => crate::tl!("AutoSave saves every change to the file. DeckCraft remembers the switch for each file.").to_string(),
    };
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, on, label));
    let text = text.on_hover_text(tip.as_str()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = resp.on_hover_text(tip.as_str()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if text.clicked() || resp.clicked() {
        app.autosave_switch_clicked();
    }
}

/// The window's title row: traffic-light space, quick access, centred title, actions.
pub fn title_bar(app: &mut SlideApp, ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    let (height, left, control_h) = title_geometry(app.integrated_titlebar);
    egui::Panel::top("title_bar").exact_size(height).frame(egui::Frame::NONE.fill(t.chrome)).show(ui, |ui| {
        let rect = ui.max_rect();
        // Dragging the empty title bar moves the window.
        let drag = ui.interact(rect, ui.scope_id().with("titledrag"), Sense::click_and_drag());
        // The drag surface is not a button: a full-width Button would be announced as one.
        drag.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Window, true, crate::tl!("Window title bar")));
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(rect.min.x + left, rect.min.y), pos2(rect.min.x + 520.0, rect.max.y)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 2.0;
        let has_doc = app.session.active().is_some();
        let dirty = app.session.active().is_some_and(|d| d.is_dirty());
        if has_doc {
            autosave_switch(app, &mut child, &t);
            child.add_space(8.0);
        }
        crate::qat::show(app, &mut child);
        let title = app
            .session
            .active()
            .map(|d| format!("{}{}", d.title(), if dirty { format!(" — {}", crate::tl!("Edited")) } else { String::new() }))
            .unwrap_or_else(|| "DeckCraft".into());
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, title, theme::font(13.0), t.text);
        // Right side. The optional avatar and Discord button sit at its far end.
        let extras = if app.ui.show_account_picture { control_h + 6.0 } else { 0.0 } + if app.ui.show_discord { 90.0 } else { 0.0 };
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(Rect::from_min_max(pos2(rect.max.x - 320.0 - extras, rect.min.y), pos2(rect.max.x - 10.0, rect.max.y)))
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right.spacing_mut().item_spacing.x = 6.0;
        if app.ui.show_account_picture {
            let (r, resp) = right.allocate_exact_size(vec2(control_h, control_h), Sense::click());
            right.painter().circle_filled(r.center(), control_h / 2.0 - 1.0, t.accent);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, crate::tl!("Profile avatar")));
            right.painter().text(r.center(), Align2::CENTER_CENTER, initials(&app.session.prefs.author), theme::bold(10.5), t.accent_text);
            if resp.on_hover_text(crate::tl!("Account: set your name in Preferences")).clicked() {
                let _ = app.run("app.preferences", json!({}));
            }
        }
        if app.ui.show_discord {
            let (r, resp) = right.allocate_exact_size(vec2(84.0, control_h - 4.0), Sense::click());
            let fill = if resp.hovered() { t.accent_text } else { t.accent };
            let ink = if resp.hovered() { t.accent } else { t.accent_text };
            right.painter().rect_filled(r, r.height() / 2.0, fill);
            right.painter().rect_stroke(r, r.height() / 2.0, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
            right.painter().text(r.center(), Align2::CENTER_CENTER, "Discord", theme::bold(12.0), ink);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Discord"));
            if resp.on_hover_text(crate::tl!("Join the ArtCraft community on Discord")).clicked() {
                right.ctx().open_url(egui::OpenUrl::new_tab(deckcraft_engine::links::DISCORD));
            }
        }
        if icon_toggle(&mut right, Icon::Search, "Search commands (⇧⌘P)", true, false).clicked() {
            app.palette = Some((String::new(), 0));
        }
        let share = right.add(
            egui::Button::new(egui::RichText::new(format!("  {}  ", crate::tl!("Present"))).color(t.accent_text).font(theme::bold(13.0)))
                .fill(t.accent)
                .corner_radius(control_h / 2.0)
                .min_size(vec2(84.0, control_h)),
        );
        if share.clicked() && has_doc {
            let from = app.session.active().map(|d| d.selection.slide).unwrap_or(0);
            app.start_show(from, false);
        }
        if icon_toggle(&mut right, Icon::CommentsBubble, "Comments", has_doc, app.ui.pane.as_deref() == Some("comments")).clicked() {
            let _ = app.run("view.pane", json!({"pane": "comments", "toggle": true}));
        }
        if icon_toggle(
            &mut right,
            if app.ui.brightness == theme::Brightness::Dark { Icon::Sparkle } else { Icon::Palette },
            "Light/Dark appearance",
            true,
            false,
        )
        .clicked()
        {
            let _ = app.run("view.dark", json!({}));
        }
    });
}
