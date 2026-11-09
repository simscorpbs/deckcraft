//! AutoSave's switch for each file, and the reasons AutoSave cannot cover a presentation.
//!
//! AutoSave is on for every presentation whose file keeps everything ([`keeps_everything`]), as
//! the preference [`crate::Prefs::autosave_by_default`] says. Turning the switch the other way on
//! a file is remembered for that file, between runs, in a small JSON file in the settings folder
//! ([`crate::Session::enable_autosave_memory`]); turning it back to the default forgets it. Save
//! As, Save to Cloud and Keep both give the new file the switch the presentation had. Headless
//! sessions remember only while they run; the browser has no files to remember.
//!
//! Whether AutoSave can write a file at all (a format that keeps everything, a disk stamp, no
//! conflict) is [`AutoSaveBlock`]; when it writes, and how it retries, is the front end's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use serde::{Deserialize, Serialize};

/// Most files remembered; the oldest choice is forgotten first.
pub const MAX_ENTRIES: usize = 2000;
/// Longest normalised path remembered, in bytes. A longer one keeps the default.
const MAX_KEY_BYTES: usize = 4096;
/// Largest memory file read back.
#[cfg(not(target_arch = "wasm32"))]
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// The memory file's format.
#[cfg(not(target_arch = "wasm32"))]
const VERSION: u32 = 1;

/// Whether saving to `name` keeps the whole presentation, so that AutoSave may write it: the
/// formats DeckCraft reads and writes without losing anything. Anything else (PDF, pictures,
/// outlines, a macro-enabled `.pptm` that DeckCraft cannot write as such) is not covered.
pub fn keeps_everything(name: &str) -> bool {
    let ext = Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "deckcraft" | "slidecraft" | "pptx" | "potx" | "ppsx")
}

/// Why AutoSave cannot cover a presentation; the title bar switch is greyed out and its tooltip
/// gives the reason.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AutoSaveBlock {
    /// The web app: saving is a download, there is no file to keep writing to.
    Browser,
    /// A presentation that has never been saved.
    Unsaved,
    /// A file in a format that cannot keep everything, by file name.
    Format { name: String },
    /// A file AutoSave cannot check before writing (opened from bytes, recovered): saving it
    /// lifts the block.
    NotSavedHere,
    /// Another device or program changed the file (or moved or deleted it, `missing`) since it
    /// was last read or written here; AutoSave waits for the user to choose what to keep.
    Conflict { missing: bool },
}

impl AutoSaveBlock {
    /// A short stable id for scripts (`file.cloud`'s `autosave.blocked`).
    pub fn id(&self) -> &'static str {
        match self {
            AutoSaveBlock::Browser => "browser",
            AutoSaveBlock::Unsaved => "unsaved",
            AutoSaveBlock::Format { .. } => "format",
            AutoSaveBlock::NotSavedHere => "notSavedHere",
            AutoSaveBlock::Conflict { missing: false } => "changed",
            AutoSaveBlock::Conflict { missing: true } => "missing",
        }
    }

    /// The reason in English, as a command reports it. The interface words it in its own language.
    pub fn reason(&self) -> String {
        match self {
            AutoSaveBlock::Browser => "AutoSave isn't available in the browser: it can't write to your files. Save downloads a copy.".into(),
            AutoSaveBlock::Unsaved => "Save the presentation to turn on AutoSave".into(),
            AutoSaveBlock::Format { name } => {
                format!("AutoSave only keeps .deckcraft, .pptx, .potx and .ppsx files; {name} can't keep everything. Save as .pptx to turn on AutoSave.")
            }
            AutoSaveBlock::NotSavedHere => "Save the presentation to turn on AutoSave (DeckCraft couldn't check this file on disk)".into(),
            AutoSaveBlock::Conflict { missing: false } => "AutoSave is paused: this file was changed on another device.".into(),
            AutoSaveBlock::Conflict { missing: true } => "AutoSave is paused: this file was moved or deleted elsewhere.".into(),
        }
    }

    /// Whether Save As can lift the block (everywhere but the browser and a waiting conflict).
    pub fn save_as_helps(&self) -> bool {
        !matches!(self, AutoSaveBlock::Browser | AutoSaveBlock::Conflict { .. })
    }
}

/// The choices the user made, oldest first, and where they are kept.
#[derive(Debug, Default)]
pub struct AutoSaveMemory {
    entries: Vec<(String, bool)>,
    /// `<settings_dir>/autosave.json` once the desktop host enabled it.
    #[cfg(not(target_arch = "wasm32"))]
    file: Option<PathBuf>,
    /// The last error reading or writing the file, if any.
    pub error: Option<String>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    files: Vec<StoredEntry>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Serialize, Deserialize)]
struct StoredEntry {
    path: String,
    autosave: bool,
}

/// The key a file is remembered by: its canonical path, without the `\\?\` prefix Windows'
/// canonical form carries, with `\` separators and ignoring case on Windows and macOS (whose
/// usual file systems ignore it). `None` for a path too long to keep.
pub fn file_key(path: &Path) -> Option<String> {
    let canonical: PathBuf = canonical_path(path);
    let key = normalise(&canonical.to_string_lossy(), cfg!(windows), cfg!(any(windows, target_os = "macos")));
    (!key.is_empty() && key.len() <= MAX_KEY_BYTES).then_some(key)
}

/// The canonical form of `path` when the file exists (symbolic links and `..` resolved), else the
/// path as given with its folder made canonical when that exists.
fn canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    match (path.parent().filter(|p| !p.as_os_str().is_empty()).and_then(|p| std::fs::canonicalize(p).ok()), path.file_name()) {
        (Some(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

/// [`file_key`]'s text rules without touching the disk: two spellings of one path (`C:/A.pptx`,
/// `\\?\c:\a.pptx`) give the same key on Windows. For lists such as Recent.
pub fn path_text_key(path: &str) -> String {
    normalise(path, cfg!(windows), cfg!(any(windows, target_os = "macos")))
}

/// [`file_key`]'s text rules, apart so tests can try Windows paths everywhere.
fn normalise(path: &str, windows: bool, fold_case: bool) -> String {
    let mut key = path.to_string();
    if windows {
        key = key.replace('/', "\\");
        if let Some(rest) = key.strip_prefix("\\\\?\\UNC\\") {
            key = format!("\\\\{rest}");
        } else if let Some(rest) = key.strip_prefix("\\\\?\\") {
            key = rest.to_string();
        }
    }
    if fold_case { key.to_lowercase() } else { key }
}

impl AutoSaveMemory {
    /// The switch the user left on `path`, when it differs from the default.
    pub fn get(&self, path: &Path) -> Option<bool> {
        let key = file_key(path)?;
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, on)| *on)
    }

    /// Remember `on` for `path`; a choice that matches `default` is forgotten instead.
    pub fn remember(&mut self, path: &Path, on: bool, default: bool) {
        let Some(key) = file_key(path) else { return };
        let before = self.entries.len();
        self.entries.retain(|(k, _)| *k != key);
        if on == default && self.entries.len() == before {
            return;
        }
        if on != default {
            self.entries.push((key, on));
            self.trim();
        }
        self.persist();
    }

    /// How many files are remembered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn trim(&mut self) {
        if let Some(extra) = self.entries.len().checked_sub(MAX_ENTRIES).filter(|n| *n > 0) {
            self.entries.drain(..extra);
        }
    }

    /// Read the choices kept in `file` (a missing or damaged file remembers nothing) and keep
    /// writing them there.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(file: PathBuf) -> Self {
        let mut memory = AutoSaveMemory { entries: Vec::new(), file: None, error: None };
        match read_limited(&file) {
            Ok(Some(bytes)) => match serde_json::from_slice::<Stored>(&bytes) {
                Ok(stored) if stored.version == VERSION => {
                    for e in stored.files {
                        if !e.path.is_empty() && e.path.len() <= MAX_KEY_BYTES && !memory.entries.iter().any(|(k, _)| *k == e.path) {
                            memory.entries.push((e.path, e.autosave));
                        }
                    }
                    memory.trim();
                }
                Ok(_) => memory.error = Some(format!("{}: unknown version", file.display())),
                Err(e) => memory.error = Some(format!("{}: {e}", file.display())),
            },
            Ok(None) => {}
            Err(e) => memory.error = Some(e),
        }
        if let Some(e) = &memory.error {
            log::warn!("AutoSave choices not read: {e}");
        }
        memory.file = Some(file);
        memory
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn persist(&mut self) {
        let Some(file) = &self.file else { return };
        let stored =
            Stored { version: VERSION, files: self.entries.iter().map(|(path, on)| StoredEntry { path: path.clone(), autosave: *on }).collect() };
        let written = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string()).and_then(|bytes| write_atomic(file, &bytes));
        match written {
            Ok(()) => self.error = None,
            Err(e) => {
                log::warn!("AutoSave choices not saved to {}: {e}", file.display());
                self.error = Some(e);
            }
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn persist(&mut self) {}
}

/// The file's bytes, `None` when there is no file, an error when it is too big or unreadable.
#[cfg(not(target_arch = "wasm32"))]
fn read_limited(file: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read;
    let f = match std::fs::File::open(file) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    let mut bytes = Vec::new();
    f.take(MAX_FILE_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", file.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{} is larger than {MAX_FILE_BYTES} bytes", file.display()));
    }
    Ok(Some(bytes))
}

/// Write `bytes` to a temporary file beside `path` and rename it over `path`, so an interrupted
/// write leaves the previous choices in place.
#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "autosave.json".into());
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written.map_err(|e| format!("{}: {e}", path.display()))
}

impl crate::Session {
    /// Desktop host wiring: remember AutoSave choices in `file` (`<settings_dir>/autosave.json`)
    /// from now on, and give every open presentation the switch remembered for its file.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_autosave_memory(&mut self, file: PathBuf) {
        self.autosave_memory = AutoSaveMemory::load(file);
        self.refresh_autosave();
    }

    /// The switch remembered for the file of presentation `index`, when it differs from the default.
    pub fn autosave_remembered(&self, index: usize) -> Option<bool> {
        self.docs.get(index)?.path.as_deref().and_then(|p| self.autosave_memory.get(Path::new(p)))
    }

    /// The switch a file with this path starts with: its remembered choice, else the default.
    pub(crate) fn autosave_for_path(&self, path: Option<&str>) -> bool {
        path.and_then(|p| self.autosave_memory.get(Path::new(p))).unwrap_or(self.prefs.autosave_by_default)
    }

    /// Set every presentation's switch from what is remembered for its file, else the default.
    pub fn refresh_autosave(&mut self) {
        for i in 0..self.docs.len() {
            let on = self.autosave_for_path(self.docs[i].path.as_deref());
            self.docs[i].autosave = on;
        }
    }

    /// The AutoSave switch of presentation `index` (`file.autosave`), remembered for its file.
    pub fn set_autosave_of(&mut self, index: usize, on: bool) {
        let default = self.prefs.autosave_by_default;
        let Some(d) = self.docs.get_mut(index) else { return };
        d.autosave = on;
        if let Some(path) = d.path.clone() {
            self.autosave_memory.remember(Path::new(&path), on, default);
        }
    }

    /// File › Options › Save: whether AutoSave is on for files with no choice of their own. Open
    /// presentations follow it unless their file has one.
    pub fn set_autosave_by_default(&mut self, on: bool) {
        self.prefs.autosave_by_default = on;
        self.refresh_autosave();
    }

    /// Why AutoSave cannot cover presentation `index`, or `None` when it can: its file was read
    /// from disk or saved here, in a format that keeps everything, with no conflict waiting.
    /// (The browser build says [`AutoSaveBlock::Browser`]; the desktop front end adds that
    /// reason for hosts that keep the files elsewhere.)
    pub fn autosave_block(&self, index: usize) -> Option<AutoSaveBlock> {
        if cfg!(target_arch = "wasm32") {
            return Some(AutoSaveBlock::Browser);
        }
        let d = self.docs.get(index)?;
        let Some(path) = &d.path else { return Some(AutoSaveBlock::Unsaved) };
        if !keeps_everything(path) {
            let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            return Some(AutoSaveBlock::Format { name });
        }
        if let Some(conflict) = d.conflict {
            return Some(AutoSaveBlock::Conflict { missing: conflict == deckcraft_cloud::DiskState::Missing });
        }
        if d.stamp.is_none() {
            return Some(AutoSaveBlock::NotSavedHere);
        }
        None
    }

    /// Whether AutoSave writes presentation `index`: its switch is on and nothing blocks it.
    pub fn autosave_covers(&self, index: usize) -> bool {
        self.docs.get(index).is_some_and(|d| d.autosave) && self.autosave_block(index).is_none()
    }

    /// The index of the presentation with this `uid`.
    pub fn index_of_uid(&self, uid: u64) -> Option<usize> {
        self.docs.iter().position(|d| d.uid == uid)
    }
}

/// How long a presentation must stay unchanged before AutoSave writes it, in seconds.
pub const SETTLE_SECS: f64 = 2.5;

/// The wait before AutoSave tries again after `failures` writes in a row failed: 5 s, doubling to
/// at most a minute.
pub fn retry_after(failures: u32) -> f64 {
    let doublings = failures.saturating_sub(1).min(4);
    (5.0 * f64::from(1u32 << doublings)).min(60.0)
}

/// Writes that keep failing (the file locked by another program, a share gone for a moment).
/// AutoSave pauses and tries again with a growing wait; the switch stays on and the choice
/// remembered for the file does not change.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoSaveFailure {
    /// How many writes in a row failed.
    pub failures: u32,
    /// The clock time (the `now` of [`crate::Session::autosave_tick`]) when the next write may be tried.
    pub retry_at: f64,
    /// The last error.
    pub error: String,
}

/// AutoSave's schedule for one presentation.
#[derive(Clone, Debug, Default)]
pub struct AutoSaveRun {
    /// Address of the presentation as last seen unsaved, and when it was first seen so: a change
    /// moves the address and restarts the wait.
    seen: Option<(usize, f64)>,
    /// The writes that keep failing, if they do.
    pub failure: Option<AutoSaveFailure>,
}

/// What a tick of AutoSave did for one presentation.
#[derive(Clone, Debug, PartialEq)]
pub enum AutoSaveEvent {
    /// The presentation was written.
    Saved { uid: u64 },
    /// Another device or program changed the file (or moved or deleted it, `missing`): nothing was
    /// written and AutoSave waits for the user (`file.resolveConflict`).
    Conflict { uid: u64, missing: bool },
    /// The write failed; AutoSave tries again `retry_in` seconds from now.
    Failed { uid: u64, error: String, failures: u32, retry_in: f64 },
}

/// What [`crate::Session::autosave_flush`] found.
#[derive(Clone, Debug, PartialEq)]
pub enum Flush {
    /// The changes were written; the presentation is clean.
    Saved,
    /// The file changed elsewhere; nothing was written and the conflict waits for the user.
    Conflict,
    /// AutoSave does not cover the presentation, or the write failed: the caller falls back to
    /// asking (Save Changes).
    NotCovered,
}

impl crate::Session {
    /// Run AutoSave's schedule at clock time `now` (seconds, any steady clock): every unsaved
    /// presentation that AutoSave covers is written once it has stayed unchanged for
    /// [`SETTLE_SECS`] and no failed write is waiting out its retry. `file.save` with `ifUnchanged`
    /// does the writing, so a file another device changed is never overwritten. Nothing is written
    /// while a pointer gesture or an input method composition is in progress.
    pub fn autosave_tick(&mut self, now: f64) -> Vec<AutoSaveEvent> {
        let mut events = Vec::new();
        if self.active().is_some_and(|d| d.interaction.is_some() || d.composition.is_some()) {
            return events;
        }
        for i in 0..self.docs.len() {
            let covered = self.autosave_covers(i);
            let d = &mut self.docs[i];
            if !covered || !d.is_dirty() {
                d.autosave_run.seen = None;
                continue;
            }
            let ptr = Arc::as_ptr(&d.doc) as usize;
            match d.autosave_run.seen {
                Some((p, _)) if p == ptr => {}
                _ => {
                    d.autosave_run.seen = Some((ptr, now));
                    continue;
                }
            }
            let since = d.autosave_run.seen.map_or(now, |(_, t)| t);
            let waiting = d.autosave_run.failure.as_ref().is_some_and(|f| now < f.retry_at);
            if now - since < SETTLE_SECS || waiting {
                continue;
            }
            if let Some(event) = self.autosave_write(i, now, None) {
                events.push(event);
            }
        }
        events
    }

    /// Write presentation `index` with `file.save` and `ifUnchanged`, and update its schedule.
    /// `trigger` is what made the write when it is not the schedule (`close`): the version
    /// history records it.
    fn autosave_write(&mut self, index: usize, now: f64, trigger: Option<&str>) -> Option<AutoSaveEvent> {
        let uid = self.docs.get(index)?.uid;
        let mut params = serde_json::json!({"document": uid, "ifUnchanged": true});
        if let Some(trigger) = trigger {
            params["trigger"] = serde_json::json!(trigger);
        }
        let result = self.guarded("file.save", |s| crate::cmd::file::save(s, &params));
        let at = self.index_of_uid(uid)?;
        let d = self.docs.get_mut(at)?;
        match result {
            Ok(v) if v.get("conflict").is_some() => {
                let missing = d.conflict == Some(deckcraft_cloud::DiskState::Missing);
                log::warn!("AutoSave paused: {} was changed elsewhere", d.title());
                Some(AutoSaveEvent::Conflict { uid, missing })
            }
            Ok(v) if v.get("saved").and_then(serde_json::Value::as_bool) == Some(true) => {
                d.autosave_run = AutoSaveRun::default();
                Some(AutoSaveEvent::Saved { uid })
            }
            // Not written for another reason (no stamp): the block says why; nothing to retry.
            Ok(_) => None,
            Err(e) => {
                let error = e.to_string();
                let failures = d.autosave_run.failure.as_ref().map_or(0, |f| f.failures).saturating_add(1);
                let retry_in = retry_after(failures);
                log::warn!("AutoSave failed ({failures} in a row): {error}");
                d.autosave_run.failure = Some(AutoSaveFailure { failures, retry_at: now + retry_in, error: error.clone() });
                Some(AutoSaveEvent::Failed { uid, error, failures, retry_in })
            }
        }
    }

    /// Word-style AutoSave before a user action closes a presentation: write its pending changes
    /// at once (ignoring the settle wait and any retry wait) so the action can go on without a
    /// Save Changes prompt. A failed write leaves the prompt in charge.
    pub fn autosave_flush(&mut self, index: usize) -> Flush {
        if !self.docs.get(index).is_some_and(|d| d.is_dirty() && d.autosave) {
            return Flush::NotCovered;
        }
        if let Some(AutoSaveBlock::Conflict { .. }) = self.autosave_block(index) {
            return Flush::Conflict;
        }
        if !self.autosave_covers(index) {
            return Flush::NotCovered;
        }
        match self.autosave_write(index, 0.0, Some("close")) {
            Some(AutoSaveEvent::Saved { .. }) => Flush::Saved,
            Some(AutoSaveEvent::Conflict { .. }) => Flush::Conflict,
            // A failed write is left to the Save Changes prompt; the schedule starts afresh.
            Some(AutoSaveEvent::Failed { .. }) | None => {
                if let Some(d) = self.docs.get_mut(index) {
                    d.autosave_run.failure = None;
                }
                Flush::NotCovered
            }
        }
    }

    /// The writes of presentation `index` that keep failing, if they do.
    pub fn autosave_failure(&self, index: usize) -> Option<&AutoSaveFailure> {
        self.docs.get(index)?.autosave_run.failure.as_ref()
    }

    /// When (on the clock of [`Self::autosave_tick`]) the next AutoSave write could be due, so a
    /// host can wake itself; `None` when nothing is waiting to be written.
    pub fn autosave_next_due(&self) -> Option<f64> {
        (0..self.docs.len())
            .filter(|&i| self.autosave_covers(i) && self.docs[i].is_dirty())
            .filter_map(|i| {
                let run = &self.docs[i].autosave_run;
                let settle = run.seen.map(|(_, t)| t + SETTLE_SECS);
                let retry = run.failure.as_ref().map(|f| f.retry_at);
                match (settle, retry) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (a, b) => a.or(b),
                }
            })
            .min_by(f64::total_cmp)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn windows_paths_lose_the_verbatim_prefix_and_case() {
        assert_eq!(normalise(r"\\?\C:\Users\Ana\Deck.PPTX", true, true), r"c:\users\ana\deck.pptx");
        assert_eq!(normalise("C:/Users/Ana/Deck.pptx", true, true), r"c:\users\ana\deck.pptx");
        assert_eq!(normalise(r"\\?\UNC\server\share\A.pptx", true, true), r"\\server\share\a.pptx");
        assert_eq!(normalise("/home/ana/Deck.pptx", false, false), "/home/ana/Deck.pptx");
    }

    #[test]
    fn only_lossless_formats_are_covered() {
        for ok in ["a.pptx", "A.PPTX", "a.potx", "a.ppsx", "a.deckcraft", "dir.pptx/b.deckcraft"] {
            assert!(keeps_everything(ok), "{ok}");
        }
        for no in ["a.pdf", "a.pptm", "a.txt", "a.md", "a.rtf", "a.png", "a", "a.pptx.bak"] {
            assert!(!keeps_everything(no), "{no}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("deckcraft-autosave-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn choices_persist_forget_the_default_and_ignore_case_on_windows() {
        let dir = temp("memory");
        let store = dir.join("settings").join("autosave.json");
        let doc = dir.join("Report.pptx");
        std::fs::write(&doc, b"x").unwrap();
        let mut m = AutoSaveMemory::load(store.clone());
        m.remember(&doc, false, true);
        assert_eq!(AutoSaveMemory::load(store.clone()).get(&doc), Some(false), "kept between runs");
        if cfg!(windows) {
            let upper = PathBuf::from(doc.to_string_lossy().to_uppercase());
            assert_eq!(m.get(&upper), Some(false), "case is ignored on Windows");
        }
        m.remember(&doc, true, true);
        assert!(m.is_empty(), "turning it back on forgets the choice");
        assert!(AutoSaveMemory::load(store).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_damaged_or_huge_memory_file_remembers_nothing_and_the_list_is_bounded() {
        let dir = temp("damaged");
        let store = dir.join("autosave.json");
        std::fs::write(&store, b"{not json").unwrap();
        let m = AutoSaveMemory::load(store.clone());
        assert!(m.is_empty() && m.error.is_some());
        std::fs::write(&store, vec![b' '; (MAX_FILE_BYTES + 10) as usize]).unwrap();
        assert!(AutoSaveMemory::load(store.clone()).is_empty());
        // Hostile content: wrong version, wrong types, an absurd path.
        for hostile in [
            r#"{"version": 99, "files": []}"#.to_string(),
            r#"{"version": 1, "files": "no"}"#.to_string(),
            r#"{"version": 1, "files": [{"path": 5, "autosave": "x"}]}"#.to_string(),
            format!(r#"{{"version": 1, "files": [{{"path": "{}", "autosave": false}}]}}"#, "p".repeat(10_000)),
        ] {
            std::fs::write(&store, hostile).unwrap();
            assert!(AutoSaveMemory::load(store.clone()).is_empty());
        }
        let mut m = AutoSaveMemory::load(dir.join("fresh.json"));
        for i in 0..MAX_ENTRIES + 5 {
            m.remember(&dir.join(format!("f{i}.pptx")), false, true);
        }
        assert_eq!(m.len(), MAX_ENTRIES);
        assert_eq!(m.get(&dir.join("f0.pptx")), None, "the oldest choice went first");
        assert_eq!(m.get(&dir.join(format!("f{}.pptx", MAX_ENTRIES + 4))), Some(false));
        assert_eq!(AutoSaveMemory::load(dir.join("fresh.json")).len(), MAX_ENTRIES);
        let _ = std::fs::remove_dir_all(dir);
    }
}
