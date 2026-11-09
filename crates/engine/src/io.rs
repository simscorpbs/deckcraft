//! Reading and writing the files a command names by path.
//!
//! Every such read and write goes through [`read_file`] and [`write_file`], so the automation
//! roots ([`crate::file_access`]) are checked in one place. A command module that called
//! `std::fs` itself would bypass them; a test in `file_access` keeps that from happening.

/// The bytes of the file at `path`, inside the automation read root when there is one.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    read_file_stamped(path).map(|(bytes, _)| bytes)
}

/// [`read_file`], and the file's disk stamp for AutoSave's conflict check: its size and date
/// taken before the read, so a file that changes during the read cannot pair the new date with
/// the old bytes. `None` when the stamp can't be made.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_file_stamped(path: &str) -> Result<(Vec<u8>, Option<deckcraft_cloud::DiskStamp>), String> {
    crate::file_access::check_read(path)?;
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path}: not a regular file"));
    }
    const MAX_FILE_SIZE: u64 = 2 << 30; // 2 GiB
    if meta.len() > MAX_FILE_SIZE {
        return Err(format!("{path}: file size {} exceeds 2 GiB limit", meta.len()));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let stamp = (bytes.len() as u64 == meta.len()).then(|| deckcraft_cloud::DiskStamp::from_metadata(&meta, &bytes));
    Ok((bytes, stamp))
}

#[cfg(target_arch = "wasm32")]
pub fn read_file(path: &str) -> Result<Vec<u8>, String> {
    Err(format!("{path}: files are opened through the browser on the web"))
}

/// Write `bytes` to the file at `path`, inside the automation write root when there is one.
///
/// The bytes go to a temporary sibling first and are renamed over `path`, so a failed save never
/// truncates the old file. The sibling is in the same folder, so it is inside the root whenever
/// `path` is.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    crate::file_access::check_write(path)?;
    let err = |e: std::io::Error| format!("{path}: {e}");

    let target_path = match std::fs::canonicalize(path) {
        Ok(canonical) => canonical,
        Err(_) => std::path::PathBuf::from(path),
    };
    let existing_perms = std::fs::metadata(&target_path).ok().map(|m| m.permissions());

    let mut attempts = 0;
    let (tmp, mut file) = loop {
        // A short fixed-size name: appending to the target's name could exceed the file name limit.
        let tmp_name = format!(".deckcraft.{}.{}.saving", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed));
        let tmp = target_path.with_file_name(tmp_name);
        // The sibling must pass the same check: when `path` is the write root itself it lands outside.
        crate::file_access::check_write(&tmp.to_string_lossy())?;
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempts < 16 => attempts += 1,
            Err(e) => return Err(err(e)),
        }
    };
    if let Some(perms) = existing_perms {
        let _ = file.set_permissions(perms);
    }
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| rename_with_retry(&tmp, &target_path, |a, b| std::fs::rename(a, b), is_locked, &LOCK_RETRY_DELAYS, std::thread::sleep));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(if is_locked(&e) { format!("{path}: {e} (another program has the file open; close it and save again)") } else { err(e) });
    }
    Ok(())
}

/// How long the final rename of a save waits between tries when the target is locked: about
/// 1.35 s in all, enough for an antivirus scan or a OneDrive sync to let go.
#[cfg(not(target_arch = "wasm32"))]
const LOCK_RETRY_DELAYS: [std::time::Duration; 6] = [
    std::time::Duration::from_millis(50),
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(200),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(300),
    std::time::Duration::from_millis(400),
];

/// Is `e` a file another program holds open for the moment? On Windows that is access denied,
/// a sharing violation or a lock violation (OneDrive, antivirus, the search indexer open files
/// without delete sharing). Elsewhere a rename is never held up by another reader, so nothing is.
#[cfg(not(target_arch = "wasm32"))]
fn is_locked(e: &std::io::Error) -> bool {
    cfg!(windows) && (e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(5 | 32 | 33)))
}

/// `rename(from, to)`, tried again after each of `delays` while it fails with an error
/// `retryable` accepts; any other error, or the last failure, is returned as it came.
#[cfg(not(target_arch = "wasm32"))]
fn rename_with_retry(
    from: &std::path::Path,
    to: &std::path::Path,
    mut rename: impl FnMut(&std::path::Path, &std::path::Path) -> std::io::Result<()>,
    retryable: impl Fn(&std::io::Error) -> bool,
    delays: &[std::time::Duration],
    mut sleep: impl FnMut(std::time::Duration),
) -> std::io::Result<()> {
    let mut tries = delays.iter();
    loop {
        match rename(from, to) {
            Err(e) if retryable(&e) => match tries.next() {
                Some(delay) => sleep(*delay),
                None => return Err(e),
            },
            other => return other,
        }
    }
}

/// [`write_file`], and the disk stamp of what was written, for AutoSave's conflict check (`None`
/// when the file cannot be looked at again).
#[cfg(not(target_arch = "wasm32"))]
pub fn write_file_stamped(path: &str, bytes: &[u8]) -> Result<Option<deckcraft_cloud::DiskStamp>, String> {
    write_file(path, bytes)?;
    Ok(deckcraft_cloud::DiskStamp::of_bytes(std::path::Path::new(path), bytes).ok())
}

#[cfg(target_arch = "wasm32")]
pub fn write_file_stamped(path: &str, _bytes: &[u8]) -> Result<Option<deckcraft_cloud::DiskStamp>, String> {
    Err(format!("{path}: saving to a path is not available in the browser"))
}

#[cfg(target_arch = "wasm32")]
pub fn write_file(path: &str, _bytes: &[u8]) -> Result<(), String> {
    Err(format!("{path}: saving to a path is not available in the browser"))
}

/// The size in bytes of the file at `path`; `None` when it can't be read (or is outside the
/// automation read root, so File › Info doesn't reveal files the agent may not read).
#[cfg(not(target_arch = "wasm32"))]
pub fn file_len(path: &str) -> Option<u64> {
    crate::file_access::check_read(path).ok()?;
    std::fs::metadata(path).ok().map(|m| m.len())
}

#[cfg(target_arch = "wasm32")]
pub fn file_len(_path: &str) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_rejects_directory() {
        let dir = std::env::temp_dir();
        let res = read_file(dir.to_str().unwrap());
        assert!(res.is_err());
    }

    #[test]
    fn write_file_preserves_permissions_and_writes_content() {
        let tmp_dir = std::env::temp_dir().join("deckcraft_io_test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let test_file = tmp_dir.join("test_write.txt");
        let path_str = test_file.to_str().unwrap();

        assert!(write_file(path_str, b"hello world").is_ok());
        let read = read_file(path_str).unwrap();
        assert_eq!(read, b"hello world");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o640);
            std::fs::set_permissions(&test_file, perms).unwrap();

            assert!(write_file(path_str, b"updated world").is_ok());
            let new_meta = std::fs::metadata(&test_file).unwrap();
            assert_eq!(new_meta.permissions().mode() & 0o777, 0o640);
        }

        let _ = std::fs::remove_file(&test_file);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn write_file_overwrites_file_with_name_near_the_length_limit() {
        let dir = std::env::temp_dir().join("deckcraft_io_long_name_test");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join(format!("{}.pptx", "a".repeat(250)));
        std::fs::write(&file, b"old").unwrap();
        let path_str = file.to_str().unwrap();

        write_file(path_str, b"new").unwrap();
        assert_eq!(read_file(path_str).unwrap(), b"new");

        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn write_file_to_the_write_root_itself_creates_no_sibling_outside_it() {
        let base = std::env::temp_dir().join(format!("deckcraft_io_root_test_{}", std::process::id()));
        let root = base.join("root");
        std::fs::create_dir_all(&root).unwrap();
        let roots = crate::file_access::AutomationRoots::new(&[], std::slice::from_ref(&root)).unwrap().unwrap();
        let res = crate::file_access::confine(Some(&roots), || write_file(root.to_str().unwrap(), b"x"));
        assert!(res.unwrap_err().contains("outside the write root"));
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 1, "only the root is there");
        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod save_retry_tests {
    use std::io::{Error, ErrorKind};
    use std::path::Path;
    use std::time::Duration;

    use super::{LOCK_RETRY_DELAYS, rename_with_retry};

    fn locked() -> Error {
        Error::new(ErrorKind::PermissionDenied, "sharing violation")
    }

    #[test]
    fn a_brief_lock_is_waited_out() {
        let (mut calls, mut slept) = (0, vec![]);
        let r = rename_with_retry(
            Path::new("a"),
            Path::new("b"),
            |_, _| {
                calls += 1;
                if calls < 4 { Err(locked()) } else { Ok(()) }
            },
            |e| e.kind() == ErrorKind::PermissionDenied,
            &LOCK_RETRY_DELAYS,
            |d| slept.push(d),
        );
        assert!(r.is_ok());
        assert_eq!(calls, 4);
        assert_eq!(slept, LOCK_RETRY_DELAYS[..3]);
    }

    #[test]
    fn a_lock_that_stays_fails_after_the_last_try() {
        let (mut calls, mut slept) = (0, Duration::ZERO);
        let r = rename_with_retry(
            Path::new("a"),
            Path::new("b"),
            |_, _| {
                calls += 1;
                Err(locked())
            },
            |e| e.kind() == ErrorKind::PermissionDenied,
            &LOCK_RETRY_DELAYS,
            |d| slept += d,
        );
        assert_eq!(r.unwrap_err().kind(), ErrorKind::PermissionDenied);
        assert_eq!(calls, LOCK_RETRY_DELAYS.len() + 1);
        assert_eq!(slept, Duration::from_millis(1350));
    }

    #[test]
    fn other_errors_are_not_retried() {
        let mut calls = 0;
        let r = rename_with_retry(
            Path::new("a"),
            Path::new("b"),
            |_, _| {
                calls += 1;
                Err(Error::new(ErrorKind::NotFound, "gone"))
            },
            |e| e.kind() == ErrorKind::PermissionDenied,
            &LOCK_RETRY_DELAYS,
            |_| panic!("no waiting for an error that will not clear"),
        );
        assert_eq!(r.unwrap_err().kind(), ErrorKind::NotFound);
        assert_eq!(calls, 1);
    }

    #[test]
    fn saving_replaces_the_file_and_keeps_its_name() {
        let dir = std::env::temp_dir().join(format!("deckcraft-save-retry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Report.pptx");
        super::write_file(&file.to_string_lossy(), b"one").unwrap();
        super::write_file(&file.to_string_lossy(), b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        assert_eq!(names, ["Report.pptx"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
