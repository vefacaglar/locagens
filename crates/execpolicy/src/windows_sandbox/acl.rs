//! File-system ACL edits for the capability SID (port of the parts of Codex
//! `windows-sandbox-rs/src/acl.rs` the legacy, non-elevated backend uses).
//!
//! Only ACEs naming the sandbox capability SID are ever added or removed;
//! existing ACEs are preserved (`SetEntriesInAclW` merges into the current
//! DACL). Each edit is skipped when an equivalent ACE already exists, so
//! repeated runs do not rewrite (and re-propagate) the DACL.

use std::ffi::c_void;
use std::io;
use std::path::Path;

use windows_sys::Win32::Foundation::{LocalFree, HLOCAL};
use windows_sys::Win32::Security::Authorization::{
    GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, ACCESS_MODE, DENY_ACCESS,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    AddAce, EqualSid, GetAce, InitializeAcl, ACCESS_ALLOWED_ACE, ACE_FLAGS, ACE_HEADER, ACL,
    CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION, INHERITED_ACE, NO_INHERITANCE,
    OBJECT_INHERIT_ACE, PSECURITY_DESCRIPTOR, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
};

use super::token::to_wide;

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;

/// Allow mask for writable roots (Codex `WRITE_ALLOW_MASK`).
pub(crate) const WRITE_ALLOW_MASK: u32 =
    FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE;

/// Deny mask for protected paths (Codex `DenyAceKind::Write`).
pub(crate) const WRITE_DENY_MASK: u32 = FILE_GENERIC_WRITE
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_WRITE_EA
    | FILE_WRITE_ATTRIBUTES
    | DELETE
    | FILE_DELETE_CHILD;

/// A DACL fetched with `GetNamedSecurityInfoW`; owns its security descriptor.
struct Dacl {
    acl: *mut ACL,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Dacl {
    fn read(path: &Path) -> io::Result<Self> {
        let wide = to_wide(&path.to_string_lossy());
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let code = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(acl_error(path, "read ACL", code));
        }
        Ok(Self { acl, descriptor })
    }

    /// Whether an ACE of `ace_type` for `sid` covers every bit of `mask`.
    fn has_ace(&self, ace_type: u8, sid: PSID, mask: u32) -> bool {
        if self.acl.is_null() {
            return false;
        }
        let count = unsafe { (*self.acl).AceCount } as u32;
        (0..count).any(|index| {
            let mut ace: *mut c_void = std::ptr::null_mut();
            if unsafe { GetAce(self.acl, index, &mut ace) } == 0 {
                return false;
            }
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            if header.AceType != ace_type {
                return false;
            }
            // ACCESS_ALLOWED_ACE and ACCESS_DENIED_ACE share one layout.
            let ace = ace as *const ACCESS_ALLOWED_ACE;
            let ace_mask = unsafe { (*ace).Mask };
            let ace_sid = unsafe { std::ptr::addr_of!((*ace).SidStart) } as PSID;
            ace_mask & mask == mask && unsafe { EqualSid(ace_sid, sid) } != 0
        })
    }
}

impl Drop for Dacl {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe { LocalFree(self.descriptor as HLOCAL) };
        }
    }
}

fn acl_error(path: &Path, action: &str, code: u32) -> io::Error {
    let err = io::Error::from_raw_os_error(code as i32);
    io::Error::new(err.kind(), format!("{action} on {}: {err}", path.display()))
}

fn inheritance_for(path: &Path) -> ACE_FLAGS {
    if path.is_dir() {
        CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE
    } else {
        NO_INHERITANCE
    }
}

/// Merges one explicit entry for `sid` into the DACL of `path`.
fn apply_entry(path: &Path, dacl: &Dacl, sid: PSID, mask: u32, mode: ACCESS_MODE) -> io::Result<()> {
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: mask,
        grfAccessMode: mode,
        grfInheritance: inheritance_for(path),
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid as *mut u16,
        },
    };
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    let code = unsafe { SetEntriesInAclW(1, &entry, dacl.acl, &mut new_acl) };
    if code != 0 {
        return Err(acl_error(path, "build ACL", code));
    }
    // SetNamedSecurityInfoW propagates the inheritable ACE to existing
    // children of a directory.
    let result = write_dacl(path, new_acl);
    unsafe { LocalFree(new_acl as HLOCAL) };
    result
}

/// Grants `sid` read/write/execute/delete on `path` (inheritable for
/// directories). Returns whether the DACL changed.
pub(crate) fn grant_write(path: &Path, sid: PSID) -> io::Result<bool> {
    let dacl = Dacl::read(path)?;
    if dacl.has_ace(ACCESS_ALLOWED_ACE_TYPE, sid, WRITE_ALLOW_MASK) {
        return Ok(false);
    }
    apply_entry(path, &dacl, sid, WRITE_ALLOW_MASK, GRANT_ACCESS)?;
    Ok(true)
}

/// Denies `sid` every write/delete right on `path`. Deny ACEs are ordered
/// before allow ACEs, so they win over the inherited workspace grant.
pub(crate) fn deny_write(path: &Path, sid: PSID) -> io::Result<bool> {
    let dacl = Dacl::read(path)?;
    if dacl.has_ace(ACCESS_DENIED_ACE_TYPE, sid, WRITE_DENY_MASK) {
        return Ok(false);
    }
    apply_entry(path, &dacl, sid, WRITE_DENY_MASK, DENY_ACCESS)?;
    Ok(true)
}

/// Removes every explicit ACE (allow and deny) naming `sid` from `path`.
///
/// `REVOKE_ACCESS` only drops allow ACEs, so the DACL is rebuilt without
/// the SID's explicit entries instead; inherited ACEs are recomputed by
/// `SetNamedSecurityInfoW` (and vanish from children once the parent's
/// inheritable ACE is gone).
pub(crate) fn revoke(path: &Path, sid: PSID) -> io::Result<bool> {
    let dacl = Dacl::read(path)?;
    if dacl.acl.is_null() {
        return Ok(false);
    }
    let (revision, size, count) = unsafe {
        let acl = &*dacl.acl;
        (u32::from(acl.AclRevision), usize::from(acl.AclSize), u32::from(acl.AceCount))
    };
    // u32 backing keeps the new ACL DWORD-aligned.
    let mut buffer = vec![0u32; size.div_ceil(4)];
    let new_acl = buffer.as_mut_ptr() as *mut ACL;
    if unsafe { InitializeAcl(new_acl, (buffer.len() * 4) as u32, revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut removed = false;
    for index in 0..count {
        let mut ace: *mut c_void = std::ptr::null_mut();
        if unsafe { GetAce(dacl.acl, index, &mut ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        let explicit = u32::from(header.AceFlags) & INHERITED_ACE == 0;
        let allow_or_deny = matches!(header.AceType, ACCESS_ALLOWED_ACE_TYPE | ACCESS_DENIED_ACE_TYPE);
        let ace_sid = unsafe { std::ptr::addr_of!((*(ace as *const ACCESS_ALLOWED_ACE)).SidStart) } as PSID;
        if explicit && allow_or_deny && unsafe { EqualSid(ace_sid, sid) } != 0 {
            removed = true;
            continue;
        }
        if unsafe { AddAce(new_acl, revision, u32::MAX, ace, u32::from(header.AceSize)) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if !removed {
        return Ok(false);
    }
    write_dacl(path, new_acl)?;
    Ok(true)
}

fn write_dacl(path: &Path, acl: *const ACL) -> io::Result<()> {
    let wide = to_wide(&path.to_string_lossy());
    let code = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    if code != 0 {
        return Err(acl_error(path, "write ACL", code));
    }
    Ok(())
}

/// Whether `path` has an allow ACE for `sid` covering `mask` (tests).
#[cfg(test)]
pub(crate) fn has_allow(path: &Path, sid: PSID, mask: u32) -> io::Result<bool> {
    Ok(Dacl::read(path)?.has_ace(ACCESS_ALLOWED_ACE_TYPE, sid, mask))
}

/// Whether `path` has a deny ACE for `sid` covering `mask` (tests).
#[cfg(test)]
pub(crate) fn has_deny(path: &Path, sid: PSID, mask: u32) -> io::Result<bool> {
    Ok(Dacl::read(path)?.has_ace(ACCESS_DENIED_ACE_TYPE, sid, mask))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows_sandbox::plan::capability_sid_for;
    use crate::windows_sandbox::token::LocalSid;

    #[test]
    fn grant_deny_and_revoke_only_touch_the_capability_sid() {
        let dir = std::env::temp_dir().join(format!("locagens-acl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("existing.txt");
        std::fs::write(&file, "x").unwrap();
        let sid = LocalSid::parse(&capability_sid_for(&format!("acl-test-{}", uuid::Uuid::new_v4()))).unwrap();
        let ace_count = |path: &Path| unsafe { (*Dacl::read(path).unwrap().acl).AceCount };
        let file_aces_before = ace_count(&file);

        assert!(!has_allow(&dir, sid.as_ptr(), WRITE_ALLOW_MASK).unwrap());
        assert!(grant_write(&dir, sid.as_ptr()).unwrap());
        assert!(!grant_write(&dir, sid.as_ptr()).unwrap(), "second grant is a no-op");
        assert!(has_allow(&dir, sid.as_ptr(), WRITE_ALLOW_MASK).unwrap());
        // The inheritable ACE reached the existing child.
        assert!(has_allow(&file, sid.as_ptr(), WRITE_ALLOW_MASK).unwrap());
        assert_eq!(ace_count(&file), file_aces_before + 1);

        assert!(deny_write(&file, sid.as_ptr()).unwrap());
        assert!(!deny_write(&file, sid.as_ptr()).unwrap());
        assert!(has_deny(&file, sid.as_ptr(), WRITE_DENY_MASK).unwrap());

        assert!(revoke(&file, sid.as_ptr()).unwrap());
        assert!(!has_deny(&file, sid.as_ptr(), 0).unwrap());
        assert!(revoke(&dir, sid.as_ptr()).unwrap());
        assert!(!has_allow(&dir, sid.as_ptr(), 0).unwrap());
        assert!(!has_allow(&file, sid.as_ptr(), 0).unwrap(), "inherited copy is gone too");
        assert_eq!(ace_count(&file), file_aces_before);
        assert!(!revoke(&dir, sid.as_ptr()).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
