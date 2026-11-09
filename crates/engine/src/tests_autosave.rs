//! AutoSave: the disk-stamp conflict guard, Save to Cloud, the switch remembered for each file,
//! and the schedule (settle wait, retries with a growing wait) driven by an injected clock.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::{Value, json};

use crate::autosave::{AutoSaveBlock, AutoSaveEvent, Flush, SETTLE_SECS, retry_after};
use crate::{DocState, Session};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("dc-engine-autosave-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Add `n` slides (an edit, and a size the saved file can be told apart by).
fn edit(s: &mut Session, n: usize) {
    for _ in 0..n {
        s.execute("slide.new", &json!({})).unwrap();
    }
}

fn slides(s: &Session) -> usize {
    s.doc().unwrap().doc.slides.len()
}

fn save_as(s: &mut Session, path: &std::path::Path) -> Value {
    s.execute("file.saveAs", &json!({"path": path.to_string_lossy()})).unwrap()
}

fn auto(s: &mut Session) -> Value {
    s.execute("file.save", &json!({"ifUnchanged": true})).unwrap()
}

fn cloud(s: &mut Session) -> Value {
    s.execute("file.cloud", &json!({})).unwrap()
}

/// Another device saves the file: AutoSave stops instead of overwriting it, and each way of
/// resolving the conflict keeps what it says.
#[test]
fn autosave_never_overwrites_a_file_changed_elsewhere() {
    let dir = scratch("conflict");
    let path = dir.join("Talk.pptx");
    let mut s = Session::with_new();
    save_as(&mut s, &path);

    // Unchanged on disk: AutoSave writes.
    edit(&mut s, 1);
    let r = auto(&mut s);
    assert_eq!(r["saved"], true);
    assert!(!s.doc().unwrap().is_dirty());

    // Another device's version arrives (more slides, so a different size).
    let mut other = Session::with_new();
    edit(&mut other, 4);
    save_as(&mut other, &path);
    let theirs = std::fs::read(&path).unwrap();

    edit(&mut s, 1);
    let r = auto(&mut s);
    assert_eq!(r["saved"], false);
    assert_eq!(r["conflict"], "changed");
    assert_eq!(std::fs::read(&path).unwrap(), theirs, "the other version is untouched");
    assert!(s.doc().unwrap().is_dirty());
    // It stays paused until resolved, even after more editing.
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["conflict"], "changed");
    assert_eq!(cloud(&mut s)["conflict"], "changed");
    assert_eq!(cloud(&mut s)["autosave"]["blocked"], "changed");
    assert_eq!(cloud(&mut s)["autosave"]["covers"], false);
    assert!(s.execute("file.autosave", &json!({"value": true})).is_err(), "can't be turned on while paused");

    // Keep both: this version goes beside the other, and editing continues there.
    let mine = slides(&s);
    let r = s.execute("file.resolveConflict", &json!({"keep": "both"})).unwrap();
    let copy = dir.join("Talk (conflict copy).pptx");
    assert_eq!(r["path"], copy.to_string_lossy().as_ref());
    assert_eq!(s.doc().unwrap().path.as_deref(), Some(copy.to_string_lossy().as_ref()));
    assert_eq!(std::fs::read(&path).unwrap(), theirs);
    assert!(cloud(&mut s)["conflict"].is_null());
    assert!(s.execute("file.resolveConflict", &json!({"keep": "mine"})).is_err(), "nothing is waiting now");
    assert_eq!(cloud(&mut s)["autosave"]["covers"], true, "the copy is covered");

    // Take theirs: the file is reopened as it is now.
    std::fs::copy(&path, &copy).unwrap();
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["conflict"], "changed");
    s.execute("file.resolveConflict", &json!({"keep": "theirs"})).unwrap();
    assert_eq!(slides(&s), slides_of(&theirs));
    assert!(slides(&s) != mine);
    assert!(!s.doc().unwrap().is_dirty());
    // …and AutoSave carries on from that version.
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["saved"], true);

    // Keep mine: overwrite.
    std::fs::write(&copy, b"not a presentation").unwrap();
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["conflict"], "changed");
    let expected = slides(&s);
    s.execute("file.resolveConflict", &json!({"keep": "mine"})).unwrap();
    assert_eq!(slides_of(&std::fs::read(&copy).unwrap()), expected);

    // Deleted or moved elsewhere: missing, and there is nothing to reopen.
    std::fs::remove_file(&copy).unwrap();
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["conflict"], "missing");
    assert!(!copy.exists(), "AutoSave doesn't bring back a file removed elsewhere");
    assert!(s.execute("file.resolveConflict", &json!({"keep": "theirs"})).is_err());
    assert!(s.execute("file.resolveConflict", &json!({"keep": "sideways"})).is_err());
    s.execute("file.resolveConflict", &json!({"keep": "mine"})).unwrap();
    assert!(copy.exists());
}

fn slides_of(bytes: &[u8]) -> usize {
    crate::cmd::file::open_presentation("x.pptx", bytes).unwrap().slides.len()
}

/// A plain Save is the person's own decision and still writes; only AutoSave asks first.
#[test]
fn explicit_save_and_opened_files() {
    let dir = scratch("explicit");
    let path = dir.join("a.pptx");
    let mut s = Session::with_new();
    edit(&mut s, 1);
    save_as(&mut s, &path);

    // A file opened here is stamped as read.
    let mut t = Session::new();
    t.execute("file.open", &json!({"path": path.to_string_lossy()})).unwrap();
    edit(&mut t, 2);
    assert_eq!(auto(&mut t)["saved"], true);
    // Now `s` is behind: its AutoSave stops, its explicit Save overwrites.
    edit(&mut s, 1);
    assert_eq!(auto(&mut s)["conflict"], "changed");
    assert_eq!(s.execute("file.save", &json!({})).unwrap()["saved"], true);
    assert!(cloud(&mut s)["conflict"].is_null());

    // A new presentation has no file to protect, and AutoSave never asks where to save.
    s.execute("file.new", &json!({})).unwrap();
    assert!(s.doc().unwrap().stamp.is_none());
    let r = auto(&mut s);
    assert_eq!((r["saved"].as_bool(), r["blocked"].as_str()), (Some(false), Some("unsaved")));
    assert!(s.ui_requests.is_empty(), "no file picker for AutoSave");
}

/// With no disk stamp (opened from bytes) AutoSave has nothing to check against, so it does not write.
#[test]
fn a_file_without_a_stamp_is_not_written_by_autosave() {
    let dir = scratch("nostamp");
    let path = dir.join("b.pptx");
    let mut s = Session::with_new();
    edit(&mut s, 1);
    save_as(&mut s, &path);
    let bytes = crate::cmd::file::save_bytes(&s.doc().unwrap().doc, "pptx").unwrap();
    let mut t = Session::new();
    t.execute("file.openBytes", &json!({"name": "b.pptx", "data": crate::cmd::base64_encode(&bytes)})).unwrap();
    assert_eq!(t.autosave_block(0), Some(AutoSaveBlock::Unsaved));
    // A recovered presentation has a path but no stamp.
    let mut d = DocState::new(Default::default(), Some(path.to_string_lossy().to_string()), "r".into());
    d.autosave = true;
    let i = t.add_document(d);
    assert_eq!(t.autosave_block(i), Some(AutoSaveBlock::NotSavedHere));
    edit(&mut t, 1);
    let r = auto(&mut t);
    assert_eq!(r["saved"], false);
    assert_eq!(r["blocked"], "notSavedHere");
    // An explicit save lifts the block.
    t.execute("file.save", &json!({})).unwrap();
    assert_eq!(t.autosave_block(i), None);
}

#[test]
fn only_formats_that_keep_everything_are_covered() {
    let dir = scratch("formats");
    let mut s = Session::with_new();
    edit(&mut s, 1);
    for (name, covered) in [("a.pptx", true), ("b.potx", true), ("c.ppsx", true), ("d.deckcraft", true), ("e.txt", false), ("f.rtf", false)] {
        let path = dir.join(name);
        s.execute("file.save", &json!({"path": path.to_string_lossy()})).unwrap();
        let block = s.autosave_block(0);
        assert_eq!(block.is_none(), covered, "{name}: {block:?}");
        if !covered {
            assert!(matches!(block, Some(AutoSaveBlock::Format { .. })));
        }
    }
}

#[test]
fn save_to_cloud_uses_the_providers_documents_folder() {
    use deckcraft_cloud::{CloudRoot, Provider};
    let dir = scratch("cloud");
    let icloud = dir.join("com~apple~CloudDocs");
    let onedrive = dir.join("OneDrive-Personal");
    std::fs::create_dir_all(icloud.join("Documents")).unwrap();
    std::fs::create_dir_all(&onedrive).unwrap();
    let mut s = Session::with_new();
    edit(&mut s, 1);

    // Headless sessions know no cloud folders until the host looks for them.
    assert!(s.execute("file.saveToCloud", &json!({})).is_err());

    s.cloud_roots = vec![
        CloudRoot { provider: Provider::ICloudDrive, path: icloud.clone(), account: None },
        CloudRoot { provider: Provider::OneDrive, path: onedrive.clone(), account: Some("Personal".into()) },
    ];
    s.doc_mut().unwrap().title = "Plan".into();
    let r = s.execute("file.saveToCloud", &json!({})).unwrap();
    assert_eq!(r["provider"], "icloud");
    let want = icloud.join("Documents").join("Plan.pptx");
    assert_eq!(s.doc().unwrap().path.as_deref(), Some(want.to_string_lossy().as_ref()));
    assert!(!s.doc().unwrap().is_dirty());
    let st = cloud(&mut s);
    assert_eq!((st["provider"].as_str(), st["name"].as_str()), (Some("icloud"), Some("iCloud Drive")));
    assert_eq!(st["roots"].as_array().unwrap().len(), 2);
    assert_eq!(st["autosave"]["covers"], true, "the new file is covered at once");

    // Named provider and name; a taken name is numbered.
    s.execute("file.saveToCloud", &json!({"provider": "onedrive", "name": "Plan"})).unwrap();
    s.execute("file.saveToCloud", &json!({"provider": "onedrive", "name": "Plan"})).unwrap();
    let want = onedrive.join("Plan 2.pptx");
    assert_eq!(s.doc().unwrap().path.as_deref(), Some(want.to_string_lossy().as_ref()));
    assert_eq!(cloud(&mut s)["provider"], "onedrive");
    assert!(s.execute("file.saveToCloud", &json!({"provider": "gdrive"})).is_err());
    assert!(s.execute("file.saveToCloud", &json!({"provider": "floppy"})).is_err());
    // Junk never panics, and a name with separators stays inside the provider's folder.
    for junk in [json!(null), json!({"provider": 5, "name": []}), json!({"name": "../../x"}), json!({"document": "a"})] {
        let _ = s.execute("file.saveToCloud", &junk);
        let _ = s.execute("file.resolveConflict", &junk);
        let _ = s.execute("file.cloud", &junk);
        let _ = s.execute("file.autosave", &junk);
    }
    let saved = std::path::PathBuf::from(s.doc().unwrap().path.clone().unwrap());
    assert!(saved.starts_with(&icloud) || saved.starts_with(&onedrive), "{saved:?}");
}

/// The switch is on by default, remembered for each file between runs, follows Save As, and the
/// preference sets the default for files with no choice of their own.
#[test]
fn the_switch_is_remembered_for_each_file() {
    let dir = scratch("memory");
    let store = dir.join("settings").join("autosave.json");
    let a = dir.join("a.pptx");
    let b = dir.join("b.pptx");
    let mut s = Session::with_new();
    s.enable_autosave_memory(store.clone());
    assert!(s.doc().unwrap().autosave, "on by default");
    edit(&mut s, 1);
    save_as(&mut s, &a);
    assert_eq!(s.execute("file.autosave", &json!({"value": false})).unwrap(), json!({"value": false}));
    assert_eq!(cloud(&mut s)["autosave"], json!({"on": false, "default": true, "remembered": false, "covers": false, "blocked": null}));
    // Save As: the new file gets the switch the presentation had.
    save_as(&mut s, &b);
    assert!(!s.doc().unwrap().autosave);

    // Another run opens a.pptx: remembered off; b.pptx too.
    let mut t = Session::new();
    t.enable_autosave_memory(store.clone());
    t.execute("file.open", &json!({"path": a.to_string_lossy()})).unwrap();
    assert!(!t.doc().unwrap().autosave, "remembered off for a.pptx");
    t.execute("file.new", &json!({})).unwrap();
    assert!(t.doc().unwrap().autosave, "a new presentation has the default");
    // Turning it back on forgets the choice.
    t.set_active_by_path(&a);
    t.execute("file.autosave", &json!({"value": true})).unwrap();
    assert_eq!(cloud(&mut t)["autosave"]["remembered"], Value::Null);

    // The default off: files with no choice come off, a file turned on stays on and is remembered.
    let mut u = Session::new();
    u.prefs.autosave_by_default = false;
    u.enable_autosave_memory(store.clone());
    u.execute("file.open", &json!({"path": b.to_string_lossy()})).unwrap();
    assert!(!u.doc().unwrap().autosave);
    u.execute("file.autosave", &json!({"value": true})).unwrap();
    let mut v = Session::new();
    v.prefs.autosave_by_default = false;
    v.enable_autosave_memory(store.clone());
    v.execute("file.open", &json!({"path": b.to_string_lossy()})).unwrap();
    assert!(v.doc().unwrap().autosave, "turned on with the default off, and remembered");
    // Changing the default moves the files that follow it.
    v.set_autosave_by_default(true);
    assert!(v.doc().unwrap().autosave);
    v.set_autosave_by_default(false);
    assert!(v.doc().unwrap().autosave, "this file has a choice of its own");
}

impl Session {
    fn set_active_by_path(&mut self, path: &std::path::Path) {
        let want = path.to_string_lossy();
        let i = self.documents().iter().position(|d| d.path.as_deref() == Some(want.as_ref())).unwrap();
        self.set_active(i);
    }
}

#[test]
fn retry_waits_double_up_to_a_minute() {
    let waits: Vec<f64> = (1..=8).map(retry_after).collect();
    assert_eq!(waits, [5.0, 10.0, 20.0, 40.0, 60.0, 60.0, 60.0, 60.0]);
    assert_eq!(retry_after(0), 5.0);
    assert_eq!(retry_after(u32::MAX), 60.0);
}

/// Make writing `path` fail without touching its bytes or date: a read-only file where the rename
/// over it is refused (Windows), a read-only folder (elsewhere).
fn lock(path: &std::path::Path) {
    if cfg!(windows) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(path, p).unwrap();
    } else {
        set_mode(path.parent().unwrap(), 0o555);
    }
}

fn unlock(path: &std::path::Path) {
    if cfg!(windows) {
        let mut p = std::fs::metadata(path).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        std::fs::set_permissions(path, p).unwrap();
    } else {
        set_mode(path.parent().unwrap(), 0o755);
    }
}

#[cfg(unix)]
fn set_mode(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_: &std::path::Path, _: u32) {}

/// The schedule on an injected clock: AutoSave waits for the presentation to settle, a change
/// restarts the wait, a failed write is retried after 5 s, 10 s, 20 s…, and the first success
/// ends it. The switch and the file's remembered choice never change.
#[test]
fn the_schedule_settles_then_retries_with_backoff() {
    let dir = scratch("schedule");
    let store = dir.join("autosave.json");
    let path = dir.join("slow.pptx");
    let mut s = Session::with_new();
    s.enable_autosave_memory(store.clone());
    save_as(&mut s, &path);

    // Nothing unsaved: nothing to do.
    assert!(s.autosave_tick(0.0).is_empty());
    assert_eq!(s.autosave_next_due(), None);

    // A change seen at t=10 is written once it has stayed still for the settle wait.
    edit(&mut s, 1);
    assert!(s.autosave_tick(10.0).is_empty());
    assert_eq!(s.autosave_next_due(), Some(10.0 + SETTLE_SECS));
    assert!(s.autosave_tick(10.0 + SETTLE_SECS - 0.1).is_empty());
    // Another change restarts the wait.
    edit(&mut s, 1);
    assert!(s.autosave_tick(12.0).is_empty());
    assert!(s.autosave_tick(12.0 + SETTLE_SECS - 0.1).is_empty());
    let uid = s.doc().unwrap().uid;
    assert_eq!(s.autosave_tick(12.0 + SETTLE_SECS), [AutoSaveEvent::Saved { uid }]);
    assert!(!s.doc().unwrap().is_dirty());
    assert_eq!(slides_of(&std::fs::read(&path).unwrap()), 1 + 2);

    // The file can no longer be written: every try fails, each wait twice the last up to a minute.
    edit(&mut s, 1);
    let mut now = 100.0;
    assert!(s.autosave_tick(now).is_empty());
    now += SETTLE_SECS;
    lock(&path);
    let mut waits = vec![];
    for n in 1..=6 {
        let events = s.autosave_tick(now);
        let [AutoSaveEvent::Failed { retry_in, failures, .. }] = events.as_slice() else { panic!("{events:?}") };
        assert_eq!(*failures, n);
        waits.push(*retry_in);
        // Nothing is tried before the retry is due, and the due time is known.
        assert!(s.autosave_tick(now + retry_in - 0.5).is_empty());
        assert_eq!(s.autosave_next_due(), Some(now + retry_in));
        now += retry_in;
    }
    assert_eq!(waits, [5.0, 10.0, 20.0, 40.0, 60.0, 60.0]);
    assert!(s.autosave_failure(0).is_some_and(|f| f.failures == 6 && !f.error.is_empty()));
    assert!(s.doc().unwrap().autosave, "the switch stays on while writes fail");
    assert_eq!(cloud(&mut s)["autosave"]["remembered"], Value::Null, "the file's remembered choice does not change");
    assert!(s.doc().unwrap().is_dirty());

    // The next try after the file is free again writes it and ends the failure.
    unlock(&path);
    assert_eq!(s.autosave_tick(now), [AutoSaveEvent::Saved { uid }]);
    assert_eq!(s.autosave_failure(0), None);
    assert!(!s.doc().unwrap().is_dirty());
}

/// Many writes: a conflict found by the schedule is reported once and then waits for the user.
#[test]
fn the_schedule_reports_a_conflict_once() {
    let dir = scratch("sched-conflict");
    let path = dir.join("c.pptx");
    let mut s = Session::with_new();
    save_as(&mut s, &path);
    let mut other = Session::with_new();
    edit(&mut other, 5);
    save_as(&mut other, &path);
    edit(&mut s, 1);
    assert!(s.autosave_tick(0.0).is_empty());
    let events = s.autosave_tick(SETTLE_SECS + 0.1);
    let uid = s.doc().unwrap().uid;
    assert_eq!(events, [AutoSaveEvent::Conflict { uid, missing: false }]);
    // Blocked: later ticks do nothing, and nothing was written.
    assert!(s.autosave_tick(100.0).is_empty());
    assert!(s.autosave_tick(200.0).is_empty());
    assert_eq!(s.autosave_block(0), Some(AutoSaveBlock::Conflict { missing: false }));
}

/// Closing flushes the pending changes first, so no Save Changes prompt is needed.
#[test]
fn flush_writes_at_once_and_reports_conflicts() {
    let dir = scratch("flush");
    let path = dir.join("f.pptx");
    let mut s = Session::with_new();
    // Never saved: not covered, the prompt stays.
    edit(&mut s, 1);
    assert_eq!(s.autosave_flush(0), Flush::NotCovered);
    save_as(&mut s, &path);
    edit(&mut s, 1);
    assert_eq!(s.autosave_flush(0), Flush::Saved);
    assert!(!s.doc().unwrap().is_dirty());
    // Clean: nothing to flush.
    assert_eq!(s.autosave_flush(0), Flush::NotCovered);
    // Switched off for the file: the prompt stays.
    edit(&mut s, 1);
    s.execute("file.autosave", &json!({"value": false})).unwrap();
    assert_eq!(s.autosave_flush(0), Flush::NotCovered);
    assert!(s.doc().unwrap().is_dirty());
    s.execute("file.autosave", &json!({"value": true})).unwrap();
    // Another device changed the file: a conflict, and the file is untouched.
    let mut other = Session::with_new();
    edit(&mut other, 6);
    save_as(&mut other, &path);
    let theirs = std::fs::read(&path).unwrap();
    assert_eq!(s.autosave_flush(0), Flush::Conflict);
    assert_eq!(std::fs::read(&path).unwrap(), theirs);
    assert_eq!(s.autosave_flush(0), Flush::Conflict, "still waiting");
}

/// AutoSave writes a presentation that is not the active one without making it active, and a
/// save names its presentation by uid.
#[test]
fn a_background_presentation_is_saved_in_place() {
    let dir = scratch("background");
    let (pa, pb) = (dir.join("a.pptx"), dir.join("b.pptx"));
    let mut s = Session::with_new();
    save_as(&mut s, &pa);
    s.execute("file.new", &json!({})).unwrap();
    edit(&mut s, 2);
    save_as(&mut s, &pb);
    s.set_active(0);
    let uid_a = s.doc().unwrap().uid;
    // Dirty b while a is active.
    s.set_active(1);
    edit(&mut s, 1);
    s.set_active(0);
    assert_eq!(s.autosave_flush(1), Flush::Saved);
    assert_eq!(s.doc().unwrap().uid, uid_a, "the active presentation did not change");
    assert!(!s.documents()[1].is_dirty());
    assert_eq!(slides_of(&std::fs::read(&pb).unwrap()), 1 + 2 + 1);
    assert!(s.execute("file.save", &json!({"document": 12345})).is_err());
}

/// The AutoRecover entry of a presentation goes when AutoSave writes it, as it does for a save.
#[test]
fn autosave_clears_the_recovery_entry() {
    let dir = scratch("recovery");
    let path = dir.join("r.pptx");
    let mut s = Session::with_new();
    s.recovery_dir = Some(dir.join("recovery"));
    save_as(&mut s, &path);
    edit(&mut s, 1);
    s.execute("file.recovery.save", &json!({})).unwrap();
    assert_eq!(crate::recovery::list(&dir.join("recovery")).len(), 1);
    assert_eq!(auto(&mut s)["saved"], true);
    assert!(crate::recovery::list(&dir.join("recovery")).is_empty());
}
