//! Link-safe writes inside a workspace (QA-017).
//!
//! [`resolve_safe_path`](crate::resolve_safe_path) checks a path once; a
//! symlink or junction swapped in afterwards (by a concurrently running
//! command, say) would still redirect a plain `std::fs::write` out of the
//! workspace. Every in-process mutation therefore goes through this module,
//! which re-checks containment on what it actually opened:
//!
//! - Unix: the path is walked from an fd of the workspace root with
//!   `openat(…, O_NOFOLLOW | O_DIRECTORY)` per component and the final
//!   `openat` / `unlinkat` / `renameat` happens relative to the verified
//!   parent fd. No component is ever resolved through a link, so there is no
//!   window between check and use.
//! - Windows (no `openat`): files are opened with
//!   `FILE_FLAG_OPEN_REPARSE_POINT` (the final component is never followed)
//!   and the opened handle is verified — not a reparse point, and its final
//!   path (`GetFinalPathNameByHandleW`) still inside the workspace — before
//!   anything is written, deleted or renamed. A file created through a
//!   swapped parent is deleted again through its handle. Each created
//!   directory is verified the same way.
//!
//! `path` arguments are absolute paths returned by `resolve_safe_path` for
//! the same `root`.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Error for a path that leaves the workspace through a link.
fn link_escape(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        format!("refusing to follow a link out of the workspace at {}", path.display()),
    )
}

/// The canonical workspace root (verbatim prefix stripped).
fn canonical_root(root: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(root).map(crate::normalize_canonical)
}

/// `path` relative to the canonical root, as plain names.
fn relative_names<'a>(root: &Path, path: &'a Path) -> io::Result<Vec<&'a OsStr>> {
    let relative = path.strip_prefix(root).map_err(|_| link_escape(path))?;
    relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name),
            _ => Err(link_escape(path)),
        })
        .collect()
}

/// Splits `path` into its parent names and file name.
fn parent_and_name<'a>(root: &Path, path: &'a Path) -> io::Result<(Vec<&'a OsStr>, &'a OsStr)> {
    let mut names = relative_names(root, path)?;
    let name = names
        .pop()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path names the workspace root itself"))?;
    Ok((names, name))
}

/// Creates or truncates the file at `path` with `contents`, creating missing
/// parent directories.
pub fn write(root: &Path, path: &Path, contents: &[u8]) -> io::Result<()> {
    let root = canonical_root(root)?;
    let (parents, name) = parent_and_name(&root, path)?;
    imp::write(&root, &parents, name, path, contents)
}

/// Creates `dir` and its missing parents.
pub fn create_dir_all(root: &Path, dir: &Path) -> io::Result<()> {
    let root = canonical_root(root)?;
    let names = relative_names(&root, dir)?;
    imp::create_dir_all(&root, &names, dir).map(|_| ())
}

/// Removes the file (or symlink) at `path`; directories are refused.
pub fn remove_file(root: &Path, path: &Path) -> io::Result<()> {
    let root = canonical_root(root)?;
    let (parents, name) = parent_and_name(&root, path)?;
    imp::remove_file(&root, &parents, name, path)
}

/// Renames `from` to `to` (replacing a file at `to`), creating missing
/// parents of `to`.
pub fn rename(root: &Path, from: &Path, to: &Path) -> io::Result<()> {
    let root = canonical_root(root)?;
    let (from_parents, from_name) = parent_and_name(&root, from)?;
    let (to_parents, to_name) = parent_and_name(&root, to)?;
    imp::rename(&root, (&from_parents, from_name, from), (&to_parents, to_name, to))
}

#[cfg(unix)]
mod imp {
    use super::link_escape;
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    fn c_name(name: &OsStr) -> io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains a NUL byte"))
    }

    fn check(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    /// `ELOOP` / `ENOTDIR` from `O_NOFOLLOW`: the name is a link.
    fn map_link_error(err: io::Error, path: &Path) -> io::Error {
        match err.raw_os_error() {
            Some(libc::ELOOP) | Some(libc::ENOTDIR) | Some(libc::EMLINK) => link_escape(path),
            _ => err,
        }
    }

    fn open_root(root: &Path) -> io::Result<OwnedFd> {
        let path = c_name(root.as_os_str())?;
        let fd = check(unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    /// Opens (optionally creating) directory `name` under `dir` without
    /// following a link.
    fn open_dir_at(dir: &OwnedFd, name: &OsStr, create: bool, path: &Path) -> io::Result<OwnedFd> {
        let c = c_name(name)?;
        if create {
            let made = unsafe { libc::mkdirat(dir.as_raw_fd(), c.as_ptr(), 0o777) };
            if made < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EEXIST) {
                    return Err(err);
                }
            }
        }
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = check(unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), flags) })
            .map_err(|err| map_link_error(err, path))?;
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn walk(root: &Path, names: &[&OsStr], create: bool, path: &Path) -> io::Result<OwnedFd> {
        let mut dir = open_root(root)?;
        for name in names {
            dir = open_dir_at(&dir, name, create, path)?;
        }
        Ok(dir)
    }

    pub(super) fn create_dir_all(root: &Path, names: &[&OsStr], dir: &Path) -> io::Result<OwnedFd> {
        walk(root, names, true, dir)
    }

    pub(super) fn write(root: &Path, parents: &[&OsStr], name: &OsStr, path: &Path, contents: &[u8]) -> io::Result<()> {
        let dir = walk(root, parents, true, path)?;
        let c = c_name(name)?;
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC;
        let fd = check(unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), flags, 0o666 as libc::c_uint) })
            .map_err(|err| map_link_error(err, path))?;
        let mut file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a regular file", path.display())));
        }
        file.set_len(0)?;
        file.write_all(contents)?;
        Ok(())
    }

    pub(super) fn remove_file(root: &Path, parents: &[&OsStr], name: &OsStr, path: &Path) -> io::Result<()> {
        let dir = walk(root, parents, false, path)?;
        // `unlinkat` refuses directories anyway; this only names the reason.
        if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is a directory", path.display())));
        }
        let c = c_name(name)?;
        check(unsafe { libc::unlinkat(dir.as_raw_fd(), c.as_ptr(), 0) })?;
        Ok(())
    }

    pub(super) fn rename(
        root: &Path,
        (from_parents, from_name, from): (&[&OsStr], &OsStr, &Path),
        (to_parents, to_name, to): (&[&OsStr], &OsStr, &Path),
    ) -> io::Result<()> {
        let from_dir = walk(root, from_parents, false, from)?;
        let to_dir = walk(root, to_parents, true, to)?;
        let (from_c, to_c) = (c_name(from_name)?, c_name(to_name)?);
        check(unsafe { libc::renameat(from_dir.as_raw_fd(), from_c.as_ptr(), to_dir.as_raw_fd(), to_c.as_ptr()) })?;
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use super::link_escape;
    use std::ffi::{OsStr, OsString};
    use std::fs::{File, OpenOptions};
    use std::io::{self, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, FileDispositionInfoEx, FileRenameInfo, GetFinalPathNameByHandleW,
        SetFileInformationByHandle, DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE, FILE_NAME_NORMALIZED,
        FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE, VOLUME_NAME_DOS,
    };

    fn raw(file: &File) -> HANDLE {
        file.as_raw_handle() as HANDLE
    }

    /// The path the handle really refers to.
    fn final_path(file: &File) -> io::Result<PathBuf> {
        let mut buffer = vec![0u16; 512];
        loop {
            let length = unsafe {
                GetFinalPathNameByHandleW(raw(file), buffer.as_mut_ptr(), buffer.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS)
            } as usize;
            if length == 0 {
                return Err(io::Error::last_os_error());
            }
            if length < buffer.len() {
                buffer.truncate(length);
                return Ok(crate::normalize_canonical(PathBuf::from(OsString::from_wide(&buffer))));
            }
            buffer.resize(length + 1, 0);
        }
    }

    /// Refuses a handle that is a reparse point (unless `allow_link`) or
    /// whose final path is outside the workspace.
    fn verify(root: &Path, file: &File, allow_link: bool, path: &Path) -> io::Result<()> {
        let attributes = file.metadata()?.file_attributes();
        if !allow_link && attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(link_escape(path));
        }
        if !final_path(file)?.starts_with(root) {
            return Err(link_escape(path));
        }
        Ok(())
    }

    /// Marks an open file for deletion (POSIX semantics where supported, so
    /// the name is free at once).
    fn delete_by_handle(file: &File) -> io::Result<()> {
        let posix = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS };
        let ok = unsafe {
            SetFileInformationByHandle(
                raw(file),
                FileDispositionInfoEx,
                &posix as *const _ as *const _,
                std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
            )
        };
        if ok != 0 {
            return Ok(());
        }
        let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
        let ok = unsafe {
            SetFileInformationByHandle(
                raw(file),
                FileDispositionInfo,
                &legacy as *const _ as *const _,
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Opens a directory (or whatever is at `path`) without following a
    /// final reparse point. `pin` leaves out `FILE_SHARE_DELETE` so it
    /// cannot be renamed or replaced while the handle is open.
    fn open_dir(path: &Path, pin: bool) -> io::Result<File> {
        let mut options = OpenOptions::new();
        options
            .access_mode(FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        if pin {
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        options.open(path)
    }

    fn verified_dir(root: &Path, dir: &Path, pin: bool) -> io::Result<File> {
        let handle = open_dir(dir, pin)?;
        verify(root, &handle, false, dir)?;
        if !handle.metadata()?.is_dir() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a directory", dir.display())));
        }
        Ok(handle)
    }

    pub(super) fn create_dir_all(root: &Path, names: &[&OsStr], _dir: &Path) -> io::Result<()> {
        let mut current = root.to_path_buf();
        for name in names {
            current.push(name);
            match std::fs::create_dir(&current) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
            verified_dir(root, &current, false)?;
        }
        Ok(())
    }

    /// Opens `path` for writing without following a final link: a new file
    /// (`true`, deletable through the handle) or an existing one.
    fn open_for_write(path: &Path) -> io::Result<(File, bool)> {
        let mut create = OpenOptions::new();
        create
            .write(true)
            .create_new(true)
            .access_mode(FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        match create.open(path) {
            Ok(file) => return Ok((file, true)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
        let file = OpenOptions::new()
            .write(true)
            .access_mode(FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        Ok((file, false))
    }

    pub(super) fn write(root: &Path, parents: &[&OsStr], _name: &OsStr, path: &Path, contents: &[u8]) -> io::Result<()> {
        create_dir_all(root, parents, path)?;
        let (mut file, created) = open_for_write(path)?;
        let checked = verify(root, &file, false, path).and_then(|()| {
            if file.metadata()?.is_file() {
                Ok(())
            } else {
                Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a regular file", path.display())))
            }
        });
        if let Err(err) = checked {
            if created {
                let _ = delete_by_handle(&file);
            }
            return Err(err);
        }
        file.set_len(0)?;
        file.write_all(contents)?;
        Ok(())
    }

    /// Opens an existing entry for delete / rename without following it.
    fn open_for_delete(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .access_mode(DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    }

    pub(super) fn remove_file(root: &Path, _parents: &[&OsStr], _name: &OsStr, path: &Path) -> io::Result<()> {
        let file = open_for_delete(path)?;
        // A link itself may be deleted (that never touches its target).
        verify(root, &file, true, path)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is a directory", path.display())));
        }
        delete_by_handle(&file)
    }

    pub(super) fn rename(
        root: &Path,
        (_, _, from): (&[&OsStr], &OsStr, &Path),
        (to_parents, _, to): (&[&OsStr], &OsStr, &Path),
    ) -> io::Result<()> {
        let source = open_for_delete(from)?;
        verify(root, &source, true, from)?;
        create_dir_all(root, to_parents, to)?;
        // Pin the destination directory while the rename runs.
        let parent = to.parent().ok_or_else(|| link_escape(to))?;
        let _pinned = verified_dir(root, parent, true)?;

        let target: Vec<u16> = to.as_os_str().encode_wide().collect();
        let header = std::mem::size_of::<FILE_RENAME_INFO>();
        let size = header + target.len() * 2;
        // u64 backing keeps the struct aligned.
        let mut buffer = vec![0u64; size.div_ceil(8)];
        let info = buffer.as_mut_ptr() as *mut FILE_RENAME_INFO;
        unsafe {
            (*info).Anonymous.ReplaceIfExists = true;
            (*info).RootDirectory = std::ptr::null_mut();
            (*info).FileNameLength = (target.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(target.as_ptr(), std::ptr::addr_of_mut!((*info).FileName) as *mut u16, target.len());
        }
        let ok = unsafe { SetFileInformationByHandle(raw(&source), FileRenameInfo, info as *const _, size as u32) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve_safe_path;

    struct Dirs {
        base: PathBuf,
        ws: PathBuf,
        outside: PathBuf,
    }

    impl Dirs {
        fn new(label: &str) -> Self {
            let base = tempfile::Builder::new().prefix(&format!("locagens-wsfs-{label}-")).tempdir().unwrap().keep();
            let base = crate::normalize_canonical(std::fs::canonicalize(&base).unwrap());
            let ws = base.join("ws");
            let outside = base.join("outside");
            std::fs::create_dir_all(&ws).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            Self { base, ws, outside }
        }
    }

    impl Drop for Dirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    /// A directory link (junction on Windows, symlink elsewhere); `false`
    /// when the platform refuses to create one.
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|out| out.status.success())
        }
    }

    /// A file symlink; `false` without the privilege (Windows).
    fn link_file(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
    }

    #[test]
    fn ordinary_mutations_work() {
        let dirs = Dirs::new("ok");
        let file = resolve_safe_path(&dirs.ws, "a/b/c.txt").unwrap();
        write(&dirs.ws, &file, b"one").unwrap();
        write(&dirs.ws, &file, b"two").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "two");

        let moved = resolve_safe_path(&dirs.ws, "d/moved.txt").unwrap();
        rename(&dirs.ws, &file, &moved).unwrap();
        assert!(!file.exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "two");
        // Rename replaces an existing destination file.
        let other = resolve_safe_path(&dirs.ws, "d/other.txt").unwrap();
        write(&dirs.ws, &other, b"other").unwrap();
        rename(&dirs.ws, &other, &moved).unwrap();
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "other");
        // Directories move too.
        let dir = resolve_safe_path(&dirs.ws, "a").unwrap();
        let dir_to = resolve_safe_path(&dirs.ws, "e/a2").unwrap();
        rename(&dirs.ws, &dir, &dir_to).unwrap();
        assert!(dir_to.join("b").is_dir());

        remove_file(&dirs.ws, &moved).unwrap();
        assert!(!moved.exists());
        let err = remove_file(&dirs.ws, &dir_to).unwrap_err();
        assert!(err.to_string().contains("directory"), "{err}");

        let nested = resolve_safe_path(&dirs.ws, "x/y/z").unwrap();
        create_dir_all(&dirs.ws, &nested).unwrap();
        assert!(nested.is_dir());
        create_dir_all(&dirs.ws, &nested).unwrap();

        // A path outside the root is refused outright.
        assert!(write(&dirs.ws, &dirs.outside.join("x.txt"), b"x").is_err());
        assert!(!dirs.outside.join("x.txt").exists());
    }

    /// The race QA-017 is about: a directory resolved inside the workspace
    /// is swapped for a link to the outside before the write happens.
    #[test]
    fn a_parent_swapped_for_a_link_after_resolution_is_refused() {
        let dirs = Dirs::new("swap");
        std::fs::create_dir_all(dirs.ws.join("sub")).unwrap();
        std::fs::write(dirs.outside.join("victim.txt"), "original").unwrap();
        let file = resolve_safe_path(&dirs.ws, "sub/victim.txt").unwrap();
        let fresh = resolve_safe_path(&dirs.ws, "sub/fresh.txt").unwrap();
        let deep = resolve_safe_path(&dirs.ws, "sub/deeper/x.txt").unwrap();

        std::fs::remove_dir(dirs.ws.join("sub")).unwrap();
        if !link_dir(&dirs.outside, &dirs.ws.join("sub")) {
            eprintln!("skipping: cannot create a directory link");
            return;
        }

        let err = write(&dirs.ws, &file, b"pwned").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
        assert_eq!(std::fs::read_to_string(dirs.outside.join("victim.txt")).unwrap(), "original");
        assert!(write(&dirs.ws, &fresh, b"pwned").is_err());
        assert!(!dirs.outside.join("fresh.txt").exists(), "nothing is left behind outside");
        assert!(write(&dirs.ws, &deep, b"pwned").is_err());
        assert!(!dirs.outside.join("deeper").join("x.txt").exists());
        assert!(create_dir_all(&dirs.ws, &deep.parent().unwrap().join("more")).is_err());

        assert!(remove_file(&dirs.ws, &file).is_err());
        assert!(dirs.outside.join("victim.txt").exists());
        let inside = resolve_safe_path(&dirs.ws, "kept.txt").unwrap();
        write(&dirs.ws, &inside, b"mine").unwrap();
        assert!(rename(&dirs.ws, &inside, &fresh).is_err(), "rename into a swapped dir");
        assert!(!dirs.outside.join("fresh.txt").exists());
        assert!(rename(&dirs.ws, &file, &inside).is_err(), "rename out of a swapped dir");
        assert!(dirs.outside.join("victim.txt").exists());
    }

    /// The final component swapped for a file link after resolution.
    #[test]
    fn a_file_swapped_for_a_link_after_resolution_is_refused() {
        let dirs = Dirs::new("fileswap");
        let target = dirs.outside.join("target.txt");
        std::fs::write(&target, "original").unwrap();
        let file = resolve_safe_path(&dirs.ws, "notes.md").unwrap();
        if !link_file(&target, &dirs.ws.join("notes.md")) {
            eprintln!("skipping: cannot create a file symlink");
            return;
        }
        assert!(write(&dirs.ws, &file, b"pwned").is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        // Deleting the link removes the link only.
        remove_file(&dirs.ws, &file).unwrap();
        assert!(std::fs::symlink_metadata(&file).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    }
}
