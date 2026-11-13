//! Post-command check of the workspace metadata a sandboxed command must not
//! replace (`.git`, `.git/hooks`, `.git/config`, `.locagens`).
//!
//! ACLs and integrity labels cannot stop the command from *renaming* these
//! entries: renaming needs `DELETE` on the entry or `FILE_DELETE_CHILD` on
//! its parent, and a write-restricted token never checks either right
//! against its restricting SIDs (deny ACEs for the capability SID are not
//! evaluated; verified), while the parents (workspace root, `.git`) must
//! stay Low so the command can create files there. A command could
//! therefore run `ren .git old && mkdir .git` and plant a `config` (e.g.
//! `core.fsmonitor`) or hooks that the user's own git would run later.
//!
//! The guard records the file ID (volume serial + file index) of each entry
//! before the command starts and compares after it (and again once
//! background descendants have exited). A changed entry is repaired: the
//! replacement is moved aside to `<name>.locagens-quarantine-<timestamp>`
//! and the original — found by its file ID wherever the command moved it —
//! is renamed back. Every repair is reported so the tool result can say
//! what happened.

use std::fs::File;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    GetFileInformationByHandle, GetFinalPathNameByHandleW, OpenFileById, BY_HANDLE_FILE_INFORMATION,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FileIdType,
};

use super::job::JobObject;
use super::plan::strip_verbatim_prefix;

/// Identity of a file system object: survives renames, changes when the
/// entry is replaced by a new object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    volume: u32,
    index: u64,
}

/// Opens `path` itself (directories too, links not followed) for metadata
/// only, sharing everything so no other access is disturbed.
fn open_entry(path: &Path) -> io::Result<File> {
    std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn file_id_of(handle: HANDLE) -> io::Result<FileId> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileId {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    })
}

/// The file ID of the object currently at `path`, `None` when nothing is there.
fn current_id(path: &Path) -> Option<FileId> {
    let file = open_entry(path).ok()?;
    file_id_of(file.as_raw_handle() as HANDLE).ok()
}

/// Where the object with `id` lives now (same volume as `volume_hint`).
fn locate(volume_hint: &Path, id: FileId) -> Option<PathBuf> {
    let hint = open_entry(volume_hint).ok()?;
    let descriptor = FILE_ID_DESCRIPTOR {
        dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id.index as i64 },
    };
    let raw = unsafe {
        OpenFileById(
            hint.as_raw_handle() as HANDLE,
            &descriptor,
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if raw == INVALID_HANDLE_VALUE || raw.is_null() {
        return None;
    }
    let handle = unsafe { OwnedHandle::from_raw_handle(raw as _) };
    if file_id_of(handle.as_raw_handle() as HANDLE).ok()? != id {
        return None;
    }
    let mut buffer = vec![0u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(handle.as_raw_handle() as HANDLE, buffer.as_mut_ptr(), buffer.len() as u32, 0)
    } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    Some(strip_verbatim_prefix(Path::new(&String::from_utf16_lossy(&buffer[..length]))))
}

/// One guarded entry and the object that was there before the command.
#[derive(Clone, Debug)]
struct GuardedEntry {
    path: PathBuf,
    id: FileId,
}

/// File IDs of a workspace's protected metadata, recorded before a command.
#[derive(Debug)]
pub struct MetadataGuard {
    root: PathBuf,
    entries: Vec<GuardedEntry>,
}

impl MetadataGuard {
    /// Records the entries of `paths` that exist now. Parents must come
    /// before their children (`.git` before `.git/hooks`).
    pub fn record(root: &Path, paths: &[PathBuf]) -> Self {
        let entries = paths
            .iter()
            .filter_map(|path| current_id(path).map(|id| GuardedEntry { path: path.clone(), id }))
            .collect();
        Self { root: root.to_path_buf(), entries }
    }

    /// Compares every entry with what was recorded and repairs replaced
    /// ones. Returns one message per entry that had been changed.
    pub fn check_and_repair(&self) -> Vec<String> {
        self.entries.iter().filter_map(|entry| self.repair(entry)).collect()
    }

    fn repair(&self, entry: &GuardedEntry) -> Option<String> {
        if current_id(&entry.path) == Some(entry.id) {
            return None;
        }
        let shown = entry.path.strip_prefix(&self.root).unwrap_or(&entry.path).display().to_string();
        let mut report = format!("the sandboxed command replaced `{shown}`");
        if current_id(&entry.path).is_some() {
            let quarantine = quarantine_path(&entry.path);
            match std::fs::rename(&entry.path, &quarantine) {
                Ok(()) => report.push_str(&format!("; the replacement was moved to `{}`", quarantine.display())),
                Err(err) => {
                    report.push_str(&format!(
                        "; the replacement could NOT be moved aside ({err}) — inspect `{}` before running git",
                        entry.path.display()
                    ));
                    return Some(report);
                }
            }
        }
        match locate(&self.root, entry.id) {
            Some(moved) => match std::fs::rename(&moved, &entry.path) {
                Ok(()) => report.push_str(&format!("; the original was restored from `{}`", moved.display())),
                Err(err) => report.push_str(&format!(
                    "; the original is at `{}` and could not be moved back ({err})",
                    moved.display()
                )),
            },
            None => report.push_str("; the original no longer exists (deleted or moved off this volume)"),
        }
        Some(report)
    }
}

fn quarantine_path(path: &Path) -> PathBuf {
    let stamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
    let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    path.with_file_name(format!("{name}.locagens-quarantine-{stamp}"))
}

/// Guards whose command (or its background descendants) may still run:
/// checked before the next sandboxed command in the same workspace and
/// before Locagens itself runs git there ([`check_workspace`]).
static ACTIVE: Mutex<Vec<Arc<MetadataGuard>>> = Mutex::new(Vec::new());

/// Reports of checks that ran after the command's result was already
/// returned (background descendants), keyed by workspace root.
static LATE_REPORTS: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

fn active() -> std::sync::MutexGuard<'static, Vec<Arc<MetadataGuard>>> {
    ACTIVE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn late_reports() -> std::sync::MutexGuard<'static, Vec<(PathBuf, String)>> {
    LATE_REPORTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Repairs what still-running sandboxed commands in `root` (or their
/// background descendants) replaced. The reports are returned and also
/// queued for the next sandboxed command, so none is lost when the caller
/// (e.g. a git file listing) cannot show it. Run it before anything trusts
/// the workspace's git metadata.
pub fn repair_active(root: &Path) -> Vec<String> {
    let guards: Vec<Arc<MetadataGuard>> = active().iter().filter(|guard| guard.root == root).cloned().collect();
    let reports: Vec<String> = guards.iter().flat_map(|guard| guard.check_and_repair()).collect();
    late_reports().extend(reports.iter().map(|report| (root.to_path_buf(), report.clone())));
    reports
}

/// What the next sandboxed command in `root` must report: repairs made now
/// and queued ones from checks that ran after earlier results returned.
pub fn check_workspace(root: &Path) -> Vec<String> {
    repair_active(root);
    let mut queue = late_reports();
    let (taken, kept): (Vec<_>, Vec<_>) = queue.drain(..).partition(|(queued, _)| queued == root);
    *queue = kept;
    taken.into_iter().map(|(_, report)| report).collect()
}
/// Starts guarding `paths` of `root` for one command. Entries another live
/// guard of this workspace already tracks keep that guard's (older)
/// identity, so a swap by a concurrent command is never taken as the
/// baseline.
pub(crate) fn start(root: &Path, paths: &[PathBuf]) -> Arc<MetadataGuard> {
    let mut guard = MetadataGuard::record(root, paths);
    let mut registry = active();
    for entry in &mut guard.entries {
        let older = registry
            .iter()
            .filter(|other| other.root == guard.root)
            .flat_map(|other| other.entries.iter())
            .find(|other| other.path == entry.path);
        if let Some(older) = older {
            entry.id = older.id;
        }
    }
    let guard = Arc::new(guard);
    registry.push(Arc::clone(&guard));
    guard
}

/// Final check of `guard`; it stops being active.
fn finish(guard: &Arc<MetadataGuard>) -> Vec<String> {
    let reports = guard.check_and_repair();
    active().retain(|other| !Arc::ptr_eq(other, guard));
    reports
}

/// Checks `guard` now that the root command exited. When processes of `job`
/// are still running (background descendants), the guard stays active and
/// is checked once more after they have all exited; what that finds is
/// queued for [`check_workspace`].
pub fn check_after_exit(guard: &Arc<MetadataGuard>, job: &Arc<JobObject>) -> Vec<String> {
    if !job.active_processes().is_ok_and(|count| count > 0) {
        return finish(guard);
    }
    let reports = guard.check_and_repair();
    let (guard, job) = (Arc::clone(guard), Arc::clone(job));
    std::thread::spawn(move || {
        while job.active_processes().is_ok_and(|count| count > 0) {
            std::thread::sleep(Duration::from_millis(500));
        }
        let reports = finish(&guard);
        late_reports().extend(reports.into_iter().map(|report| (guard.root.clone(), report)));
    });
    reports
}

/// Final check after the tree was killed.
pub fn check_after_kill(guard: &Arc<MetadataGuard>) -> Vec<String> {
    finish(guard)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-guard-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        super::super::plan::normalize_path(&dir)
    }

    #[test]
    fn untouched_metadata_reports_nothing() {
        let root = temp_root("same");
        std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
        std::fs::write(root.join(".git").join("config"), "[core]\n").unwrap();
        let paths = [root.join(".git"), root.join(".git").join("hooks"), root.join(".git").join("config")];
        let guard = MetadataGuard::record(&root, &paths);
        assert_eq!(guard.entries.len(), 3);
        // Editing contents keeps the object.
        std::fs::write(root.join(".git").join("config"), "[core]\nbare = false\n").unwrap();
        assert!(guard.check_and_repair().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renamed_and_replaced_entries_are_quarantined_and_restored() {
        let root = temp_root("swap");
        std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
        std::fs::write(root.join(".git").join("config"), "original").unwrap();
        let guard = MetadataGuard::record(&root, &[root.join(".git"), root.join(".git").join("config")]);

        std::fs::rename(root.join(".git"), root.join("old-git")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git").join("config"), "planted").unwrap();

        let reports = guard.check_and_repair();
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].contains("restored"), "{reports:?}");
        assert_eq!(std::fs::read_to_string(root.join(".git").join("config")).unwrap(), "original");
        assert!(!root.join("old-git").exists());
        let quarantined: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name().to_string_lossy().to_string()))
            .filter(|name| name.starts_with(".git.locagens-quarantine-"))
            .collect();
        assert_eq!(quarantined.len(), 1);
        assert_eq!(std::fs::read_to_string(root.join(&quarantined[0]).join("config")).unwrap(), "planted");
        assert!(guard.check_and_repair().is_empty(), "repaired state is intact");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleted_originals_are_reported() {
        let root = temp_root("gone");
        std::fs::write(root.join("config"), "original").unwrap();
        let guard = MetadataGuard::record(&root, &[root.join("config")]);
        std::fs::remove_file(root.join("config")).unwrap();
        std::fs::write(root.join("config"), "planted").unwrap();
        let reports = guard.check_and_repair();
        assert!(reports[0].contains("no longer exists"), "{reports:?}");
        assert!(!root.join("config").exists(), "the planted file is quarantined");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn late_reports_are_taken_per_workspace() {
        let first = temp_root("late-one");
        let second = temp_root("late-two");
        late_reports().push((first.clone(), "a".into()));
        late_reports().push((second.clone(), "b".into()));
        assert_eq!(check_workspace(&first), vec!["a".to_string()]);
        assert!(check_workspace(&first).is_empty());
        assert_eq!(check_workspace(&second), vec!["b".to_string()]);
        for root in [first, second] {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn active_guards_are_repaired_on_demand_and_keep_the_oldest_baseline() {
        let root = temp_root("active");
        std::fs::write(root.join("config"), "original").unwrap();
        let first = start(&root, &[root.join("config")]);
        // A concurrent command swaps the file before a second command starts.
        std::fs::rename(root.join("config"), root.join("moved")).unwrap();
        std::fs::write(root.join("config"), "planted").unwrap();
        let second = start(&root, &[root.join("config")]);
        assert_eq!(second.entries[0].id, first.entries[0].id, "the swap is not the new baseline");

        let reports = check_workspace(&root);
        assert!(!reports.is_empty() && reports[0].contains("restored"), "{reports:?}");
        assert_eq!(std::fs::read_to_string(root.join("config")).unwrap(), "original");
        assert!(finish(&second).is_empty() && finish(&first).is_empty());
        assert!(!active().iter().any(|guard| guard.root == root), "finished guards are dropped");
        let _ = std::fs::remove_dir_all(&root);
    }
}