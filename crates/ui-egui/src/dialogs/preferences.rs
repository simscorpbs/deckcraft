use super::{Dialog, SlideApp, Value, json, theme};

/// What the Preferences dialog can change, as it was when the dialog opened.
pub(super) fn preferences_snapshot(app: &SlideApp) -> Value {
    json!({
        "dark": app.ui.brightness == theme::Brightness::Dark,
        "uiScale": app.ui.ui_scale,
        "accent": app.ui.accent_color.hex(),
        "showDiscord": app.ui.show_discord,
        "showAccountPicture": app.ui.show_account_picture,
        "language": app.ui.language,
        "prefs": serde_json::to_value(&app.session.prefs).unwrap_or(Value::Null),
    })
}

/// Puts back the settings the Preferences dialog opened with (Cancel, Esc, the close button).
pub(super) fn restore_preferences(app: &mut SlideApp, d: &Dialog) {
    let Some(before) = d.params.get("before") else { return };
    if let Some(dark) = before.get("dark").and_then(Value::as_bool)
        && dark != (app.ui.brightness == theme::Brightness::Dark)
    {
        let _ = app.run("view.dark", json!({ "on": dark }));
    }
    if let Some(scale) = before.get("uiScale").and_then(Value::as_f64) {
        app.ui.ui_scale = scale as f32;
    }
    if let Some(accent) = before.get("accent").and_then(Value::as_str)
        && accent != app.ui.accent_color.hex()
    {
        let _ = app.run("ui.accentColor", json!({ "value": accent }));
    }
    if let Some(on) = before.get("showDiscord").and_then(Value::as_bool) {
        app.ui.show_discord = on;
    }
    if let Some(on) = before.get("showAccountPicture").and_then(Value::as_bool) {
        app.ui.show_account_picture = on;
    }
    if let Some(language) = before.get("language").and_then(Value::as_str) {
        app.ui.language = language.to_string();
    }
    if let Some(prefs) = before.get("prefs").and_then(|p| serde_json::from_value(p.clone()).ok()) {
        app.session.prefs = prefs;
        app.session.refresh_autosave();
    }
}
