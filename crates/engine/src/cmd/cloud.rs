//! AutoSave and cloud folders (iCloud Drive, Google Drive, OneDrive) through the folders their
//! sync apps keep (see `deckcraft-cloud`).
//!
//! - `file.save` with `ifUnchanged` (what AutoSave sends) first checks that the presentation's
//!   file still holds what DeckCraft last read or wrote. When another device or program changed
//!   it, nothing is written: the save reports the conflict and AutoSave waits for
//!   `file.resolveConflict` (keep both, keep mine, or take theirs).
//! - `file.cloud` says where the presentation is kept, whether a conflict is waiting and what
//!   AutoSave does for it.
//! - `file.autosave` is the AutoSave switch, remembered for the presentation's file.
//! - `file.saveToCloud` saves a presentation straight into a provider's folder, as PowerPoint
//!   offers to save to OneDrive when AutoSave is turned on for a new presentation.
//!
//! Cloud folders are known only once the host looks for them ([`Session::discover_cloud`]); the
//! desktop app does, headless sessions and tests do not, so a script never writes into someone's
//! cloud by accident.

use std::path::Path;

use deckcraft_cloud::{CloudRoot, DiskState, Provider};
use serde_json::{Value, json};

use super::{CommandSpec, bad, bool_param, cmd, has_doc, str_param};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "file.cloud",
            "Cloud Status",
            ["File"],
            None,
            "{document?: uid of an open presentation (default: the active one)} → {provider, name, root, path, conflict, notDownloaded, roots, autosave: {on, default, remembered, covers, blocked}}",
            has_doc,
            status
        ),
        cmd!(
            noundo "file.saveToCloud",
            "Save to Cloud",
            ["File", "Save As"],
            None,
            "{provider?: icloud|gdrive|onedrive (default: the first found, in that order), name?: file name without extension (default: the presentation's name), root?: one of the folders file.cloud lists (for an account other than the first), document?: uid} → {path, bytes, saved, provider}",
            has_doc,
            save_to_cloud
        ),
        cmd!(
            noundo "file.autosave",
            "AutoSave",
            ["File"],
            None,
            "{value?: bool (default: toggle), document?: uid} → {value}: the AutoSave switch, remembered for the presentation's file; turning it on fails, with the reason, when AutoSave cannot cover the presentation",
            has_doc,
            autosave
        ),
        cmd!(
            noundo "file.resolveConflict",
            "Resolve Save Conflict",
            ["File"],
            None,
            "{keep: both|mine|theirs (both: save this version beside the other as \"Name (conflict copy)\" and keep editing that; mine: overwrite the other version; theirs: reopen the file as it is now, discarding unsaved changes here), document?: uid}",
            has_doc,
            resolve_conflict
        ),
    ]
}

/// The JSON for a provider's folder.
fn root_json(root: &CloudRoot) -> Value {
    json!({"provider": root.provider.id(), "name": root.provider.name(), "path": root.path, "account": root.account})
}

/// `file.cloud`: where the presentation is kept, the conflict waiting (if any), what AutoSave
/// does for it and the cloud folders found on this computer.
fn status(s: &mut Session, p: &Value) -> Result<Value> {
    let index = super::file::target_index(s, p, "file.cloud")?;
    let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
    let path = d.path.as_deref().map(Path::new);
    let location = path.and_then(|path| deckcraft_cloud::locate(path, &s.cloud_roots));
    let not_downloaded = path.is_some_and(|path| deckcraft_cloud::icloud_placeholder(path).is_some());
    Ok(json!({
        "provider": location.as_ref().map(|l| l.provider.id()),
        "name": location.as_ref().map(|l| l.provider.name()),
        "root": location.as_ref().map(|l| &l.root),
        "path": d.path,
        "conflict": d.conflict.map(DiskState::id),
        "notDownloaded": not_downloaded,
        "roots": s.cloud_roots.iter().map(root_json).collect::<Vec<_>>(),
        // The AutoSave switch for this presentation, the default for files without a choice, and
        // the choice remembered for this file (null: it follows the default); whether AutoSave
        // writes the file and, when not, why.
        "autosave": {
            "on": d.autosave,
            "default": s.prefs.autosave_by_default,
            "remembered": s.autosave_remembered(index),
            "covers": s.autosave_covers(index),
            "blocked": s.autosave_block(index).map(|b| b.id()),
        },
    }))
}

/// `file.autosave`.
fn autosave(s: &mut Session, p: &Value) -> Result<Value> {
    let index = super::file::target_index(s, p, "file.autosave")?;
    let on = bool_param(p, "value").unwrap_or_else(|| !s.autosave_covers(index));
    if on && let Some(block) = s.autosave_block(index) {
        return Err(EngineError::Other(block.reason()));
    }
    s.set_autosave_of(index, on);
    Ok(json!({"value": on}))
}

/// AutoSave's check before it writes the presentation's own file (`file.save` with
/// `ifUnchanged`): `Some(state)` when the file no longer holds what DeckCraft last read or wrote,
/// and the conflict is recorded. A conflict already waiting stays until it is resolved.
pub(crate) fn conflict_before_save(s: &mut Session, index: usize, path: &str) -> Result<Option<DiskState>> {
    let d = s.docs.get_mut(index).ok_or(EngineError::NoDocument)?;
    if let Some(conflict) = d.conflict {
        return Ok(Some(conflict));
    }
    if d.path.as_deref() != Some(path) {
        return Ok(None);
    }
    let Some(stamp) = d.stamp.as_mut() else { return Ok(None) };
    let state = stamp.check(Path::new(path)).map_err(|e| EngineError::Other(format!("{path}: {e}")))?;
    if state == DiskState::Unchanged {
        return Ok(None);
    }
    d.conflict = Some(state);
    Ok(Some(state))
}

/// `file.saveToCloud`: save the presentation into a provider's documents folder under a name not
/// taken there, and keep it there (AutoSave can then cover it).
fn save_to_cloud(s: &mut Session, p: &Value) -> Result<Value> {
    let index = super::file::target_index(s, p, "file.saveToCloud")?;
    let wanted = match str_param(p, "provider") {
        Some(id) => Some(Provider::from_id(id).ok_or_else(|| bad("file.saveToCloud", format!("unknown provider `{id}`; use icloud, gdrive or onedrive")))?),
        None => None,
    };
    // `root` names one of the folders found, for a provider with several accounts.
    let named_root = str_param(p, "root");
    let root = s.cloud_roots.iter().find(|r| named_root.is_none_or(|n| r.path == Path::new(n)) && wanted.is_none_or(|p| r.provider == p)).cloned().ok_or_else(|| {
        EngineError::Other(match wanted {
            Some(p) => format!("No {} folder was found on this computer. Install and sign in to its app, then try again.", p.name()),
            None => "No iCloud Drive, Google Drive or OneDrive folder was found on this computer.".to_string(),
        })
    })?;
    let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
    let uid = d.uid;
    let stem = match str_param(p, "name") {
        Some(name) => name.to_string(),
        None => d.title(),
    };
    let path = deckcraft_cloud::unique_path(&root.save_dir(), &stem, "pptx")
        .ok_or_else(|| EngineError::Other(format!("Too many presentations named {stem} in {}", root.provider.name())))?;
    let mut r = super::file::save(s, &json!({"path": path.to_string_lossy(), "document": uid, "format": "pptx"}))?;
    if let Some(o) = r.as_object_mut() {
        o.insert("provider".into(), json!(root.provider.id()));
    }
    Ok(r)
}

/// `file.resolveConflict`: what to do once AutoSave found the file changed elsewhere.
fn resolve_conflict(s: &mut Session, p: &Value) -> Result<Value> {
    let index = super::file::target_index(s, p, "file.resolveConflict")?;
    let none = || EngineError::Disabled("file.resolveConflict".into(), "no save conflict is waiting".into());
    let d = s.docs.get_mut(index).ok_or(EngineError::NoDocument)?;
    let Some(conflict) = d.conflict else { return Err(none()) };
    let uid = d.uid;
    let Some(path) = d.path.clone() else {
        d.conflict = None;
        return Err(none());
    };
    let save = |s: &mut Session, path: &str| super::file::save(s, &json!({"path": path, "document": uid}));
    match str_param(p, "keep") {
        Some("mine") => {
            s.docs[index].conflict = None;
            let r = save(s, &path);
            if r.is_err()
                && let Some(d) = s.docs.get_mut(index)
            {
                d.conflict = Some(conflict);
            }
            r
        }
        Some("both") => {
            let copy = deckcraft_cloud::conflict_copy_path(Path::new(&path), "conflict copy")
                .ok_or_else(|| EngineError::Other(format!("Too many conflict copies of {path}")))?;
            // The session now edits the copy; the other version is left as it is. Its first
            // version in the history says it is a conflict copy.
            s.set_next_version_kind(index, crate::versions::VersionKind::ConflictCopy);
            let r = save(s, &copy.to_string_lossy());
            if r.is_err() {
                s.clear_next_version_kind(index);
            }
            let r = r?;
            if let Some(d) = s.docs.get_mut(index) {
                d.conflict = None;
            }
            Ok(r)
        }
        Some("theirs") => {
            if conflict == DiskState::Missing {
                return Err(EngineError::Other(format!("{path} is no longer there to reopen")));
            }
            reopen(s, index, &path)
        }
        other => Err(bad("file.resolveConflict", format!("`keep` must be both, mine or theirs, not `{}`", other.unwrap_or("missing")))),
    }
}

/// Read the file at `path` again into presentation `index`, in its place (take theirs: the version
/// another device saved replaces this one, unsaved changes and all).
#[cfg(target_arch = "wasm32")]
fn reopen(_s: &mut Session, _index: usize, path: &str) -> Result<Value> {
    Err(EngineError::Other(format!("{path}: files are opened through the browser on the web")))
}

#[cfg(not(target_arch = "wasm32"))]
fn reopen(s: &mut Session, index: usize, path: &str) -> Result<Value> {
    let f = super::file::read_presentation_file(path)?;
    if !f.native || !f.damaged.is_empty() {
        return Err(EngineError::Other(format!("{path} can't be reopened as it is now; use Keep both instead")));
    }
    crate::fonts::install_embedded_fonts(&f.doc);
    let missing = crate::fonts::missing_fonts(&f.doc);
    let slides = f.doc.slides.len();
    let d = s.docs.get_mut(index).ok_or(EngineError::NoDocument)?;
    d.doc = std::sync::Arc::new(f.doc);
    d.saved_doc = d.doc.clone();
    d.selection = crate::Selection::default();
    d.history = crate::History { limit: d.history.limit, ..Default::default() };
    d.interaction = None;
    d.composition = None;
    d.compare = None;
    d.missing_fonts = missing;
    d.stamp = f.stamp;
    d.conflict = None;
    // The file is read afresh: a new editing session, and what is on disk is not yet known to be
    // in the history (it holds the other device's changes).
    let epoch = d.versions.epoch.saturating_add(1);
    d.versions = crate::versions::DocVersions::started();
    d.versions.epoch = epoch;
    d.revision += 1;
    let uid = d.uid;
    if let Some(dir) = &s.recovery_dir {
        crate::recovery::discard(dir, uid);
    }
    Ok(json!({"path": path, "slides": slides}))
}
