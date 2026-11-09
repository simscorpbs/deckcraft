//! Version history: `file.versions` (the versions of a presentation, delete, open, restore) and
//! `file.options` (the folders the history is kept in). The history itself is
//! [`crate::versions`]; the pane that shows it is the front end's.
//!
//! - `file.versions` with no parameters lists the versions of the presentation, newest first.
//! - `{"delete": "document"}` deletes its history and `{"delete": "all"}` every presentation's,
//!   in every history folder.
//! - `{"open": "id"}` answers with a version's record and bytes. It changes nothing in the
//!   session; `file.openVersion` is what shows a version read only (`cmd::version_view`).
//! - While a previous version is in front, the commands act on the history of the presentation it
//!   came from. A restore first closes the version, then replaces that presentation.
//! - `{"restore": "id"}` replaces the presentation with the version in one Undo step. Changes
//!   not yet written to its file are first stored as an `autosave` version, and the restored bytes
//!   are recorded at once as a `restore` version.

use serde_json::{Value, json};

use super::{CommandSpec, always, bad, base64_encode, cmd, str_param};
use crate::versions::{DeleteScope, Relocation, VersionInfo, local_timestamp};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "file.versions",
            "Version History",
            ["File", "Info"],
            None,
            "{open?: version id | restore?: version id | delete?: document|all, document?: uid of an open presentation (default: the active one)} → with no parameters, the versions newest first: [{id, savedAt, author, size, stored, kind: save|autosave|restore|original|external|saveAs|conflictCopy, storage, documentId, supersedes, details}]; delete → {deleted}; open → {version, name, bytes, data: base64}; restore → {restored, savedAt}",
            always,
            versions
        ),
        cmd!(
            noundo "file.rename",
            "Rename",
            ["File"],
            None,
            "{name: the new file name (the extension is kept), document?: uid of an open presentation (default: the active one)} → {renamed, path}: renames the presentation's file in its folder; its version history follows",
            super::has_doc,
            rename
        ),
        cmd!(
            noundo "file.options",
            "Save Options",
            ["File", "Options"],
            None,
            "{versionsFolder?: folder|null (keep the version history there instead of the settings folder; null: the settings folder), versionsMirror?: bool (keep it in both), versionsExisting?: copy|move|leave (what happens to the history already kept; copy), deviceLabel?: text|null (what versions call this computer)} → {versionsFolder, versionsMirror, versionsFolders, versionsUnavailable, versionsRelocating, deviceLabel, deviceLabelDefault, enabled}",
            always,
            options
        ),
    ]
}

/// A version as the commands show it.
pub fn version_json(info: &VersionInfo) -> Value {
    json!({
        "id": info.id,
        "savedAt": info.saved_at,
        "author": info.author,
        "size": info.size,
        // The disk space the version added (shared parts count once); `size` for older entries.
        "stored": info.stored.unwrap_or(info.size),
        "kind": info.kind.id(),
        "storage": info.storage,
        "documentId": info.document_id,
        "supersedes": info.supersedes,
        // Where the version comes from and what it holds: counts and hashes, never text.
        "details": info.details,
    })
}

fn unavailable() -> EngineError {
    EngineError::Other("local version history is available on the desktop app only".into())
}

fn versions(s: &mut Session, p: &Value) -> Result<Value> {
    let Some(object) = p.as_object() else {
        if p.is_null() {
            return list(s, &Value::Null);
        }
        return Err(bad("file.versions", "parameters must be an object"));
    };
    for key in object.keys() {
        if !matches!(key.as_str(), "open" | "restore" | "delete" | "document") {
            return Err(bad("file.versions", format!("unknown parameter `{key}`")));
        }
    }
    let chosen = ["open", "restore", "delete"].iter().filter(|k| object.get(**k).is_some_and(|v| !v.is_null())).count();
    if chosen > 1 {
        return Err(bad("file.versions", "choose one of open, restore or delete"));
    }
    if let Some(scope) = object.get("delete").filter(|v| !v.is_null()) {
        let scope = match scope.as_str() {
            Some("document") => DeleteScope::Document,
            Some("all") => DeleteScope::All,
            _ => return Err(bad("file.versions", "`delete` is \"document\" (this presentation's history) or \"all\"")),
        };
        if !s.versions_enabled() {
            return Err(unavailable());
        }
        let index = if scope == DeleteScope::Document { Some(history_index(s, p)?) } else { None };
        let deleted = s.delete_version_history(index, scope).map_err(EngineError::Other)?;
        return Ok(json!({"deleted": deleted}));
    }
    if let Some(id) = object.get("open").filter(|v| !v.is_null()) {
        let id = id.as_str().ok_or_else(|| bad("file.versions", "`open` must be a version id"))?;
        return open(s, p, id);
    }
    if let Some(id) = object.get("restore").filter(|v| !v.is_null()) {
        let id = id.as_str().ok_or_else(|| bad("file.versions", "`restore` must be a version id"))?;
        return restore(s, p, id);
    }
    list(s, p)
}

/// The presentation whose history a command means: the one it names (default the active one), or
/// for a previous version the presentation it came from.
fn history_index(s: &Session, p: &Value) -> Result<usize> {
    let at = super::file::target_index(s, p, "file.versions")?;
    s.history_owner(at).ok_or_else(|| EngineError::Other("the presentation this version belongs to is closed".into()))
}

fn list(s: &mut Session, p: &Value) -> Result<Value> {
    let index = history_index(s, p)?;
    let list = s.version_infos(index);
    Ok(Value::Array(list.iter().map(version_json).collect()))
}

/// `{"open": id}`: the version's record and bytes. A front end opens them read only, as
/// `Name (version <local date time>)`; nothing changes in the session.
fn open(s: &mut Session, p: &Value, id: &str) -> Result<Value> {
    if !s.versions_enabled() {
        return Err(unavailable());
    }
    let index = history_index(s, p)?;
    let (info, bytes) = s.read_version(index, id).map_err(EngineError::Other)?;
    let stem = s
        .documents()
        .get(index)
        .and_then(|d| d.path.as_deref())
        .and_then(|path| std::path::Path::new(path).file_stem().map(|n| n.to_string_lossy().into_owned()));
    let stem = stem.filter(|n| !n.is_empty()).unwrap_or_else(|| "Presentation".into());
    Ok(json!({
        "version": version_json(&info),
        "name": format!("{stem} (version {})", local_timestamp(&info.saved_at)),
        "bytes": bytes.len(),
        "data": base64_encode(&bytes),
    }))
}

/// `{"restore": id}`: the version becomes the presentation, in one Undo step.
fn restore(s: &mut Session, p: &Value, id: &str) -> Result<Value> {
    if !s.versions_enabled() {
        return Err(unavailable());
    }
    // Restoring from a previous version first goes back to the presentation it came from.
    let at = super::file::target_index(s, p, "file.versions")?;
    let index = match s.history_owner(at) {
        Some(owner) if owner != at => {
            let uid = s.documents().get(owner).map_or(0, |d| d.uid);
            s.close_previous_version(at);
            s.index_of_uid(uid).ok_or(EngineError::NoDocument)?
        }
        Some(owner) => owner,
        None => return Err(EngineError::Other("the presentation this version belongs to is closed".into())),
    };
    if s.active_index() != Some(index) {
        return Err(bad("file.versions", "only the active presentation can be restored"));
    }
    let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
    let path = d.path.clone().ok_or_else(|| EngineError::Other("save the presentation to have a version history".into()))?;
    let (info, bytes) = s.read_version(index, id).map_err(EngineError::Other)?;
    // Read the version before touching anything: a damaged one fails here and changes nothing.
    let (restored, _) = super::file::open_checked(&path, &bytes)?;
    // The state being replaced stays in the history: unsaved changes are stored first.
    let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
    if d.is_dirty() {
        let format = super::file::format_for_path(&path);
        let current = d.doc.clone();
        if let Ok(kept) = super::file::save_bytes(&current, format) {
            s.keep_before_restore(index, kept, &current);
        }
    }
    crate::fonts::install_embedded_fonts(&restored);
    let record = restored.clone();
    s.edit(|doc, _| {
        *doc = restored;
        Ok(())
    })?;
    if let Some(st) = s.active_mut() {
        st.missing_fonts = crate::fonts::missing_fonts(&st.doc);
    }
    // The restore is a version at once, with the restored bytes.
    s.record_restore(index, bytes, &info.id, &record);
    Ok(json!({"restored": info.id, "savedAt": info.saved_at}))
}

fn options(s: &mut Session, p: &Value) -> Result<Value> {
    // Everything is checked before anything changes: a refused change leaves the settings alone.
    let folder = match p.get("versionsFolder") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) if !text.trim().is_empty() && !std::path::Path::new(text.trim()).is_absolute() => {
            return Err(bad("file.options", "`versionsFolder` is the full path of a folder"));
        }
        Some(Value::String(text)) => Some(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Some(_) => return Err(bad("file.options", "`versionsFolder` is a folder or null")),
    };
    let mirror = match p.get("versionsMirror") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| bad("file.options", "`versionsMirror` is true or false"))?),
    };
    let existing = match str_param(p, "versionsExisting") {
        None | Some("copy") => Relocation::Copy,
        Some("move") => Relocation::Move,
        Some("leave") => Relocation::Leave,
        Some(_) => return Err(bad("file.options", "`versionsExisting` is copy, move or leave")),
    };
    let label = match p.get("deviceLabel") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.trim().chars().take(64).collect::<String>()).filter(|t| !t.is_empty())),
        Some(_) => return Err(bad("file.options", "`deviceLabel` is text or null")),
    };
    let mut folders_changed = false;
    if let Some(folder) = folder {
        folders_changed |= folder != s.prefs.versions_folder;
        s.prefs.versions_folder = folder;
    }
    if let Some(on) = mirror {
        folders_changed |= on != s.prefs.versions_mirror;
        s.prefs.versions_mirror = on;
    }
    if let Some(label) = label {
        s.prefs.device_label = label;
    }
    if folders_changed {
        s.apply_version_folders(existing).map_err(EngineError::Other)?;
    }
    let (folders, unavailable, relocating) = s.version_folders();
    let text = |paths: Vec<std::path::PathBuf>| paths.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>();
    Ok(json!({
        "versionsFolder": s.prefs.versions_folder,
        "versionsMirror": s.prefs.versions_mirror,
        "versionsFolders": text(folders),
        "versionsUnavailable": text(unavailable),
        "versionsRelocating": relocating,
        "deviceLabel": s.prefs.device_label,
        "deviceLabelDefault": crate::versions::default_device_label(),
        "enabled": s.versions_enabled(),
    }))
}

/// Characters a file name cannot hold on Windows, which is the strictest of the systems DeckCraft
/// runs on: a name that works there works everywhere.
const BAD_NAME_CHARS: [char; 9] = ['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// The file name `name` asks for, next to a file whose extension is `ext` (with its dot, or
/// empty): the extension is kept, and a name that cannot be a file name is refused.
fn renamed_file_name(name: &str, ext: &str) -> std::result::Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the name is empty".into());
    }
    if name.chars().count() > 200 {
        return Err("the name is longer than 200 characters".into());
    }
    if name.chars().any(|c| c.is_control() || BAD_NAME_CHARS.contains(&c)) {
        return Err("a file name cannot hold any of / \\ : * ? \" < > |".into());
    }
    if name.ends_with('.') || name == "." || name == ".." {
        return Err("a file name cannot end with a dot".into());
    }
    let cut = name.len().saturating_sub(ext.len());
    let stem = match name.get(cut..) {
        Some(tail) if !ext.is_empty() && tail.eq_ignore_ascii_case(ext) => name.get(..cut).unwrap_or(name),
        _ => name,
    };
    let stem = stem.trim_end();
    if stem.is_empty() {
        return Err("the name is empty".into());
    }
    Ok(format!("{stem}{ext}"))
}

/// `file.rename`: the presentation's file gets a new name in the same folder, and its version
/// history follows (a file kept by path is moved with it; one kept by id needs nothing).
fn rename(s: &mut Session, p: &Value) -> Result<Value> {
    let index = super::file::target_index(s, p, "file.rename")?;
    let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
    if d.previous.is_some() {
        return Err(EngineError::Other("a previous version has no file to rename; save a copy first".into()));
    }
    let path = d.path.clone().ok_or_else(|| EngineError::Other("save the presentation before renaming its file".into()))?;
    let name = str_param(p, "name").ok_or_else(|| bad("file.rename", "missing `name`"))?;
    let from = std::path::PathBuf::from(&path);
    let ext = from.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let file_name = renamed_file_name(name, &ext).map_err(|m| bad("file.rename", m))?;
    let to = from.with_file_name(&file_name);
    if to == from {
        return Ok(json!({"renamed": false, "path": path}));
    }
    // The history is keyed by the canonical path, so take it while the file is still there.
    let before = crate::versions::canonical_path(&from);
    crate::io::rename_file(&path, &to.to_string_lossy()).map_err(EngineError::Other)?;
    let new_path = to.to_string_lossy().into_owned();
    let on = {
        let d = s.docs.get_mut(index).ok_or(EngineError::NoDocument)?;
        d.path = Some(new_path.clone());
        d.revision += 1;
        d.autosave
    };
    // The AutoSave choice follows the file: forget it under the old name, remember it under the new.
    let default = s.prefs.autosave_by_default;
    s.autosave_memory.remember(&from, default, default);
    s.set_autosave_of(index, on);
    s.versions_renamed(index, &before, &to);
    Ok(json!({"renamed": true, "path": new_path}))
}

#[cfg(test)]
mod name_tests {
    use super::renamed_file_name;

    #[test]
    fn a_new_name_keeps_the_extension_and_refuses_what_is_not_a_file_name() {
        assert_eq!(renamed_file_name("Budget", ".pptx").unwrap(), "Budget.pptx");
        assert_eq!(renamed_file_name("  Budget.PPTX ", ".pptx").unwrap(), "Budget.pptx");
        assert_eq!(renamed_file_name("v1.2", ".pptx").unwrap(), "v1.2.pptx");
        assert_eq!(renamed_file_name("Notes", "").unwrap(), "Notes");
        for bad in ["", "   ", ".pptx", "a/b", "a\\b", "a:b", "what?", "tab\tname", "dots.", "..", &"x".repeat(201)] {
            assert!(renamed_file_name(bad, ".pptx").is_err(), "{bad:?}");
        }
    }
}
