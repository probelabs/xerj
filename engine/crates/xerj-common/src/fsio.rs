//! Power-loss-ordered file I/O primitives (RC4 Wave-1 blocker #10).
//!
//! The segment publish chain (`.seg` → `.ids`/`.dv`/FTS side-cars →
//! `snapshot.json` → WAL prune) is only crash-safe if every link is
//! durable **before** the WAL entries it supersedes are destroyed.
//! `fs::write` + `rename` alone leaves both the file bytes and the
//! directory entry in the volatile page cache: a power loss after the
//! WAL was pruned can then GC a fully-flushed segment as an orphan —
//! acked-data loss.
//!
//! Every side-car / manifest write on the publish chain must go through
//! [`write_file_durable`] or [`replace_file_durable`]. Unix makes the rename
//! durable with parent-directory fsync; Windows uses a same-directory Win32
//! write-through replacement because it has no equivalent directory-fsync
//! contract.

use std::io::Write as _;
use std::path::Path;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt as _;

/// fsync a directory so previously-renamed/created entries in it survive
/// power loss. No-op errors are surfaced to the caller; on filesystems
/// where directories cannot be fsynced (rare), callers may choose to
/// ignore the error.
#[cfg(not(windows))]
pub fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    let d = std::fs::File::open(dir)?;
    d.sync_all()
}

/// Windows has no supported directory-flush primitive to call here:
/// `File::open` on a
/// directory fails with `ERROR_ACCESS_DENIED` (os error 5) because std
/// cannot pass `FILE_FLAG_BACKUP_SEMANTICS`, and `FlushFileBuffers` is
/// not defined for directory handles. This compatibility shim makes no
/// durability claim. Code whose correctness requires a durable namespace
/// replacement must use [`write_file_durable`] or [`replace_file_durable`],
/// whose Windows implementation uses `MoveFileExW` with write-through.
///
/// This is not a cosmetic cfg: the Unix body returned `Err` for *every*
/// call on Windows, and `save_snapshot` propagates that error, so the
/// server could not create an index — it aborted at boot with
/// `create .xerj_users: I/O error: Access is denied. (os error 5)`.
#[cfg(windows)]
pub fn fsync_dir(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_for_durable_write(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

fn require_same_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    if source.parent() == destination.parent() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "durable file replacement requires source and destination in the same directory",
        ))
    }
}

/// Atomically replace one same-directory file and wait for the move to reach
/// disk on Windows.
///
/// Windows cannot use the Unix rename + parent-directory-fsync sequence:
/// `std::fs::File` cannot open a directory with `FILE_FLAG_BACKUP_SEMANTICS`,
/// and `FlushFileBuffers` does not define a directory-handle contract. Win32's
/// `MoveFileExW`, however, defines `MOVEFILE_WRITE_THROUGH` to wait until the
/// move is on disk. Callers create the temporary file beside `destination`, so
/// this remains a same-volume namespace replacement rather than the API's
/// optional copy/delete fallback.
#[cfg(windows)]
fn replace_file_for_durable_write(source: &Path, destination: &Path) -> std::io::Result<()> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x0000_0001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x0000_0008;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(
            existing_file_name: *const u16,
            new_file_name: *const u16,
            flags: u32,
        ) -> i32;
    }

    fn wide_path(path: &Path) -> std::io::Result<Vec<u16>> {
        const BACKSLASH: u16 = b'\\' as u16;
        const QUESTION: u16 = b'?' as u16;
        const UPPER_U: u16 = b'U' as u16;
        const UPPER_N: u16 = b'N' as u16;
        const UPPER_C: u16 = b'C' as u16;

        let absolute = std::path::absolute(path)?;
        let original: Vec<u16> = absolute.as_os_str().encode_wide().collect();
        if original.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "path contains an embedded NUL",
            ));
        }

        // MoveFileExW does not get std's automatic long-path conversion.
        // Supply an absolute verbatim path so the durable replacement keeps
        // working beyond MAX_PATH. Preserve an existing verbatim prefix and
        // translate ordinary UNC paths to `\\?\UNC\...`.
        let mut wide = if original.starts_with(&[BACKSLASH, BACKSLASH, QUESTION, BACKSLASH]) {
            original
        } else if original.starts_with(&[BACKSLASH, BACKSLASH]) {
            let mut verbatim = vec![
                BACKSLASH, BACKSLASH, QUESTION, BACKSLASH, UPPER_U, UPPER_N, UPPER_C, BACKSLASH,
            ];
            verbatim.extend_from_slice(&original[2..]);
            verbatim
        } else {
            let mut verbatim = vec![BACKSLASH, BACKSLASH, QUESTION, BACKSLASH];
            verbatim.extend_from_slice(&original);
            verbatim
        };
        wide.push(0);
        Ok(wide)
    }

    let source = wide_path(source)?;
    let destination = wide_path(destination)?;
    // SAFETY: both buffers are NUL-terminated and live through the call.
    // The flags request replacement plus synchronous persistence; no
    // COPY_ALLOWED flag is present, and our temp file is same-directory.
    let succeeded = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Replace `destination` with an already-written, already-synced sibling file
/// and wait for the namespace replacement to become durable.
///
/// Both paths must have the same lexical parent. Unix uses rename followed by
/// a parent-directory fsync. Windows uses a same-volume
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` call and
/// does not claim that directory handles can be flushed.
pub fn replace_file_durable(source: &Path, destination: &Path) -> std::io::Result<()> {
    require_same_directory(source, destination)?;
    replace_file_for_durable_write(source, destination)?;
    #[cfg(not(windows))]
    if let Some(parent) = destination.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically **and durably**:
///
/// 1. write to a same-directory temp file,
/// 2. `fsync` the temp file (data + metadata),
/// 3. replace the target with the temp file,
/// 4. wait for that replacement to become durable: parent-directory `fsync`
///    on Unix, or `MoveFileExW(MOVEFILE_WRITE_THROUGH)` on Windows.
///
/// A crash or power loss at any point leaves either the old file or the
/// complete new file — never a torn one, and never a "file that
/// evaporates on power loss because only the page cache had it".
///
/// The temp name embeds the process + thread ids so concurrent writers to the
/// same target never clobber each other's temp file (see the
/// `save_snapshot` race note in `xerj-storage`).
pub fn write_file_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_file_durable_with_hook(path, bytes, |_| Ok(()))
}

/// Durable-write boundaries exposed for deterministic cross-crate fault tests.
///
/// Production callers should use [`write_file_durable`]. The hook-capable form
/// exists so publication code can prove that every failure boundary prevents
/// later, format-dependent writes without relying on filesystem permissions.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableWriteStage {
    BeforeTempWrite,
    BeforeRename,
    /// The target name has switched. On Unix the parent-directory fsync has
    /// not run yet; on Windows the write-through replacement has completed,
    /// but callers can still inject a conservative post-replace failure before
    /// recording their process-local durability confirmation.
    BeforeParentFsync,
}

/// [`write_file_durable`] with a deterministic observer/failure hook.
#[doc(hidden)]
pub fn write_file_durable_with_hook(
    path: &Path,
    bytes: &[u8],
    mut hook: impl FnMut(DurableWriteStage) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let nonce = format!("{:x}-{:?}", std::process::id(), std::thread::current().id());
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let tmp = path.with_file_name(format!("{file_name}.tmp.{nonce}"));
    hook(DurableWriteStage::BeforeTempWrite)?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    if let Err(error) = hook(DurableWriteStage::BeforeRename) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    require_same_directory(&tmp, path)?;
    if let Err(e) = replace_file_for_durable_write(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    hook(DurableWriteStage::BeforeParentFsync)?;
    #[cfg(not(windows))]
    if let Some(parent) = path.parent() {
        fsync_dir(parent)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_file_durable_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.bin");
        write_file_durable(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        // Overwrite is atomic.
        write_file_durable(&p, b"world").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"world");
        // No stray temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "temp files left: {leftovers:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn fsync_dir_works() {
        let dir = tempfile::tempdir().unwrap();
        fsync_dir(dir.path()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn fsync_dir_is_only_a_compatibility_shim_on_windows() {
        let dir = tempfile::tempdir().unwrap();
        fsync_dir(dir.path()).unwrap();
        // This return value intentionally carries no durability proof. The
        // durable replacement tests below exercise the write-through API that
        // publication code must use on Windows.
    }

    #[test]
    fn durable_replace_overwrites_synced_sibling() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("replacement.tmp");
        let destination = dir.path().join("marker.json");
        std::fs::write(&destination, b"old").unwrap();
        let mut staged = std::fs::File::create(&source).unwrap();
        staged.write_all(b"new").unwrap();
        staged.sync_all().unwrap();
        drop(staged);

        replace_file_durable(&source, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        assert!(!source.exists());
    }

    #[test]
    fn durable_replace_rejects_cross_directory_move() {
        let source_dir = tempfile::tempdir().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("replacement.tmp");
        let destination = destination_dir.path().join("marker.json");
        std::fs::write(&source, b"new").unwrap();
        let error = replace_file_durable(&source, &destination).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(source.exists());
        assert!(!destination.exists());
    }

    #[test]
    fn durable_write_hook_classifies_pre_and_post_rename_failures() {
        for (stage, expected) in [
            (DurableWriteStage::BeforeTempWrite, b"old".as_slice()),
            (DurableWriteStage::BeforeRename, b"old".as_slice()),
            (DurableWriteStage::BeforeParentFsync, b"new".as_slice()),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("marker.json");
            std::fs::write(&path, b"old").unwrap();
            let result = write_file_durable_with_hook(&path, b"new", |boundary| {
                if boundary == stage {
                    Err(std::io::Error::other("injected durable-write failure"))
                } else {
                    Ok(())
                }
            });
            assert!(result.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), expected);
            assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp.")
            }));
        }
    }
}
