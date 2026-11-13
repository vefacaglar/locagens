//! Post-command check of the workspace metadata a sandboxed command must not
//! replace or create: `.git`, the git entries of
//! [`crate::protected_paths::SANDBOX_PROTECTED_PATHS`] (`hooks`, `config`,
//! `commondir`, `config.worktree`) and `.locagens`.
//!
//! Why a check after the fact is needed:
//! - **Windows**: ACLs and integrity labels cannot stop a *rename* of these
//!   entries (it needs `DELETE` or the parent's `FILE_DELETE_CHILD`, which a
//!   write-restricted token never checks against its restricting SIDs —
//!   deny ACEs for the capability SID are ignored too; verified), and no ACE
//!   can be placed on an entry that does not exist yet (`commondir`). A
//!   command could run `ren .git old && mkdir .git` or `echo … >
//!   .git\commondir` and plant config/hooks the user's own git runs later.
//! - **Linux**: Bubblewrap pins existing entries as mount points, but cannot
//!   bind over a path that does not exist (`commondir`, `config.worktree`).
//! - **macOS**: Seatbelt path rules deny all of it; the guard never fires.
//!
//! The guard records the identity of each entry (volume + file index on
//! Windows, device + inode on Unix), or that it is absent, before the
//! command, and compares when the command exits, once its background
//! descendants have exited, before the next sandboxed command in that
//! workspace and before Locagens runs git itself. A replaced or newly
//! created entry is moved aside to `<name>.locagens-quarantine-<timestamp>`;
//! a moved original is renamed back (Windows can find it by file ID).
//! Every repair is reported so the tool result can say what happened.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Identity of a file system object: survives renames and content edits,
/// changes when the entry is replaced by a new object.
///
/// Unix file systems hand a freed inode straight back, so after
/// `rm config && echo planted > config` the (device, inode) pair can be the
/// same; the birth time (`statx` btime, where the file system records it)
/// tells the two files apart. Windows file IDs are not reused that way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileId {
    volume: u64,
    index: u64,
    birth_nanos: u128,
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdType, GetFileInformationByHandle, GetFinalPathNameByHandleW, OpenFileById,
        BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    use super::FileId;

    /// Opens `path` itself (directories too, links not followed) for
    /// metadata only, sharing everything so no other access is disturbed.
    fn open_entry(path: &Path) -> io::Result<File> {
        std::fs::OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }

    fn file_id_of(handle: HANDLE) -> Option<FileId> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return None;
        }
        Some(FileId {
            volume: u64::from(info.dwVolumeSerialNumber),
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            birth_nanos: 0,
        })
    }

    pub(super) fn current_id(path: &Path) -> Option<FileId> {
        let file = open_entry(path).ok()?;
        file_id_of(file.as_raw_handle() as HANDLE)
    }

    /// Where the object with `id` lives now (same volume as `volume_hint`).
    pub(super) fn locate(volume_hint: &Path, id: FileId) -> Option<PathBuf> {
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
        if file_id_of(handle.as_raw_handle() as HANDLE)? != id {
            return None;
        }
        let mut buffer = vec![0u16; 32_768];
        let length = unsafe {
            GetFinalPathNameByHandleW(handle.as_raw_handle() as HANDLE, buffer.as_mut_ptr(), buffer.len() as u32, 0)
        } as usize;
        if length == 0 || length >= buffer.len() {
            return None;
        }
        let text = String::from_utf16_lossy(&buffer[..length]);
        Some(crate::windows_sandbox::plan::strip_verbatim_prefix(Path::new(&text)))
    }
}

#[cfg(unix)]
mod os {
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};

    use super::FileId;

    pub(super) fn current_id(path: &Path) -> Option<FileId> {
        let metadata = std::fs::symlink_metadata(path).ok()?;
        let birth_nanos = metadata
            .created()
            .ok()
            .and_then(|created| created.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_nanos());
        Some(FileId { volume: metadata.dev(), index: metadata.ino(), birth_nanos })
    }

    /// Unix has no open-by-inode for unprivileged processes; the sandboxes
    /// there stop renames of existing entries, so there is nothing to find.
    pub(super) fn locate(_volume_hint: &Path, _id: FileId) -> Option<PathBuf> {
        None
    }
}

#[cfg(not(any(windows, unix)))]
mod os {
    use std::path::{Path, PathBuf};

    use super::FileId;

    pub(super) fn current_id(_path: &Path) -> Option<FileId> {
        None
    }

    pub(super) fn locate(_volume_hint: &Path, _id: FileId) -> Option<PathBuf> {
        None
    }
}

/// The entries of `root` to guard: `.git` when it exists, every protected
/// path that exists, and — inside an existing `.git` directory — the
/// protected git paths that do not exist yet (creating them is an attack;
/// without a `.git`, `git init` creating them is legitimate). Parents come
/// before their children.
pub fn guarded_paths(root: &Path) -> Vec<PathBuf> {
    let git_dir = root.join(".git");
    let repository = git_dir.is_dir();
    let mut paths = Vec::new();
    if std::fs::symlink_metadata(&git_dir).is_ok() {
        paths.push(git_dir);
    }
    for entry in crate::protected_paths::SANDBOX_PROTECTED_PATHS {
        let path = entry.under(root);
        let watch_absent = repository && entry.relative.starts_with(".git/");
        if watch_absent || std::fs::symlink_metadata(&path).is_ok() {
            paths.push(path);
        }
    }
    paths
}

/// What was at a guarded path: the object's identity (see [`FileId`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EntryState {
    id: FileId,
}

impl EntryState {
    fn of(path: &Path) -> Option<Self> {
        Some(Self { id: os::current_id(path)? })
    }
}

/// One guarded entry and what was there before the command (`None`:
/// nothing was there).
#[derive(Clone, Debug)]
struct GuardedEntry {
    path: PathBuf,
    state: Option<EntryState>,
}

/// Identities of a workspace's protected metadata, recorded before a command.
#[derive(Debug)]
pub struct MetadataGuard {
    root: PathBuf,
    entries: Vec<GuardedEntry>,
}

impl MetadataGuard {
    /// Records the current state of `paths` (parents before children).
    fn record(root: &Path, paths: &[PathBuf]) -> Self {
        let entries = paths.iter().map(|path| GuardedEntry { path: path.clone(), state: EntryState::of(path) }).collect();
        Self { root: root.to_path_buf(), entries }
    }

    /// Compares every entry with what was recorded and repairs changed
    /// ones. Returns one message per entry that had been changed.
    pub fn check_and_repair(&self) -> Vec<String> {
        self.entries.iter().filter_map(|entry| self.repair(entry)).collect()
    }

    fn repair(&self, entry: &GuardedEntry) -> Option<String> {
        let current = EntryState::of(&entry.path);
        if current == entry.state {
            return None;
        }
        let shown = entry.path.strip_prefix(&self.root).unwrap_or(&entry.path).display().to_string();
        let verb = if entry.state.is_some() { "replaced" } else { "created" };
        let mut report = format!("the sandboxed command {verb} `{shown}`");
        if current.is_some() {
            let quarantine = quarantine_path(&entry.path);
            match std::fs::rename(&entry.path, &quarantine) {
                Ok(()) => report.push_str(&format!("; it was moved to `{}`", quarantine.display())),
                Err(err) => {
                    report.push_str(&format!(
                        "; it could NOT be moved aside ({err}) — inspect `{}` before running git",
                        entry.path.display()
                    ));
                    return Some(report);
                }
            }
        }
        let Some(original) = entry.state else { return Some(report) };
        match os::locate(&self.root, original.id) {
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
/// before Locagens itself runs git there.
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
/// background descendants) changed. The reports are returned and also
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
/// guard of this workspace already tracks keep that guard's (older) state,
/// so a change by a concurrent command is never taken as the baseline.
pub fn start(root: &Path, paths: &[PathBuf]) -> Arc<MetadataGuard> {
    let mut guard = MetadataGuard::record(root, paths);
    let mut registry = active();
    for entry in &mut guard.entries {
        let older = registry
            .iter()
            .filter(|other| other.root == guard.root)
            .flat_map(|other| other.entries.iter())
            .find(|other| other.path == entry.path);
        if let Some(older) = older {
            entry.state = older.state;
        }
    }
    let guard = Arc::new(guard);
    registry.push(Arc::clone(&guard));
    guard
}

/// Repairs what earlier commands in `root` changed, then guards `root` for
/// a new command. Returns the guard and the earlier repairs to report.
pub fn start_command(root: &Path) -> (Arc<MetadataGuard>, Vec<String>) {
    let earlier = check_workspace(root);
    (start(root, &guarded_paths(root)), earlier)
}

/// Final check of `guard`; it stops being active.
pub fn finish(guard: &Arc<MetadataGuard>) -> Vec<String> {
    let reports = guard.check_and_repair();
    active().retain(|other| !Arc::ptr_eq(other, guard));
    reports
}

/// Checks `guard` now that the root command exited. While `still_running`
/// says background descendants are alive, the guard stays active and is
/// checked once more after they have all exited; what that finds is queued
/// for [`check_workspace`].
pub fn check_after_exit(guard: &Arc<MetadataGuard>, still_running: impl Fn() -> bool + Send + 'static) -> Vec<String> {
    if !still_running() {
        return finish(guard);
    }
    let reports = guard.check_and_repair();
    let guard = Arc::clone(guard);
    std::thread::spawn(move || {
        while still_running() {
            std::thread::sleep(Duration::from_millis(500));
        }
        let reports = finish(&guard);
        late_reports().extend(reports.into_iter().map(|report| (guard.root.clone(), report)));
    });
    reports
}

/// Whether any process of the process group `pgid` is still alive.
#[cfg(unix)]
pub fn process_group_alive(pgid: u32) -> bool {
    let Ok(pgid) = i32::try_from(pgid) else { return false };
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// The canonical long form of `path` used as the guard registry key.
pub fn workspace_root(path: &Path) -> PathBuf {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(windows)]
    return crate::windows_sandbox::plan::strip_verbatim_prefix(&canonical);
    #[cfg(not(windows))]
    canonical
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("locagens-guard-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        workspace_root(&dir)
    }

    #[test]
    fn guarded_paths_watch_absent_git_entries_only_inside_a_repository() {
        let root = temp_root("paths");
        assert_eq!(guarded_paths(&root), Vec::<PathBuf>::new(), "no .git, no .locagens: nothing");
        std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
        std::fs::create_dir_all(root.join(".locagens")).unwrap();
        let git = root.join(".git");
        assert_eq!(
            guarded_paths(&root),
            vec![
                git.clone(),
                root.join(".locagens"),
                git.join("hooks"),
                git.join("config"),
                git.join("commondir"),
                git.join("config.worktree"),
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untouched_metadata_reports_nothing() {
        let root = temp_root("same");
        std::fs::create_dir_all(root.join(".git").join("hooks")).unwrap();
        std::fs::write(root.join(".git").join("config"), "[core]\n").unwrap();
        let guard = MetadataGuard::record(&root, &guarded_paths(&root));
        assert_eq!(guard.entries.len(), 5);
        // Editing contents keeps the object.
        std::fs::write(root.join(".git").join("config"), "[core]\nbare = false\n").unwrap();
        std::fs::write(root.join(".git").join("hooks").join("README"), "x").unwrap();
        assert!(guard.check_and_repair().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn created_entries_are_quarantined() {
        let root = temp_root("created");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let guard = MetadataGuard::record(&root, &guarded_paths(&root));
        std::fs::write(root.join(".git").join("commondir"), "../evil\n").unwrap();
        std::fs::write(root.join(".git").join("config.worktree"), "[alias]\n").unwrap();
        let reports = guard.check_and_repair();
        assert_eq!(reports.len(), 2, "{reports:?}");
        assert!(reports.iter().all(|report| report.contains("created")), "{reports:?}");
        assert!(!root.join(".git").join("commondir").exists());
        assert!(!root.join(".git").join("config.worktree").exists());
        let names: Vec<String> = std::fs::read_dir(root.join(".git"))
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name().to_string_lossy().to_string()))
            .collect();
        assert!(names.iter().any(|name| name.starts_with("commondir.locagens-quarantine-")), "{names:?}");
        assert!(guard.check_and_repair().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Restoring a moved original needs open-by-file-ID (Windows only).
    #[cfg(windows)]
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
        // Windows: a new file ID; Unix may reuse the freed inode, the birth
        // time still differs (CI on ubuntu caught the inode reuse).
        assert_eq!(reports.len(), 1, "{reports:?}");
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
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let first = start(&root, &[root.join(".git").join("commondir")]);
        // A concurrent command plants the file before a second one starts.
        std::fs::write(root.join(".git").join("commondir"), "../evil").unwrap();
        let second = start(&root, &[root.join(".git").join("commondir")]);
        assert_eq!(second.entries[0].state, None, "the planted file is not the new baseline");

        let reports = check_workspace(&root);
        assert!(!reports.is_empty() && reports[0].contains("created"), "{reports:?}");
        assert!(!root.join(".git").join("commondir").exists());
        assert!(finish(&second).is_empty() && finish(&first).is_empty());
        assert!(!active().iter().any(|guard| guard.root == root), "finished guards are dropped");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn background_checks_run_once_the_descendants_are_gone() {
        let root = temp_root("background");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let guard = start(&root, &guarded_paths(&root));
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let flag = Arc::clone(&alive);
        let now = check_after_exit(&guard, move || flag.load(std::sync::atomic::Ordering::SeqCst));
        assert!(now.is_empty());
        std::fs::write(root.join(".git").join("commondir"), "../evil").unwrap();
        alive.store(false, std::sync::atomic::Ordering::SeqCst);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while root.join(".git").join("commondir").exists() {
            assert!(std::time::Instant::now() < deadline, "the late check never ran");
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(check_workspace(&root).iter().any(|report| report.contains("commondir")));
        let _ = std::fs::remove_dir_all(&root);
    }
}
