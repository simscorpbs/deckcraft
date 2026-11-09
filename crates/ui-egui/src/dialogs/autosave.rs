//! Save Conflict (AutoSave found the file changed, moved or deleted elsewhere) and Save to Cloud
//! (turning AutoSave on for a presentation that was never saved).

use egui::{Ui, vec2};
use serde_json::json;

use super::{Dialog, SlideApp, field};
use crate::theme::Tokens;

fn primary(ui: &mut Ui, label: &str) -> egui::Response {
    let t = Tokens::get(ui.ctx());
    ui.add(egui::Button::new(egui::RichText::new(label).color(t.accent_text)).fill(t.accent).min_size(vec2(72.0, 24.0)))
}

/// The dialog is put back for the command, which closes it when it succeeds; a failed answer
/// leaves it open.
fn answer(app: &mut SlideApp, d: &Dialog, command: &str, params: serde_json::Value) -> bool {
    app.dialog = Some(d.clone());
    let result = app.run(command, params);
    result.is_ok()
}

/// AutoSave found the file changed on another device (or moved or deleted, `missing`): keep both
/// versions, keep this one, open the other, or decide later (Esc or the close button).
pub(super) fn save_conflict_dialog(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    let missing = d.params.get("missing").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let name = format!("“{}”", d.params.get("name").and_then(serde_json::Value::as_str).unwrap_or_default());
    let heading = if missing { crate::tl!("{name} was moved or deleted elsewhere") } else { crate::tl!("{name} was changed on another device") };
    ui.label(egui::RichText::new(crate::i18n::fmt(heading, &[("name", &name)])).strong());
    ui.label(crate::tl!("AutoSave stopped so that neither version is lost."));
    if !missing {
        ui.label(crate::tl!("Keep both saves this version as a copy beside the other."));
    }
    ui.add_space(8.0);
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        if missing {
            if primary(ui, crate::tl!("Save it here again")).clicked() {
                chosen = Some("mine");
            }
        } else {
            if primary(ui, crate::tl!("Keep both")).clicked() {
                chosen = Some("both");
            }
            if ui.button(crate::tl!("Keep this version")).clicked() {
                chosen = Some("mine");
            }
            if ui.button(crate::tl!("Open the other version")).clicked() {
                chosen = Some("theirs");
            }
        }
        if ui.button(crate::tl!("Decide later")).clicked() {
            chosen = Some("later");
        }
    });
    match chosen {
        Some(choice) => answer(app, d, "ui.saveConflict", json!({"answer": choice})),
        None => false,
    }
}

/// Which cloud folder keeps a presentation that AutoSave is being turned on for.
pub(super) fn save_to_cloud_dialog(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    ui.label(egui::RichText::new(crate::tl!("Where should AutoSave keep this presentation?")).strong());
    ui.add_space(4.0);
    let default = d.params.get("name").and_then(serde_json::Value::as_str).unwrap_or_default().to_string();
    field(ui, d, "File name:", "name", &default);
    ui.add_space(8.0);
    let roots = app.session.cloud_roots.clone();
    let mut picked = None;
    for root in &roots {
        let label = match &root.account {
            Some(account) => format!("{} ({account})", root.provider.name()),
            None => root.provider.name().to_string(),
        };
        let button = ui.add(egui::Button::new(label).min_size(vec2(200.0, 24.0)));
        if button.on_hover_text(root.path.to_string_lossy()).clicked() {
            picked = Some(root.clone());
        }
    }
    ui.add_space(8.0);
    let (mut other, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        other = ui.button(crate::tl!("Other location…")).clicked();
        cancel = ui.button(crate::tl!("Cancel")).clicked();
    });
    if let Some(root) = picked {
        let name = super::field_of(d, "name");
        // The folder this button names, even when a provider has several accounts.
        return answer(app, d, "ui.saveToCloud", json!({"provider": root.provider.id(), "root": root.path, "name": name}));
    }
    if other {
        app.save_as_for_autosave();
        return true;
    }
    cancel
}
