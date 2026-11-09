//! The version history as the session uses it: which presentations have one, what a save does
//! before and after writing, and the calls the `file.versions` and `file.options` commands make.
//!
//! The store itself (`versions.rs`) knows files and bytes. Here is what only the session knows:
//! each presentation's editing session, its document id, its disk stamp, and the preferences that
//! name the history folders. Every failure of the history is kept (`Session::versions_error`,
//! `DocVersions::error`) and logged; none of it reaches a save.

use std::path::Path;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;

use super::{DeleteScope, DiskBefore, Relocation, VersionInfo, VersionKind, VersionOrigin, VersionStats, document_uuid};
#[cfg(not(target_arch = "wasm32"))]
use super::{
    VersionDetails, VersionProperties, VersionWrite, Versions, canonical_path, default_device_label, disk_before, iso_from_unix_secs, local_offset,
    meta, same_file_path,
};
use deckcraft_model::Presentation;

/// Formats whose package carries a document id in `docProps/custom.xml`.
pub(crate) fn id_format(format: &str) -> bool {
    matches!(format, "pptx" | "potx" | "ppsx")
}

/// Whether a file at `path` has a history: the formats that keep the whole presentation (see
/// [`crate::autosave::keeps_everything`]). An export in another format (PDF, pictures, outline) is
/// not a version of the presentation, and a macro-enabled `.pptm` is not written as such, so it
/// has none either.
pub(crate) fn has_history(path: &Path) -> bool {
    crate::autosave::keeps_everything(&path.to_string_lossy())
}

/// What a save worked out before it wrote, for the version recorded after it.
#[derive(Clone, Debug, Default)]
pub(crate) struct VersionPrep {
    /// The save has a history to record into.
    pub active: bool,
    /// The id to write into the package before it is encoded (`None`: it already carries it, or
    /// the format has none).
    pub embed_id: Option<String>,
    pub kind: Option<VersionKind>,
    pub origin: Option<VersionOrigin>,
    pub trigger: String,
    pub before: Option<DiskBefore>,
    /// The id the history is kept under (`None`: kept by path).
    pub document_id: Option<String>,
    pub autosave: bool,
}

impl crate::Session {
    /// Desktop host wiring for the local history: `root` is the settings folder's `versions`,
    /// the default; the folders in Preferences apply on top (`Prefs::versions_folder`). `wake`
    /// is called from the worker when it finishes something, so a host can repaint.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_versions(&mut self, root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<(), String> {
        let roots = version_roots(&root, &self.prefs);
        self.version_store = Some(Versions::start(root, roots, wake)?);
        Ok(())
    }

    /// Whether this session keeps a version history (the desktop host turned it on).
    pub fn versions_enabled(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.version_store.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// Collect what the worker finished; a host calls it every frame (it is cheap).
    pub fn versions_tick(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = self.version_store.as_mut() {
            store.poll();
        }
    }

    /// Wait until every version queued so far is on disk (a host about to quit, tests).
    pub fn version_wait_idle(&mut self, limit: std::time::Duration) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = self.version_store.as_mut() {
            return store.wait_idle(limit);
        }
        let _ = limit;
        true
    }

    /// The last error of the history, if any (a failed history write never fails a save).
    pub fn versions_error(&mut self) -> Option<String> {
        self.versions_tick();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = self.version_store.as_ref() {
            return store.error.clone();
        }
        None
    }

    /// Work by `clock` (seconds since the epoch) instead of the system clock: retention and the
    /// AutoSave window count from the time of the write.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_version_clock(&mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) {
        if let Some(store) = self.version_store.as_mut() {
            store.set_clock(clock);
        }
    }

    /// The id the history of presentation `index` is kept under: the id its package carries (for
    /// `.pptx`, `.potx` and `.ppsx`; none for other formats, which are kept by path).
    pub fn version_document_id(&self, index: usize) -> Option<String> {
        let d = self.docs.get(index)?;
        let path = d.path.as_deref()?;
        if !has_history(Path::new(path)) || !id_format(crate::cmd::file::format_for_path(path)) {
            return None;
        }
        document_uuid(&d.doc).or_else(|| d.versions.id.clone())
    }

    /// The versions of presentation `index`, newest first (empty without a history or a file).
    pub fn version_infos(&mut self, index: usize) -> Vec<VersionInfo> {
        self.versions_tick();
        #[cfg(not(target_arch = "wasm32"))]
        {
            let id = self.version_document_id(index);
            let cloud = self.cloud_roots.clone();
            let path = self.docs.get(index).and_then(|d| d.path.clone());
            if let (Some(path), Some(store)) = (path, self.version_store.as_mut())
                && has_history(Path::new(&path))
            {
                return store.list(Path::new(&path), id.as_deref(), &cloud);
            }
        }
        let _ = index;
        Vec::new()
    }

    /// What a version records about this save (see [`VersionDetails`]): a random installation id
    /// and the user's device label instead of the computer's name, the path without the account,
    /// and counts instead of text.
    #[cfg(not(target_arch = "wasm32"))]
    fn version_details(&self, index: usize, path: &Path, trigger: &str, before: Option<DiskBefore>, doc: Option<&Presentation>) -> VersionDetails {
        let now = self.version_store.as_ref().map_or_else(super::now_unix, Versions::now);
        let opened = self.docs.get(index).map_or(0, |d| d.versions.opened_at);
        let initials: String = self.prefs.author.split_whitespace().filter_map(|w| w.chars().next()).take(4).collect();
        let mut details = VersionDetails {
            utc_offset: local_offset(now),
            session_start: (opened > 0).then(|| iso_from_unix_secs(opened)),
            editing_seconds: (opened > 0).then(|| now.saturating_sub(opened)),
            trigger: trigger.to_owned(),
            initials,
            installation: self.version_store.as_ref().map(|s| s.installation.clone()).unwrap_or_default(),
            device: self.prefs.device_label.clone().filter(|l| !l.trim().is_empty()).unwrap_or_else(|| default_device_label().to_owned()),
            os: std::env::consts::OS.to_owned(),
            os_version: String::new(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("DECKCRAFT_COMMIT").map(str::to_owned),
            path: meta::portable_path(path, &self.cloud_roots),
            provider: meta::provider_of(path, &self.cloud_roots).to_owned(),
            before,
            ..VersionDetails::default()
        };
        if let Some(doc) = doc {
            details.properties = Some(VersionProperties {
                title: doc.props.title.clone(),
                creator: doc.props.author.clone(),
                last_modified_by: doc.props.last_modified_by.clone(),
                revision: u64::from(doc.props.revision),
            });
            details.stats = Some(stats_of(doc));
        }
        details
    }

    /// Before `file.save` encodes and writes `path` in `format`: settle the presentation's
    /// identity and keep what is on disk.
    ///
    /// - A `.pptx`, `.potx` or `.ppsx` gets a random document id, in its custom properties, at
    ///   its first save (made here or received from someone else); the answer's `embed_id` is
    ///   what the caller writes into the package. Save As (and Save to Cloud, Keep both) makes
    ///   a new file: it gets a new id and records which document and version it came from. A
    ///   file that carries the id of another file that still exists (a copy made outside the
    ///   app) gets a new id at its next save, also recording where it came from. Other formats
    ///   have no id and keep their history by path.
    /// - Then the bytes on disk are kept if the history lacks them (an `original` or an
    ///   `external` version), synchronously, before the file is replaced.
    ///
    /// Nothing here can fail the save.
    pub(crate) fn version_prepare(&mut self, index: usize, path: &Path, format: &str, trigger: Option<&str>, autosave: bool) -> VersionPrep {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // A file written in a format other than its name says (`format` overriding the
            // extension) is not the presentation's own file.
            let named = crate::cmd::file::format_for_path(&path.to_string_lossy());
            if self.version_store.is_none() || !has_history(path) || named != format || self.docs.get(index).is_none() {
                return VersionPrep::default();
            }
            self.version_prepare_desktop(index, path, format, trigger, autosave)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (index, path, format, trigger, autosave);
            VersionPrep::default()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn version_prepare_desktop(&mut self, index: usize, path: &Path, format: &str, trigger: Option<&str>, autosave: bool) -> VersionPrep {
        let packaged = id_format(format);
        let cloud = self.cloud_roots.clone();
        let canonical = canonical_path(path);
        let (own, has_path, in_doc, remembered, checked, verified, kind) = {
            let d = &self.docs[index];
            (
                d.path.as_deref().is_some_and(|own| same_file_path(Path::new(own), path)),
                d.path.is_some(),
                document_uuid(&d.doc),
                d.versions.id.clone(),
                d.versions.identity_checked.as_ref() == Some(&canonical),
                d.versions.verified.as_ref() == Some(&canonical),
                d.versions.next_kind,
            )
        };
        let current = in_doc.clone().or(remembered);
        let mut kind = kind;
        let mut trigger = trigger.map(str::to_owned).unwrap_or_else(|| if autosave { "autosave".into() } else { "save".into() });
        let mut origin = None;
        let mut id = None;
        if packaged {
            let previous = current.as_deref().and_then(|cur| self.version_store.as_ref().and_then(|s| s.newest(path, cur, &cloud)));
            if !own && has_path {
                // A new file from this presentation.
                origin = Some(VersionOrigin { document_id: current.clone(), version_id: previous.map(|(v, _)| v) });
                kind = kind.or(Some(VersionKind::SaveAs));
                trigger = if kind == Some(VersionKind::ConflictCopy) { "conflictCopy".into() } else { "saveAs".into() };
            } else if let Some(cur) = current.clone() {
                if checked {
                    id = Some(cur);
                } else if let Some((version, Some(saved_at))) = previous
                    && !same_file_path(&saved_at, path)
                    && crate::file_access::check_read(&saved_at.to_string_lossy()).is_ok()
                    && saved_at.is_file()
                    && meta::file_document_id(&saved_at).as_deref() == Some(cur.as_str())
                {
                    // Another file with this id is still there: this one is a copy.
                    origin = Some(VersionOrigin { document_id: Some(cur), version_id: Some(version) });
                    trigger = "copy".into();
                } else {
                    id = Some(cur);
                }
            }
            if id.is_none() {
                match meta::new_document_id() {
                    Ok(new) => id = Some(new),
                    Err(error) => self.note_version_error(index, error),
                }
            }
        }
        let embed_id = id.clone().filter(|new| in_doc.as_deref() != Some(new.as_str()));
        if packaged {
            let d = &mut self.docs[index];
            d.versions.identity_checked = Some(canonical.clone());
            if let Some(new) = &id {
                d.versions.id = Some(new.clone());
            }
        }
        // Keep what is on disk, into the history this save goes to.
        let changed_outside =
            own && self.docs[index].stamp.as_mut().is_some_and(|stamp| !matches!(stamp.check(path), Ok(deckcraft_cloud::DiskState::Unchanged)));
        let details = self.version_details(index, path, "beforeSave", None, None);
        let verified = verified && own && !changed_outside;
        let mut before = None;
        let readable = crate::file_access::check_read(&path.to_string_lossy()).is_ok();
        if readable && let Some(store) = self.version_store.as_mut() {
            match store.protect(path, id.as_deref(), details, &cloud, verified) {
                Ok(found) => before = found,
                Err(error) => {
                    log::warn!("version history could not keep {} before saving: {error}", path.display());
                    store.error = Some(error);
                }
            }
        }
        if before.is_none() && readable {
            // Known to hold the previous version; its hash comes from that version's record.
            before = disk_before(path);
        }
        VersionPrep { active: true, embed_id, kind, origin, trigger, before, document_id: id, autosave }
    }

    /// After a save wrote `bytes` to `path`: hand them to the history (a worker writes them).
    /// `doc` is the presentation as encoded. Nothing here can fail the save.
    pub(crate) fn version_record(&mut self, index: usize, path: &Path, bytes: Vec<u8>, prep: VersionPrep, doc: &Presentation) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if !prep.active || self.version_store.is_none() {
                return;
            }
            let kind = prep.kind.unwrap_or(if prep.autosave { VersionKind::Autosave } else { VersionKind::Save });
            let trigger = if prep.trigger.is_empty() { if prep.autosave { "autosave".to_owned() } else { "save".to_owned() } } else { prep.trigger };
            let canonical = canonical_path(path);
            if let Some(d) = self.docs.get_mut(index) {
                d.versions.verified = Some(canonical);
                d.versions.next_kind = None;
            }
            self.record_version_with(index, path, bytes, kind, &trigger, prep.document_id, prep.origin, prep.before, doc);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (index, path, bytes, prep, doc);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn record_version_with(
        &mut self,
        index: usize,
        path: &Path,
        bytes: Vec<u8>,
        kind: VersionKind,
        trigger: &str,
        document_id: Option<String>,
        origin: Option<VersionOrigin>,
        before: Option<DiskBefore>,
        doc: &Presentation,
    ) {
        let Some((uid, epoch)) = self.docs.get(index).map(|d| (d.uid, d.versions.epoch)) else { return };
        let mut details = self.version_details(index, path, trigger, before, Some(doc));
        details.origin = origin;
        let author = self.prefs.author.clone();
        let cloud = self.cloud_roots.clone();
        let Some(store) = self.version_store.as_mut() else { return };
        // An AutoSave write joins the version its editing session started; a write made to keep
        // the state before a restore never does.
        let session = (kind == VersionKind::Autosave && trigger != "beforeRestore").then(|| format!("{}-{uid}-{epoch}", store.run_id));
        let now = store.now();
        store.save(VersionWrite {
            path: canonical_path(path),
            bytes,
            author,
            kind,
            saved_at: iso_from_unix_secs(now),
            session,
            document_id,
            details,
            cloud,
            now: Some(now),
        });
    }

    /// Restore is about to replace presentation `index`, which holds changes not yet written to
    /// its file: store them as a version first, so the state before the restore stays in the
    /// history (OneDrive likewise keeps the replaced current version in the list).
    pub(crate) fn keep_before_restore(&mut self, index: usize, bytes: Vec<u8>, doc: &Presentation) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = self.docs.get(index).and_then(|d| d.path.clone()) else { return };
            let id = self.version_document_id(index);
            // `Autosave` without a session: never joined into, nor joining, another version.
            self.record_version_with(index, Path::new(&path), bytes, VersionKind::Autosave, "beforeRestore", id, None, None, doc);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (index, bytes, doc);
    }

    /// Restore replaced presentation `index` with version `version`: record the restore at once
    /// with those exact bytes, as OneDrive makes the restored version the newest one in the list
    /// right away (whether or not AutoSave writes the file afterwards).
    pub(crate) fn record_restore(&mut self, index: usize, bytes: Vec<u8>, version: &str, doc: &Presentation) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(path) = self.docs.get(index).and_then(|d| d.path.clone()) else { return };
            let id = self.version_document_id(index);
            self.record_version_with(index, Path::new(&path), bytes, VersionKind::Restore, &format!("restore:{version}"), id, None, None, doc);
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (index, bytes, version, doc);
    }

    /// The bytes of version `id` of presentation `index`, and its record.
    pub fn read_version(&mut self, index: usize, id: &str) -> Result<(VersionInfo, Vec<u8>), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.versions_tick();
            let document = self.version_document_id(index);
            let path =
                self.docs.get(index).and_then(|d| d.path.clone()).ok_or_else(|| "save the presentation to have a version history".to_string())?;
            let store = self.version_store.as_ref().ok_or_else(|| "local version history is unavailable".to_string())?;
            store.read(&canonical_path(Path::new(&path)), document.as_deref(), id)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (index, id);
            Err("local version history is available on desktop only".into())
        }
    }

    /// Delete the history of presentation `index` (`DeleteScope::Document`), or all history
    /// (`DeleteScope::All`, `index` ignored), in every history folder. Returns how many
    /// document folders went.
    pub fn delete_version_history(&mut self, index: Option<usize>, scope: DeleteScope) -> Result<usize, String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.versions_tick();
            let document = index.and_then(|i| self.version_document_id(i));
            let path = index.and_then(|i| self.docs.get(i)).and_then(|d| d.path.clone());
            if scope == DeleteScope::Document && path.is_none() {
                return Err("save the presentation to have a version history".into());
            }
            let store = self.version_store.as_mut().ok_or_else(|| "local version history is unavailable".to_string())?;
            let result = store.delete(scope, path.as_deref().map(Path::new), document.as_deref());
            for d in &mut self.docs {
                d.versions.verified = None;
            }
            result
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (index, scope);
            Err("local version history is available on desktop only".into())
        }
    }

    /// Preferences › Version history changed: use the folders it names now, and copy, move or
    /// leave the history already kept.
    pub fn apply_version_folders(&mut self, existing: Relocation) -> Result<(), String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let prefs = self.prefs.clone();
            if let Some(store) = self.version_store.as_mut() {
                let roots = version_roots(&store.default_root, &prefs);
                store.set_folders(roots, existing)?;
            }
            for d in &mut self.docs {
                d.versions.verified = None;
            }
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = existing;
            Ok(())
        }
    }

    /// The history folders in use (empty where there is no history), those the last write
    /// could not reach, and whether history is being copied to new folders.
    pub fn version_folders(&self) -> (Vec<std::path::PathBuf>, Vec<std::path::PathBuf>, bool) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(store) = self.version_store.as_ref() {
            return store.folders();
        }
        (Vec::new(), Vec::new(), false)
    }

    /// A file kept by path was renamed or moved (`from` its canonical path before the move): its
    /// history follows it, as OneDrive keeps a file's history through a rename or move. A
    /// presentation with an id needs nothing moved: the id goes with the file.
    pub fn versions_renamed(&mut self, index: usize, from: &Path, to: &Path) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let by_path = self.version_document_id(index).is_none();
            let from = canonical_path(from);
            if let Some(d) = self.docs.get_mut(index) {
                let moved = d.versions.verified.as_ref() == Some(&from);
                d.versions.verified = moved.then(|| canonical_path(to));
                d.versions.identity_checked = d.versions.identity_checked.take().map(|_| canonical_path(to));
            }
            if by_path
                && has_history(to)
                && let Some(store) = self.version_store.as_mut()
            {
                store.renamed(from, to.to_path_buf());
            }
        }
        #[cfg(target_arch = "wasm32")]
        let _ = (index, from, to);
    }

    /// The next version of presentation `index` is a different kind (Keep both makes a
    /// `conflictCopy`).
    pub(crate) fn set_next_version_kind(&mut self, index: usize, kind: VersionKind) {
        if let Some(d) = self.docs.get_mut(index) {
            d.versions.next_kind = Some(kind);
        }
    }

    pub(crate) fn clear_next_version_kind(&mut self, index: usize) {
        if let Some(d) = self.docs.get_mut(index) {
            d.versions.next_kind = None;
        }
    }

    fn note_version_error(&mut self, index: usize, error: String) {
        log::warn!("version history: {error}");
        if let Some(d) = self.docs.get_mut(index) {
            d.versions.error = Some(error);
        }
    }
}

/// The counts a version records about `doc`: never any of its text.
pub(crate) fn stats_of(doc: &Presentation) -> VersionStats {
    fn count(shapes: &[deckcraft_model::Shape], total: &mut u64, pictures: &mut u64) {
        for shape in shapes {
            *total += 1;
            match &shape.kind {
                deckcraft_model::ShapeKind::Picture { .. } => *pictures += 1,
                deckcraft_model::ShapeKind::Group { children, .. } => count(children, total, pictures),
                _ => {}
            }
        }
    }
    let (mut shapes, mut pictures) = (0, 0);
    for slide in &doc.slides {
        count(&slide.shapes, &mut shapes, &mut pictures);
    }
    VersionStats {
        slides: doc.slides.len() as u64,
        shapes,
        pictures,
        media: doc.media.len() as u64,
        comments: doc.slides.iter().map(|s| s.comments.len() as u64).sum(),
        notes_pages: doc.slides.iter().filter(|s| !s.notes.is_empty()).count() as u64,
    }
}

/// `doc` with the id set in its custom properties, for encoding.
pub(crate) fn with_document_id(doc: &Presentation, id: &str) -> Presentation {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut copy = doc.clone();
        meta::set_presentation_document_id(&mut copy, id);
        copy
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = id;
        doc.clone()
    }
}

/// The history folders the settings name: the settings folder's alone (the default), another
/// folder instead, or both.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn version_roots(default: &Path, prefs: &crate::Prefs) -> Vec<PathBuf> {
    match prefs.versions_folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        None => vec![default.to_path_buf()],
        Some(folder) if prefs.versions_mirror => vec![default.to_path_buf(), PathBuf::from(folder)],
        Some(folder) => vec![PathBuf::from(folder)],
    }
}
