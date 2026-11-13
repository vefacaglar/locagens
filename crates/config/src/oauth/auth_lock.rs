//! `auth.json` lock compatible with pi's `proper-lockfile` usage: the lock is
//! a `<auth.json path>.lock` directory created with `mkdir`, considered stale
//! when its mtime is older than 10 seconds, and always removed on release.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const RETRY_DELAY: Duration = Duration::from_millis(100);
const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_AFTER: Duration = Duration::from_secs(10);

/// RAII guard: the lock directory is removed on drop.
#[derive(Debug)]
pub struct AuthFileLock {
    lock_dir: PathBuf,
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
                Ok(()) => return Ok(Some(Self { lock_dir })),
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
                    let _ = std::fs::remove_dir(&lock_dir);
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(None)
    }
}

impl Drop for AuthFileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.lock_dir);
    }
}

fn lock_dir_for(auth_path: &Path) -> PathBuf {
    let mut name = auth_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
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
