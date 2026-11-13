//! Cross-process advisory locks: the root `.lock` and per-run owner locks.

use super::*;

/// How long a mutation waits for the cross-process lock before degrading to
/// "no lock" (a warning is logged; the write still happens).
pub(super) const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
/// Directory (under the root) holding one owner lock file per active run.
pub(super) const RUN_LOCKS_DIR: &str = "run_locks";

/// Held for the duration of one cross-process mutation of a shared file.
/// Backed by an advisory exclusive lock on `<root>/.lock` so the desktop app
/// and `lcgn` can run concurrently on the same root without overwriting each
/// other's `projects.json` / `permissions.json` / memory files / usage ids.
/// Rollouts are not locked (they are private to the run that owns them and
/// only ever appended to). `None` means the lock could not be taken (a
/// read-only or unreachable root); the caller proceeds without it.
pub(super) struct CrossProcessLock(Option<std::fs::File>);

impl Drop for CrossProcessLock {
    fn drop(&mut self) {
        if let Some(file) = &self.0 {
            let _ = file.unlock();
        }
    }
}

/// Takes the advisory lock, retrying up to [`LOCK_TIMEOUT`]. Never panics:
/// when the lock file cannot be opened or locked the store degrades to
/// running unlocked with a warning.
pub(super) fn lock_root(root: &Path) -> CrossProcessLock {
    let path = root.join(".lock");
    let file = match OpenOptions::new().create(true).truncate(false).write(true).open(&path) {
        Ok(file) => file,
        Err(err) => {
            tracing::warn!("[Database] Cannot open lock file {}: {err}; continuing without it", path.display());
            return CrossProcessLock(None);
        }
    };
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return CrossProcessLock(Some(file)),
            Err(err) if is_lock_contended(&err) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => {
                tracing::warn!("[Database] Advisory lock not taken on {}: {err}", path.display());
                return CrossProcessLock(None);
            }
        }
    }
}

pub(super) fn is_lock_contended(err: &std::io::Error) -> bool {
    err.kind() == fs2::lock_contended_error().kind()
        || err.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

/// Owner lock of one active run: `<root>/run_locks/<run_id>.lock`.
pub(super) fn run_lock_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(RUN_LOCKS_DIR).join(format!("{}.lock", safe_file_part(run_id)))
}

/// Takes the exclusive owner lock of a run, held until released or the
/// process exits (the OS frees it on a crash, which is what makes it a
/// reliable "is the owner alive" probe).
pub(super) fn acquire_run_lock(root: &Path, run_id: &str) -> Option<std::fs::File> {
    let path = run_lock_path(root, run_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path).ok()?;
    file.try_lock_exclusive().ok()?;
    Some(file)
}

/// True when another live process (or `Db` instance) holds the run's owner lock.
pub(super) fn run_owned_by_live_process(root: &Path, run_id: &str) -> bool {
    let path = run_lock_path(root, run_id);
    let Ok(file) = OpenOptions::new().write(true).open(&path) else {
        return false;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = file.unlock();
            false
        }
        Err(err) => is_lock_contended(&err),
    }
}
