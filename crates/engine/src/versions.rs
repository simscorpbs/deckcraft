//! Local version history for desktop presentations (phase 2 of the version history: the engine,
//! the storage and the commands; the History pane is the front end's).
//!
//! The service backed histories of iCloud Drive, Google Drive and OneDrive remain owned by
//! those services.  This module is the small local safety net DeckCraft keeps in the profile:
//! presentation bytes are written by a worker, as immutable files.
//!
//! This module and [`meta`] are the only engine code that touches the file system by itself
//! (besides `io`, `recovery`, `spell` and `autosave`): the history lives in the settings folder
//! or in folders the user picked in Preferences, never at a path a command names, so the
//! automation roots (`file_access`) do not apply to the store. The presentation's own file is
//! still read through the roots' check (see `Versions::ensure_path`).
//!
//! Storage, per presentation, under `<root>/<key>/`, where the key is the presentation's random
//! id (`.pptx`, `.potx` and `.ppsx` packages, see [`meta`]) or a hash of its canonical path
//! (other files):
//! - `<id>.parts.json` and `objects/<sha256>`: a version of a zip package (`.pptx`…)
//!   kept as its parts, each part an object named by the hash of its content, so what did not
//!   change between versions is stored once and what changed is a small delta (see [`pack`]);
//! - `<id>.bin`: the exact bytes of one version, for the `original` copy, files that are not a
//!   zip, and versions written by earlier builds; verified after writing;
//! - `<id>.meta.json`: the metadata of a whole version (a manifest carries its own);
//! - `gc/`: dated marks of objects no version needs, deleted after a grace period;
//! - `document.json` (folders kept by path): the path the folder last belonged to, so a file
//!   moved outside DeckCraft can find its history again (see `adopt_moved`).
//!
//! Nothing a version writes is ever rewritten, so a history folder can be one a cloud app
//! syncs between two computers: versions and objects have unique names, the list of versions
//! is rebuilt from their own files, and an object is deleted only after a week unneeded. One or
//! two history folders can be used (File › Options › Save); every write goes to all of them and
//! they are reconciled when one comes back.
//!
//! Safety rules: before any save writes over a file whose current bytes are not in the history
//! (a file that never had history, or one changed by another program), those bytes are stored
//! first as an `original` version ([`crate::Session::protect_before_overwrite`]). A failure of the
//! history never fails or alters the save itself: it is reported in the store's `error`.

// The browser keeps no history: the types stay (the commands and `DocState` name them), the code
// that fills them is desktop only.
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::collections::BTreeSet;
#[cfg(not(target_arch = "wasm32"))]
use std::fs::{self, File, OpenOptions};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex, OnceLock, mpsc};

/// A version's origin.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionKind {
    #[default]
    Save,
    Autosave,
    Restore,
    /// The bytes that were on disk before DeckCraft first wrote over them: a file that had no
    /// history yet (a `.pptx` made elsewhere, a file Save As replaced).
    Original,
    /// The bytes another program or computer wrote to the file, found on disk when DeckCraft
    /// was about to write over them.
    External,
    /// The first version of a file made with Save As (or Save to Cloud) from another document.
    #[serde(rename = "saveAs")]
    SaveAs,
    /// The first version of the copy Keep both makes after a save conflict.
    #[serde(rename = "conflictCopy")]
    ConflictCopy,
}

impl VersionKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Autosave => "autosave",
            Self::Restore => "restore",
            Self::Original => "original",
            Self::External => "external",
            Self::SaveAs => "saveAs",
            Self::ConflictCopy => "conflictCopy",
        }
    }
}

/// How a version's bytes are kept on disk.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VersionStorage {
    /// The bytes as they were saved, in `<id>.bin`.
    #[default]
    Whole,
    /// A zip package split into content-addressed parts, in `<id>.parts.json` (see [`pack`]).
    Parts,
}

fn is_whole(storage: &VersionStorage) -> bool {
    *storage == VersionStorage::Whole
}

/// The metadata shown by `file.versions` and the Version History pane.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub id: String,
    pub saved_at: String,
    pub author: String,
    pub size: u64,
    pub kind: VersionKind,
    /// AutoSave versions only: when the ten-minute window this version gathers began. Later
    /// AutoSave writes replace the version until the window closes, so a long editing session
    /// still leaves one version every ten minutes instead of a single sliding one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_start: Option<String>,
    /// SHA-256 of the stored bytes (lower-case hex). Checked when the version is read back, and
    /// used to tell whether the file on disk is already in the history. Older entries lack it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// AutoSave versions only: the editing session (one opening of the document in one run of
    /// the app) that wrote it. AutoSave joins only a window its own session opened, so the first
    /// change after opening a document always starts a new version, as PowerPoint's AutoSave does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// `original` versions only: when DeckCraft stored the copy. `savedAt` is the file's own
    /// modification time; retention counts from this capture time, so an old file's original
    /// is not pruned the moment it is stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<String>,
    /// How the bytes are kept. `size` stays the size of the document as it was saved.
    #[serde(default, skip_serializing_if = "is_whole")]
    pub storage: VersionStorage,
    /// Disk space this version added when it was stored: its manifest and the parts no earlier
    /// version had, or its whole bytes. Absent for versions written by earlier builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<u64>,
    /// An AutoSave version that replaced this one in the same window: the replaced version is
    /// no longer listed. (Records are never rewritten, so a later write supersedes instead.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    /// The document's random id (`DeckCraftDocumentId` in its custom properties) when its
    /// history is kept by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    /// Where the version comes from and what it holds (see [`VersionDetails`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<VersionDetails>>,
    /// The order versions were written in within one second (`savedAt` has whole seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<u64>,
}

/// Format a stored UTC timestamp for the history pane.  Version metadata stays UTC on disk so
/// pruning and clients have a stable value; the pane converts it to the computer's local clock.
/// A missing or unreadable zone database falls back to UTC, which is still an unambiguous time.
pub fn local_timestamp(saved_at: &str) -> String {
    let Some(secs) = parse_iso_timestamp(saved_at) else { return saved_at.to_owned() };
    #[cfg(not(target_arch = "wasm32"))]
    let offset = local_offset(secs);
    #[cfg(target_arch = "wasm32")]
    let offset = 0i32;
    let Some(local) = i64::try_from(secs).ok().and_then(|value| value.checked_add(i64::from(offset))) else {
        return saved_at.to_owned();
    };
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", seconds / 3_600, seconds / 60 % 60)
}

fn parse_iso_timestamp(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 20
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
        || bytes.get(19) != Some(&b'Z')
    {
        return None;
    }
    let year = i64::try_from(parse_decimal(bytes.get(0..4)?)?).ok()?;
    let month = parse_decimal(bytes.get(5..7)?)?;
    let day = parse_decimal(bytes.get(8..10)?)?;
    let hour = parse_decimal(bytes.get(11..13)?)?;
    let minute = parse_decimal(bytes.get(14..16)?)?;
    let second = parse_decimal(bytes.get(17..19)?)?;
    if !(1..=12).contains(&month) || day == 0 || day > 31 || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let month = i64::try_from(month).ok()?;
    let day = i64::try_from(day).ok()?;
    let y = year - if month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era.checked_mul(146_097)?.checked_add(doe)?.checked_sub(719_468)?;
    if days < 0 {
        return None;
    }
    u64::try_from(days).ok()?.checked_mul(86_400)?.checked_add(hour.checked_mul(3_600)?)?.checked_add(minute.checked_mul(60)?)?.checked_add(second)
}

fn parse_decimal(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut value = 0u64;
    for byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))?;
    }
    Some(value)
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct LocalZone {
    default_offset: i32,
    transitions: Vec<(i64, i32)>,
}

#[cfg(not(target_arch = "wasm32"))]
impl LocalZone {
    fn offset_at(&self, secs: i64) -> i32 {
        match self.transitions.binary_search_by_key(&secs, |(at, _)| *at) {
            Ok(index) => self.transitions.get(index).map(|(_, offset)| *offset).unwrap_or(self.default_offset),
            Err(0) => self.default_offset,
            Err(index) => self.transitions.get(index.saturating_sub(1)).map(|(_, offset)| *offset).unwrap_or(self.default_offset),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn local_offset(secs: u64) -> i32 {
    static ZONE: OnceLock<Option<LocalZone>> = OnceLock::new();
    let Ok(secs) = i64::try_from(secs) else { return 0 };
    ZONE.get_or_init(load_local_zone).as_ref().map_or(0, |zone| zone.offset_at(secs))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_local_zone() -> Option<LocalZone> {
    #[cfg(windows)]
    {
        // The standard library has no local-time conversion API on Windows.  PowerShell is part
        // of every supported desktop installation, and this is run once per process (the result
        // is held by `OnceLock`), so the history pane does not start a process for every row.
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "[DateTimeOffset]::Now.Offset.TotalSeconds"])
            .output()
            .ok()?;
        if !output.status.success() || output.stdout.len() > 64 {
            return None;
        }
        let text = String::from_utf8(output.stdout).ok()?;
        let seconds = text.trim().parse::<i32>().ok()?;
        if seconds.unsigned_abs() > 86_400 {
            return None;
        }
        Some(LocalZone { default_offset: seconds, transitions: Vec::new() })
    }
    #[cfg(not(windows))]
    {
        let path = std::env::var_os("TZ")
            .filter(|value| value.as_encoded_bytes().first() == Some(&b'/'))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/etc/localtime"));
        let metadata = std::fs::metadata(&path).ok()?;
        if !metadata.is_file() || metadata.len() > 1 << 20 {
            return None;
        }
        let bytes = std::fs::read(path).ok()?;
        parse_tzif(&bytes)
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn parse_tzif(bytes: &[u8]) -> Option<LocalZone> {
    let (version, counts) = tzif_header(bytes, 0)?;
    let (offset, time_size) = if matches!(version, b'2' | b'3' | b'4') {
        let first_size = tzif_block_size(counts, 4)?;
        let second = 44usize.checked_add(first_size)?;
        (second, 8)
    } else {
        (0, 4)
    };
    let (_, counts) = tzif_header(bytes, offset)?;
    let (timecnt, typecnt) = (counts.3, counts.4);
    if typecnt == 0 || typecnt > 256 || timecnt > 200_000 {
        return None;
    }
    let mut cursor = offset.checked_add(44)?;
    let mut transition_times = Vec::with_capacity(timecnt);
    for _ in 0..timecnt {
        let end = cursor.checked_add(time_size)?;
        let raw = bytes.get(cursor..end)?;
        let at = if time_size == 8 { i64::from_be_bytes(raw.try_into().ok()?) } else { i64::from(i32::from_be_bytes(raw.try_into().ok()?)) };
        transition_times.push(at);
        cursor = end;
    }
    let transition_types = bytes.get(cursor..cursor.checked_add(timecnt)?)?;
    cursor = cursor.checked_add(timecnt)?;
    let mut offsets = Vec::with_capacity(typecnt);
    for _ in 0..typecnt {
        let end = cursor.checked_add(6)?;
        let info = bytes.get(cursor..end)?;
        offsets.push(i32::from_be_bytes(info.get(..4)?.try_into().ok()?));
        cursor = end;
    }
    let default_offset = *offsets.first()?;
    let mut transitions = Vec::with_capacity(timecnt);
    for (at, kind) in transition_times.into_iter().zip(transition_types) {
        let offset = offsets.get(usize::from(*kind)).copied()?;
        transitions.push((at, offset));
    }
    Some(LocalZone { default_offset, transitions })
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn tzif_header(bytes: &[u8], offset: usize) -> Option<(u8, (usize, usize, usize, usize, usize, usize))> {
    let header = bytes.get(offset..offset.checked_add(44)?)?;
    if header.get(..4)? != b"TZif" {
        return None;
    }
    let version = *header.get(4)?;
    let mut counts = [0usize; 6];
    for (index, count) in counts.iter_mut().enumerate() {
        let start = 20usize.checked_add(index.checked_mul(4)?)?;
        let raw = header.get(start..start.checked_add(4)?)?;
        *count = usize::try_from(u32::from_be_bytes(raw.try_into().ok()?)).ok()?;
    }
    Some((version, (counts[0], counts[1], counts[2], counts[3], counts[4], counts[5])))
}

#[cfg(all(not(target_arch = "wasm32"), not(windows)))]
fn tzif_block_size(counts: (usize, usize, usize, usize, usize, usize), time_size: usize) -> Option<usize> {
    let (leapcnt, timecnt, typecnt, charcnt) = (counts.2, counts.3, counts.4, counts.5);
    timecnt
        .checked_mul(time_size)?
        .checked_add(timecnt)?
        .checked_add(typecnt.checked_mul(6)?)?
        .checked_add(charcnt)?
        .checked_add(leapcnt.checked_mul(time_size.checked_add(4)?)?)?
        .checked_add(counts.0)?
        .checked_add(counts.1)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod meta;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod pack;
mod session;
pub(crate) use session::with_document_id;
#[cfg(test)]
mod tests;

/// The device label versions record when the user has not chosen one: the system's name only
/// ("Windows", "Mac"), never the computer's own name.
pub fn default_device_label() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Mac",
        "linux" => "Linux",
        "freebsd" | "openbsd" | "netbsd" | "dragonfly" => "BSD",
        _ => "Computer",
    }
}

/// The id a presentation carries in its `docProps/custom.xml` (see [`meta::DOC_ID_PROP`]), when
/// it is a valid one.
pub fn document_uuid(doc: &deckcraft_model::Presentation) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        meta::presentation_document_id(doc)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = doc;
        None
    }
}

/// Days since 1970-01-01 → (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    crate::cmd::design::civil(days)
}

/// Seconds since the Unix epoch (the browser's clock on wasm, which has no `SystemTime`).
pub(crate) fn now_unix() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        (js_sys::Date::now() / 1000.0).max(0.0) as u64
    }
}

/// A UTC timestamp as the history stores it: `2026-10-10T13:10:35Z`.
pub(crate) fn iso_from_unix_secs(secs: u64) -> String {
    let (y, m, d) = civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let t = secs % 86_400;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", t / 3600, t / 60 % 60, t % 60)
}

/// Legacy, read only: the one index file earlier builds rewrote on every save. Two computers
/// syncing a folder would both rewrite it, so the history no longer keeps one: the list is
/// rebuilt from the versions' own immutable files (and cached in memory).
#[cfg(not(target_arch = "wasm32"))]
const INDEX_FILE: &str = "index.json";
#[cfg(not(target_arch = "wasm32"))]
const PARTS_SUFFIX: &str = ".parts.json";
/// The metadata of a version kept whole (`<id>.bin`), written once beside it.
#[cfg(not(target_arch = "wasm32"))]
const META_SUFFIX: &str = ".meta.json";
#[cfg(not(target_arch = "wasm32"))]
const OBJECTS_DIR: &str = "objects";
/// Marks of objects no version needed when last looked at: `<sha256>.<unix seconds>`.
#[cfg(not(target_arch = "wasm32"))]
const GC_DIR: &str = "gc";
#[cfg(not(target_arch = "wasm32"))]
const MAX_MANIFEST_BYTES: u64 = 8 << 20;
/// Largest object file read: a whole part compressed (incompressible data grows a little).
#[cfg(not(target_arch = "wasm32"))]
const MAX_OBJECT_FILE: u64 = pack::MAX_PART_BYTES + (16 << 20);
/// The identity record of a folder kept by path (files that carry no document id).
#[cfg(not(target_arch = "wasm32"))]
const META_FILE: &str = "document.json";
#[cfg(not(target_arch = "wasm32"))]
const MAX_INDEX_BYTES: u64 = 8 << 20;
#[cfg(not(target_arch = "wasm32"))]
const MAX_META_BYTES: u64 = 64 << 10;
/// A single document is never allowed to make the profile larger than 500 MiB, counted as the
/// space its history really takes on disk (shared parts once). This is DeckCraft's own disk
/// budget: OneDrive and SharePoint count versions against the account's storage instead, which
/// a local profile does not have. A single saved document may not be larger either.
pub const MAX_DOCUMENT_BYTES: u64 = 500 << 20;
/// At most 500 versions per document: the count limit of SharePoint's and OneDrive for work's
/// automatic version limits (Microsoft Learn, "Plan version storage for document libraries").
/// The pinned original (see [`prune`]) is counted in this limit.
pub const MAX_VERSIONS: usize = 500;
/// AutoSave writes inside one window update a single version. PowerPoint's AutoSave on OneDrive adds a
/// version at the first change after the document is opened and then only about every ten
/// minutes for the rest of the editing session (Microsoft Support, "What is AutoSave?").
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const AUTOSAVE_WINDOW_SECS: u64 = 10 * 60;
// Retention follows the automatic version limits of SharePoint and OneDrive for work (Microsoft
// Learn, "Plan version storage for document libraries"): every version for the first 30 days,
// then one per hour up to 60 days, one per day up to 180 days, and one per week after that, for
// as long as the count limit allows. Age alone therefore never empties a history: a document
// left untouched keeps its weekly versions. (A personal OneDrive keeps fewer, the last 25
// versions; DeckCraft keeps the larger schedule because this copy is local and private.)
// Microsoft describes the kept versions as those at the start of each hour, day or week;
// DeckCraft keeps the newest version of each period, the state the document was left in.
#[cfg(not(target_arch = "wasm32"))]
const DAY_SECS: u64 = 24 * 60 * 60;
/// Every version younger than this is kept.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const KEEP_ALL_SECS: u64 = 30 * DAY_SECS;
/// Up to this age, one version per hour is kept.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const KEEP_HOURLY_SECS: u64 = 60 * DAY_SECS;
/// Up to this age, one version per day is kept; older versions are kept one per week.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const KEEP_DAILY_SECS: u64 = 180 * DAY_SECS;
/// An object no kept version needs is deleted only once it has been unneeded this long. In a
/// folder two computers sync, one computer may see a new object before the manifest that uses
/// it arrives; the wait keeps it from deleting what the other computer just wrote.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const ORPHAN_GRACE_SECS: u64 = 7 * DAY_SECS;
#[cfg(not(target_arch = "wasm32"))]
const MAX_ID_LEN: usize = 32;
/// Most per-document folders looked at when a file with no history may have been moved.
#[cfg(not(target_arch = "wasm32"))]
const MAX_BUCKET_SCAN: usize = 20_000;
/// Most files looked at in one document's folder.
#[cfg(not(target_arch = "wasm32"))]
const MAX_RECOVERED_FILES: usize = 30_000;
/// Version history folders: the settings folder, another folder, or both.
pub const MAX_HISTORY_FOLDERS: usize = 2;

/// Another version this one came from: the document a copy or Save As was made of.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionOrigin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<String>,
}

/// The file as it was on disk just before the save that made this version replaced it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiskBefore {
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Counts about the presentation at this version: never any of its text.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionStats {
    pub slides: u64,
    pub shapes: u64,
    pub pictures: u64,
    /// Media items in the package (pictures, audio, video).
    pub media: u64,
    pub comments: u64,
    /// Slides with notes.
    pub notes_pages: u64,
}

/// The document's own properties (`docProps/core.xml`) at this version.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionProperties {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub creator: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_modified_by: String,
    #[serde(default)]
    pub revision: u64,
}

/// Where a version comes from and what it holds, recorded in its manifest. Nothing here names
/// the computer or the account: the device is a random installation id and a label the user
/// can change, and the path is relative to the user's or the provider's folder.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct VersionDetails {
    /// The version before this one in the document's history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<VersionOrigin>,
    /// Parts of the package that differ from the parent version (all of them for the first).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changed_parts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub removed_parts: Vec<String>,
    /// The local time zone's offset from UTC when it was saved, in seconds.
    pub utc_offset: i32,
    /// When the editing session (this opening of the document) began, and how long it had run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editing_seconds: Option<u64>,
    /// What made the save: `save`, `autosave`, `close`, `restore:<version>`, `saveAs`, `copy`,
    /// `beforeSave` (the file kept before DeckCraft wrote over it), `beforeRestore`.
    pub trigger: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub initials: String,
    /// This installation's random id and the device label (Options › Save).
    pub installation: String,
    pub device: String,
    pub os: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub os_version: String,
    pub app_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    /// `~/Documents/x.pptx`, `onedrive:/Folder/x.pptx`, or a path outside both.
    pub path: String,
    /// `local`, `icloud`, `onedrive` or `gdrive`.
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<DiskBefore>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<VersionProperties>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<VersionStats>,
}

/// Which history `file.versions={"delete": …}` removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteScope {
    Document,
    All,
}

/// What happens to the history already kept when the history folders change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relocation {
    Move,
    Copy,
    Leave,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct VersionWrite {
    path: PathBuf,
    bytes: Vec<u8>,
    author: String,
    kind: VersionKind,
    saved_at: String,
    /// The editing session of an AutoSave write; `None` never joins an existing version.
    session: Option<String>,
    /// The document's id: its history is kept under it. `None` keeps it by path.
    document_id: Option<String>,
    details: VersionDetails,
    cloud: Vec<deckcraft_cloud::CloudRoot>,
    /// The clock the store works by (seconds since the epoch): the time of the write, which
    /// retention and the AutoSave window count from. `None` reads the system clock.
    now: Option<u64>,
}

#[cfg(not(target_arch = "wasm32"))]
enum Job {
    Save(Box<VersionWrite>),
    /// List the versions of a document. With `probe`, a file kept by path and without history
    /// may look for the history of a file moved outside DeckCraft (it reads the file).
    List {
        path: PathBuf,
        document_id: Option<String>,
        probe: bool,
        cloud: Vec<deckcraft_cloud::CloudRoot>,
    },
    /// `file.rename` moved a document kept by path: its history follows it.
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    /// The history folders changed: copy (and with `keep_old` false, move) what the old ones
    /// hold into the new ones.
    Relocate {
        from: Vec<PathBuf>,
        to: Vec<PathBuf>,
        keep_old: bool,
    },
}

#[cfg(not(target_arch = "wasm32"))]
struct Event {
    result: Result<(), String>,
}

/// The identity record of a folder kept by path.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize, Serialize)]
struct DocumentMeta {
    path: String,
}

/// A version kept whole: its metadata, written once beside `<id>.bin`.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize, Serialize)]
struct WholeRecord {
    format: u32,
    info: VersionInfo,
}

/// What the worker and the synchronous history writes share, under one lock.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Shared {
    /// The history folders: the first is written first, all are written and read.
    roots: Vec<PathBuf>,
    /// Folders the last write could not reach (unplugged, no permission). They are brought up
    /// to date when they can be reached again.
    unavailable: Vec<PathBuf>,
    relocating: bool,
}

/// Desktop worker state, one for the whole session (every open presentation shares it).
/// Nothing in this struct is shared with the worker by reference, so a slow disk never holds a
/// borrow of the editing session. What belongs to one presentation lives in its
/// [`DocVersions`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Versions {
    /// Closed (taken) when the store is dropped, so the worker finishes what is queued and ends.
    tx: Option<mpsc::SyncSender<Job>>,
    worker: Option<std::thread::JoinHandle<()>>,
    rx: mpsc::Receiver<Event>,
    /// The settings folder's history folder, the default.
    default_root: PathBuf,
    shared: Arc<Mutex<Shared>>,
    /// Random per run of the app: with a presentation's uid it names an editing session.
    run_id: String,
    /// This installation's random id (see [`meta::installation_id`]).
    installation: String,
    /// Files whose history was already searched for (a file moved outside DeckCraft).
    probed: BTreeSet<PathBuf>,
    /// The clock versions are written by; `None` is the system clock (see [`Versions::set_clock`]).
    clock: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    /// Jobs queued and not yet reported back.
    queued: usize,
    pub busy: bool,
    pub error: Option<String>,
    alive: bool,
}

/// What a presentation keeps for its history: the editing session and what the store knows of its
/// file. Reset when another file is opened in its place.
#[derive(Clone, Debug, Default)]
pub struct DocVersions {
    /// Canonical path whose bytes on disk are known to be in the history.
    pub(crate) verified: Option<PathBuf>,
    /// Canonical path whose document id was checked against copies.
    pub(crate) identity_checked: Option<PathBuf>,
    /// The id this presentation's history is kept under, as last written to (or read from) its
    /// file. An Undo that goes back to a state without the id in its package must not make a
    /// second history, so the id is remembered here as well.
    pub(crate) id: Option<String>,
    /// When the editing session began (seconds since the epoch; 0 for none).
    pub(crate) opened_at: u64,
    /// How many times the file was read into this presentation again (taking the other
    /// version of a conflict): each starts a new editing session.
    pub(crate) epoch: u32,
    /// The kind the next version will have (a Keep both copy is a `conflictCopy`).
    pub(crate) next_kind: Option<VersionKind>,
    /// The last error of this presentation's history, if any.
    pub error: Option<String>,
}

impl DocVersions {
    /// A presentation just opened or created: its editing session begins now.
    pub fn started() -> Self {
        DocVersions { opened_at: now_unix(), ..Default::default() }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn private_dir(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            if metadata.is_dir() { Ok(()) } else { Err(io::Error::new(io::ErrorKind::AlreadyExists, "version history path is not a directory")) }
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn private_file(path: &Path, truncate: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(not(target_arch = "wasm32"))]
fn id_ok(id: &str) -> bool {
    id.len() == MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A document's folder name: 16 hex digits for a folder kept by path, 32 for a document id.
#[cfg(not(target_arch = "wasm32"))]
fn key_ok(name: &str) -> bool {
    matches!(name.len(), 16 | 32) && name.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(not(target_arch = "wasm32"))]
fn new_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// SHA-256 of `bytes` as lower-case hex: a digest that stays the same across runs and builds,
/// unlike the standard library's hasher.
#[cfg(not(target_arch = "wasm32"))]
fn digest_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Canonical paths are stable across runs.  A file that does not exist yet uses its canonical
/// parent and its own name, which keeps a Save As target in the same history bucket after the
/// first write.
#[cfg(not(target_arch = "wasm32"))]
pub fn canonical_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = fs::canonicalize(path) {
        return canonical;
    }
    let file = path.file_name().map(PathBuf::from);
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
    match file {
        Some(file) => parent.join(file),
        None => parent,
    }
}

#[cfg(target_arch = "wasm32")]
pub fn canonical_path(path: &Path) -> PathBuf {
    path.to_path_buf()
}

/// Two paths that name the same file (case aside on Windows).
#[cfg(not(target_arch = "wasm32"))]
fn same_file_path(a: &Path, b: &Path) -> bool {
    let (a, b) = (canonical_path(a), canonical_path(b));
    if cfg!(windows) { a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() } else { a == b }
}

/// FNV-1a of `text`, as 16 hex digits: a stable folder name for a document's history.
fn fnv_key(text: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// The history folder name of the document at `path`: a hash of its canonical path. Windows
/// file names ignore case, so there the path is lower-cased first and `Report.pptx` and
/// `report.pptx` share one history whichever spelling reaches the engine.
pub fn document_key(path: &Path) -> String {
    let canonical = canonical_path(path);
    let text = canonical.to_string_lossy();
    if cfg!(windows) { fnv_key(&text.to_lowercase()) } else { fnv_key(&text) }
}

/// The folder name earlier builds used: the canonical path with its case as given.
#[cfg(not(target_arch = "wasm32"))]
fn legacy_key(path: &Path) -> String {
    fnv_key(&canonical_path(path).to_string_lossy())
}

/// The history folder of the document at `path` kept by path. A folder an earlier build made
/// under the case-sensitive key is moved to the current key the first time it is looked up; if
/// the move fails it is used where it is, so nothing is hidden.
#[cfg(not(target_arch = "wasm32"))]
fn document_dir(root: &Path, path: &Path) -> PathBuf {
    let dir = root.join(document_key(path));
    if !cfg!(windows) || fs::symlink_metadata(&dir).is_ok() {
        return dir;
    }
    let legacy = root.join(legacy_key(path));
    if legacy == dir || !fs::symlink_metadata(&legacy).is_ok_and(|m| m.is_dir()) {
        return dir;
    }
    if fs::rename(&legacy, &dir).is_ok() || fs::symlink_metadata(&dir).is_ok() { dir } else { legacy }
}

/// The folder in `root` that holds a document's history: by its id, else by its path.
#[cfg(not(target_arch = "wasm32"))]
fn bucket_dir(root: &Path, path: &Path, document_id: Option<&str>) -> PathBuf {
    match document_id {
        Some(id) => root.join(meta::id_bucket(id)),
        None => document_dir(root, path),
    }
}

/// The document's folder in every history folder that can be reached now, and the history
/// folders that cannot.
#[cfg(not(target_arch = "wasm32"))]
fn bucket_dirs(roots: &[PathBuf], path: &Path, document_id: Option<&str>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut dirs = Vec::new();
    let mut unavailable = Vec::new();
    for root in roots {
        if private_dir(root).is_ok() {
            dirs.push(bucket_dir(root, path, document_id));
        } else {
            unavailable.push(root.clone());
        }
    }
    (dirs, unavailable)
}

#[cfg(not(target_arch = "wasm32"))]
fn parse_iso_secs(value: &str) -> Option<u64> {
    parse_iso_timestamp(value)
}

/// What is on disk at `path` right now, from its metadata alone (no hash).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn disk_before(path: &Path) -> Option<DiskBefore> {
    let meta = fs::metadata(path).ok()?;
    Some(DiskBefore { size: meta.len(), modified: modified_secs(&meta).map(iso_from_unix_secs), sha256: None })
}

#[cfg(not(target_arch = "wasm32"))]
fn modified_secs(meta: &fs::Metadata) -> Option<u64> {
    meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Newest first; within one second, the one written last first.
#[cfg(not(target_arch = "wasm32"))]
fn sort_newest_first(list: &mut [VersionInfo]) {
    list.sort_by_key(|v| std::cmp::Reverse((parse_iso_secs(&v.saved_at).unwrap_or(0), v.written.unwrap_or(0))));
}

/// A number that grows with every version this process writes, within one second.
#[cfg(not(target_arch = "wasm32"))]
fn write_order(now: u64) -> u64 {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    now.saturating_mul(1_000_000).saturating_add(COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % 1_000_000)
}

/// Read a small regular file, refusing anything larger than `max` or not a plain file.
#[cfg(not(target_arch = "wasm32"))]
fn read_small(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(max.saturating_add(1)).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > max {
        return None;
    }
    Some(bytes)
}

/// Parsed version records, by file: they never change once written, so a file seen with the
/// same size and date is not parsed again. Bounded; cleared when full.
#[cfg(not(target_arch = "wasm32"))]
fn record_cache() -> &'static Mutex<std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, VersionInfo)>> {
    static CACHE: OnceLock<Mutex<std::collections::HashMap<PathBuf, (u64, Option<std::time::SystemTime>, VersionInfo)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(not(target_arch = "wasm32"))]
fn cached_record(path: &Path, parse: impl FnOnce() -> Option<VersionInfo>) -> Option<VersionInfo> {
    let meta = fs::symlink_metadata(path).ok()?;
    let stamp = (meta.len(), meta.modified().ok());
    let cache = record_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((len, modified, info)) = cache.get(path)
        && (*len, *modified) == stamp
    {
        return Some(info.clone());
    }
    drop(cache);
    let info = parse()?;
    let mut cache = record_cache().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if cache.len() > 50_000 {
        cache.clear();
    }
    cache.insert(path.to_path_buf(), (stamp.0, stamp.1, info.clone()));
    Some(info)
}

#[cfg(not(target_arch = "wasm32"))]
fn info_ok(v: &VersionInfo) -> bool {
    id_ok(&v.id) && v.size <= MAX_DOCUMENT_BYTES && parse_iso_secs(&v.saved_at).is_some()
}

/// Every version recorded in one document folder, newest first, superseded ones included.
/// Each version describes itself in its own file (a manifest, or the metadata beside a whole
/// copy); the index earlier builds kept is still read. Files that are not ours, half-synced or
/// named like a sync app's conflict copy (`x (1).parts.json`) are ignored, never deleted, and a
/// version file nothing describes comes back as a `save` dated by its file.
#[cfg(not(target_arch = "wasm32"))]
fn read_index(dir: &Path) -> Vec<VersionInfo> {
    let Ok(dir_meta) = fs::symlink_metadata(dir) else { return Vec::new() };
    if !dir_meta.is_dir() {
        return Vec::new();
    }
    let mut found: std::collections::BTreeMap<String, VersionInfo> = std::collections::BTreeMap::new();
    let mut bins: Vec<(String, fs::Metadata)> = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten().take(MAX_RECOVERED_FILES) {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(id) = name.strip_suffix(PARTS_SUFFIX).filter(|id| id_ok(id)) {
                if let Some(manifest) = cached_record(&entry.path(), || read_manifest(dir, id).map(|m| m.info)) {
                    found.insert(id.to_owned(), VersionInfo { storage: VersionStorage::Parts, ..manifest });
                }
            } else if let Some(id) = name.strip_suffix(META_SUFFIX).filter(|id| id_ok(id)) {
                let record = cached_record(&entry.path(), || read_whole_record(dir, id));
                if let Some(info) = record
                    && fs::symlink_metadata(dir.join(format!("{id}.bin"))).is_ok_and(|m| m.is_file() && m.len() == info.size)
                {
                    found.entry(id.to_owned()).or_insert(VersionInfo { storage: VersionStorage::Whole, ..info });
                }
            } else if let Some(id) = name.strip_suffix(".bin").filter(|id| id_ok(id))
                && let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                bins.push((id.to_owned(), meta));
            }
        }
    }
    // The index of earlier builds: read, never written again.
    for v in legacy_index(dir) {
        if found.contains_key(&v.id) {
            continue;
        }
        let exists = match v.storage {
            VersionStorage::Whole => fs::symlink_metadata(dir.join(format!("{}.bin", v.id))).is_ok_and(|m| m.is_file() && m.len() == v.size),
            VersionStorage::Parts => false,
        };
        if exists {
            found.insert(v.id.clone(), v);
        }
    }
    for (id, meta) in bins {
        if found.contains_key(&id) || meta.len() > MAX_DOCUMENT_BYTES {
            continue;
        }
        found.insert(
            id.clone(),
            VersionInfo {
                id,
                saved_at: iso_from_unix_secs(modified_secs(&meta).unwrap_or(0)),
                size: meta.len(),
                kind: VersionKind::Save,
                ..VersionInfo::default()
            },
        );
    }
    let mut list: Vec<VersionInfo> = found.into_values().filter(info_ok).collect();
    sort_newest_first(&mut list);
    list
}

/// The versions a person sees: those no AutoSave write in the same window replaced.
#[cfg(not(target_arch = "wasm32"))]
fn visible(list: &[VersionInfo]) -> Vec<VersionInfo> {
    let superseded: BTreeSet<&str> = list.iter().filter_map(|v| v.supersedes.as_deref()).collect();
    list.iter().filter(|v| !superseded.contains(v.id.as_str())).cloned().collect()
}

/// The versions of a document across all its folders (a mirror may hold versions the first
/// folder does not have yet), newest first.
#[cfg(not(target_arch = "wasm32"))]
fn read_union(dirs: &[PathBuf]) -> Vec<VersionInfo> {
    let mut seen = BTreeSet::new();
    let mut all = Vec::new();
    for dir in dirs {
        for v in read_index(dir) {
            if seen.insert(v.id.clone()) {
                all.push(v);
            }
        }
    }
    sort_newest_first(&mut all);
    all
}

#[cfg(not(target_arch = "wasm32"))]
fn legacy_index(dir: &Path) -> Vec<VersionInfo> {
    read_small(&dir.join(INDEX_FILE), MAX_INDEX_BYTES)
        .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| serde_json::from_value::<VersionInfo>(value).ok())
        .filter(info_ok)
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn read_whole_record(dir: &Path, id: &str) -> Option<VersionInfo> {
    let bytes = read_small(&dir.join(format!("{id}{META_SUFFIX}")), MAX_META_BYTES)?;
    let record: WholeRecord = serde_json::from_slice(&bytes).ok()?;
    (record.format == 2 && record.info.id == id).then_some(record.info)
}

/// Rewrite an earlier build's folder in the current layout: the metadata of every whole version
/// moves from the shared `index.json` into its own file, then the index goes.
#[cfg(not(target_arch = "wasm32"))]
fn migrate_layout(dir: &Path) {
    let legacy = legacy_index(dir);
    if fs::symlink_metadata(dir.join(INDEX_FILE)).is_err() {
        return;
    }
    let mut complete = true;
    for v in legacy.iter().filter(|v| v.storage == VersionStorage::Whole) {
        let bin = dir.join(format!("{}.bin", v.id));
        if !bin.is_file() || dir.join(format!("{}{META_SUFFIX}", v.id)).is_file() {
            continue;
        }
        let record = WholeRecord { format: 2, info: v.clone() };
        let written = serde_json::to_vec_pretty(&record)
            .map_err(|e| e.to_string())
            .and_then(|json| write_atomic(&dir.join(format!("{}{META_SUFFIX}", v.id)), &json));
        complete &= written.is_ok();
    }
    if complete {
        let _ = fs::remove_file(dir.join(INDEX_FILE));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_meta(dir: &Path) -> Option<DocumentMeta> {
    let bytes = read_small(&dir.join(META_FILE), MAX_META_BYTES)?;
    serde_json::from_slice(&bytes).ok()
}

/// Record which file a folder kept by path belongs to (only when it changed), as a path
/// without the account name.
#[cfg(not(target_arch = "wasm32"))]
fn write_meta(dir: &Path, path: &Path, cloud: &[deckcraft_cloud::CloudRoot]) -> Result<(), String> {
    let text = meta::portable_path(path, cloud);
    if read_meta(dir).is_some_and(|m| m.path == text) {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(&DocumentMeta { path: text }).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(META_FILE), &bytes)
}

/// Write `bytes` to `path` through a temporary file: the bytes are flushed, read back and
/// compared before the temporary file replaces `path`, so a short or damaged write never
/// replaces good data and never enters the history.
#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    private_dir(parent).map_err(|e| e.to_string())?;
    let id = new_id()?;
    let tmp = parent.join(format!(".{}.{}.tmp", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), id));
    let result = (|| {
        let mut file = private_file(&tmp, true).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let limit = u64::try_from(bytes.len()).map_err(|e| e.to_string())?;
        let mut written = Vec::with_capacity(bytes.len());
        File::open(&tmp).map_err(|e| e.to_string())?.take(limit.saturating_add(1)).read_to_end(&mut written).map_err(|e| e.to_string())?;
        if written != bytes {
            return Err("version history write could not be verified".to_string());
        }
        drop(written);
        match fs::rename(&tmp, path) {
            Ok(()) => Ok(()),
            Err(first) if first.kind() == io::ErrorKind::AlreadyExists => {
                // Unix rename replaces an existing destination; Windows does not. Move the old
                // file aside so the prepared bytes can still be committed without truncating it.
                let backup = parent.join(format!(".{}.{}.bak", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), new_id()?));
                fs::rename(path, &backup).map_err(|error| format!("{first}; {error}"))?;
                match fs::rename(&tmp, path) {
                    Ok(()) => {
                        let _ = fs::remove_file(&backup);
                        Ok(())
                    }
                    Err(error) => {
                        let _ = fs::rename(&backup, path);
                        Err(error.to_string())
                    }
                }
            }
            Err(error) => Err(error.to_string()),
        }
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Copy `source` to `target` so that `target` appears whole or not at all.
#[cfg(not(target_arch = "wasm32"))]
fn copy_atomic(source: &Path, target: &Path) -> Result<(), String> {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    private_dir(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".{}.{}.tmp", target.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), new_id()?));
    let copied = fs::copy(source, &tmp).map_err(|e| e.to_string());
    let expected = fs::symlink_metadata(source).map(|m| m.len()).map_err(|e| e.to_string());
    let result = match (copied, expected) {
        (Ok(n), Ok(len)) if n == len => fs::rename(&tmp, target).map_err(|e| e.to_string()),
        (Err(e), _) | (_, Err(e)) => Err(e),
        _ => Err("copy was short".into()),
    };
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(not(target_arch = "wasm32"))]
fn write_version_file(dir: &Path, id: &str, bytes: &[u8]) -> Result<(), String> {
    write_atomic(&dir.join(format!("{id}.bin")), bytes)
}

#[cfg(not(target_arch = "wasm32"))]
fn manifest_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}{PARTS_SUFFIX}"))
}

#[cfg(not(target_arch = "wasm32"))]
fn object_path(dir: &Path, hash: &str) -> PathBuf {
    dir.join(OBJECTS_DIR).join(hash)
}

/// The manifest of version `id`, checked before anything is built from it.
#[cfg(not(target_arch = "wasm32"))]
fn read_manifest(dir: &Path, id: &str) -> Option<pack::Manifest> {
    let bytes = read_small(&manifest_path(dir, id), MAX_MANIFEST_BYTES)?;
    let manifest: pack::Manifest = serde_json::from_slice(&bytes).ok()?;
    (pack::manifest_ok(&manifest) && manifest.info.id == id).then_some(manifest)
}

/// The header of an object file, and the file's size.
#[cfg(not(target_arch = "wasm32"))]
fn object_header(dir: &Path, hash: &str) -> Result<(pack::ObjectHeader, u64), String> {
    let path = object_path(dir, hash);
    let meta = fs::symlink_metadata(&path).map_err(|_| "a part of this version is missing".to_string())?;
    if !meta.is_file() {
        return Err("a part of this version is missing".into());
    }
    let mut head = Vec::with_capacity(pack::HEADER_MAX);
    File::open(&path).map_err(|e| e.to_string())?.take(pack::HEADER_MAX as u64).read_to_end(&mut head).map_err(|e| e.to_string())?;
    Ok((pack::ObjectHeader::parse(&head)?, meta.len()))
}

/// The content of object `hash`, looked for in each of `dirs` (a document's folders): its chain
/// of deltas is followed back to a whole copy, and every step is checked against its own name,
/// so a damaged object anywhere in the chain is an error for the versions that use it and
/// nothing else.
#[cfg(not(target_arch = "wasm32"))]
fn load_object(dirs: &[PathBuf], hash: &str) -> Result<Vec<u8>, String> {
    let mut chain: Vec<(String, Vec<u8>)> = Vec::new();
    let mut current = hash.to_owned();
    loop {
        if !pack::hash_ok(&current) || chain.len() >= pack::MAX_CHAIN {
            return Err("a part of this version is damaged".into());
        }
        let bytes =
            dirs.iter().find_map(|dir| read_small(&object_path(dir, &current), MAX_OBJECT_FILE)).ok_or("a part of this version is missing")?;
        let base = pack::ObjectHeader::parse(&bytes)?.base;
        chain.push((current, bytes));
        match base {
            Some(base) => current = base,
            None => break,
        }
    }
    let mut content: Option<Vec<u8>> = None;
    for (name, bytes) in chain.iter().rev() {
        let next = pack::decode(bytes, content.as_deref())?;
        if digest_hex(&next) != *name {
            return Err("a part of this version is damaged (its checksum does not match)".into());
        }
        content = Some(next);
    }
    content.ok_or_else(|| "a part of this version is missing".into())
}

/// Store the parts of a version as objects in `dir`, each a delta against the same part of
/// `base` when that pays. An object already there is reused only when it still reads back
/// correctly; a damaged one is written again whole, which also repairs every version that uses
/// it (an object's name is its content, so rewriting it changes nothing for anyone else).
/// Returns the bytes newly written.
#[cfg(not(target_arch = "wasm32"))]
fn store_parts(dir: &Path, parts: &[pack::Part], base: Option<&pack::Manifest>) -> Result<u64, String> {
    private_dir(&dir.join(OBJECTS_DIR)).map_err(|e| e.to_string())?;
    let here = [dir.to_path_buf()];
    let mut written = 0u64;
    for part in parts.iter().filter(|part| !part.meta.dir) {
        let hash = part.meta.sha256.as_str();
        let path = object_path(dir, hash);
        let object = if fs::symlink_metadata(&path).is_ok() {
            if load_object(&here, hash).is_ok() {
                continue;
            }
            pack::encode_full(&part.data)
        } else {
            let base_part = base.and_then(|manifest| manifest.parts.iter().find(|p| !p.dir && p.name == part.meta.name && p.sha256 != hash));
            let base = base_part.and_then(|p| {
                let depth = object_header(dir, &p.sha256).ok()?.0.depth;
                Some((p.sha256.clone(), depth, load_object(&here, &p.sha256).ok()?))
            });
            pack::encode(&part.data, base.as_ref().map(|(hash, depth, content)| (hash.as_str(), *depth, content.as_slice())))
        };
        write_atomic(&path, &object)?;
        if let Err(error) = load_object(&here, hash) {
            let _ = fs::remove_file(&path);
            return Err(error);
        }
        written = written.saturating_add(object.len() as u64);
    }
    Ok(written)
}

/// The newest version kept as parts whose manifest reads: the base for the next deltas.
#[cfg(not(target_arch = "wasm32"))]
fn newest_manifest(dir: &Path, list: &[VersionInfo]) -> Option<pack::Manifest> {
    list.iter().filter(|v| v.storage == VersionStorage::Parts).find_map(|v| read_manifest(dir, &v.id))
}

/// Which parts changed from `parent` (all of them without one) and which went.
#[cfg(not(target_arch = "wasm32"))]
fn part_changes(parts: &[pack::ManifestPart], parent: Option<&pack::Manifest>) -> (Vec<String>, Vec<String>) {
    let Some(parent) = parent else { return (parts.iter().filter(|p| !p.dir).map(|p| p.name.clone()).collect(), Vec::new()) };
    let before: std::collections::HashMap<&str, &str> = parent.parts.iter().map(|p| (p.name.as_str(), p.sha256.as_str())).collect();
    let now: BTreeSet<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    let changed = parts.iter().filter(|p| !p.dir && before.get(p.name.as_str()) != Some(&p.sha256.as_str())).map(|p| p.name.clone()).collect();
    let removed = parent.parts.iter().filter(|p| !now.contains(p.name.as_str())).map(|p| p.name.clone()).collect();
    (changed, removed)
}

/// Keep `bytes` (a zip package) as parts for version `info`: objects first, then the manifest,
/// which is never rewritten. The archive is rebuilt in memory and split again before anything is
/// written, so a version stored as parts is known to come back with the same parts. Returns the
/// space it added.
#[cfg(not(target_arch = "wasm32"))]
fn write_parts_version(
    dir: &Path,
    info: &VersionInfo,
    bytes: &[u8],
    base: Option<&pack::Manifest>,
    parent: Option<&pack::Manifest>,
) -> Result<u64, String> {
    let (parts, comment) = pack::split_zip(bytes).ok_or("not a zip package that can be kept as parts")?;
    let metas: Vec<pack::ManifestPart> = parts.iter().map(|part| part.meta.clone()).collect();
    let mut info = VersionInfo { storage: VersionStorage::Parts, stored: None, ..info.clone() };
    if let Some(details) = info.details.as_mut() {
        let (changed, removed) = part_changes(&metas, parent);
        details.changed_parts = changed;
        details.removed_parts = removed;
    }
    let manifest = pack::Manifest { format: 1, info, comment, parts: metas };
    let contents: Vec<Vec<u8>> = parts.iter().map(|part| part.data.clone()).collect();
    let rebuilt = pack::assemble_zip(&manifest, &contents)?;
    drop(contents);
    let same = pack::split_zip(&rebuilt)
        .is_some_and(|(again, again_comment)| again_comment == manifest.comment && again.iter().map(|part| &part.meta).eq(manifest.parts.iter()));
    if !same {
        return Err("the package does not come back the same when rebuilt".into());
    }
    let added = store_parts(dir, &parts, base)?;
    let json = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    write_atomic(&manifest_path(dir, &manifest.info.id), &json)?;
    Ok(added.saturating_add(json.len() as u64))
}

/// Keep `bytes` whole for version `info`: the bytes, then their metadata.
#[cfg(not(target_arch = "wasm32"))]
fn write_whole_version(dir: &Path, info: &VersionInfo, bytes: &[u8]) -> Result<u64, String> {
    write_version_file(dir, &info.id, bytes)?;
    let record = WholeRecord { format: 2, info: VersionInfo { storage: VersionStorage::Whole, ..info.clone() } };
    let json = serde_json::to_vec_pretty(&record).map_err(|e| e.to_string())?;
    if let Err(error) = write_atomic(&dir.join(format!("{}{META_SUFFIX}", info.id)), &json) {
        let _ = fs::remove_file(dir.join(format!("{}.bin", info.id)));
        return Err(error);
    }
    Ok(bytes.len() as u64)
}

/// The bytes of a version kept as parts: the archive rebuilt from its manifest. Every part is
/// checked against its hash; the zip around them may differ from the saved file's bytes.
#[cfg(not(target_arch = "wasm32"))]
fn read_parts_version(dirs: &[PathBuf], id: &str) -> Result<Vec<u8>, String> {
    let manifest = dirs.iter().find_map(|dir| read_manifest(dir, id)).ok_or("this version's list of parts is missing or damaged")?;
    let mut contents = Vec::with_capacity(manifest.parts.len());
    for part in &manifest.parts {
        if part.dir {
            contents.push(Vec::new());
            continue;
        }
        let content = load_object(dirs, &part.sha256)?;
        if content.len() as u64 != part.size {
            return Err("a part of this version is damaged".into());
        }
        contents.push(content);
    }
    pack::assemble_zip(&manifest, &contents)
}

/// The bytes of a version kept whole, from whichever folder has them.
#[cfg(not(target_arch = "wasm32"))]
fn read_whole_version(dirs: &[PathBuf], info: &VersionInfo) -> Result<Vec<u8>, String> {
    let mut last = "version bytes are missing".to_string();
    for dir in dirs {
        let file_path = dir.join(format!("{}.bin", info.id));
        let Ok(meta) = fs::symlink_metadata(&file_path) else { continue };
        if !meta.is_file() || meta.len() != info.size || meta.len() > MAX_DOCUMENT_BYTES {
            last = "version bytes are missing or invalid".into();
            continue;
        }
        let Some(bytes) = read_small(&file_path, MAX_DOCUMENT_BYTES) else { continue };
        if info.digest.as_deref().is_some_and(|digest| digest != digest_hex(&bytes)) {
            last = "version bytes are damaged (their checksum does not match)".into();
            continue;
        }
        return Ok(bytes);
    }
    Err(last)
}

/// Versions written whole by earlier builds move to parts a little at a time: at most one per
/// save, newest first, and only after the parts read back. The new manifest has the version's
/// id, so the version keeps its identity; the whole copy goes after.
#[cfg(not(target_arch = "wasm32"))]
fn migrate_one(dir: &Path, list: &[VersionInfo]) {
    let base = newest_manifest(dir, list);
    let Some(entry) = list.iter().find(|v| v.storage == VersionStorage::Whole && v.stored.is_none() && v.kind != VersionKind::Original) else {
        return;
    };
    let bin = dir.join(format!("{}.bin", entry.id));
    let Some(bytes) = read_small(&bin, MAX_DOCUMENT_BYTES) else { return };
    let digest = digest_hex(&bytes);
    let mut info = entry.clone();
    let convertible = bytes.starts_with(b"PK\x03\x04") && info.digest.as_ref().is_none_or(|known| *known == digest);
    info.digest = Some(digest);
    if convertible && let Ok(stored) = write_parts_version(dir, &info, &bytes, base.as_ref(), None) {
        let _ = stored;
        let _ = fs::remove_file(&bin);
        let _ = fs::remove_file(dir.join(format!("{}{META_SUFFIX}", entry.id)));
        return;
    }
    // Not tried again: the metadata now says how much it takes.
    let record = WholeRecord { format: 2, info: VersionInfo { stored: Some(entry.size), ..info } };
    if let Ok(json) = serde_json::to_vec_pretty(&record) {
        let _ = write_atomic(&dir.join(format!("{}{META_SUFFIX}", entry.id)), &json);
    }
}

/// What versions take on disk, with shared parts counted once.
#[cfg(not(target_arch = "wasm32"))]
struct Usage<'a> {
    dir: &'a Path,
    objects: std::collections::HashMap<String, (u64, Option<String>)>,
    /// Something could not be read: object files are then left alone this time.
    incomplete: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> Usage<'a> {
    fn new(dir: &'a Path) -> Self {
        Usage { dir, objects: std::collections::HashMap::new(), incomplete: false }
    }

    /// The objects a version needs (its parts and the bases of their deltas) and the size of
    /// its own files.
    fn needs(&mut self, entry: &VersionInfo) -> (BTreeSet<String>, u64) {
        let mut needed = BTreeSet::new();
        if entry.storage == VersionStorage::Whole {
            return (needed, entry.size);
        }
        let own = fs::symlink_metadata(manifest_path(self.dir, &entry.id)).map_or(0, |m| m.len());
        let Some(manifest) = read_manifest(self.dir, &entry.id) else {
            self.incomplete = true;
            return (needed, own);
        };
        for part in manifest.parts.iter().filter(|part| !part.dir) {
            let mut current = Some(part.sha256.clone());
            let mut steps = 0;
            while let Some(hash) = current.take() {
                steps += 1;
                if steps > pack::MAX_CHAIN || !needed.insert(hash.clone()) {
                    break;
                }
                if !self.objects.contains_key(&hash) {
                    match object_header(self.dir, &hash) {
                        Ok((header, len)) => {
                            self.objects.insert(hash.clone(), (len, header.base));
                        }
                        Err(_) => {
                            self.incomplete = true;
                            continue;
                        }
                    }
                }
                current = self.objects.get(&hash).and_then(|(_, base)| base.clone());
            }
        }
        (needed, own)
    }

    fn size(&self, hash: &str) -> u64 {
        self.objects.get(hash).map_or(0, |(len, _)| *len)
    }
}

/// The time retention counts from: the capture time of an `original`, else when it was saved.
#[cfg(not(target_arch = "wasm32"))]
fn retention_secs(entry: &VersionInfo) -> Option<u64> {
    entry.captured_at.as_deref().and_then(parse_iso_secs).or_else(|| parse_iso_secs(&entry.saved_at))
}

/// Delete the files of versions `ids` in `dir`.
#[cfg(not(target_arch = "wasm32"))]
fn remove_versions(dir: &Path, ids: &BTreeSet<String>) {
    for id in ids.iter().filter(|id| id_ok(id)) {
        for name in [format!("{id}.bin"), format!("{id}{META_SUFFIX}"), format!("{id}{PARTS_SUFFIX}")] {
            let _ = fs::remove_file(dir.join(name));
        }
    }
}

/// Apply the retention rules to every version in `all` (superseded ones included) and delete
/// the files of the versions that go. The decision depends only on the versions' own records,
/// so two computers sharing a folder make the same one. Returns what is kept (newest first) and
/// the ids removed:
/// - versions an AutoSave write in the same window replaced go;
/// - every version kept for [`KEEP_ALL_SECS`], then the newest of each hour until
///   [`KEEP_HOURLY_SECS`], of each day until [`KEEP_DAILY_SECS`] and of each week after that;
/// - at most [`MAX_VERSIONS`] versions and [`MAX_DOCUMENT_BYTES`] of real disk space per
///   document (a part shared by several versions counts once), newest first;
/// - the oldest `original` (the file as it was before DeckCraft first wrote it) is pinned: it
///   outlives the time rules and is the last to go for the count and size limits. This goes
///   beyond OneDrive on purpose: AutoSave is on for local files that never had a server-side
///   history, so this copy is the only way back to what was there.
///
/// Objects are not touched here; [`collect_objects`] does that, with a grace period.
#[cfg(not(target_arch = "wasm32"))]
fn prune(dir: &Path, all: Vec<VersionInfo>, now: u64) -> (Vec<VersionInfo>, BTreeSet<String>) {
    let mut list = visible(&all);
    sort_newest_first(&mut list);
    let pinned = list.iter().rev().find(|v| v.kind == VersionKind::Original).map(|v| v.id.clone());
    let room = if pinned.is_some() { MAX_VERSIONS.saturating_sub(1) } else { MAX_VERSIONS };
    let mut kept = Vec::new();
    let mut pinned_entry = None;
    let mut periods = BTreeSet::new();
    for entry in list {
        if pinned.as_deref() == Some(entry.id.as_str()) {
            pinned_entry = Some(entry);
            continue;
        }
        if kept.len() >= room {
            continue;
        }
        let age = now.saturating_sub(retention_secs(&entry).unwrap_or(now));
        let at = parse_iso_secs(&entry.saved_at).unwrap_or(0);
        // Hours, days and weeks since the epoch; weeks start on Monday (the epoch was a Thursday).
        let period = if age <= KEEP_ALL_SECS {
            None
        } else if age <= KEEP_HOURLY_SECS {
            Some((0u8, at / 3_600))
        } else if age <= KEEP_DAILY_SECS {
            Some((1, at / DAY_SECS))
        } else {
            Some((2, (at / DAY_SECS).saturating_add(3) / 7))
        };
        if period.is_none_or(|period| periods.insert(period)) {
            kept.push(entry);
        }
    }
    // Real space: each version's own file plus the objects it needs, shared objects once.
    let mut usage = Usage::new(dir);
    let mut needs: Vec<(BTreeSet<String>, u64)> = kept.iter().map(|entry| usage.needs(entry)).collect();
    let pinned_needs = pinned_entry.as_ref().map(|entry| usage.needs(entry));
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut total = 0u64;
    for (objects, own) in needs.iter().chain(pinned_needs.iter()) {
        total = total.saturating_add(*own);
        for hash in objects {
            let count = counts.entry(hash.clone()).or_insert(0);
            if *count == 0 {
                total = total.saturating_add(usage.size(hash));
            }
            *count += 1;
        }
    }
    // The newest version always stays, even alone over the budget.
    while total > MAX_DOCUMENT_BYTES && kept.len() > 1 {
        kept.pop();
        let Some((objects, own)) = needs.pop() else { break };
        total = total.saturating_sub(own);
        for hash in objects {
            if let Some(count) = counts.get_mut(&hash) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    total = total.saturating_sub(usage.size(&hash));
                }
            }
        }
    }
    if let Some(entry) = pinned_entry {
        kept.push(entry);
        sort_newest_first(&mut kept);
    }
    let keep: BTreeSet<&str> = kept.iter().map(|v| v.id.as_str()).collect();
    let removed: BTreeSet<String> = all.iter().filter(|v| !keep.contains(v.id.as_str())).map(|v| v.id.clone()).collect();
    remove_versions(dir, &removed);
    (kept, removed)
}

/// Delete the objects of `dir` no kept version needs, directly or as the base of a delta, once
/// they have been unneeded for [`ORPHAN_GRACE_SECS`]. The first time an object is found
/// unneeded, a dated mark is left in `gc/`; a mark goes again when a version needs the object.
/// When anything could not be read, nothing is marked or deleted: an unreadable manifest must
/// not cost its parts.
#[cfg(not(target_arch = "wasm32"))]
fn collect_objects(dir: &Path, kept: &[VersionInfo], now: u64) {
    let mut usage = Usage::new(dir);
    let mut needed = BTreeSet::new();
    for entry in kept {
        needed.extend(usage.needs(entry).0);
    }
    if usage.incomplete {
        return;
    }
    let gc = dir.join(GC_DIR);
    let mut marks: std::collections::HashMap<String, (u64, Vec<PathBuf>)> = std::collections::HashMap::new();
    if let Ok(entries) = fs::read_dir(&gc) {
        for entry in entries.flatten().take(MAX_RECOVERED_FILES) {
            let name = entry.file_name();
            let Some((hash, at)) = name.to_str().and_then(|n| n.split_once('.')) else { continue };
            let (Some(at), true) = (at.parse::<u64>().ok(), pack::hash_ok(hash)) else { continue };
            let slot = marks.entry(hash.to_owned()).or_insert((at, Vec::new()));
            slot.0 = slot.0.min(at);
            slot.1.push(entry.path());
        }
    }
    let mut present = BTreeSet::new();
    if let Ok(entries) = fs::read_dir(dir.join(OBJECTS_DIR)) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(hash) = name.to_str().filter(|n| pack::hash_ok(n)) else { continue };
            present.insert(hash.to_owned());
            if needed.contains(hash) {
                continue;
            }
            match marks.get(hash) {
                None => {
                    let _ = private_dir(&gc).and_then(|()| private_file(&gc.join(format!("{hash}.{now}")), true).map(drop));
                }
                Some((since, files)) if now.saturating_sub(*since) >= ORPHAN_GRACE_SECS => {
                    if fs::remove_file(entry.path()).is_ok() {
                        for file in files {
                            let _ = fs::remove_file(file);
                        }
                    }
                }
                Some(_) => {}
            }
        }
    }
    // Marks of objects needed again, or gone.
    for (hash, (_, files)) in &marks {
        if needed.contains(hash) || !present.contains(hash) {
            for file in files {
                let _ = fs::remove_file(file);
            }
        }
    }
}

/// Is `name` one of the immutable files of a document folder (a version file)?
#[cfg(not(target_arch = "wasm32"))]
fn version_file_ok(name: &str) -> bool {
    [PARTS_SUFFIX, META_SUFFIX, ".bin"].iter().any(|suffix| name.strip_suffix(suffix).is_some_and(id_ok))
}

/// Bring a document's folders in step: each gets the version files and objects another one has
/// and it lacks. They are immutable, so copying never overwrites anything.
#[cfg(not(target_arch = "wasm32"))]
fn reconcile(dirs: &[PathBuf]) {
    if dirs.len() < 2 {
        return;
    }
    let list = |dir: &Path, sub: Option<&str>, ok: fn(&str) -> bool| -> BTreeSet<String> {
        let at = sub.map_or_else(|| dir.to_path_buf(), |s| dir.join(s));
        fs::read_dir(at)
            .map(|entries| {
                entries.flatten().filter_map(|e| e.file_name().to_str().map(str::to_owned)).filter(|n| ok(n)).take(MAX_RECOVERED_FILES).collect()
            })
            .unwrap_or_default()
    };
    for (sub, ok) in [(Some(OBJECTS_DIR), pack::hash_ok as fn(&str) -> bool), (None, version_file_ok as fn(&str) -> bool)] {
        let sets: Vec<BTreeSet<String>> = dirs.iter().map(|dir| list(dir, sub, ok)).collect();
        for (i, dir) in dirs.iter().enumerate() {
            for (j, other) in dirs.iter().enumerate() {
                if i == j {
                    continue;
                }
                let (Some(mine), Some(theirs)) = (sets.get(i), sets.get(j)) else { continue };
                for name in theirs.difference(mine) {
                    let (from, to) = match sub {
                        Some(s) => (other.join(s).join(name), dir.join(s).join(name)),
                        None => (other.join(name), dir.join(name)),
                    };
                    if fs::symlink_metadata(&to).is_err() {
                        let _ = copy_atomic(&from, &to);
                    }
                }
            }
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn store_save(roots: &[PathBuf], write: VersionWrite) -> Result<(Vec<VersionInfo>, Vec<PathBuf>), String> {
    let now = write.now.unwrap_or_else(now_unix);
    store_save_at(roots, write, now)
}

/// A document folder in each history folder, brought up to date: the layout of earlier builds
/// migrated, a document with an id given the history it had by path, and the folders
/// reconciled.
#[cfg(not(target_arch = "wasm32"))]
fn prepare_dirs(
    roots: &[PathBuf],
    path: &Path,
    document_id: Option<&str>,
    cloud: &[deckcraft_cloud::CloudRoot],
    now: u64,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (dirs, unavailable) = bucket_dirs(roots, path, document_id);
    for (dir, root) in dirs.iter().zip(roots.iter().filter(|r| !unavailable.contains(r))) {
        migrate_layout(dir);
        // The history this file had by path before it carried an id moves under the id.
        if document_id.is_some() {
            let by_path = document_dir(root, path);
            if by_path != *dir && fs::symlink_metadata(&by_path).is_ok() {
                let _ = merge_bucket(&by_path, dir, path, cloud, now);
            }
        }
    }
    reconcile(&dirs);
    (dirs, unavailable)
}

#[cfg(not(target_arch = "wasm32"))]
fn store_save_at(roots: &[PathBuf], mut write: VersionWrite, now: u64) -> Result<(Vec<VersionInfo>, Vec<PathBuf>), String> {
    let size = u64::try_from(write.bytes.len()).map_err(|_| "version is too large".to_string())?;
    if size > MAX_DOCUMENT_BYTES {
        return Err("version exceeds the 500 MiB document history limit".into());
    }
    let (dirs, unavailable) = prepare_dirs(roots, &write.path, write.document_id.as_deref(), &write.cloud, now);
    let dir = dirs.first().cloned().ok_or("no version history folder can be reached")?;
    private_dir(&dir).map_err(|e| e.to_string())?;
    let list = visible(&read_index(&dir));
    let digest = digest_hex(&write.bytes);
    let window_start = |v: &VersionInfo| v.window_start.clone().unwrap_or_else(|| v.saved_at.clone());
    // AutoSave joins the newest version only when its own editing session opened that window
    // less than AUTOSAVE_WINDOW_SECS ago. Joining writes a new version that supersedes it:
    // nothing already written is ever rewritten.
    let replace = write.kind == VersionKind::Autosave
        && write.session.is_some()
        && list.first().is_some_and(|v| {
            v.kind == VersionKind::Autosave
                && v.session == write.session
                && now.saturating_sub(parse_iso_secs(&window_start(v)).unwrap_or(0)) < AUTOSAVE_WINDOW_SECS
        });
    let replaced = if replace { list.first().cloned() } else { None };
    let id = new_id()?;
    let parent_info = match &replaced {
        Some(old) => old.details.as_ref().and_then(|d| d.parent.clone()).and_then(|p| list.iter().find(|v| v.id == p).cloned()),
        None => list.first().cloned(),
    };
    let details = &mut write.details;
    details.parent = parent_info.as_ref().map(|v| v.id.clone());
    if details.os_version.is_empty() {
        details.os_version = meta::os_version();
    }
    if let Some(before) = details.before.as_mut()
        && before.sha256.is_none()
    {
        before.sha256 = parent_info.as_ref().and_then(|v| v.digest.clone());
    }
    let captured_at = matches!(write.kind, VersionKind::Original | VersionKind::External).then(|| iso_from_unix_secs(now));
    let session = if write.kind == VersionKind::Autosave { write.session.clone() } else { None };
    let mut info = VersionInfo {
        id: id.clone(),
        saved_at: write.saved_at.clone(),
        author: write.author.clone(),
        size,
        kind: write.kind,
        window_start: replaced.as_ref().map(window_start),
        digest: Some(digest),
        session,
        captured_at,
        storage: VersionStorage::Whole,
        stored: Some(size),
        supersedes: replaced.as_ref().map(|v| v.id.clone()),
        document_id: write.document_id.clone(),
        details: Some(Box::new(write.details.clone())),
        written: Some(write_order(now)),
    };
    // A zip package is kept as parts; the `original` and `external` copies are kept byte for
    // byte, as they were on disk. Anything that goes wrong with the parts falls back to the
    // whole bytes. An AutoSave that replaces the newest version deltas against the one before
    // it, so the replaced version's parts are not kept alive as a base.
    let verbatim = matches!(write.kind, VersionKind::Original | VersionKind::External);
    let base_list: &[VersionInfo] = if replace { list.get(1..).unwrap_or_default() } else { &list };
    let base = newest_manifest(&dir, base_list);
    let parent_manifest = parent_info.as_ref().and_then(|v| read_manifest(&dir, &v.id));
    let as_parts = !verbatim
        && write.bytes.starts_with(b"PK\x03\x04")
        && match write_parts_version(&dir, &info, &write.bytes, base.as_ref(), parent_manifest.as_ref()) {
            Ok(stored) => {
                info.storage = VersionStorage::Parts;
                info.stored = Some(stored);
                true
            }
            Err(error) => {
                log::debug!("version history keeps this version whole: {error}");
                false
            }
        };
    if !as_parts {
        info.stored = Some(write_whole_version(&dir, &info, &write.bytes)?);
    }
    let all = read_index(&dir);
    migrate_one(&dir, &visible(&all));
    let (kept, removed) = prune(&dir, read_index(&dir), now);
    for (i, other) in dirs.iter().enumerate() {
        if i > 0 {
            remove_versions(other, &removed);
        }
        collect_objects(other, &kept, now);
    }
    reconcile(&dirs);
    if write.document_id.is_none() {
        // The identity record only helps find a moved file; failing to write it loses nothing.
        let _ = write_meta(&dir, &write.path, &write.cloud);
    }
    let mut kept = kept;
    // `stored` of the version just written is known here, not in its immutable record.
    if let Some(entry) = kept.iter_mut().find(|v| v.id == info.id) {
        entry.stored = info.stored;
    }
    Ok((kept, unavailable))
}

/// The digest of a stored version: from its record, or computed from its file for older ones.
#[cfg(not(target_arch = "wasm32"))]
fn entry_digest(dirs: &[PathBuf], entry: &VersionInfo) -> Option<String> {
    if let Some(digest) = &entry.digest {
        return Some(digest.clone());
    }
    let bytes = dirs.iter().find_map(|dir| read_small(&dir.join(format!("{}.bin", entry.id)), MAX_DOCUMENT_BYTES))?;
    Some(digest_hex(&bytes))
}

/// Move the history in `from_dir` into `to_dir` (the folder of `to_path`), merging with what
/// `to_dir` holds. Every file keeps its name (random ids, content hashes), so merging never
/// overwrites.
#[cfg(not(target_arch = "wasm32"))]
fn merge_bucket(from_dir: &Path, to_dir: &Path, to_path: &Path, cloud: &[deckcraft_cloud::CloudRoot], now: u64) -> Result<Vec<VersionInfo>, String> {
    let _ = now;
    if from_dir == to_dir {
        return Ok(visible(&read_index(to_dir)));
    }
    migrate_layout(from_dir);
    let moving = read_index(from_dir);
    if moving.is_empty() {
        return Ok(visible(&read_index(to_dir)));
    }
    let by_path = to_dir.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.len() == 16);
    if fs::symlink_metadata(to_dir).is_err() && fs::rename(from_dir, to_dir).is_ok() {
        if by_path {
            let _ = write_meta(to_dir, to_path, cloud);
        }
        return Ok(visible(&read_index(to_dir)));
    }
    private_dir(to_dir).map_err(|e| e.to_string())?;
    migrate_layout(to_dir);
    let mut left_behind = false;
    // Objects first, so a moved manifest never names a part that is not there yet.
    if let Ok(objects) = fs::read_dir(from_dir.join(OBJECTS_DIR)) {
        private_dir(&to_dir.join(OBJECTS_DIR)).map_err(|e| e.to_string())?;
        for object in objects.flatten() {
            let name = object.file_name();
            if !name.to_str().is_some_and(pack::hash_ok) {
                continue;
            }
            let target = to_dir.join(OBJECTS_DIR).join(&name);
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            let source = object.path();
            if fs::rename(&source, &target).is_err() && copy_atomic(&source, &target).is_err() {
                left_behind = true;
            }
        }
    }
    if let Ok(entries) = fs::read_dir(from_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_str().is_some_and(version_file_ok) {
                continue;
            }
            let target = to_dir.join(&name);
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            let source = entry.path();
            if fs::rename(&source, &target).is_err() && copy_atomic(&source, &target).is_err() {
                left_behind = true;
            }
        }
    }
    if by_path {
        let _ = write_meta(to_dir, to_path, cloud);
    }
    if !left_behind {
        let _ = fs::remove_dir_all(from_dir);
    }
    Ok(visible(&read_index(to_dir)))
}

/// The history of a file kept by path that was moved or renamed outside DeckCraft. A folder
/// qualifies when the file it last belonged to no longer exists (a copy keeps its original, so
/// a copy never takes the history, as with OneDrive, where only a move keeps it) and its newest
/// version holds exactly `digest`, the bytes now at `path`. Several candidates: the most
/// recently saved wins.
#[cfg(not(target_arch = "wasm32"))]
fn adopt_moved(root: &Path, path: &Path, digest: &str, cloud: &[deckcraft_cloud::CloudRoot], now: u64) -> Option<Vec<VersionInfo>> {
    let target = document_dir(root, path);
    let mut best: Option<(u64, PathBuf)> = None;
    for entry in fs::read_dir(root).ok()?.flatten().take(MAX_BUCKET_SCAN) {
        let dir = entry.path();
        if dir == target || !entry.file_name().to_str().is_some_and(|n| n.len() == 16 && key_ok(n)) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(record) = read_meta(&dir) else { continue };
        let Some(previous) = meta::resolve_portable_path(&record.path, cloud) else { continue };
        if same_file_path(&previous, path) || fs::symlink_metadata(&previous).is_ok() {
            continue;
        }
        let list = visible(&read_index(&dir));
        let Some(newest) = list.first() else { continue };
        if newest.digest.as_deref() != Some(digest) {
            continue;
        }
        let saved = parse_iso_secs(&newest.saved_at).unwrap_or(0);
        if best.as_ref().is_none_or(|(at, _)| saved > *at) {
            best = Some((saved, dir));
        }
    }
    let (_, dir) = best?;
    merge_bucket(&dir, &target, path, cloud, now).ok().filter(|list| !list.is_empty())
}

/// Read the document file at `path` for the history (at most [`MAX_DOCUMENT_BYTES`]).
#[cfg(not(target_arch = "wasm32"))]
fn read_document(path: &Path) -> Result<Option<(Vec<u8>, fs::Metadata)>, String> {
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    if !meta.is_file() {
        return Ok(None);
    }
    if meta.len() > MAX_DOCUMENT_BYTES {
        return Err("the file is larger than the 500 MiB document history limit; its current bytes were not kept".into());
    }
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_DOCUMENT_BYTES.saturating_add(1)).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err("the file is larger than the 500 MiB document history limit".into());
    }
    Ok(Some((bytes, meta)))
}

/// Make sure the bytes at `path` are in the document's history before a save replaces them,
/// storing them as an `original` version (no history yet) or an `external` one (another program
/// changed the file) when they are not. Returns what was on disk.
#[cfg(not(target_arch = "wasm32"))]
fn store_protect(
    roots: &[PathBuf],
    path: &Path,
    document_id: Option<&str>,
    mut details: VersionDetails,
    cloud: &[deckcraft_cloud::CloudRoot],
    now: u64,
) -> Result<Option<DiskBefore>, String> {
    let Some((bytes, file_meta)) = read_document(path)? else { return Ok(None) };
    let digest = digest_hex(&bytes);
    let modified = modified_secs(&file_meta);
    let before = DiskBefore { size: bytes.len() as u64, modified: modified.map(iso_from_unix_secs), sha256: Some(digest.clone()) };
    if bytes.is_empty() {
        // An empty file (File Explorer's New › PowerPoint presentation) has nothing to go back to.
        return Ok(Some(before));
    }
    let (dirs, _) = prepare_dirs(roots, path, document_id, cloud, now);
    let Some(dir) = dirs.first() else { return Err("no version history folder can be reached".into()) };
    let mut list = visible(&read_union(&dirs));
    if list.is_empty()
        && document_id.is_none()
        && let Some(root) = roots.iter().find(|r| dir.starts_with(r))
        && let Some(adopted) = adopt_moved(root, path, &digest, cloud, now)
    {
        list = adopted;
    }
    if list.iter().any(|entry| entry_digest(&dirs, entry).as_deref() == Some(digest.as_str())) {
        return Ok(Some(before));
    }
    let kind = if list.is_empty() { VersionKind::Original } else { VersionKind::External };
    let saved_at = modified.map_or(now, |at| at.min(now));
    details.trigger = "beforeSave".into();
    details.before = None;
    details.stats = None;
    details.properties = None;
    store_save_at(
        roots,
        VersionWrite {
            path: path.to_path_buf(),
            bytes,
            author: String::new(),
            kind,
            saved_at: iso_from_unix_secs(saved_at),
            session: None,
            document_id: document_id.map(str::to_owned),
            details,
            cloud: cloud.to_vec(),
            now: Some(now),
        },
        now,
    )
    .map(|_| Some(before))
}

#[cfg(not(target_arch = "wasm32"))]
fn store_list(
    roots: &[PathBuf],
    path: &Path,
    document_id: Option<&str>,
    probe: bool,
    cloud: &[deckcraft_cloud::CloudRoot],
) -> Result<Vec<VersionInfo>, String> {
    let (dirs, _) = bucket_dirs(roots, path, document_id);
    let mut list = visible(&read_union(&dirs));
    if list.is_empty() && document_id.is_some() {
        // A file that only now carries an id: its history by path, until the next save moves it.
        let by_path: Vec<PathBuf> = roots.iter().map(|root| document_dir(root, path)).collect();
        list = visible(&read_union(&by_path));
    }
    if !list.is_empty() || !probe || document_id.is_some() {
        return Ok(list);
    }
    let Ok(Some((bytes, _))) = read_document(path) else { return Ok(list) };
    if bytes.is_empty() {
        return Ok(list);
    }
    let digest = digest_hex(&bytes);
    let now = now_unix();
    Ok(roots.iter().find_map(|root| adopt_moved(root, path, &digest, cloud, now)).unwrap_or(list))
}

#[cfg(not(target_arch = "wasm32"))]
fn store_rename(roots: &[PathBuf], from: &Path, to: &Path) -> Result<Vec<VersionInfo>, String> {
    let now = now_unix();
    let mut list = Vec::new();
    for root in roots {
        if fs::symlink_metadata(root).is_err() {
            continue;
        }
        list = merge_bucket(&document_dir(root, from), &document_dir(root, to), to, &[], now)?;
    }
    Ok(list)
}

/// Copy every document folder of the history folders `from` that are not in `to` into each of
/// `to`, and with `keep_old` false delete them from `from` once copied.
#[cfg(not(target_arch = "wasm32"))]
fn store_relocate(from: &[PathBuf], to: &[PathBuf], keep_old: bool) -> Result<(), String> {
    let mut failed = false;
    for old in from.iter().filter(|old| !to.contains(old)) {
        let Ok(entries) = fs::read_dir(old) else { continue };
        for entry in entries.flatten().take(MAX_BUCKET_SCAN) {
            let name = entry.file_name();
            if !name.to_str().is_some_and(key_ok) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            migrate_layout(&entry.path());
            let mut dirs = vec![entry.path()];
            for new in to {
                if private_dir(new).is_ok() {
                    dirs.push(new.join(&name));
                    let _ = private_dir(&new.join(&name));
                } else {
                    failed = true;
                }
            }
            reconcile(&dirs);
            // Every new folder holds every file before the old one goes.
            let complete = dirs.iter().skip(1).all(|dir| {
                read_index(dir).len() >= read_index(&entry.path()).len()
                    && fs::read_dir(entry.path().join(OBJECTS_DIR))
                        .map(|objects| objects.flatten().all(|o| dir.join(OBJECTS_DIR).join(o.file_name()).is_file()))
                        .unwrap_or(true)
            });
            if !keep_old && complete && dirs.len() > 1 {
                let _ = fs::remove_dir_all(entry.path());
            } else if !complete {
                failed = true;
            }
        }
    }
    if failed { Err("some version history could not be copied to the new folder".into()) } else { Ok(()) }
}

/// Delete a document's history (or all history) in every history folder. Returns how many
/// document folders went.
#[cfg(not(target_arch = "wasm32"))]
fn store_delete(roots: &[PathBuf], scope: DeleteScope, path: Option<&Path>, document_id: Option<&str>) -> Result<usize, String> {
    let mut deleted = 0;
    let mut failed = None;
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else { continue };
        let targets: Vec<PathBuf> = match scope {
            DeleteScope::All => entries
                .flatten()
                .filter(|e| e.file_name().to_str().is_some_and(key_ok) && e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect(),
            DeleteScope::Document => {
                let mut t = Vec::new();
                if let Some(id) = document_id {
                    t.push(root.join(meta::id_bucket(id)));
                }
                if let Some(path) = path {
                    t.push(document_dir(root, path));
                    t.push(root.join(legacy_key(path)));
                }
                t
            }
        };
        for dir in targets {
            if fs::symlink_metadata(&dir).is_err() {
                continue;
            }
            match fs::remove_dir_all(&dir) {
                Ok(()) => deleted += 1,
                Err(e) => failed = Some(e.to_string()),
            }
        }
    }
    match failed {
        Some(error) => Err(format!("some version history could not be deleted: {error}")),
        None => Ok(deleted),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn hold(lock: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Write `bytes` to a private file (used for the installation id).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic(path, bytes)
}

/// Quitting (or closing a session) lets the worker finish the versions already queued, for a
/// little while: the last write before a presentation closes must reach the history. A history
/// folder that hangs (a share that went away) is not waited on for more than a few seconds.
#[cfg(not(target_arch = "wasm32"))]
impl Drop for Versions {
    fn drop(&mut self) {
        self.tx = None;
        let Some(worker) = self.worker.take() else { return };
        let start = std::time::Instant::now();
        while !worker.is_finished() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if worker.is_finished() {
            let _ = worker.join();
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Versions {
    pub fn start(root: PathBuf, roots: Vec<PathBuf>, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        private_dir(&root).map_err(|e| e.to_string())?;
        let installation = meta::installation_id(root.parent().unwrap_or(&root));
        let roots = if roots.is_empty() { vec![root.clone()] } else { roots };
        let (jobs, rx_jobs) = mpsc::sync_channel(16);
        let (events, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared { roots, unavailable: Vec::new(), relocating: false }));
        let worker_shared = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("deckcraft-versions".into())
            .spawn(move || {
                while let Ok(job) = rx_jobs.recv() {
                    let mut guard = hold(&worker_shared);
                    let roots = guard.roots.clone();
                    let result = match job {
                        Job::Save(write) => store_save(&roots, *write).map(|(_, unavailable)| guard.unavailable = unavailable),
                        Job::List { path, document_id, probe, cloud } => store_list(&roots, &path, document_id.as_deref(), probe, &cloud).map(|_| ()),
                        Job::Rename { from, to } => store_rename(&roots, &from, &to).map(|_| ()),
                        Job::Relocate { from, to, keep_old } => {
                            let result = store_relocate(&from, &to, keep_old);
                            guard.relocating = false;
                            result
                        }
                    };
                    drop(guard);
                    if events.send(Event { result }).is_err() {
                        break;
                    }
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            tx: Some(jobs),
            worker: Some(worker),
            rx,
            default_root: root,
            shared,
            run_id: new_id()?,
            installation,
            probed: BTreeSet::new(),
            clock: None,
            queued: 0,
            busy: false,
            error: None,
            alive: true,
        })
    }

    fn roots(&self) -> Vec<PathBuf> {
        hold(&self.shared).roots.clone()
    }

    /// Seconds since the epoch by the store's clock.
    pub(crate) fn now(&self) -> u64 {
        self.clock.as_ref().map_or_else(now_unix, |clock| clock())
    }

    /// Work by `clock` instead of the system clock: for tests of retention and the AutoSave
    /// window, which count from the time of the write.
    pub(crate) fn set_clock(&mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) {
        self.clock = Some(clock);
    }

    fn queue(&mut self, job: Job) -> Result<(), String> {
        if !self.alive {
            return Err("version history worker is unavailable".into());
        }
        self.tx.as_ref().ok_or_else(|| "version history worker is unavailable".to_string())?.try_send(job).map_err(|e| e.to_string())?;
        self.queued += 1;
        self.busy = true;
        Ok(())
    }

    /// Collect what the worker finished. A failed background write is kept in `error` (and
    /// logged); it never reaches a save.
    pub fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(event) => {
                    self.queued = self.queued.saturating_sub(1);
                    self.busy = self.queued > 0;
                    if let Err(error) = event.result {
                        log::warn!("version history: {error}");
                        self.error = Some(error);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.busy = false;
                    self.alive = false;
                    if self.error.is_none() {
                        self.error = Some("version history worker is unavailable".into());
                    }
                    break;
                }
            }
        }
    }

    /// Wait until the worker has finished everything queued (tests, and a host closing).
    pub fn wait_idle(&mut self, limit: std::time::Duration) -> bool {
        let start = std::time::Instant::now();
        loop {
            self.poll();
            if !self.busy {
                return true;
            }
            if start.elapsed() > limit {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The versions of a presentation. The versions' records are small and cached, so they are
    /// read here at once and the first `file.versions` after opening already lists them. Only
    /// when there are none (and the file carries no id) does the worker look, once, for the
    /// history of a file moved outside DeckCraft; it moves what it finds under the new path, so
    /// the next call lists it.
    pub fn list(&mut self, path: &Path, document_id: Option<&str>, cloud: &[deckcraft_cloud::CloudRoot]) -> Vec<VersionInfo> {
        let canonical = canonical_path(path);
        let roots = self.roots();
        let list = store_list(&roots, &canonical, document_id, false, cloud).unwrap_or_default();
        if !list.is_empty() || document_id.is_some() || self.probed.contains(&canonical) {
            return list;
        }
        // The worker may read the file to find a moved file's history; only where this thread
        // may read it (automation roots).
        self.probed.insert(canonical.clone());
        if crate::file_access::check_read(&canonical.to_string_lossy()).is_ok()
            && let Err(error) = self.queue(Job::List { path: canonical, document_id: None, probe: true, cloud: cloud.to_vec() })
        {
            self.error = Some(error);
        }
        list
    }

    fn save(&mut self, write: VersionWrite) {
        if write.bytes.len() as u64 > MAX_DOCUMENT_BYTES {
            self.error = Some("version exceeds the 500 MiB document history limit".into());
            return;
        }
        if let Err(error) = self.queue(Job::Save(Box::new(write))) {
            self.error = Some(error);
        }
    }

    /// The history of a file kept by path follows it when it is renamed, in order with the saves
    /// queued before it. (A file with an id needs nothing: the id moves with the file.)
    pub fn renamed(&mut self, from: PathBuf, to: PathBuf) {
        let to = canonical_path(&to);
        if let Err(error) = self.queue(Job::Rename { from, to }) {
            // The folder stays under the old name; the moved-file search still finds it at the
            // next save, since the old path no longer exists.
            self.error = Some(error);
        }
    }

    /// The history folders in use, those the last write could not reach, and whether the history
    /// is being copied to new folders.
    pub(crate) fn folders(&self) -> (Vec<PathBuf>, Vec<PathBuf>, bool) {
        let shared = hold(&self.shared);
        (shared.roots.clone(), shared.unavailable.clone(), shared.relocating)
    }

    /// Use `roots` from now on; what the old folders hold is copied, moved or left.
    pub(crate) fn set_folders(&mut self, roots: Vec<PathBuf>, existing: Relocation) -> Result<(), String> {
        let roots = if roots.is_empty() { vec![self.default_root.clone()] } else { roots };
        let old = {
            let mut shared = hold(&self.shared);
            let old = std::mem::replace(&mut shared.roots, roots.clone());
            shared.unavailable.clear();
            shared.relocating = existing != Relocation::Leave && old != roots;
            old
        };
        self.probed.clear();
        if existing != Relocation::Leave && old != roots {
            self.queue(Job::Relocate { from: old, to: roots, keep_old: existing == Relocation::Copy })?;
        }
        Ok(())
    }

    /// Synchronously keep the bytes at `path` before they are overwritten (see [`store_protect`]).
    /// With `verified` the presentation already knows them to be in the history.
    fn protect(
        &mut self,
        path: &Path,
        doc_uuid: Option<&str>,
        details: VersionDetails,
        cloud: &[deckcraft_cloud::CloudRoot],
        verified: bool,
    ) -> Result<Option<DiskBefore>, String> {
        if verified {
            return Ok(None);
        }
        let canonical = canonical_path(path);
        let mut shared = hold(&self.shared);
        let roots = shared.roots.clone();
        let now = self.now();
        let result = store_protect(&roots, &canonical, doc_uuid, details, cloud, now);
        let (_, unavailable) = bucket_dirs(&roots, &canonical, doc_uuid);
        shared.unavailable = unavailable;
        result
    }

    pub fn read(&self, path: &Path, document_id: Option<&str>, id: &str) -> Result<(VersionInfo, Vec<u8>), String> {
        if !id_ok(id) {
            return Err("invalid version id".into());
        }
        // Held while parts are read, so the worker's collection of unused parts cannot remove
        // one in between.
        let shared = hold(&self.shared);
        let mut dirs = bucket_dirs(&shared.roots, path, document_id).0;
        if document_id.is_some() {
            dirs.extend(shared.roots.iter().map(|root| document_dir(root, path)));
        }
        let info = read_union(&dirs).into_iter().find(|entry| entry.id == id).ok_or_else(|| "no such version".to_string())?;
        let bytes = match info.storage {
            VersionStorage::Parts => read_parts_version(&dirs, id)?,
            VersionStorage::Whole => read_whole_version(&dirs, &info)?,
        };
        Ok((info, bytes))
    }

    /// Delete history now (and in every mirror folder).
    fn delete(&mut self, scope: DeleteScope, path: Option<&Path>, document_id: Option<&str>) -> Result<usize, String> {
        let shared = hold(&self.shared);
        let result = store_delete(&shared.roots, scope, path.map(canonical_path).as_deref(), document_id);
        drop(shared);
        self.probed.clear();
        result
    }

    /// The newest version of a document and the path it was saved at, for telling a copy.
    fn newest(&self, path: &Path, document_id: &str, cloud: &[deckcraft_cloud::CloudRoot]) -> Option<(String, Option<PathBuf>)> {
        let roots = self.roots();
        let (dirs, _) = bucket_dirs(&roots, path, Some(document_id));
        let newest = visible(&read_union(&dirs)).into_iter().next()?;
        let saved_at = newest.details.as_ref().and_then(|d| meta::resolve_portable_path(&d.path, cloud));
        Some((newest.id, saved_at))
    }
}
