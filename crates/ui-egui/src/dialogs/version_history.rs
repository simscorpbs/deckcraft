//! Preferences › Version history: the folder the history is kept in, the mirror folder, and the
//! two Delete buttons (each asks first). The history itself is the engine's
//! (`deckcraft_engine::versions`, commands `file.versions` and `file.options`).
//!
//! Folder choices wait in the dialog and apply when Done is pressed (`file.options`), because
//! applying one moves or copies the history already kept; Cancel, Esc and the close button leave
//! everything as it was. The Delete buttons act at once, after their confirmation, as they say.

use egui::{RichText, Ui};
use serde_json::json;

use super::{Dialog, SlideApp, check, field, field_of, theme};

/// What the dialog does with the history already kept when the folder changes.
const EXISTING: [(&str, &str); 3] =
    [("copy", "Copy the history already kept"), ("move", "Move the history already kept"), ("leave", "Leave it where it is")];

/// The section, drawn between Saving and Editing. Only the desktop app has a history to set up.
pub(super) fn version_history_section(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) {
    if !app.session.versions_enabled() {
        return;
    }
    ui.label(RichText::new(crate::tl!("Version History")).font(theme::bold(13.0)));
    let folder = app.session.prefs.versions_folder.clone().unwrap_or_default();
    field(ui, d, "Version history folder:", "versionsFolder", &folder);
    check(ui, d, "Mirror folder", "versionsMirror", app.session.prefs.versions_mirror);
    ui.label(
        RichText::new(crate::tl!(
            "Leave the folder empty to keep the history in DeckCraft's settings folder. With a mirror folder, every version is written to both."
        ))
        .color(ui.visuals().weak_text_color()),
    );
    if changes_folders(app, d) {
        let chosen = d.fields.entry("versionsExisting".into()).or_insert_with(|| "copy".into());
        for (value, label) in EXISTING {
            ui.radio_value(chosen, value.to_string(), crate::tl!(label));
        }
    }
    unavailable_note(app, ui);
    delete_buttons(app, ui, d);
}

/// Whether Done would change the folders the history is kept in.
fn changes_folders(app: &SlideApp, d: &Dialog) -> bool {
    let (folder, mirror) = chosen(d, app);
    folder != app.session.prefs.versions_folder || mirror != app.session.prefs.versions_mirror
}

/// The folder and mirror setting the dialog shows now.
fn chosen(d: &Dialog, app: &SlideApp) -> (Option<String>, bool) {
    let folder = d.fields.get("versionsFolder").map(|f| f.trim().to_owned()).filter(|f| !f.is_empty());
    let folder = if d.fields.contains_key("versionsFolder") { folder } else { app.session.prefs.versions_folder.clone() };
    let mirror = d.fields.get("versionsMirror").map_or(app.session.prefs.versions_mirror, |m| m == "1");
    (folder, mirror)
}

/// A folder that could not be reached at the last write is said, so a person knows why a copy is
/// missing there.
fn unavailable_note(app: &SlideApp, ui: &mut Ui) {
    let (_, away, _) = app.session.version_folders();
    if away.is_empty() {
        return;
    }
    let names = away.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join(", ");
    ui.label(
        RichText::new(crate::i18n::fmt(
            crate::tl!("Not reachable now: {folders}. It is brought up to date when it is back."),
            &[("folders", &names)],
        ))
        .color(ui.visuals().warn_fg_color),
    );
}

/// Delete this document's history, or all of it, after asking.
fn delete_buttons(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) {
    let saved = app.session.active().is_some_and(|doc| doc.path.is_some());
    if let Some(scope) = d.fields.get("confirmDelete").cloned() {
        let all = scope == "all";
        ui.label(
            RichText::new(if all {
                crate::tl!("Delete the version history of every document? This can't be undone.")
            } else {
                crate::tl!("Delete the version history of this document? This can't be undone.")
            })
            .strong(),
        );
        ui.horizontal(|ui| {
            if ui.button(crate::tl!("Delete")).clicked() {
                let result = app.run("file.versions", json!({"delete": scope}));
                d.fields.remove("confirmDelete");
                d.fields.insert("deleted".into(), if result.is_ok() { scope } else { String::new() });
            }
            if ui.button(crate::tl!("Don't delete")).clicked() {
                d.fields.remove("confirmDelete");
            }
        });
        return;
    }
    ui.horizontal_wrapped(|ui| {
        if ui.add_enabled(saved, egui::Button::new(crate::tl!("Delete this document's history"))).clicked() {
            d.fields.insert("confirmDelete".into(), "document".into());
            d.fields.remove("deleted");
        }
        if ui.button(crate::tl!("Delete all history")).clicked() {
            d.fields.insert("confirmDelete".into(), "all".into());
            d.fields.remove("deleted");
        }
    });
    match d.fields.get("deleted").map(String::as_str) {
        Some("all") => {
            ui.label(RichText::new(crate::tl!("The version history was deleted.")).color(ui.visuals().weak_text_color()));
        }
        Some("document") => {
            ui.label(RichText::new(crate::tl!("This document's version history was deleted.")).color(ui.visuals().weak_text_color()));
        }
        _ => {}
    }
}

/// Done: apply the folders the dialog names. `false` keeps the dialog open (the command refused,
/// and has said why in the status line).
pub(super) fn apply_version_options(app: &mut SlideApp, d: &Dialog) -> bool {
    if !app.session.versions_enabled() || !changes_folders(app, d) {
        return true;
    }
    let (folder, mirror) = chosen(d, app);
    let existing = match field_of(d, "versionsExisting").as_str() {
        "move" => "move",
        "leave" => "leave",
        _ => "copy",
    };
    app.run("file.options", json!({"versionsFolder": folder, "versionsMirror": mirror, "versionsExisting": existing})).is_ok()
}
