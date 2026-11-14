//! `auth.json` lock compatible with pi's `proper-lockfile` usage: the lock is
//! a `<auth.json path>.lock` directory created with `mkdir`, considered stale
//! when its mtime is older than 10 seconds. A held lock refreshes its mtime
//! every 2 s (heartbeat) so it is never stale while alive, and release only
//! removes the directory when the owner token still matches.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RETRY_DELAY: Duration = Duration::from_millis(100);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_AFTER: Duration = Duration::from_secs(10);

/// How often a held lock refreshes its mtime (proper-lockfile uses stale / 2).
const HEARTBEAT: Duration = Duration::from_secs(2);

/// RAII guard. While held, a heartbeat thread keeps the lock directory's
/// mtime fresh so other processes never see it as stale. On drop the lock
/// directory is removed only if it is still ours: the random owner token in
/// the sibling `<lock dir>.owner` file must match (a file inside the lock
/// directory would make pi's `rmdir`-based stale break fail).
#[derive(Debug)]
pub struct AuthFileLock {
    lock_dir: PathBuf,
    owner_path: PathBuf,
    token: String,
    stop: Option<std::sync::mpsc::Sender<()>>,
    heartbeat: Option<std::thread::JoinHandle<()>>,
}

impl AuthFileLock {
    /// Waits for the lock without blocking the async runtime.
    pub async fn acquire(auth_path: &Path) -> Result<Self> {
        let started = std::time::Instant::now();
        loop {
            if let Some(lock) = Self::try_acquire(auth_path, STALE_AFTER)? {
                return Ok(lock);
            }
            if started.elapsed() >= ACQUIRE_TIMEOUT {
                bail!("Timed out waiting for the lock on {}", auth_path.display());
            }
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    /// Blocking variant for synchronous callers.
    pub fn acquire_blocking(auth_path: &Path) -> Result<Self> {
        let started = std::time::Instant::now();
        loop {
            if let Some(lock) = Self::try_acquire(auth_path, STALE_AFTER)? {
                return Ok(lock);
            }
            if started.elapsed() >= ACQUIRE_TIMEOUT {
                bail!("Timed out waiting for the lock on {}", auth_path.display());
            }
            std::thread::sleep(RETRY_DELAY);
        }
    }

    /// One attempt; breaks a stale lock directory and retries once.
    /// `None` means someone else holds a fresh lock.
    fn try_acquire(auth_path: &Path, stale_after: Duration) -> Result<Option<Self>> {
        if let Some(parent) = auth_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let lock_dir = lock_dir_for(auth_path);
        for _ in 0..2 {
            match std::fs::create_dir(&lock_dir) {
                Ok(()) => return Ok(Some(Self::held(lock_dir))),
                // Windows reports EPERM while a lock directory is being removed.
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    if !is_stale(&lock_dir, stale_after) {
                        return Ok(None);
                    }
                    break_stale(&lock_dir, stale_after);
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(None)
    }
}

impl AuthFileLock {
    /// Records ownership and starts the heartbeat for a freshly created lock dir.
    fn held(lock_dir: PathBuf) -> Self {
        let mut owner_path = lock_dir.as_os_str().to_os_string();
        owner_path.push(".owner");
        let owner_path = PathBuf::from(owner_path);
        let token = format!("{}-{:016x}", std::process::id(), rand::random::<u64>());
        let _ = std::fs::write(&owner_path, &token);

        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        let beat_dir = lock_dir.clone();
        let heartbeat = std::thread::spawn(move || {
            // Any message or a dropped sender ends the loop; only a timeout beats.
            while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(HEARTBEAT) {
                touch(&beat_dir);
            }
        });
        Self { lock_dir, owner_path, token, stop: Some(stop), heartbeat: Some(heartbeat) }
    }

    fn still_owner(&self) -> bool {
        std::fs::read_to_string(&self.owner_path).is_ok_and(|current| current == self.token)
    }
}

impl Drop for AuthFileLock {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(handle) = self.heartbeat.take() {
            let _ = handle.join();
        }
        if self.still_owner() {
            let _ = std::fs::remove_dir(&self.lock_dir);
            let _ = std::fs::remove_file(&self.owner_path);
        }
    }
}

/// Sets the directory's mtime to now (best effort).
fn touch(dir: &Path) {
    let mut options = std::fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, share all, FILE_FLAG_BACKUP_SEMANTICS (opens a directory).
        options.access_mode(0x100).share_mode(7).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    if let Ok(handle) = options.open(dir) {
        let _ = handle.set_modified(SystemTime::now());
    }
}

fn lock_dir_for(auth_path: &Path) -> PathBuf {
    let mut name = auth_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Removes a lock directory that was seen stale. It is first renamed to a
/// unique name, so of two waiters that both saw it stale only one moves it.
/// If the moved directory is fresh after all (another waiter broke the stale
/// one and re-took the lock in between), it is put back untouched.
fn break_stale(lock_dir: &Path, stale_after: Duration) {
    let mut moved = lock_dir.as_os_str().to_os_string();
    moved.push(format!(".stale-{:016x}", rand::random::<u64>()));
    let moved = PathBuf::from(moved);
    if std::fs::rename(lock_dir, &moved).is_err() {
        return;
    }
    if is_stale(&moved, stale_after) || std::fs::rename(&moved, lock_dir).is_err() {
        let _ = std::fs::remove_dir(&moved);
    }
}

fn is_stale(lock_dir: &Path, stale_after: Duration) -> bool {
    std::fs::metadata(lock_dir)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| age >= stale_after)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_dir_exists_while_held_and_is_removed_after() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        let lock = AuthFileLock::acquire_blocking(&auth).unwrap();
        assert!(dir.path().join("auth.json.lock").is_dir());
        assert!(AuthFileLock::try_acquire(&auth, STALE_AFTER).unwrap().is_none());
        drop(lock);
        assert!(!dir.path().join("auth.json.lock").exists());
    }

    #[test]
    fn stale_lock_dir_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        std::fs::create_dir(dir.path().join("auth.json.lock")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let lock = AuthFileLock::try_acquire(&auth, Duration::from_millis(5)).unwrap();
        assert!(lock.is_some());
    }

    /// FR-036: a waiter that saw a stale lock never removes a fresh one that
    /// another waiter took in the meantime.
    #[test]
    fn breaking_a_lock_that_turned_fresh_puts_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        let lock = AuthFileLock::acquire_blocking(&auth).unwrap();
        let lock_dir = dir.path().join("auth.json.lock");
        break_stale(&lock_dir, STALE_AFTER);
        assert!(lock_dir.is_dir());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2, "only the lock dir and its owner file");
        drop(lock);
        assert!(!lock_dir.exists());
    }

    #[test]
    fn held_lock_is_kept_fresh_by_the_heartbeat() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        let lock = AuthFileLock::acquire_blocking(&auth).unwrap();
        std::thread::sleep(Duration::from_millis(4500));
        let lock_dir = dir.path().join("auth.json.lock");
        assert!(!is_stale(&lock_dir, Duration::from_secs(3)));
        assert!(AuthFileLock::try_acquire(&auth, Duration::from_secs(3)).unwrap().is_none());
        drop(lock);
    }

    #[test]
    fn drop_does_not_remove_a_lock_another_process_owns() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        let lock = AuthFileLock::acquire_blocking(&auth).unwrap();
        // Simulate the lock having been broken and re-taken by someone else.
        std::fs::write(dir.path().join("auth.json.lock.owner"), "someone-else").unwrap();
        drop(lock);
        assert!(dir.path().join("auth.json.lock").is_dir());
    }

    #[tokio::test]
    async fn async_acquire_waits_for_release() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        let held = AuthFileLock::acquire(&auth).await.unwrap();
        let releaser = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(held);
        });
        let second = AuthFileLock::acquire(&auth).await.unwrap();
        releaser.await.unwrap();
        drop(second);
        assert!(!dir.path().join("auth.json.lock").exists());
    }
}
