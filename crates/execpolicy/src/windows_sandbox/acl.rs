//! File-system ACL edits for the capability SID (port of the parts of Codex
//! `windows-sandbox-rs/src/acl.rs` the legacy, non-elevated backend uses).
//!
//! Only ACEs naming the sandbox capability SID are ever added or removed;
//! existing ACEs are preserved (`SetEntriesInAclW` merges into the current
//! DACL). Each edit is skipped when an equivalent ACE already exists, so
//! repeated runs do not rewrite (and re-propagate) the DACL.
//!
//! Mandatory integrity labels (the SACL's label ACE, written with
//! `LABEL_SECURITY_INFORMATION`, no privilege needed) are edited the same
//! way: set only when different, removed only when they are the label the
//! sandbox wrote.

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
    AddAce, AddMandatoryAce, EqualSid, GetAce, GetLengthSid, InitializeAcl, ACCESS_ALLOWED_ACE,
    ACE_FLAGS, ACE_HEADER, ACL, ACL_REVISION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
    INHERITED_ACE, LABEL_SECURITY_INFORMATION, NO_INHERITANCE, OBJECT_INHERIT_ACE,
    OBJECT_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SYSTEM_MANDATORY_LABEL_ACE,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
};

use super::token::to_wide;

const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
const ACCESS_DENIED_ACE_TYPE: u8 = 1;
const SYSTEM_MANDATORY_LABEL_ACE_TYPE: u8 = 0x11;

/// Mandatory label policies (`SYSTEM_MANDATORY_LABEL_NO_WRITE_UP` /
/// `_NO_READ_UP`): what a process below the label's level may not do.
pub(crate) const NO_WRITE_UP: u32 = 0x1;
pub(crate) const NO_READ_UP: u32 = 0x2;

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

/// A DACL, or the mandatory-label part of a SACL, fetched with
/// `GetNamedSecurityInfoW`; owns its security descriptor.
struct SecurityAcl {
    acl: *mut ACL,
    descriptor: PSECURITY_DESCRIPTOR,
}

impl SecurityAcl {
    /// `info` is `DACL_SECURITY_INFORMATION` or `LABEL_SECURITY_INFORMATION`
    /// (the latter needs only `READ_CONTROL`, unlike a full SACL read).
    fn read(path: &Path, info: OBJECT_SECURITY_INFORMATION) -> io::Result<Self> {
        let wide = to_wide(&path.to_string_lossy());
        let mut acl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let (dacl_out, sacl_out): (*mut *mut ACL, *mut *mut ACL) = if info == LABEL_SECURITY_INFORMATION {
            (std::ptr::null_mut(), &mut acl)
        } else {
            (&mut acl, std::ptr::null_mut())
        };
        let code = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                info,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl_out,
                sacl_out,
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(acl_error(path, "read ACL", code));
        }
        Ok(Self { acl, descriptor })
    }

    /// The explicit (not inherited) mandatory label ACE: `(level SID, policy, flags)`.
    fn explicit_label(&self) -> Option<(PSID, u32, u8)> {
        if self.acl.is_null() {
            return None;
        }
        let count = unsafe { (*self.acl).AceCount } as u32;
        (0..count).find_map(|index| {
            let mut ace: *mut c_void = std::ptr::null_mut();
            if unsafe { GetAce(self.acl, index, &mut ace) } == 0 {
                return None;
            }
            let header = unsafe { &*(ace as *const ACE_HEADER) };
            if header.AceType != SYSTEM_MANDATORY_LABEL_ACE_TYPE || u32::from(header.AceFlags) & INHERITED_ACE != 0 {
                return None;
            }
            let ace = ace as *const SYSTEM_MANDATORY_LABEL_ACE;
            let sid = unsafe { std::ptr::addr_of!((*ace).SidStart) } as PSID;
            Some((sid, unsafe { (*ace).Mask }, header.AceFlags))
        })
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

impl Drop for SecurityAcl {
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
fn apply_entry(path: &Path, dacl: &SecurityAcl, sid: PSID, mask: u32, mode: ACCESS_MODE) -> io::Result<()> {
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
    let dacl = SecurityAcl::read(path, DACL_SECURITY_INFORMATION)?;
    if dacl.has_ace(ACCESS_ALLOWED_ACE_TYPE, sid, WRITE_ALLOW_MASK) {
        return Ok(false);
    }
    apply_entry(path, &dacl, sid, WRITE_ALLOW_MASK, GRANT_ACCESS)?;
    Ok(true)
}

/// Denies `sid` every write/delete right on `path`. Deny ACEs are ordered
/// before allow ACEs, so they win over the inherited workspace grant.
pub(crate) fn deny_write(path: &Path, sid: PSID) -> io::Result<bool> {
    let dacl = SecurityAcl::read(path, DACL_SECURITY_INFORMATION)?;
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
    let dacl = SecurityAcl::read(path, DACL_SECURITY_INFORMATION)?;
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

/// Sets the explicit mandatory label of `path` to `level` with `policy`
/// (`NO_WRITE_UP` / `NO_READ_UP`), inheritable for directories. Writing a
/// label at or below the caller's own integrity needs only `WRITE_OWNER`
/// (no privilege). Like DACL edits it propagates to existing children's
/// inherited labels. Returns whether the label changed.
pub(crate) fn set_label(path: &Path, level: PSID, policy: u32) -> io::Result<bool> {
    let flags = inheritance_for(path) as u8;
    let current = SecurityAcl::read(path, LABEL_SECURITY_INFORMATION)?;
    if let Some((sid, mask, ace_flags)) = current.explicit_label() {
        let inherit_bits = (CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE) as u8;
        if mask == policy && ace_flags & inherit_bits == flags && unsafe { EqualSid(sid, level) } != 0 {
            return Ok(false);
        }
    }
    let size = std::mem::size_of::<ACL>()
        + std::mem::size_of::<SYSTEM_MANDATORY_LABEL_ACE>()
        + unsafe { GetLengthSid(level) } as usize;
    let mut buffer = vec![0u32; size.div_ceil(4)];
    let sacl = buffer.as_mut_ptr() as *mut ACL;
    if unsafe { InitializeAcl(sacl, (buffer.len() * 4) as u32, ACL_REVISION) } == 0
        || unsafe { AddMandatoryAce(sacl, ACL_REVISION, ACE_FLAGS::from(flags), policy, level) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    write_label(path, sacl)?;
    Ok(true)
}

/// Removes the explicit label of `path` when it is exactly `level` with
/// `policy` (one [`set_label`] wrote); any other label is left alone.
/// Children then inherit from the parent again.
pub(crate) fn remove_label(path: &Path, level: PSID, policy: u32) -> io::Result<bool> {
    let current = SecurityAcl::read(path, LABEL_SECURITY_INFORMATION)?;
    let ours = current
        .explicit_label()
        .is_some_and(|(sid, mask, _)| mask == policy && unsafe { EqualSid(sid, level) } != 0);
    if !ours {
        return Ok(false);
    }
    let mut buffer = vec![0u32; std::mem::size_of::<ACL>().div_ceil(4)];
    let empty = buffer.as_mut_ptr() as *mut ACL;
    if unsafe { InitializeAcl(empty, (buffer.len() * 4) as u32, ACL_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    write_label(path, empty)?;
    Ok(true)
}

fn write_label(path: &Path, sacl: *const ACL) -> io::Result<()> {
    let wide = to_wide(&path.to_string_lossy());
    let code = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            sacl,
        )
    };
    if code != 0 {
        return Err(acl_error(path, "write integrity label", code));
    }
    Ok(())
}

/// The explicit label of `path` as `(level SID string, policy)` (tests).
#[cfg(test)]
pub(crate) fn explicit_label_of(path: &Path) -> io::Result<Option<(String, u32)>> {
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    let current = SecurityAcl::read(path, LABEL_SECURITY_INFORMATION)?;
    Ok(current.explicit_label().map(|(sid, mask, _)| {
        let mut text: *mut u16 = std::ptr::null_mut();
        unsafe { ConvertSidToStringSidW(sid, &mut text) };
        let len = (0..).take_while(|i| unsafe { *text.add(*i) } != 0).count();
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        unsafe { LocalFree(text as HLOCAL) };
        (value, mask)
    }))
}

/// Whether `path` has an allow ACE for `sid` covering `mask` (tests).
#[cfg(test)]
pub(crate) fn has_allow(path: &Path, sid: PSID, mask: u32) -> io::Result<bool> {
    Ok(SecurityAcl::read(path, DACL_SECURITY_INFORMATION)?.has_ace(ACCESS_ALLOWED_ACE_TYPE, sid, mask))
}

/// Whether `path` has a deny ACE for `sid` covering `mask` (tests).
#[cfg(test)]
pub(crate) fn has_deny(path: &Path, sid: PSID, mask: u32) -> io::Result<bool> {
    Ok(SecurityAcl::read(path, DACL_SECURITY_INFORMATION)?.has_ace(ACCESS_DENIED_ACE_TYPE, sid, mask))
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
        let ace_count = |path: &Path| unsafe { (*SecurityAcl::read(path, DACL_SECURITY_INFORMATION).unwrap().acl).AceCount };
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

    #[test]
    fn labels_propagate_are_idempotent_and_only_our_label_is_removed() {
        use crate::windows_sandbox::token::{LOW_INTEGRITY_SID, MEDIUM_INTEGRITY_SID};
        let dir = std::env::temp_dir().join(format!("locagens-label-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let file = dir.join("sub").join("existing.txt");
        std::fs::write(&file, "x").unwrap();
        let low = LocalSid::parse(LOW_INTEGRITY_SID).unwrap();
        let medium = LocalSid::parse(MEDIUM_INTEGRITY_SID).unwrap();
        let inherited = |path: &Path| {
            let current = SecurityAcl::read(path, LABEL_SECURITY_INFORMATION).unwrap();
            if current.acl.is_null() { 0 } else { unsafe { (*current.acl).AceCount } }
        };

        assert_eq!(explicit_label_of(&dir).unwrap(), None);
        assert!(set_label(&dir, low.as_ptr(), NO_WRITE_UP).unwrap());
        assert!(!set_label(&dir, low.as_ptr(), NO_WRITE_UP).unwrap(), "second label is a no-op");
        assert_eq!(explicit_label_of(&dir).unwrap(), Some((LOW_INTEGRITY_SID.to_string(), NO_WRITE_UP)));
        assert_eq!(inherited(&file), 1, "the existing child inherited the label");
        assert_eq!(explicit_label_of(&file).unwrap(), None);

        // A different label on the child overrides the inherited one.
        assert!(set_label(&file, medium.as_ptr(), NO_WRITE_UP | NO_READ_UP).unwrap());
        assert_eq!(
            explicit_label_of(&file).unwrap(),
            Some((MEDIUM_INTEGRITY_SID.to_string(), NO_WRITE_UP | NO_READ_UP))
        );
        assert!(!remove_label(&file, medium.as_ptr(), NO_WRITE_UP).unwrap(), "another policy is not ours");
        assert!(remove_label(&file, medium.as_ptr(), NO_WRITE_UP | NO_READ_UP).unwrap());
        assert_eq!(explicit_label_of(&file).unwrap(), None);

        assert!(!remove_label(&dir, medium.as_ptr(), NO_WRITE_UP).unwrap(), "another level is not ours");
        assert!(remove_label(&dir, low.as_ptr(), NO_WRITE_UP).unwrap());
        assert_eq!(explicit_label_of(&dir).unwrap(), None);
        assert_eq!(inherited(&file), 0, "the inherited copy is gone too");
        assert!(!remove_label(&dir, low.as_ptr(), NO_WRITE_UP).unwrap());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
