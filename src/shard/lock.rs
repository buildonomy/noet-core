//! Exclusive writer lock over a shard output directory.
//!
//! Once shards are the identity store, two processes exporting to one output
//! directory can interleave their writes and produce a mixed generation — which
//! costs every BID in the corpus, not merely a re-render. This enforces what
//! `docs/design/annotation/living_corpus.md` §2 already asserts about Layer 2:
//! **single-owner-per-node, one writer**. Layer 3 (annotations) is many-writer by
//! design and is not affected — annotations are written to the halo of stores, never
//! through this path.
//!
//! ## Why an OS advisory lock rather than a lock file
//!
//! A hand-rolled lock file holding a pid must be reaped when the writer dies, and a
//! process killed with `SIGKILL` never runs its cleanup — stranding the output
//! directory until someone deletes the file by hand. `flock(2)` is released by the
//! kernel when the file descriptor closes, *including* on abnormal termination, so
//! there is no stale state to reap.
//!
//! ## Platforms
//!
//! | Platform | Primitive | Released on abnormal exit |
//! |---|---|---|
//! | Unix | `flock(LOCK_EX \| LOCK_NB)` | yes, on fd close |
//! | Windows | `LockFileEx(EXCLUSIVE \| FAIL_IMMEDIATELY)` | yes, on handle close |
//! | other | none — warns loudly, does not protect | n/a |
//!
//! Both real implementations were chosen for the same property: the kernel owns the
//! release, so there is no stale-lock recovery path to get wrong.
//!
//! The pid and start time are written into the file purely so a contending process
//! can name the holder in its error message. They are diagnostics, never the lock.
//!
//! ## Contention fails, it does not block
//!
//! Layer 2 is a pure function of Layer 1 (`living_corpus.md` §2), so a refused parse
//! has written nothing and discarded nothing — re-running reproduces the identical
//! graph. Blocking would instead turn a hand-run `noet parse` into a silent hang
//! behind a `noet serve` daemon, and give CI a queue where it wants an error.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::BuildonomyError;

/// Name of the lock file inside the output directory.
pub const LOCK_FILE_NAME: &str = ".noet-write-lock";

/// An acquired exclusive writer lock. Releasing happens on drop, when the
/// underlying file descriptor closes.
#[derive(Debug)]
pub struct WriteLock {
    path: PathBuf,
    /// Held for its `Drop`: closing the descriptor releases the advisory lock.
    _file: File,
}

impl WriteLock {
    /// Try to acquire the exclusive writer lock for `output_dir`.
    ///
    /// Returns `Err` immediately if another process holds it — the message names
    /// the holder when the diagnostic content is readable.
    pub fn acquire(output_dir: &Path) -> Result<Self, BuildonomyError> {
        std::fs::create_dir_all(output_dir).map_err(|e| {
            BuildonomyError::Custom(format!(
                "cannot create output directory {}: {e}",
                output_dir.display()
            ))
        })?;
        let path = output_dir.join(LOCK_FILE_NAME);

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| {
                BuildonomyError::Custom(format!("cannot open lock file {}: {e}", path.display()))
            })?;

        if !try_lock_exclusive(&file)? {
            let holder = std::fs::read_to_string(&path).unwrap_or_default();
            let holder = holder.trim();
            let detail = if holder.is_empty() {
                "another process".to_string()
            } else {
                holder.to_string()
            };
            return Err(BuildonomyError::Custom(format!(
                "another noet process is writing to {} ({detail}).\n\
                 Only one writer may hold an output directory at a time. Stop that \
                 process, or run against a different --html-output.",
                output_dir.display()
            )));
        }

        // We hold the lock: record who we are, for the next contender's message.
        let mut f = &file;
        let _ = f.set_len(0);
        let _ = writeln!(
            f,
            "pid {} since {}",
            std::process::id(),
            humantime_now_or_epoch()
        );
        let _ = f.flush();

        tracing::debug!("acquired shard writer lock on {}", output_dir.display());
        Ok(Self { path, _file: file })
    }

    /// Path to the lock file backing this lock.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WriteLock {
    fn drop(&mut self) {
        // The kernel releases the advisory lock when `_file` closes. The file
        // itself is left in place deliberately: unlinking it races a contender
        // that has already opened it, and an unlocked empty file is harmless.
        tracing::debug!("released shard writer lock on {}", self.path.display());
    }
}

/// RFC-3339-ish timestamp without pulling in a date formatting dependency.
fn humantime_now_or_epoch() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("unix:{}", d.as_secs()),
        Err(_) => "unix:0".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Platform-specific advisory locking
// ---------------------------------------------------------------------------

/// Attempt a non-blocking exclusive lock. `Ok(false)` means "held by someone else".
#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> Result<bool, BuildonomyError> {
    use std::os::unix::io::AsRawFd;

    // SAFETY: `fd` is a valid descriptor for the lifetime of this call because
    // `file` is borrowed. `flock` only inspects the descriptor.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EINTR => Ok(false),
        _ => Err(BuildonomyError::Custom(format!(
            "failed to acquire advisory lock: {err}"
        ))),
    }
}

/// Attempt a non-blocking exclusive lock. `Ok(false)` means "held by someone else".
///
/// `LockFileEx` is the Windows counterpart to `flock` for this purpose: with
/// `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY` it either takes the lock or
/// returns at once, and the kernel releases it when the handle closes — including on
/// abnormal termination. That last property is why this is a real lock rather than a
/// pid file: nothing can be stranded by a killed process.
///
/// Byte range is the whole 64-bit space (`u32::MAX, u32::MAX`), matching what the
/// rest of the ecosystem does for whole-file locks. The lock file itself carries only
/// a diagnostic line, so the range is nominal.
#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> Result<bool, BuildonomyError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, ERROR_LOCK_VIOLATION};
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::OVERLAPPED;

    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    // Zeroed OVERLAPPED = lock from offset 0. Required even for a synchronous call.
    let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };

    // SAFETY: `handle` is valid for the duration of this call because `file` is
    // borrowed, and `overlapped` is a live, correctly-zeroed local.
    let ok = unsafe {
        LockFileEx(
            handle,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
            &mut overlapped,
        )
    };

    if ok != 0 {
        return Ok(true);
    }

    let err = std::io::Error::last_os_error();
    match err.raw_os_error().map(|c| c as u32) {
        // Another process holds it. `FAIL_IMMEDIATELY` reports contention as
        // LOCK_VIOLATION; IO_PENDING can surface on some paths and means the same
        // thing here — we asked not to wait.
        Some(ERROR_LOCK_VIOLATION) | Some(ERROR_IO_PENDING) => Ok(false),
        _ => Err(BuildonomyError::Custom(format!(
            "failed to acquire advisory lock: {err}"
        ))),
    }
}

/// Platforms that are neither Unix nor Windows have no implementation.
///
/// This arm exists so the crate still builds somewhere exotic. It is **not**
/// protection: returning `Ok(true)` lets a second writer proceed, and two writers
/// against one output directory interleave their shard writes and corrupt the
/// generation — which now costs every BID in the corpus, not merely a re-render.
/// The warning is the only signal, so it is worded to say exactly that.
#[cfg(not(any(unix, windows)))]
fn try_lock_exclusive(_file: &File) -> Result<bool, BuildonomyError> {
    tracing::warn!(
        "shard writer lock is NOT enforced on this platform: concurrent `noet` \
         processes writing one output directory will corrupt the generation and \
         re-mint every BID. Run only one writer at a time."
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_creates_the_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let lock = WriteLock::acquire(dir.path()).unwrap();
        assert!(lock.path().exists(), "lock file should be created");
        let contents = std::fs::read_to_string(lock.path()).unwrap();
        assert!(
            contents.contains(&std::process::id().to_string()),
            "lock file should name the holding pid, got {contents:?}"
        );
    }

    /// The exclusion property itself — the whole point of the type.
    ///
    /// Runs on **both** real platforms rather than Unix only: a Windows build that
    /// silently stopped excluding would otherwise pass a green suite, which is exactly
    /// how the no-op fallback went unnoticed. Both primitives are per-handle rather
    /// than per-process, so a second `open` + lock from this process contends the same
    /// way a separate process does.
    #[cfg(any(unix, windows))]
    #[test]
    fn second_acquire_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let _first = WriteLock::acquire(dir.path()).unwrap();
        let second = WriteLock::acquire(dir.path());
        assert!(
            second.is_err(),
            "a second writer must be refused — if this passes on a platform whose \
             try_lock_exclusive is a no-op, the writer lock is not protecting anything"
        );
        let msg = second.unwrap_err().to_string();
        assert!(
            msg.contains("another noet process"),
            "error should name the conflict, got {msg:?}"
        );
    }

    /// Dropping the guard closes the handle, and the kernel releases the lock with
    /// it — the property that makes stale-lock recovery unnecessary on both platforms.
    #[cfg(any(unix, windows))]
    #[test]
    fn lock_is_released_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        {
            let _first = WriteLock::acquire(dir.path()).unwrap();
        }
        assert!(
            WriteLock::acquire(dir.path()).is_ok(),
            "lock should be re-acquirable after the holder drops"
        );
    }
}
