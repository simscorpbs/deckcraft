//! Tests of the version history: the store (ported from WordCraft's version-store tests, with
//! `.pptx` packages as fixtures) and the save paths that feed it.

#![cfg(not(target_arch = "wasm32"))]

use super::*;
use crate::Session;
use serde_json::json;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn canonical_key_is_stable_and_bounded() {
    let path = Path::new("relative/notes.pptx");
    assert_eq!(document_key(path), document_key(path));
    assert_eq!(document_key(path).len(), 16);
}

#[test]
fn iso_parser_accepts_engine_timestamps() {
    let now = 1_800_000_000;
    let value = iso_from_unix_secs(now);
    assert_eq!(parse_iso_secs(&value), Some(now));
}

fn temp(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("deckcraft-versions-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

fn put(root: &Path, file: &Path, bytes: &[u8], kind: VersionKind, when: u64, now: u64, session: Option<&str>) -> Vec<VersionInfo> {
    store_save_at(
        &[root.to_path_buf()],
        VersionWrite {
            path: file.to_path_buf(),
            bytes: bytes.to_vec(),
            author: "Tester".into(),
            kind,
            saved_at: iso_from_unix_secs(when),
            session: session.map(str::to_owned),
            document_id: None,
            details: VersionDetails { trigger: "save".into(), ..VersionDetails::default() },
            cloud: Vec::new(),
            now: None,
        },
        now,
    )
    .unwrap()
    .0
}

fn write(root: &Path, file: &Path, text: &str, kind: VersionKind, when: u64, now: u64) -> Vec<VersionInfo> {
    put(root, file, text.as_bytes(), kind, when, now, Some("run-1"))
}

fn save_bytes_at(root: &Path, file: &Path, bytes: &[u8], when: u64) -> Vec<VersionInfo> {
    put(root, file, bytes, VersionKind::Save, when, when, None)
}

/// Two zip packages with the same parts: names, order, methods, dates and contents.
fn same_parts(a: &[u8], b: &[u8]) -> bool {
    let parts = |bytes: &[u8]| pack::split_zip(bytes).map(|(parts, comment)| (parts.into_iter().map(|p| p.meta).collect::<Vec<_>>(), comment));
    parts(a).is_some() && parts(a) == parts(b)
}

fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// The body of a document with `paragraphs` paragraphs, the `edit`-th one changed.
fn body_xml(paragraphs: usize, edit: usize) -> Vec<u8> {
    let mut xml = String::from("<?xml version=\"1.0\"?><w:document><w:body>");
    for i in 0..paragraphs {
        let words = if i == edit % paragraphs.max(1) {
            format!("Paragraph {i} as revised in edit {edit}, with a new sentence.")
        } else {
            format!("Paragraph {i} of the report says what it says.")
        };
        xml.push_str(&format!(
            "<w:p><w:pPr><w:pStyle w:val=\"Normal\"/></w:pPr><w:r><w:rPr><w:lang w:val=\"en-US\"/></w:rPr><w:t>{words}</w:t></w:r></w:p>"
        ));
    }
    xml.push_str("</w:body></w:document>");
    xml.into_bytes()
}

/// A body of varied prose (it compresses like real text, not like a repeated line).
fn varied_body_xml(paragraphs: usize, edit: usize) -> Vec<u8> {
    const WORDS: &[&str] = &[
        "the",
        "court",
        "found",
        "that",
        "contract",
        "was",
        "signed",
        "after",
        "notice",
        "parties",
        "agreed",
        "payment",
        "within",
        "thirty",
        "days",
        "account",
        "access",
        "restored",
        "evidence",
        "shows",
        "request",
        "denied",
        "claim",
        "damages",
        "moral",
        "platform",
        "user",
        "terms",
        "breach",
        "hearing",
        "judge",
        "appeal",
        "record",
        "filed",
        "deadline",
        "expert",
        "report",
        "witness",
        "statement",
        "judgment",
    ];
    let mut xml = String::from("<?xml version=\"1.0\"?><w:document><w:body>");
    for i in 0..paragraphs {
        let seed = if i == edit % paragraphs.max(1) { (i as u64) << 20 | edit as u64 } else { i as u64 };
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut words = Vec::new();
        for _ in 0..(12 + (seed % 30)) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            words.push(WORDS[(state % WORDS.len() as u64) as usize]);
        }
        xml.push_str(&format!("<w:p><w:pPr><w:pStyle w:val=\"Normal\"/></w:pPr><w:r><w:t>{}.</w:t></w:r></w:p>", words.join(" ")));
    }
    xml.push_str("</w:body></w:document>");
    xml.into_bytes()
}

/// A `.pptx`-shaped package: a deflated body and styles and a stored picture, like Word's.
fn package(body: &[u8], picture: &[u8], extra: Option<(&str, &[u8])>) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut part = |name: &str, data: &[u8], options| {
        writer.start_file(name, options).unwrap();
        writer.write_all(data).unwrap();
    };
    part("[Content_Types].xml", b"<Types><Default Extension=\"xml\"/><Default Extension=\"png\"/></Types>", deflated);
    part("ppt/slides/slide1.xml", body, deflated);
    part("ppt/presProps.xml", "<w:styles>".repeat(2_000).as_bytes(), deflated);
    part("ppt/media/image1.png", picture, stored);
    if let Some((name, data)) = extra {
        part(name, data, deflated);
    }
    writer.finish().unwrap().into_inner()
}

fn read_at(root: &Path, file: &Path, id: &str) -> Result<Vec<u8>, String> {
    let dir = document_dir(root, file);
    let dirs = [dir.clone()];
    match read_index(&dir).into_iter().find(|v| v.id == id) {
        Some(info) if info.storage == VersionStorage::Parts => read_parts_version(&dirs, id),
        Some(info) => read_whole_version(&dirs, &info),
        None => Err("no such version".into()),
    }
}

fn objects_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.join(OBJECTS_DIR))
        .map(|entries| entries.flatten().filter_map(|e| e.file_name().to_str().map(str::to_owned)).collect())
        .unwrap_or_default();
    names.sort();
    names
}

fn disk_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let meta = entry.metadata().unwrap();
        total += if meta.is_dir() { disk_bytes(&entry.path()) } else { meta.len() };
    }
    total
}

/// Every version record (manifest or metadata file) under `dir`, as text.
fn records(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(records(&path));
            } else if path.to_string_lossy().ends_with(".json") {
                out.push(fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    out
}

// ---- cadence and retention --------------------------------------------------------------

#[test]
fn autosaves_join_within_ten_minutes_but_not_after() {
    let root = temp("join");
    let file = root.join("note.txt");
    let first = write(&root, &file, "one", VersionKind::Autosave, 1_800_000_000, 1_800_000_000);
    let second = write(&root, &file, "two", VersionKind::Autosave, 1_800_000_599, 1_800_000_599);
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].size, 3);
    assert_eq!(second[0].supersedes.as_deref(), Some(first[0].id.as_str()), "a join writes a new record, never rewrites one");
    // The window is counted from its first write, not the latest: continuous editing still
    // leaves a version every ten minutes.
    let third = write(&root, &file, "three", VersionKind::Autosave, 1_800_000_601, 1_800_000_601);
    assert_eq!(third.len(), 2, "ten minutes after the window opened, a new version starts");
    let fourth = write(&root, &file, "four", VersionKind::Autosave, 1_800_001_200, 1_800_001_200);
    assert_eq!(fourth.len(), 2, "joins the window that opened at the third write");
    assert_eq!(fourth[0].size, 4);
    let fifth = write(&root, &file, "five", VersionKind::Autosave, 1_800_000_601 + AUTOSAVE_WINDOW_SECS, 1_800_000_601 + AUTOSAVE_WINDOW_SECS);
    assert_eq!(fifth.len(), 3);
    // An explicit save is always its own version.
    let saved = write(&root, &file, "saved", VersionKind::Save, 1_800_001_202, 1_800_001_202);
    assert_eq!(saved.len(), 4);
    // Superseded records are gone from the folder too.
    let dir = document_dir(&root, &file);
    assert_eq!(read_index(&dir).len(), 4);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn first_autosave_after_reopening_starts_a_new_version() {
    let root = temp("reopen");
    let file = root.join("note.txt");
    let now = 1_800_000_000;
    assert_eq!(put(&root, &file, b"first run", VersionKind::Autosave, now, now, Some("run-1")).len(), 1);
    let list = put(&root, &file, b"second run", VersionKind::Autosave, now + 120, now + 120, Some("run-2"));
    assert_eq!(list.len(), 2, "a new editing session never overwrites the previous session's version");
    assert_eq!(list[1].size, "first run".len() as u64);
    assert_eq!(put(&root, &file, b"second run, more", VersionKind::Autosave, now + 180, now + 180, Some("run-2")).len(), 2);
    assert_eq!(put(&root, &file, b"kept", VersionKind::Autosave, now + 200, now + 200, None).len(), 3);
    assert_eq!(put(&root, &file, b"after", VersionKind::Autosave, now + 210, now + 210, Some("run-2")).len(), 4);
    let _ = fs::remove_dir_all(root);
}

fn entry(n: usize, at: u64, kind: VersionKind) -> VersionInfo {
    VersionInfo { id: format!("{n:032x}"), saved_at: iso_from_unix_secs(at), author: "Tester".into(), size: 1, kind, ..VersionInfo::default() }
}

#[test]
fn pruning_follows_the_automatic_schedule() {
    let root = temp("prune");
    let day = DAY_SECS;
    let now: u64 = 1_800_000_000 / day * day;
    let mut entries = Vec::new();
    let mut n = 0usize;
    let mut add = |entries: &mut Vec<VersionInfo>, at: u64| {
        n += 1;
        entries.push(entry(n, at, VersionKind::Save));
        n
    };
    let recent: Vec<usize> = (0..5).map(|i| add(&mut entries, now - 29 * day + i)).collect();
    let hour_a_old = add(&mut entries, now - 40 * day + 60);
    let hour_a_new = add(&mut entries, now - 40 * day + 120);
    let hour_b = add(&mut entries, now - 40 * day + 3_600 + 60);
    let day_old = add(&mut entries, now - 100 * day + 3_600);
    let day_new = add(&mut entries, now - 100 * day + 7_200);
    let next_day = add(&mut entries, now - 99 * day + 3_600);
    let week = (((now - 400 * day) / day + 3) / 7 * 7 - 3) * day;
    let week_old = add(&mut entries, week + 3_600);
    let week_new = add(&mut entries, week + day);
    let ancient = add(&mut entries, now - 3_000 * day);
    let (kept, removed) = prune(&root, entries, now);
    let kept: BTreeSet<String> = kept.into_iter().map(|v| v.id).collect();
    let has = |n: usize| kept.contains(&format!("{n:032x}"));
    assert!(recent.iter().all(|&n| has(n)), "every version of the last 30 days stays");
    assert!(has(hour_a_new) && !has(hour_a_old) && has(hour_b), "one per hour between 30 and 60 days");
    assert!(has(day_new) && !has(day_old) && has(next_day), "one per day between 60 and 180 days");
    assert!(has(week_new) && !has(week_old), "one per week after 180 days");
    assert!(has(ancient), "a document left alone keeps its old history");
    assert!(removed.contains(&format!("{hour_a_old:032x}")));
    let many: Vec<VersionInfo> = (0..MAX_VERSIONS + 20).map(|i| entry(i + 1, now - u64::try_from(i).unwrap(), VersionKind::Save)).collect();
    let (kept, _) = prune(&root, many, now);
    assert_eq!(kept.len(), MAX_VERSIONS);
    let large: Vec<VersionInfo> =
        (0..100).map(|i| VersionInfo { size: 6 << 20, ..entry(i + 1, now - u64::try_from(i).unwrap(), VersionKind::Save) }).collect();
    let (kept, _) = prune(&root, large, now);
    assert!(kept.iter().map(|v| v.size).sum::<u64>() <= MAX_DOCUMENT_BYTES);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_first_original_is_pinned_past_every_limit() {
    let root = temp("pinned");
    let now: u64 = 1_800_000_000;
    let mut list = vec![VersionInfo { size: 10 << 20, ..entry(1, now - 5_000 * DAY_SECS, VersionKind::Original) }];
    list.extend((0..MAX_VERSIONS + 10).map(|i| VersionInfo { size: 1 << 20, ..entry(i + 2, now - u64::try_from(i).unwrap(), VersionKind::Save) }));
    let (kept, _) = prune(&root, list, now);
    assert!(kept.len() <= MAX_VERSIONS);
    assert_eq!(kept.last().map(|v| v.kind), Some(VersionKind::Original));
    let _ = fs::remove_dir_all(root);
}

// ---- damaged, foreign and legacy files ---------------------------------------------------

#[test]
fn records_from_earlier_builds_migrate_and_damaged_files_cost_nothing() {
    let root = temp("legacy-index");
    let file = root.join("note.txt");
    let dir = document_dir(&root, &file);
    fs::create_dir_all(&dir).unwrap();
    // An earlier build's folder: `.bin` files and one shared index.json.
    let ids = ["0123456789abcdef0123456789abcdef", "fedcba9876543210fedcba9876543210"];
    let mut legacy = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        fs::write(dir.join(format!("{id}.bin")), format!("old {i}")).unwrap();
        legacy.push(json!({"id": id, "savedAt": iso_from_unix_secs(1_800_000_000 + i as u64), "author": "Tester", "size": 5, "kind": "save"}));
    }
    legacy.push(json!({"id": 7, "kind": "future"}));
    fs::write(dir.join(INDEX_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(read_index(&dir).len(), 2, "read as it is, one bad entry costing only itself");
    let list = write(&root, &file, "new", VersionKind::Save, 1_800_000_100, 1_800_000_100);
    assert_eq!(list.len(), 3);
    assert!(!dir.join(INDEX_FILE).exists(), "the shared index is gone once each version has its own record");
    assert!(ids.iter().all(|id| dir.join(format!("{id}{META_SUFFIX}")).is_file()));
    // A damaged record or a version file nothing describes never costs the bytes.
    fs::write(dir.join(format!("{}{META_SUFFIX}", ids[0])), b"{ damaged").unwrap();
    let recovered = read_index(&dir);
    assert_eq!(recovered.len(), 3);
    assert!(dir.join(format!("{}.bin", ids[0])).is_file());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_sync_apps_conflict_copies_are_ignored_and_left_alone() {
    let root = temp("conflicts");
    let file = root.join("Shared.pptx");
    let picture = noise(2_000, 3);
    let list = save_bytes_at(&root, &file, &package(&body_xml(50, 0), &picture, None), 1_800_000_000);
    let dir = document_dir(&root, &file);
    let id = list[0].id.clone();
    // What OneDrive, Dropbox and iCloud leave beside a file both computers changed.
    let strays = [
        format!("{id} (1){PARTS_SUFFIX}"),
        format!("{id}-DESKTOP-4F2K9{PARTS_SUFFIX}"),
        format!("{id} (conflicted copy){META_SUFFIX}"),
        "index (1).json".to_owned(),
        "document-LAPTOP.json".to_owned(),
    ];
    for stray in &strays {
        fs::copy(manifest_path(&dir, &id), dir.join(stray)).unwrap();
    }
    let object = objects_in(&dir)[0].clone();
    fs::copy(object_path(&dir, &object), dir.join(OBJECTS_DIR).join(format!("{object} (1)"))).unwrap();
    assert_eq!(visible(&read_index(&dir)).len(), 1);
    let list = save_bytes_at(&root, &file, &package(&body_xml(50, 1), &picture, None), 1_800_000_060);
    assert_eq!(list.len(), 2);
    assert!(strays.iter().all(|stray| dir.join(stray).is_file()), "never deleted on a guess");
    assert!(dir.join(OBJECTS_DIR).join(format!("{object} (1)")).is_file());
    assert!(same_parts(&read_at(&root, &file, &id).unwrap(), &package(&body_xml(50, 0), &picture, None)));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn damaged_version_bytes_are_refused() {
    let root = temp("checksum");
    let file = root.join("note.txt");
    let list = write(&root, &file, "good", VersionKind::Save, 1_800_000_000, 1_800_000_000);
    let id = list[0].id.clone();
    fs::write(document_dir(&root, &file).join(format!("{id}.bin")), b"evil").unwrap();
    let store = Versions::start(root.clone(), Vec::new(), std::sync::Arc::new(|| {})).unwrap();
    assert!(store.read(&file, None, &id).is_err(), "same size, different bytes");
    drop(store);
    let _ = fs::remove_dir_all(root);
}

// ---- folders --------------------------------------------------------------------------------

#[test]
fn a_mirror_that_is_away_is_brought_up_to_date_when_it_returns() {
    let root = temp("mirror");
    let local = root.join("local");
    let away = root.join("usb");
    fs::write(&away, b"the drive is not there").unwrap();
    let file = root.join("Mirror.pptx");
    let picture = noise(2_000, 5);
    let roots = vec![local.clone(), away.clone()];
    let save = |edit: usize, when: u64| {
        store_save_at(
            &roots,
            VersionWrite {
                path: file.clone(),
                bytes: package(&body_xml(80, edit), &picture, None),
                author: "Tester".into(),
                kind: VersionKind::Save,
                saved_at: iso_from_unix_secs(when),
                session: None,
                document_id: None,
                details: VersionDetails::default(),
                cloud: Vec::new(),
                now: None,
            },
            when,
        )
    };
    let (list, unavailable) = save(0, 1_800_000_000).unwrap();
    assert_eq!(list.len(), 1, "the save goes on without the mirror");
    assert_eq!(unavailable, vec![away.clone()]);
    // The drive comes back.
    fs::remove_file(&away).unwrap();
    fs::create_dir_all(&away).unwrap();
    let (list, unavailable) = save(1, 1_800_000_060).unwrap();
    assert!(unavailable.is_empty());
    let (a, b) = (document_dir(&local, &file), document_dir(&away, &file));
    assert_eq!(visible(&read_index(&b)).len(), 2, "the version it missed was copied over");
    assert_eq!(objects_in(&a), objects_in(&b));
    // Either folder alone gives every version back.
    fs::remove_dir_all(&a).unwrap();
    for v in &list {
        assert!(read_parts_version(&[a.clone(), b.clone()], &v.id).is_ok());
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn two_computers_writing_to_one_synced_folder_never_undo_each_other() {
    let root = temp("two-computers");
    let shared = root.join("OneDrive-history");
    let file = root.join("Joint.pptx");
    let picture = noise(3_000, 9);
    let id = meta::new_document_id().unwrap();
    let write_from = |device: &str, edit: usize, when: u64| {
        store_save_at(
            std::slice::from_ref(&shared),
            VersionWrite {
                path: file.clone(),
                bytes: package(&body_xml(60, edit), &picture, None),
                author: device.into(),
                kind: VersionKind::Autosave,
                saved_at: iso_from_unix_secs(when),
                session: Some(device.into()),
                document_id: Some(id.clone()),
                details: VersionDetails { device: device.into(), ..VersionDetails::default() },
                cloud: Vec::new(),
                now: None,
            },
            when,
        )
        .unwrap()
        .0
    };
    let now = 1_800_000_000;
    write_from("Windows", 0, now);
    write_from("Mac", 1, now + 30);
    write_from("Windows", 2, now + 60);
    let list = write_from("Mac", 3, now + 90);
    assert_eq!(list.len(), 4, "each computer's session keeps its own versions");
    let dir = shared.join(meta::id_bucket(&id));
    assert!(!dir.join(INDEX_FILE).exists(), "no shared index to fight over");
    // An object that arrived before its manifest (the sync app is behind) is not deleted.
    let late = noise(4_000, 77);
    let late_hash = digest_hex(&late);
    fs::write(object_path(&dir, &late_hash), pack::encode_full(&late)).unwrap();
    let kept = visible(&read_index(&dir));
    collect_objects(&dir, &kept, now + 100);
    assert!(object_path(&dir, &late_hash).is_file(), "marked, not deleted");
    collect_objects(&dir, &kept, now + 100 + 6 * DAY_SECS);
    assert!(object_path(&dir, &late_hash).is_file(), "still inside the grace period");
    // Its manifest arrives: the mark goes and the object stays for good.
    let mut info = entry(99, now + 3 * DAY_SECS, VersionKind::Save);
    info.size = 1;
    let manifest = pack::Manifest {
        format: 1,
        info: VersionInfo { storage: VersionStorage::Parts, ..info.clone() },
        comment: String::new(),
        parts: vec![pack::ManifestPart {
            name: "late.bin".into(),
            method: pack::Method::Stored,
            date: 0,
            time: 0,
            mode: None,
            dir: false,
            size: late.len() as u64,
            sha256: late_hash.clone(),
        }],
    };
    fs::write(manifest_path(&dir, &info.id), serde_json::to_vec(&manifest).unwrap()).unwrap();
    let kept = visible(&read_index(&dir));
    collect_objects(&dir, &kept, now + 100 + 8 * DAY_SECS);
    assert!(object_path(&dir, &late_hash).is_file());
    // Without one, it goes after the grace period.
    fs::remove_file(manifest_path(&dir, &info.id)).unwrap();
    let kept = visible(&read_index(&dir));
    collect_objects(&dir, &kept, now + 100 + 9 * DAY_SECS);
    collect_objects(&dir, &kept, now + 100 + 17 * DAY_SECS);
    assert!(!object_path(&dir, &late_hash).exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
#[test]
fn windows_keys_ignore_case_and_older_folders_move_over() {
    let root = temp("case");
    let file = root.join("Case.rtf");
    fs::write(&file, b"x").unwrap();
    let upper = root.join("CASE.RTF");
    assert_eq!(document_key(&file), document_key(&upper));
    let versions = root.join("versions");
    let legacy = versions.join(legacy_key(&file));
    assert_ne!(legacy, versions.join(document_key(&file)));
    let list = write(&versions, &file, "kept", VersionKind::Save, 1_800_000_000, 1_800_000_000);
    fs::rename(document_dir(&versions, &file), &legacy).unwrap();
    assert_eq!(
        visible(&read_index(&document_dir(&versions, &upper))).iter().map(|v| v.id.clone()).collect::<Vec<_>>(),
        list.iter().map(|v| v.id.clone()).collect::<Vec<_>>()
    );
    assert!(!legacy.exists());
    let _ = fs::remove_dir_all(root);
}

// ---- parts and deltas --------------------------------------------------------------------

#[test]
fn a_picture_is_stored_once_and_edits_are_small_deltas() {
    let root = temp("dedupe");
    let file = root.join("Report.pptx");
    let picture = noise(300_000, 7);
    let now = 1_800_000_000;
    let first = package(&body_xml(400, 0), &picture, None);
    let list = save_bytes_at(&root, &file, &first, now);
    assert_eq!(list[0].storage, VersionStorage::Parts);
    let first_stored = list[0].stored.unwrap();
    let second = package(&body_xml(400, 1), &picture, None);
    let list = save_bytes_at(&root, &file, &second, now + 60);
    let second_stored = list[0].stored.unwrap();
    assert!(first_stored > picture.len() as u64);
    assert!(second_stored < 5_000, "an edit costs a small delta, not the picture again: {second_stored} bytes");
    let dir = document_dir(&root, &file);
    assert_eq!(object_header(&dir, &digest_hex(&picture)).unwrap().0.depth, 0);
    assert!(same_parts(&read_at(&root, &file, &list[1].id).unwrap(), &first));
    assert!(same_parts(&read_at(&root, &file, &list[0].id).unwrap(), &second));
    assert_eq!(list[0].size, second.len() as u64);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn changed_parts_chain_deltas_and_start_again_whole() {
    let root = temp("chain");
    let file = root.join("Chain.pptx");
    let picture = noise(1_000, 9);
    let now = 1_800_000_000;
    let count = usize::from(pack::KEYFRAME_INTERVAL) + 3;
    let packages: Vec<Vec<u8>> = (0..count).map(|edit| package(&body_xml(200, edit), &picture, None)).collect();
    for (i, bytes) in packages.iter().enumerate() {
        let _ = save_bytes_at(&root, &file, bytes, now + i as u64);
    }
    let dir = document_dir(&root, &file);
    let depths: Vec<u16> = packages
        .iter()
        .map(|bytes| {
            let (parts, _) = pack::split_zip(bytes).unwrap();
            let body = parts.iter().find(|p| p.meta.name == "ppt/slides/slide1.xml").unwrap();
            object_header(&dir, &body.meta.sha256).unwrap().0.depth
        })
        .collect();
    let interval = pack::KEYFRAME_INTERVAL;
    let mut expected: Vec<u16> = (0..=interval).collect();
    expected.extend(0..u16::try_from(count).unwrap() - interval - 1);
    assert_eq!(depths, expected);
    let list = visible(&read_index(&dir));
    for (entry, bytes) in list.iter().rev().zip(&packages) {
        assert!(same_parts(&read_at(&root, &file, &entry.id).unwrap(), bytes));
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_damaged_part_only_affects_the_versions_that_need_it() {
    let root = temp("damaged-part");
    let file = root.join("Fragile.pptx");
    let picture = noise(2_000, 11);
    let now = 1_800_000_000;
    let packages: Vec<Vec<u8>> = (0..4).map(|edit| package(&body_xml(150, edit), &picture, None)).collect();
    for (i, bytes) in packages.iter().enumerate() {
        let _ = save_bytes_at(&root, &file, bytes, now + i as u64);
    }
    let dir = document_dir(&root, &file);
    let list = visible(&read_index(&dir));
    let body_of = |bytes: &[u8]| pack::split_zip(bytes).unwrap().0.into_iter().find(|p| p.meta.name == "ppt/slides/slide1.xml").unwrap().meta.sha256;
    let third = object_path(&dir, &body_of(&packages[2]));
    let mut bytes = fs::read(&third).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x55;
    fs::write(&third, bytes).unwrap();
    assert!(same_parts(&read_at(&root, &file, &list[3].id).unwrap(), &packages[0]));
    assert!(same_parts(&read_at(&root, &file, &list[2].id).unwrap(), &packages[1]));
    assert!(read_at(&root, &file, &list[1].id).is_err());
    assert!(read_at(&root, &file, &list[0].id).is_err());
    fs::remove_file(object_path(&dir, &digest_hex(&picture))).unwrap();
    assert!(read_at(&root, &file, &list[3].id).is_err());
    let _ = save_bytes_at(&root, &file, &packages[2], now + 10);
    assert!(same_parts(&read_at(&root, &file, &list[1].id).unwrap(), &packages[2]));
    assert!(same_parts(&read_at(&root, &file, &list[0].id).unwrap(), &packages[3]));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unused_parts_are_collected_after_the_grace_period_but_delta_bases_stay() {
    let root = temp("gc");
    let file = root.join("Collect.pptx");
    let picture = noise(5_000, 13);
    let only_first = noise(4_000, 14);
    let now = 1_800_000_000;
    let first = package(&body_xml(150, 0), &picture, Some(("ppt/media/old.bin", &only_first)));
    let list = save_bytes_at(&root, &file, &first, now);
    let first_id = list[0].id.clone();
    let second = package(&body_xml(150, 1), &picture, None);
    let list = save_bytes_at(&root, &file, &second, now + 1);
    let dir = document_dir(&root, &file);
    let body_first = pack::split_zip(&first).unwrap().0.into_iter().find(|p| p.meta.name == "ppt/slides/slide1.xml").unwrap().meta.sha256;
    let kept: Vec<VersionInfo> = list.into_iter().filter(|v| v.id != first_id).collect();
    remove_versions(&dir, &[first_id.clone()].into_iter().collect());
    collect_objects(&dir, &kept, now + 2);
    assert!(objects_in(&dir).contains(&digest_hex(&only_first)), "marked, kept for the grace period");
    collect_objects(&dir, &kept, now + 2 + ORPHAN_GRACE_SECS);
    let objects = objects_in(&dir);
    assert!(!objects.contains(&digest_hex(&only_first)), "a part only the removed version had is collected");
    assert!(objects.contains(&body_first), "the base of a delta still in use stays");
    assert!(same_parts(&read_at(&root, &file, &kept[0].id).unwrap(), &second));
    fs::write(manifest_path(&dir, &kept[0].id), b"{ damaged").unwrap();
    let before = objects_in(&dir);
    collect_objects(&dir, &kept, now + 3 + 2 * ORPHAN_GRACE_SECS);
    assert_eq!(objects_in(&dir), before, "an unreadable manifest stops collection");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn whole_versions_from_earlier_builds_stay_readable_and_move_to_parts() {
    let root = temp("migrate");
    let file = root.join("Old.pptx");
    let picture = noise(20_000, 15);
    let now = 1_800_000_000;
    let old = package(&body_xml(100, 0), &picture, None);
    let dir = document_dir(&root, &file);
    fs::create_dir_all(&dir).unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    fs::write(dir.join(format!("{id}.bin")), &old).unwrap();
    let legacy =
        json!([{"id": id, "savedAt": iso_from_unix_secs(now), "author": "Tester", "size": old.len(), "kind": "save", "digest": digest_hex(&old)}]);
    fs::write(dir.join(INDEX_FILE), serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(read_at(&root, &file, id).unwrap(), old);
    let newer = package(&body_xml(100, 1), &picture, None);
    let _ = save_bytes_at(&root, &file, &newer, now + 60);
    let list = visible(&read_index(&dir));
    let migrated = list.iter().find(|v| v.id == id).unwrap();
    assert_eq!(migrated.storage, VersionStorage::Parts, "moved to parts at the next save");
    assert!(!dir.join(format!("{id}.bin")).exists());
    assert!(same_parts(&read_at(&root, &file, id).unwrap(), &old));
    assert_eq!(migrated.digest.as_deref(), Some(digest_hex(&old).as_str()));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_original_and_files_that_are_not_zips_stay_byte_for_byte() {
    let root = temp("verbatim");
    let file = root.join("Original.pptx");
    let original = package(&body_xml(50, 0), &noise(1_000, 17), None);
    let list = put(&root, &file, &original, VersionKind::Original, 1_800_000_000, 1_800_000_000, None);
    assert_eq!(list[0].storage, VersionStorage::Whole);
    assert_eq!(read_at(&root, &file, &list[0].id).unwrap(), original);
    let broken = original.get(..original.len() / 2).unwrap().to_vec();
    let list = save_bytes_at(&root, &file, &broken, 1_800_000_100);
    assert_eq!(list[0].storage, VersionStorage::Whole);
    assert_eq!(read_at(&root, &file, &list[0].id).unwrap(), broken);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_size_limit_counts_real_disk_space() {
    let root = temp("space");
    let file = root.join("Space.pptx");
    let dir = document_dir(&root, &file);
    let picture = noise(50_000, 19);
    let now = 1_800_000_000;
    let mut logical = 0u64;
    for edit in 0..20 {
        let bytes = package(&body_xml(100, edit), &picture, None);
        logical += bytes.len() as u64;
        let _ = save_bytes_at(&root, &file, &bytes, now + edit as u64);
    }
    assert_eq!(visible(&read_index(&dir)).len(), 20);
    let real = disk_bytes(&dir);
    assert!(real * 5 < logical, "20 versions take {real} bytes on disk instead of {logical}");
    let _ = fs::remove_dir_all(root);
}

/// The figure for the docs: 100 versions of a ~1.8 MB `.pptx` with a picture, one paragraph
/// edited each time. Run with `cargo test -p deckcraft-engine --lib storage_figure -- --ignored --nocapture`.
#[test]
#[ignore = "slow; prints the storage figure for docs/cloud.md"]
fn storage_figure_for_a_hundred_versions() {
    let root = temp("figure");
    let file = root.join("Figure.pptx");
    let dir = document_dir(&root, &file);
    let picture = noise(1_700_000, 21);
    let now = 1_800_000_000;
    let mut whole = 0u64;
    let mut size = 0;
    for edit in 0..100 {
        let bytes = package(&varied_body_xml(3_000, edit * 7), &picture, None);
        size = bytes.len();
        whole += bytes.len() as u64;
        let _ = save_bytes_at(&root, &file, &bytes, now + edit as u64 * 60);
    }
    let parts = disk_bytes(&dir);
    println!("100 versions of a {size}-byte .pptx: {whole} bytes kept whole, {parts} bytes as parts ({:.1}%)", parts as f64 * 100.0 / whole as f64);
    assert!(parts * 10 < whole);
    let _ = fs::remove_dir_all(root);
}

// ---- the save paths -----------------------------------------------------------------------------

/// A session with the history enabled in `root/versions`, never the real profile, and a new
/// presentation open.
fn deck_in(root: &Path) -> Session {
    let mut s = Session::new();
    s.prefs.author = "Tester Two".into();
    s.enable_versions(root.join("versions"), std::sync::Arc::new(|| {})).unwrap();
    s.execute("file.new", &json!({})).unwrap();
    s
}

/// A session with the history enabled and the presentation at `file` open.
fn open_in(root: &Path, file: &Path) -> Session {
    let mut s = Session::new();
    s.prefs.author = "Tester Two".into();
    s.enable_versions(root.join("versions"), std::sync::Arc::new(|| {})).unwrap();
    s.execute("file.open", &json!({"path": file.to_string_lossy()})).unwrap();
    s
}

fn set_title(s: &mut Session, title: &str) {
    s.execute("file.properties", &json!({"title": title})).unwrap();
}

fn title_of(s: &Session) -> String {
    s.active().unwrap().doc.props.title.clone()
}

/// The active presentation's versions once the worker has written everything queued.
fn listed(s: &mut Session) -> Vec<VersionInfo> {
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)), "the worker did not finish");
    let index = s.active_index().unwrap();
    s.version_infos(index)
}

fn bytes_of(s: &mut Session, id: &str) -> Vec<u8> {
    let index = s.active_index().unwrap();
    s.read_version(index, id).unwrap().1
}

fn doc_id(s: &Session) -> Option<String> {
    s.version_document_id(s.active_index().unwrap())
}

fn save_to(s: &mut Session, file: &Path) {
    let r = s.execute("file.save", &json!({"path": file.to_string_lossy()})).unwrap();
    assert_eq!(r["saved"], true, "{r}");
}

fn autosave(s: &mut Session) -> serde_json::Value {
    s.execute("file.save", &json!({"ifUnchanged": true})).unwrap()
}

/// The active presentation as a `.pptx` with `title`, carrying `id` when given: what another
/// program (or computer) that kept the document id would write.
fn foreign_pptx(s: &Session, title: &str, id: Option<&str>) -> Vec<u8> {
    let mut doc = (*s.active().unwrap().doc).clone();
    doc.props.title = title.into();
    if let Some(id) = id {
        meta::set_presentation_document_id(&mut doc, id);
    }
    crate::cmd::file::save_bytes(&doc, "pptx").unwrap()
}

/// The text of `docProps/custom.xml` in the package at `file`.
fn custom_xml(file: &Path) -> String {
    let mut archive = zip::ZipArchive::new(fs::File::open(file).unwrap()).unwrap();
    let mut xml = String::new();
    archive.by_name("docProps/custom.xml").unwrap().read_to_string(&mut xml).unwrap();
    xml
}

/// A clock the test moves.
fn clock_at(s: &mut Session, start: u64) -> std::sync::Arc<AtomicU64> {
    let now = std::sync::Arc::new(AtomicU64::new(start));
    let reader = std::sync::Arc::clone(&now);
    s.set_version_clock(std::sync::Arc::new(move || reader.load(Ordering::SeqCst)));
    now
}

#[test]
fn an_explicit_save_creates_a_version_and_records_where_and_how_without_the_text() {
    let root = temp("details");
    let file = root.join("Contract.pptx");
    let mut s = deck_in(&root);
    s.execute("file.options", &json!({"deviceLabel": "Office laptop"})).unwrap();
    s.execute("insert.textBox", &json!({"rect": [100.0, 100.0, 300.0, 80.0], "text": "Confidential settlement clause 4821"})).unwrap();
    s.execute("slide.notes", &json!({"text": "Confidential notes about 4821"})).unwrap();
    save_to(&mut s, &file);
    let list = listed(&mut s);
    assert_eq!(list.len(), 1, "{list:?}");
    let v = &list[0];
    assert_eq!(v.kind, VersionKind::Save);
    assert_eq!(v.storage, VersionStorage::Parts, "a zip package is kept as parts");
    assert_eq!(v.author, "Tester Two");
    let d = v.details.as_ref().unwrap();
    assert_eq!(d.trigger, "save");
    assert_eq!(d.device, "Office laptop");
    assert_eq!(d.installation.len(), 32);
    assert_eq!(d.os, std::env::consts::OS);
    assert_eq!(d.app_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(d.provider, "local");
    assert!(d.path.ends_with("Contract.pptx"), "{}", d.path);
    assert!(d.session_start.is_some() && d.editing_seconds.is_some());
    assert_eq!(d.initials, "TT");
    let stats = d.stats.as_ref().unwrap();
    assert!(stats.slides >= 1 && stats.shapes >= 1, "{stats:?}");
    assert_eq!(stats.pictures, 0);
    assert!(d.changed_parts.iter().any(|p| p.starts_with("ppt/")), "{:?}", d.changed_parts);
    // The bytes come back as the parts that were written.
    assert!(same_parts(&bytes_of(&mut s, &v.id), &fs::read(&file).unwrap()));
    // The records hold no text of the presentation, no host name and no account name.
    let all = records(&root.join("versions")).join("\n").to_lowercase();
    assert!(!all.is_empty());
    for word in ["confidential", "settlement", "4821"] {
        assert!(!all.contains(word), "the text {word:?} is in a version record");
    }
    for key in ["COMPUTERNAME", "HOSTNAME", "USERNAME", "USER", "USERDOMAIN"] {
        if let Ok(value) = std::env::var(key)
            && value.len() > 2
        {
            assert!(!all.contains(&value.to_lowercase()), "{key} ({value}) is in a version record");
        }
    }
    // The file carries only the random id.
    let custom = custom_xml(&file);
    assert_eq!(custom.matches("<property").count(), 1);
    assert!(custom.contains(meta::DOC_ID_PROP));
    // `file.versions` shows the same fields.
    let shown = s.execute("file.versions", &json!({})).unwrap();
    let first = &shown.as_array().unwrap()[0];
    assert_eq!(first["kind"], "save");
    assert!(first["documentId"].is_string() && first["stored"].is_u64() && first["size"].is_u64());
    assert_eq!(first["details"]["trigger"], "save");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_presentation_gets_an_id_at_its_first_save_and_keeps_it_through_a_rename_and_a_move() {
    let root = temp("doc-id");
    let docs = root.join("docs");
    fs::create_dir_all(docs.join("archive")).unwrap();
    let file = docs.join("Plan.pptx");
    let mut s = deck_in(&root);
    assert!(doc_id(&s).is_none());
    save_to(&mut s, &file);
    let id = doc_id(&s).expect("an id at the first save");
    assert!(meta::document_id_ok(&id));
    assert_eq!(meta::file_document_id(&file).as_deref(), Some(id.as_str()), "written into docProps/custom.xml");
    // The package reader and writer keep it: open and save again without the app's help.
    let doc = crate::cmd::file::open_presentation("Plan.pptx", &fs::read(&file).unwrap()).unwrap();
    assert_eq!(document_uuid(&doc).as_deref(), Some(id.as_str()));
    let again = crate::cmd::file::save_bytes(&doc, "pptx").unwrap();
    assert_eq!(meta::package_document_id(std::io::Cursor::new(again)).as_deref(), Some(id.as_str()));
    assert_eq!(listed(&mut s).len(), 1);
    // A second save, an Undo that goes back to a state without the id: still the one id.
    set_title(&mut s, "plan, revised");
    save_to(&mut s, &file);
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("edit.undo", &json!({})).ok();
    save_to(&mut s, &file);
    assert_eq!(doc_id(&s).as_deref(), Some(id.as_str()));
    assert_eq!(meta::file_document_id(&file).as_deref(), Some(id.as_str()));
    assert_eq!(listed(&mut s).len(), 3);
    // Moved and renamed in File Explorer: another session finds the same history by the id.
    let moved = docs.join("archive").join("Plan final.pptx");
    fs::rename(&file, &moved).unwrap();
    let mut t = open_in(&root, &moved);
    assert_eq!(doc_id(&t).as_deref(), Some(id.as_str()), "a move is not a copy");
    assert_eq!(listed(&mut t).len(), 3, "the history follows the id");
    set_title(&mut t, "plan, moved");
    save_to(&mut t, &moved);
    assert_eq!(doc_id(&t).as_deref(), Some(id.as_str()));
    let list = listed(&mut t);
    assert_eq!(list.len(), 4);
    assert!(list.iter().all(|v| v.document_id.as_deref() == Some(id.as_str())));
    assert!(root.join("versions").join(meta::id_bucket(&id)).is_dir(), "kept under the id");
    drop((s, t));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn autosave_starts_a_version_per_editing_session_and_updates_it_for_ten_minutes() {
    let root = temp("window");
    let file = root.join("Talk.pptx");
    let mut s = deck_in(&root);
    let clock = clock_at(&mut s, 1_800_000_000);
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 1);
    // The first AutoSave write of the session starts a version...
    set_title(&mut s, "one");
    assert_eq!(autosave(&mut s)["saved"], true);
    let list = listed(&mut s);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].kind, VersionKind::Autosave);
    let first = list[0].id.clone();
    // ...later writes update it, writing a new record that supersedes the old (nothing already
    // written is rewritten)...
    clock.fetch_add(30, Ordering::SeqCst);
    set_title(&mut s, "two");
    autosave(&mut s);
    let list = listed(&mut s);
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].supersedes.as_deref(), Some(first.as_str()));
    assert_eq!(list[0].details.as_ref().map(|d| d.trigger.as_str()), Some("autosave"));
    // ...until ten minutes have passed since the version was started.
    clock.fetch_add(590, Ordering::SeqCst);
    set_title(&mut s, "three");
    autosave(&mut s);
    assert_eq!(listed(&mut s).len(), 3, "a new window");
    // Closing writes the last one into the version of its window.
    clock.fetch_add(20, Ordering::SeqCst);
    set_title(&mut s, "four");
    let index = s.active_index().unwrap();
    assert_eq!(s.autosave_flush(index), crate::autosave::Flush::Saved);
    let list = listed(&mut s);
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list[0].details.as_ref().map(|d| d.trigger.as_str()), Some("close"));
    let (doc, _) = crate::cmd::file::open_checked("x.pptx", &bytes_of(&mut s, &list[0].id)).unwrap();
    assert_eq!(doc.props.title, "four", "the last write before closing is in the history");
    // An explicit save is always its own version.
    set_title(&mut s, "five");
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 4);
    // Opened again, the first AutoSave write starts a new version, whatever the time.
    drop(s);
    let mut t = open_in(&root, &file);
    clock_at(&mut t, 1_800_000_700);
    set_title(&mut t, "six");
    autosave(&mut t);
    assert_eq!(listed(&mut t).len(), 5, "an editing session never overwrites the previous session's version");
    drop(t);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_file_without_history_keeps_its_original_pinned_through_retention() {
    let root = temp("original");
    let file = root.join("Report.pptx");
    let mut maker = Session::new();
    maker.execute("file.new", &json!({})).unwrap();
    set_title(&mut maker, "written elsewhere");
    let original = foreign_pptx(&maker, "written elsewhere", None);
    fs::write(&file, &original).unwrap();
    let mut s = open_in(&root, &file);
    let clock = clock_at(&mut s, 1_800_000_000);
    set_title(&mut s, "edited here");
    assert_eq!(autosave(&mut s)["saved"], true);
    let list = listed(&mut s);
    assert_eq!(list.len(), 2, "{list:?}");
    let first = list.iter().find(|v| v.kind == VersionKind::Original).unwrap();
    assert_eq!(first.storage, VersionStorage::Whole, "kept byte for byte");
    assert_eq!(bytes_of(&mut s, &first.id), original, "the bytes that were there before the first overwrite");
    assert_eq!(first.details.as_ref().map(|d| d.trigger.as_str()), Some("beforeSave"));
    assert_eq!(list[0].kind, VersionKind::Autosave);
    // Saved again: the history already holds what is on disk, so no second original.
    set_title(&mut s, "edited again");
    save_to(&mut s, &file);
    set_title(&mut s, "edited a third time");
    save_to(&mut s, &file);
    let list = listed(&mut s);
    assert_eq!(list.iter().filter(|v| v.kind == VersionKind::Original).count(), 1);
    assert_eq!(list.len(), 4);
    // Seventy days later retention keeps the newest version of each day, and the original stays.
    clock.fetch_add(70 * DAY_SECS, Ordering::SeqCst);
    set_title(&mut s, "much later");
    save_to(&mut s, &file);
    let list = listed(&mut s);
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list.iter().map(|v| v.kind).collect::<Vec<_>>(), [VersionKind::Save, VersionKind::Save, VersionKind::Original]);
    assert_eq!(bytes_of(&mut s, &list[2].id), original, "the pinned original outlives the time rules");
    // A year later, still.
    clock.fetch_add(300 * DAY_SECS, Ordering::SeqCst);
    set_title(&mut s, "a year on");
    save_to(&mut s, &file);
    let list = listed(&mut s);
    assert!(list.iter().any(|v| v.kind == VersionKind::Original), "{list:?}");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn bytes_changed_by_another_program_are_kept_before_saving_over_them() {
    let root = temp("outside");
    let file = root.join("Notes.pptx");
    let mut s = deck_in(&root);
    set_title(&mut s, "mine");
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 1);
    let id = doc_id(&s).unwrap();
    // Another program writes the file, keeping the document id (as PowerPoint does).
    let theirs = foreign_pptx(&s, "their edit", Some(&id));
    fs::write(&file, &theirs).unwrap();
    set_title(&mut s, "mine, later");
    // AutoSave notices and does not overwrite.
    let r = autosave(&mut s);
    assert_eq!(r["saved"], false, "{r}");
    assert_eq!(r["conflict"], "changed");
    assert_eq!(fs::read(&file).unwrap(), theirs, "nothing was written");
    // An explicit save overwrites; their bytes are in the history first.
    s.execute("file.resolveConflict", &json!({"keep": "mine"})).unwrap();
    let list = listed(&mut s);
    assert_eq!(list.len(), 3, "{list:?}");
    let kept = list.iter().find(|v| v.kind == VersionKind::External).unwrap();
    assert_eq!(bytes_of(&mut s, &kept.id), theirs, "byte for byte");
    assert_eq!(doc_id(&s).as_deref(), Some(id.as_str()), "the same document");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn save_as_and_keep_both_make_new_documents_that_record_where_they_came_from() {
    let root = temp("save-as");
    let first = root.join("Draft.pptx");
    let mut s = deck_in(&root);
    save_to(&mut s, &first);
    let source = doc_id(&s).unwrap();
    let source_version = listed(&mut s)[0].id.clone();
    let copy = root.join("Copy.pptx");
    s.execute("file.saveAs", &json!({"path": copy.to_string_lossy()})).unwrap();
    let id = doc_id(&s).unwrap();
    assert_ne!(id, source, "a new file, a new id");
    let list = listed(&mut s);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, VersionKind::SaveAs);
    let origin = list[0].details.as_ref().and_then(|d| d.origin.clone()).unwrap();
    assert_eq!(origin.document_id.as_deref(), Some(source.as_str()));
    assert_eq!(origin.version_id.as_deref(), Some(source_version.as_str()));
    // The original keeps its own history.
    let roots = vec![root.join("versions")];
    assert_eq!(store_list(&roots, &first, Some(&source), false, &[]).unwrap().len(), 1);
    // A copy in another format is an export, not a version of the presentation.
    let pdf = root.join("Copy.pdf");
    s.execute("file.saveAs", &json!({"path": pdf.to_string_lossy()})).unwrap();
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    assert!(!document_dir(&root.join("versions"), &pdf).exists());

    // Keep both after a save conflict: the copy's first version is a `conflictCopy`.
    let mut t = open_in(&root, &first);
    let theirs = foreign_pptx(&t, "from the other computer", meta::file_document_id(&first).as_deref());
    fs::write(&first, &theirs).unwrap();
    set_title(&mut t, "mine");
    assert_eq!(autosave(&mut t)["conflict"], "changed");
    t.execute("file.resolveConflict", &json!({"keep": "both"})).unwrap();
    let conflict_copy = root.join("Draft (conflict copy).pptx");
    assert!(conflict_copy.is_file());
    let list = listed(&mut t);
    assert_eq!(list[0].kind, VersionKind::ConflictCopy, "{list:?}");
    assert_eq!(list[0].details.as_ref().map(|d| d.trigger.as_str()), Some("conflictCopy"));
    let origin = list[0].details.as_ref().and_then(|d| d.origin.clone()).unwrap();
    assert_eq!(origin.document_id.as_deref(), Some(source.as_str()));
    assert_ne!(doc_id(&t).as_deref(), Some(source.as_str()));
    assert_eq!(fs::read(&first).unwrap(), theirs, "the other version is left as it is");
    drop((s, t));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_copy_made_outside_the_app_gets_its_own_id_when_saved() {
    let root = temp("copy-id");
    let original = root.join("Plan.pptx");
    let mut s = deck_in(&root);
    save_to(&mut s, &original);
    let id = doc_id(&s).unwrap();
    let version = listed(&mut s)[0].id.clone();
    // Copied in File Explorer while the original is still there.
    let copy = root.join("Plan copy.pptx");
    fs::copy(&original, &copy).unwrap();
    let mut t = open_in(&root, &copy);
    assert_eq!(doc_id(&t).as_deref(), Some(id.as_str()), "the copy carries the id until it is saved");
    set_title(&mut t, "copy");
    autosave(&mut t);
    let copy_id = doc_id(&t).unwrap();
    assert_ne!(copy_id, id, "the copy gets an id of its own");
    assert_eq!(meta::file_document_id(&copy).as_deref(), Some(copy_id.as_str()));
    let list = listed(&mut t);
    let origin = list[0].details.as_ref().and_then(|d| d.origin.clone()).unwrap();
    assert_eq!((origin.document_id.as_deref(), origin.version_id.as_deref()), (Some(id.as_str()), Some(version.as_str())));
    assert_eq!(list[0].details.as_ref().map(|d| d.trigger.as_str()), Some("copy"));
    assert_eq!(listed(&mut s).len(), 1, "the original keeps its history alone");
    drop((s, t));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_file_kept_by_path_follows_a_rename_and_a_move() {
    let root = temp("by-path");
    let docs = root.join("docs");
    fs::create_dir_all(docs.join("archive")).unwrap();
    let file = docs.join("Letter.deckcraft");
    let mut s = deck_in(&root);
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 1);
    assert!(doc_id(&s).is_none(), "the native format carries no id");
    // Renamed in the app: the history follows the file.
    let renamed = docs.join("Letter final.deckcraft");
    let from = canonical_path(&file);
    fs::rename(&file, &renamed).unwrap();
    s.docs[0].path = Some(renamed.to_string_lossy().into_owned());
    let index = s.active_index().unwrap();
    s.versions_renamed(index, &from, &renamed);
    assert_eq!(listed(&mut s).len(), 1, "the history follows the rename");
    // Moved in File Explorer: found again by the bytes of its newest version.
    let moved = docs.join("archive").join("Letter final.deckcraft");
    fs::rename(&renamed, &moved).unwrap();
    let mut t = open_in(&root, &moved);
    let mut found = Vec::new();
    for _ in 0..500 {
        let index = t.active_index().unwrap();
        found = t.version_infos(index);
        if !found.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(found.len(), 1, "the moved file's history is found by its bytes");
    drop((s, t));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn history_kept_by_path_moves_under_the_id_at_the_first_save() {
    let root = temp("by-path-to-id");
    let file = root.join("Old.pptx");
    let mut maker = Session::new();
    maker.execute("file.new", &json!({})).unwrap();
    let old = foreign_pptx(&maker, "old", None);
    fs::write(&file, &old).unwrap();
    let versions = root.join("versions");
    // What an earlier build kept for this file, by path.
    let _ = save_bytes_at(&versions, &file, &old, 1_800_000_000);
    let by_path = document_dir(&versions, &file);
    assert!(by_path.is_dir());
    let mut s = open_in(&root, &file);
    assert_eq!(s.execute("file.versions", &json!({})).unwrap().as_array().map(Vec::len), Some(1), "listed before the first save");
    set_title(&mut s, "new");
    save_to(&mut s, &file);
    let id = doc_id(&s).unwrap();
    let list = listed(&mut s);
    assert_eq!(list.len(), 2, "{list:?}");
    assert!(!by_path.exists(), "the folder kept by path was taken over");
    assert!(versions.join(meta::id_bucket(&id)).is_dir());
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_macro_enabled_file_and_exports_have_no_history() {
    let root = temp("no-history");
    let mut s = deck_in(&root);
    for name in ["Macros.pptm", "Slides.pdf", "Outline.txt"] {
        let r = s.execute("file.saveAs", &json!({"path": root.join(name).to_string_lossy()}));
        assert!(r.is_ok(), "{name}: {r:?}");
    }
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    let any: Vec<_> = fs::read_dir(root.join("versions")).unwrap().flatten().collect();
    assert!(any.is_empty(), "{any:?}");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_broken_history_never_stops_or_changes_the_save() {
    let root = temp("broken");
    let file = root.join("Safe.pptx");
    let mut maker = Session::new();
    maker.execute("file.new", &json!({})).unwrap();
    fs::write(&file, foreign_pptx(&maker, "before", None)).unwrap();
    let mut s = open_in(&root, &file);
    let versions = root.join("versions");
    fs::remove_dir_all(&versions).unwrap();
    fs::write(&versions, b"not a folder").unwrap();
    set_title(&mut s, "saved anyway");
    assert_eq!(autosave(&mut s)["saved"], true);
    let reopened = open_in(&root.join("elsewhere-versions"), &file);
    assert_eq!(title_of(&reopened), "saved anyway");
    let mut reported = None;
    for _ in 0..500 {
        reported = s.versions_error();
        if reported.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(reported.is_some(), "the failure is reported, not hidden");
    drop((s, reopened));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn restore_records_a_version_at_once_and_keeps_the_unsaved_state_before_it() {
    let root = temp("restore-now");
    let file = root.join("Letter.pptx");
    let mut s = deck_in(&root);
    set_title(&mut s, "first draft");
    save_to(&mut s, &file);
    let first = listed(&mut s);
    set_title(&mut s, "unsaved work");
    let r = s.execute("file.versions", &json!({"restore": first[0].id.clone()})).unwrap();
    assert_eq!(r["restored"], json!(first[0].id));
    assert_eq!(title_of(&s), "first draft");
    let list = listed(&mut s);
    assert_eq!(list.len(), 3, "{list:?}");
    assert_eq!(list[0].kind, VersionKind::Restore);
    assert_eq!(list[0].details.as_ref().map(|d| d.trigger.clone()), Some(format!("restore:{}", first[0].id)));
    assert!(same_parts(&bytes_of(&mut s, &list[0].id), &bytes_of(&mut s, &first[0].id)), "with the restored content");
    let kept = &list[1];
    assert_eq!(kept.kind, VersionKind::Autosave);
    assert_eq!(kept.details.as_ref().map(|d| d.trigger.as_str()), Some("beforeRestore"));
    let (doc, _) = crate::cmd::file::open_checked("x.pptx", &bytes_of(&mut s, &kept.id)).unwrap();
    assert_eq!(doc.props.title, "unsaved work", "the state before the restore stays in the history");
    // One Undo step brings the unsaved work back.
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(title_of(&s), "unsaved work");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn open_answers_with_the_version_and_its_bytes_and_changes_nothing() {
    let root = temp("open");
    let file = root.join("Minutes.pptx");
    let mut s = deck_in(&root);
    set_title(&mut s, "one");
    save_to(&mut s, &file);
    set_title(&mut s, "two");
    save_to(&mut s, &file);
    let list = listed(&mut s);
    assert_eq!(list.len(), 2);
    let opened = s.execute("file.versions", &json!({"open": list[1].id.clone()})).unwrap();
    assert_eq!(opened["version"]["id"], json!(list[1].id));
    assert!(opened["name"].as_str().unwrap().starts_with("Minutes (version "), "{opened}");
    let data = crate::cmd::base64_decode(opened["data"].as_str().unwrap()).unwrap();
    assert_eq!(opened["bytes"], data.len());
    let (doc, _) = crate::cmd::file::open_checked("x.pptx", &data).unwrap();
    assert_eq!(doc.props.title, "one");
    assert_eq!(title_of(&s), "two", "the presentation is as it was");
    assert!(!s.active().unwrap().is_dirty());
    assert_eq!(s.documents().len(), 1);
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_first_listing_after_opening_is_complete() {
    let root = temp("first-list");
    let file = root.join("Minutes.pptx");
    let mut s = deck_in(&root);
    save_to(&mut s, &file);
    set_title(&mut s, "two");
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 2);
    let mut other = open_in(&root, &file);
    let listed = other.execute("file.versions", &json!({})).unwrap();
    assert_eq!(listed.as_array().map(Vec::len), Some(2), "{listed}");
    drop((s, other));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn several_presentations_keep_separate_histories_and_editing_sessions() {
    let root = temp("several");
    let (a, b) = (root.join("A.pptx"), root.join("B.pptx"));
    let mut s = deck_in(&root);
    save_to(&mut s, &a);
    s.execute("file.new", &json!({})).unwrap();
    save_to(&mut s, &b);
    // AutoSave each in turn: the version of one never joins the other's.
    for (n, title) in [(0, "a1"), (1, "b1"), (0, "a2"), (1, "b2")] {
        s.set_active(n);
        set_title(&mut s, title);
        assert_eq!(autosave(&mut s)["saved"], true);
    }
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    for (n, want) in [(0, "a2"), (1, "b2")] {
        let list = s.version_infos(n);
        assert_eq!(list.len(), 2, "{list:?}");
        s.set_active(n);
        let (doc, _) = crate::cmd::file::open_checked("x.pptx", &bytes_of(&mut s, &list[0].id)).unwrap();
        assert_eq!(doc.props.title, want);
    }
    assert_ne!(s.version_document_id(0), s.version_document_id(1));
    drop(s);
    let _ = fs::remove_dir_all(root);
}

// ---- folders, options and the commands --------------------------------------------------------

#[test]
fn history_folders_change_from_options_and_history_moves_along() {
    let root = temp("relocate");
    let elsewhere = root.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let file = root.join("Moving.pptx");
    let mut s = deck_in(&root);
    save_to(&mut s, &file);
    assert_eq!(listed(&mut s).len(), 1);
    let id = doc_id(&s).unwrap();
    let r = s.execute("file.options", &json!({"versionsFolder": elsewhere.to_string_lossy(), "versionsExisting": "move"})).unwrap();
    assert_eq!(r["versionsFolder"], json!(elsewhere.to_string_lossy()));
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    assert!(elsewhere.join(meta::id_bucket(&id)).is_dir(), "moved to the new folder");
    assert!(!root.join("versions").join(meta::id_bucket(&id)).exists(), "and gone from the old one");
    assert_eq!(s.execute("file.versions", &json!({})).unwrap().as_array().map(Vec::len), Some(1));
    // Mirror: both folders, every write to both.
    s.execute("file.options", &json!({"versionsMirror": true, "versionsExisting": "copy"})).unwrap();
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    assert_eq!(s.version_folders().0, vec![root.join("versions"), elsewhere.clone()]);
    set_title(&mut s, "mirrored");
    save_to(&mut s, &file);
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    let a = root.join("versions").join(meta::id_bucket(&id));
    let b = elsewhere.join(meta::id_bucket(&id));
    assert_eq!(visible(&read_index(&a)).len(), 2);
    assert_eq!(visible(&read_index(&b)).len(), 2);
    // Delete this presentation's history: in both folders.
    assert_eq!(s.execute("file.versions", &json!({"delete": "document"})).unwrap()["deleted"], 2);
    assert!(!a.exists() && !b.exists());
    // Bad settings change nothing.
    assert!(s.execute("file.options", &json!({"versionsFolder": "relative/path"})).is_err());
    assert!(s.execute("file.options", &json!({"versionsExisting": "sideways", "versionsMirror": false})).is_err());
    assert!(s.prefs.versions_mirror, "a refused change leaves the settings alone");
    assert!(s.execute("file.versions", &json!({"delete": "everything"})).is_err());
    // Back to the settings folder alone.
    let r = s.execute("file.options", &json!({"versionsFolder": null, "versionsMirror": false})).unwrap();
    assert!(r["versionsFolder"].is_null());
    assert_eq!(s.version_folders().0, vec![root.join("versions")]);
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn deleting_all_history_empties_every_folder() {
    let root = temp("delete-all");
    let mut s = deck_in(&root);
    for name in ["A.pptx", "B.pptx", "c.deckcraft"] {
        save_to(&mut s, &root.join(name));
    }
    assert!(s.version_wait_idle(std::time::Duration::from_secs(30)));
    assert!(s.execute("file.versions", &json!({"delete": "all"})).unwrap()["deleted"].as_u64().unwrap() >= 3);
    let left: Vec<_> = fs::read_dir(root.join("versions")).unwrap().flatten().filter(|e| e.path().is_dir()).collect();
    assert!(left.is_empty(), "{left:?}");
    assert!(root.join("installation-id").is_file(), "the installation id is not history");
    drop(s);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_version_parameters_return_errors_and_no_history_is_a_clear_error() {
    let mut s = Session::new();
    s.execute("file.new", &json!({})).unwrap();
    // Without the desktop store: the list is empty and the actions say why.
    assert_eq!(s.execute("file.versions", &json!({})).unwrap().as_array().map(Vec::len), Some(0));
    assert!(s.execute("file.versions", &json!({"delete": "all"})).is_err());
    assert!(s.execute("file.versions", &json!({"open": "0123456789abcdef0123456789abcdef"})).is_err());
    assert!(s.execute("file.versions", &json!({"restore": "0123456789abcdef0123456789abcdef"})).is_err());
    assert!(s.execute("file.versions", &json!({"unknown": true})).is_err());
    assert!(s.execute("file.versions", &json!({"open": 7})).is_err());
    assert!(s.execute("file.versions", &json!({"restore": []})).is_err());
    assert!(s.execute("file.versions", &json!({"delete": "all", "open": "x"})).is_err());
    assert!(s.execute("file.versions", &json!([1])).is_err());
    assert!(s.execute("file.save", &json!({"path": "x.pptx", "trigger": "rm -rf /"})).is_err());
    let root = temp("params");
    let mut t = deck_in(&root);
    assert!(t.execute("file.versions", &json!({"restore": "not an id"})).is_err());
    assert!(t.execute("file.versions", &json!({"open": "0123456789abcdef0123456789abcdef"})).is_err(), "unsaved: no history, no such version");
    assert!(t.execute("file.versions", &json!({"delete": "document"})).is_err(), "an unsaved presentation has no history to delete");
    drop(t);
    let _ = fs::remove_dir_all(root);
}

// ---- the store and zip bombs ---------------------------------------------------------------------

#[test]
fn a_zip_bomb_is_stored_whole_and_never_inflated() {
    let root = temp("bomb");
    let file = root.join("Bomb.pptx");
    // A part that inflates past the per-part limit: 300 MiB of zeros in a few hundred kilobytes.
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("ppt/slides/slide1.xml", deflated).unwrap();
    let chunk = vec![0u8; 1 << 20];
    for _ in 0..300 {
        writer.write_all(&chunk).unwrap();
    }
    let bomb = writer.finish().unwrap().into_inner();
    assert!(bomb.len() < 1 << 20, "{} bytes", bomb.len());
    assert!(pack::split_zip(&bomb).is_none(), "refused before anything is inflated");
    let list = save_bytes_at(&root, &file, &bomb, 1_800_000_000);
    assert_eq!(list[0].storage, VersionStorage::Whole);
    assert_eq!(read_at(&root, &file, &list[0].id).unwrap(), bomb, "kept byte for byte");
    assert!(objects_in(&document_dir(&root, &file)).is_empty(), "no object was made");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_damaged_object_is_repaired_by_the_next_save_that_has_it() {
    let root = temp("repair");
    let file = root.join("Repair.pptx");
    let picture = noise(3_000, 23);
    let package_a = package(&body_xml(120, 0), &picture, None);
    let list = save_bytes_at(&root, &file, &package_a, 1_800_000_000);
    let dir = document_dir(&root, &file);
    let picture_hash = digest_hex(&picture);
    // The picture's object is damaged on disk.
    let path = object_path(&dir, &picture_hash);
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xAA;
    fs::write(&path, bytes).unwrap();
    assert!(read_at(&root, &file, &list[0].id).is_err(), "a damaged object is refused");
    // The next save has the same picture: the object is written again whole, repairing every
    // version that uses it.
    let _ = save_bytes_at(&root, &file, &package(&body_xml(120, 1), &picture, None), 1_800_000_060);
    assert!(same_parts(&read_at(&root, &file, &list[0].id).unwrap(), &package_a));
    let _ = fs::remove_dir_all(root);
}
