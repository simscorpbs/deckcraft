//! A previous version opened from Version History, shown the way PowerPoint shows one: read only,
//! comparable with the current presentation, and closed to get the current presentation back.
//! Restoring it is `file.versions={"restore":id}`; saving a copy is `file.saveAs`.
//!
//! DeckCraft keeps several presentations open, so the version is a presentation of its own,
//! opened beside the one it came from (`DocState::previous`); closing it only removes it. The
//! presentation it came from is never touched while the version is shown.

use serde_json::{Value, json};

use super::{CommandSpec, bad, cmd, str_param};
use crate::versions::{PreviousVersion, VersionOrigin, local_timestamp};
use crate::{DocState, EngineError, Result, Session, UiRequest};

pub use crate::versions::READ_ONLY;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "file.openVersion",
            "Open Version",
            ["File", "Info", "Version History"],
            None,
            "{id: version id, document?: uid of an open presentation (default: the active one)} → {opened, document, uid, name, savedAt, author}: shows the version read only beside the presentation, as a presentation of its own (a version already shown for it is replaced)",
            has_history,
            open_version
        ),
        cmd!(
            noundo "file.closeVersion",
            "Close Version",
            ["File", "Info", "Version History"],
            None,
            "{} → {closed, document}: closes the previous version in front and puts the presentation it came from in front, exactly as it was",
            shown,
            close_version
        ),
        cmd!(
            noundo "file.compareVersion",
            "Compare Version",
            ["File", "Info", "Version History"],
            None,
            "{} → {comparing, name, changes}: shows the previous version as the original and the presentation it came from as the revised one, with the differences as review.revisions lists them. The comparison stays read only; running it again shows the version",
            shown,
            compare_version
        ),
    ]
}

fn has_history(s: &Session) -> std::result::Result<(), String> {
    super::has_doc(s)?;
    if s.versions_enabled() { Ok(()) } else { Err("local version history is available on the desktop app only".into()) }
}

fn shown(s: &Session) -> std::result::Result<(), String> {
    if s.active_previous_version().is_some() { Ok(()) } else { Err("no previous version is open".into()) }
}

/// What still works while a previous version is shown: the file commands (Save a copy, Close,
/// export, print), the view, selecting, moving the insertion point, copying, going through the
/// slides and playing the show. Everything else that would change the version is refused, Undo
/// and Redo included: the version has no edits of its own to take back.
fn allowed(id: &str) -> bool {
    if matches!(id, "file.properties" | "file.revert") {
        return false;
    }
    ["file.", "view.", "selection.", "window.", "show.", "media."].iter().any(|p| id.starts_with(p))
        || matches!(
            id,
            "edit.copy"
                | "edit.selectAll"
                | "edit.deselect"
                | "edit.select"
                | "slide.go"
                | "slide.next"
                | "slide.previous"
                | "slide.first"
                | "slide.last"
                | "slide.selectRange"
                | "slide.selectSlides"
                | "table.selectCells"
                | "text.edit"
                | "text.exit"
                | "text.move"
                | "text.select"
                | "text.selectAll"
                | "text.selectWord"
                | "text.selectParagraph"
                | "review.endCompare"
        )
}

/// Refuse command `spec` while a previous version is in front, unless it only looks (queries) or
/// is one of the [`allowed`] ones. Called by `Session::execute` before the command runs.
pub(crate) fn check_before(s: &Session, spec: &CommandSpec) -> Result<()> {
    if s.active_previous_version().is_none() || allowed(spec.id) || (!spec.journal && !spec.undoable) {
        return Ok(());
    }
    Err(EngineError::Other(READ_ONLY.into()))
}

/// `file.openVersion`: the version's bytes, read from the history and shown read only.
fn open_version(s: &mut Session, p: &Value) -> Result<Value> {
    let id = str_param(p, "id").ok_or_else(|| bad("file.openVersion", "missing `id` (a version id from file.versions)"))?.to_owned();
    let at = super::file::target_index(s, p, "file.openVersion")?;
    let index = s.history_owner(at).ok_or(EngineError::NoDocument)?;
    let (info, bytes) = s.read_version(index, &id).map_err(EngineError::Other)?;
    let (path, source, slide) = {
        let d = s.documents().get(index).ok_or(EngineError::NoDocument)?;
        (d.path.clone().ok_or_else(|| EngineError::Other("save the presentation to have a version history".into()))?, d.uid, d.selection.slide)
    };
    // Read the version before touching anything: a damaged one fails here and changes nothing.
    let (version, _) = super::file::open_checked(&path, &bytes)?;
    let stem = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty());
    let name = format!("{} (version {})", stem.as_deref().unwrap_or("Presentation"), local_timestamp(&info.saved_at));
    s.drop_previous_versions(source);
    let document_id = s.index_of_uid(source).and_then(|i| s.version_document_id(i));
    crate::fonts::install_embedded_fonts(&version);
    let mut st = DocState::new(version, None, name.clone());
    st.missing_fonts = crate::fonts::missing_fonts(&st.doc);
    // Where the presentation is looking, so the two line up.
    st.selection.slide = slide.min(st.doc.slides.len().saturating_sub(1));
    st.versions.copy_of = Some(VersionOrigin { document_id, version_id: Some(info.id.clone()) });
    st.previous = Some(Box::new(PreviousVersion { source, info: info.clone() }));
    let document = s.add_document(st);
    // A version has no file: AutoSave has nothing to write.
    if let Some(d) = s.docs.get_mut(document) {
        d.autosave = false;
    }
    let uid = s.documents().get(document).map_or(0, |d| d.uid);
    Ok(json!({"opened": info.id, "document": document, "uid": uid, "name": name, "savedAt": info.saved_at, "author": info.author}))
}

/// `file.closeVersion`: back to the presentation, exactly as it was.
fn close_version(s: &mut Session, _: &Value) -> Result<Value> {
    let index = s.active_index().ok_or(EngineError::NoDocument)?;
    let id = s.previous_version(index).map(|v| v.info.id.clone()).ok_or_else(|| EngineError::Other("no previous version is open".into()))?;
    let back = s.close_previous_version(index);
    Ok(json!({"closed": id, "document": back}))
}

/// `file.compareVersion`: Review › Compare with the previous version as the original and the
/// presentation it came from as the revised one. Running it again shows the version.
fn compare_version(s: &mut Session, _: &Value) -> Result<Value> {
    let index = s.active_index().ok_or(EngineError::NoDocument)?;
    let source = s.previous_version(index).map(|v| v.source).ok_or_else(|| EngineError::Other("no previous version is open".into()))?;
    let source_index = s.index_of_uid(source).ok_or_else(|| EngineError::Other("the presentation this version belongs to is closed".into()))?;
    let (current, name) = {
        let d = s.documents().get(source_index).ok_or(EngineError::NoDocument)?;
        let name = d.path.clone().unwrap_or_else(|| d.title());
        (d.doc.clone(), name)
    };
    let st = s.doc_mut()?;
    if st.compare.take().is_some() {
        return Ok(json!({"comparing": false}));
    }
    let by_position = crate::compare::unrelated(&st.doc, &current);
    st.compare = Some(crate::compare::Compare { name: name.clone(), revised: current, by_position, rejected: Default::default() });
    let changes = st.compare.as_ref().map_or(0, |c| crate::compare::pending(&st.doc, c).len());
    s.ui_requests.push(UiRequest::Pane { id: "revisions".into() });
    Ok(json!({"comparing": true, "name": name, "changes": changes}))
}
