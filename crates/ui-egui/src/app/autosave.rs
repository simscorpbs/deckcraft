//! AutoSave in the window: the title bar switch's rules, writing after changes settle, closing
//! without a Save Changes prompt, the Save Conflict and Save to Cloud dialogs, and the cloud
//! folder discovery. The engine does the writing and the conflict check (`file.save` with
//! `ifUnchanged`, `deckcraft_engine::autosave`); this module decides when and what to tell the user.

use deckcraft_engine::autosave::{AutoSaveBlock, AutoSaveEvent, Flush};

use super::{SlideApp, dialogs, json, monotonic_secs};

/// What the user is told about why AutoSave can't cover a presentation, in the interface language.
pub fn block_reason(block: &AutoSaveBlock) -> String {
    match block {
        AutoSaveBlock::Browser => {
            crate::tl!("AutoSave isn't available in the browser: it can't write to your files. Save downloads a copy.").to_string()
        }
        AutoSaveBlock::Unsaved => crate::tl!("Save the presentation to turn on AutoSave").to_string(),
        AutoSaveBlock::Format { name } => crate::i18n::fmt(
            crate::tl!("AutoSave only keeps .deckcraft, .pptx, .potx and .ppsx files; {name} can't keep everything. Save as .pptx to turn on AutoSave."),
            &[("name", name)],
        ),
        AutoSaveBlock::NotSavedHere => {
            crate::tl!("Save the presentation to turn on AutoSave (DeckCraft couldn't check this file on disk)").to_string()
        }
        AutoSaveBlock::Conflict { missing: false } => {
            crate::tl!("AutoSave is paused: this file was changed on another device. Click to choose which version to keep.").to_string()
        }
        AutoSaveBlock::Conflict { missing: true } => {
            crate::tl!("AutoSave is paused: this file was moved or deleted elsewhere. Click to choose what to do.").to_string()
        }
    }
}

impl SlideApp {
    /// Why AutoSave can't cover presentation `index`, or `None` when it can. A page that keeps the
    /// files, or the browser's downloads, can't be written to; the engine adds the rest (never
    /// saved, a format that loses things, no disk stamp, a conflict waiting).
    pub fn autosave_block(&self, index: usize) -> Option<AutoSaveBlock> {
        if self.services.download.is_some() || self.services.host_save.is_some() {
            return Some(AutoSaveBlock::Browser);
        }
        self.session.autosave_block(index)
    }

    /// Whether AutoSave writes presentation `index`: its switch is on and nothing blocks it.
    pub fn autosave_covers(&self, index: usize) -> bool {
        self.session.documents().get(index).is_some_and(|d| d.autosave) && self.autosave_block(index).is_none()
    }

    /// Look for the iCloud Drive, Google Drive and OneDrive folders on this computer, off the UI
    /// thread (on Windows it looks at drive letters); `repaint` wakes the app when done. The
    /// desktop app calls this once; until it finishes, no cloud folder is offered.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn discover_cloud(&mut self, repaint: std::sync::Arc<dyn Fn() + Send + Sync>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("cloud-discovery".into()).spawn(move || {
            let roots = deckcraft_engine::cloud::discover(&deckcraft_engine::cloud::Env::current());
            if tx.send(roots).is_ok() {
                repaint();
            }
        });
        match spawned {
            Ok(_) => self.cloud_discovery = Some(rx),
            Err(e) => log::warn!("cloud folders not looked for: {e}"),
        }
    }

    /// Take the folders the discovery thread found, once it has finished.
    pub(super) fn poll_cloud_discovery(&mut self) {
        let Some(rx) = &self.cloud_discovery else { return };
        match rx.try_recv() {
            Ok(roots) => {
                self.session.cloud_roots = roots;
                self.cloud_discovery = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => self.cloud_discovery = None,
        }
    }

    /// Where presentation `index` is kept in the cloud, if it is: the provider's name ("iCloud Drive").
    pub fn cloud_name(&self, index: usize) -> Option<&'static str> {
        let path = self.session.documents().get(index)?.path.as_deref()?;
        deckcraft_engine::cloud::locate(std::path::Path::new(path), &self.session.cloud_roots).map(|l| l.provider.name())
    }

    /// Every frame: write what has settled, tell the user what AutoSave found, and wake up when
    /// the next write is due. `now` is the monotonic clock in seconds.
    pub(crate) fn autosave_tick(&mut self, now: f64) {
        self.poll_cloud_discovery();
        for event in self.session.autosave_tick(now) {
            match event {
                AutoSaveEvent::Saved { uid } => self.autosave_finished(uid),
                AutoSaveEvent::Conflict { uid, .. } => {
                    // One dialog at a time: a conflict found while another is open waits behind
                    // the switch, which stays grey and reopens the dialog when clicked.
                    if self.dialog.is_none() {
                        self.show_save_conflict(uid);
                    }
                }
                // Shown in the status bar while writes keep failing ([`Self::autosave_failure_text`]).
                AutoSaveEvent::Failed { .. } => {}
            }
        }
    }

    /// Wake the window when AutoSave has a write due, so a still window still saves.
    pub(super) fn autosave_wake(&self, ctx: &egui::Context) {
        if let Some(due) = self.session.autosave_next_due() {
            let wait = (due - monotonic_secs()).clamp(0.25, 60.0);
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
        }
    }

    /// A write of presentation `uid` went through: if its Save Changes prompt is open, the pending
    /// close goes on without it.
    fn autosave_finished(&mut self, uid: u64) {
        let closing = self.dialog.as_ref().is_some_and(|d| d.id == "close");
        if closing && self.session.active().is_some_and(|d| d.uid == uid && !d.is_dirty()) {
            self.dialog = None;
            let _ = self.run("file.close", json!({}));
        }
    }

    /// The error to keep on the status bar while AutoSave's writes keep failing: the active
    /// presentation's, else another one's. It goes away with the first write that succeeds.
    pub fn autosave_failure_text(&self) -> Option<String> {
        let active = self.session.active_index();
        let failing = |i: usize| self.session.autosave_failure(i).map(|f| f.error.clone());
        let error = active.and_then(failing).or_else(|| (0..self.session.documents().len()).find_map(failing))?;
        Some(crate::i18n::fmt(crate::tl!("AutoSave couldn't write the file: {error}. It will try again shortly."), &[("error", &error)]))
    }

    /// Word-style AutoSave before a user action closes presentation `index`: its pending changes
    /// are written at once, so the action goes on without a Save Changes prompt. A file another
    /// device changed opens the Save Conflict dialog and the presentation stays open.
    pub(crate) fn autosave_before_close(&mut self, index: usize) -> Flush {
        if self.autosave_block(index) == Some(AutoSaveBlock::Browser) {
            return Flush::NotCovered;
        }
        let flush = self.session.autosave_flush(index);
        if flush == Flush::Conflict
            && let Some(uid) = self.session.documents().get(index).map(|d| d.uid)
        {
            self.show_save_conflict(uid);
        }
        flush
    }

    /// Quitting: every presentation AutoSave covers is written, and the ones it can't are left to
    /// the Save Changes prompt. Returns whether a conflict dialog now holds the quit back.
    pub(crate) fn autosave_before_quit(&mut self) -> bool {
        let dirty: Vec<usize> = self.session.documents().iter().enumerate().filter(|(_, d)| d.is_dirty()).map(|(i, _)| i).collect();
        let mut conflict = false;
        for i in dirty {
            if self.autosave_before_close(i) == Flush::Conflict {
                conflict = true;
                break;
            }
        }
        conflict
    }

    /// Ask what to do about the conflict AutoSave found in presentation `uid`: the AutoSave
    /// switch, closing, and AutoSave itself open this dialog.
    pub fn show_save_conflict(&mut self, uid: u64) {
        let Some(d) = self.session.documents().iter().find(|d| d.uid == uid) else { return };
        let Some(conflict) = d.conflict else { return };
        let missing = conflict == deckcraft_engine::cloud::DiskState::Missing;
        let mut dialog = dialogs::Dialog::new("saveConflict");
        dialog.params = json!({"uid": uid, "name": d.title(), "missing": missing});
        self.dialog = Some(dialog);
    }

    /// Turning AutoSave on for a presentation that was never saved: offer the cloud folders found
    /// (as PowerPoint offers OneDrive), else Save As.
    pub fn start_autosave_new(&mut self) {
        if self.session.cloud_roots.is_empty() {
            self.save_as_for_autosave();
        } else {
            let name = self.session.active().map(|d| d.title()).unwrap_or_default();
            let mut dialog = dialogs::Dialog::new("saveToCloud");
            dialog.params = json!({"name": name});
            self.dialog = Some(dialog);
        }
    }

    /// Save As, and AutoSave on for the new file once it is saved (the switch asked for it).
    pub fn save_as_for_autosave(&mut self) {
        self.save_as_dialog("pptx");
        if let Some(i) = self.session.active_index()
            && self.session.documents().get(i).is_some_and(|d| d.path.is_some())
            && self.autosave_block(i).is_none()
        {
            self.session.set_autosave_of(i, true);
        }
    }

    /// The title bar switch was clicked: toggle AutoSave where it can be on, else do what helps
    /// (save the file first, choose what to keep, or say why not).
    pub fn autosave_switch_clicked(&mut self) {
        let Some(i) = self.session.active_index() else { return };
        match self.autosave_block(i) {
            None => {
                let _ = self.run("file.autosave", json!({}));
            }
            Some(AutoSaveBlock::Unsaved) => self.start_autosave_new(),
            Some(b @ (AutoSaveBlock::Format { .. } | AutoSaveBlock::NotSavedHere)) => {
                if b.save_as_helps() {
                    self.save_as_for_autosave();
                }
            }
            Some(AutoSaveBlock::Conflict { .. }) => {
                if let Some(uid) = self.session.documents().get(i).map(|d| d.uid) {
                    self.show_save_conflict(uid);
                }
            }
            Some(b @ AutoSaveBlock::Browser) => self.set_status(block_reason(&b)),
        }
    }
}

impl SlideApp {
    /// `ui.saveConflict`: the answer to the Save Conflict dialog (also what its buttons run).
    pub(super) fn answer_save_conflict(&mut self, p: &serde_json::Value) -> Result<serde_json::Value, String> {
        let Some(uid) = self
            .dialog
            .as_ref()
            .filter(|d| d.id == "saveConflict")
            .and_then(|d| d.params.get("uid").and_then(serde_json::Value::as_u64))
        else {
            return Err("no save conflict is being asked about".into());
        };
        match p.get("answer").and_then(serde_json::Value::as_str) {
            Some("later") => {
                self.dialog = None;
                Ok(json!({"resolved": false}))
            }
            Some(keep @ ("both" | "mine" | "theirs")) => {
                let result = self.run("file.resolveConflict", json!({"keep": keep, "document": uid}));
                if result.is_ok() {
                    self.dialog = None;
                }
                result
            }
            _ => Err("`answer` must be both, mine, theirs or later".into()),
        }
    }

    /// `ui.saveToCloud`: save the presentation into a provider's folder and turn AutoSave on for
    /// it (what the Save to Cloud dialog's folder buttons run). Without a dialog open it opens one.
    pub(super) fn answer_save_to_cloud(&mut self, p: &serde_json::Value) -> Result<serde_json::Value, String> {
        if self.dialog.as_ref().is_none_or(|d| d.id != "saveToCloud") {
            self.start_autosave_new();
            return Ok(json!({"asking": self.dialog.as_ref().is_some_and(|d| d.id == "saveToCloud")}));
        }
        let name = p
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .or_else(|| self.dialog.as_ref().and_then(|d| d.fields.get("name").cloned()))
            .unwrap_or_default();
        let mut params = json!({});
        for key in ["provider", "root"] {
            if let Some(v) = p.get(key) {
                params[key] = v.clone();
            }
        }
        if !name.trim().is_empty() {
            params["name"] = json!(name.trim());
        }
        let result = self.run("file.saveToCloud", params).and_then(|r| self.run("file.autosave", json!({"value": true})).map(|_| r));
        if result.is_ok() {
            self.dialog = None;
        }
        result
    }
}
