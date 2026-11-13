//! Restricted primary tokens (port of Codex `windows-sandbox-rs/src/token.rs`).
//!
//! The sandbox token is the caller's own token passed through
//! `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED)`
//! with the restricting SIDs `[capability, logon session, Everyone]`.
//! `WRITE_RESTRICTED` means reads are checked as usual, but every *write*
//! must additionally be granted to one of those restricting SIDs — so the
//! command can only write where the sandbox granted its capability SID
//! (plus the rare objects writable by Everyone / the logon session).

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{LocalFree, GENERIC_ALL, HANDLE, HLOCAL, LUID};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, SetEntriesInAclW, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE,
    TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, CreateRestrictedToken, CreateWellKnownSid, GetTokenInformation,
    LookupPrivilegeValueW, SetTokenInformation, TokenDefaultDacl, TokenGroups, WinWorldSid, ACL,
    DISABLE_MAX_PRIVILEGE, LUA_TOKEN, NO_INHERITANCE, PSID, SE_PRIVILEGE_ENABLED,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_PRIVILEGES,
    TOKEN_QUERY, WRITE_RESTRICTED,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// `SE_GROUP_LOGON_ID` — marks the logon-session SID in `TokenGroups`.
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

pub(crate) fn to_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error(context: &str) -> io::Error {
    let err = io::Error::last_os_error();
    io::Error::new(err.kind(), format!("{context}: {err}"))
}

/// A SID allocated by `ConvertStringSidToSidW` (freed with `LocalFree`).
pub(crate) struct LocalSid(PSID);

impl LocalSid {
    pub(crate) fn parse(sid: &str) -> io::Result<Self> {
        let mut psid: PSID = std::ptr::null_mut();
        let wide = to_wide(sid);
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut psid) } == 0 {
            return Err(last_error(&format!("ConvertStringSidToSidW({sid})")));
        }
        Ok(Self(psid))
    }

    pub(crate) fn as_ptr(&self) -> PSID {
        self.0
    }
}

impl Drop for LocalSid {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0 as HLOCAL) };
    }
}

/// A SID copied into an owned byte buffer.
struct SidBytes(Vec<u8>);

impl SidBytes {
    fn as_ptr(&mut self) -> PSID {
        self.0.as_mut_ptr() as PSID
    }
}

fn current_process_token() -> io::Result<OwnedHandle> {
    let access = TOKEN_DUPLICATE
        | TOKEN_QUERY
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_SESSIONID
        | TOKEN_ADJUST_PRIVILEGES;
    let mut handle: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut handle) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as _) })
}

/// The logon-session SID of `token` (from `TokenGroups`).
fn logon_sid(token: &OwnedHandle) -> io::Result<SidBytes> {
    let raw = token.as_raw_handle() as HANDLE;
    let mut needed = 0u32;
    unsafe { GetTokenInformation(raw, TokenGroups, std::ptr::null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(last_error("GetTokenInformation(TokenGroups) size"));
    }
    // u64 backing keeps the TOKEN_GROUPS header properly aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    if unsafe {
        GetTokenInformation(raw, TokenGroups, buffer.as_mut_ptr() as *mut c_void, needed, &mut needed)
    } == 0
    {
        return Err(last_error("GetTokenInformation(TokenGroups)"));
    }
    let groups = buffer.as_ptr() as *const TOKEN_GROUPS;
    let count = unsafe { (*groups).GroupCount } as usize;
    let first = unsafe { std::ptr::addr_of!((*groups).Groups) as *const SID_AND_ATTRIBUTES };
    for index in 0..count {
        let group = unsafe { &*first.add(index) };
        if group.Attributes & SE_GROUP_LOGON_ID == SE_GROUP_LOGON_ID {
            return Ok(copy_sid(group.Sid));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "the current token has no logon-session SID",
    ))
}

fn copy_sid(sid: PSID) -> SidBytes {
    let length = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) } as usize;
    let mut bytes = vec![0u8; length];
    unsafe { std::ptr::copy_nonoverlapping(sid as *const u8, bytes.as_mut_ptr(), length) };
    SidBytes(bytes)
}

fn everyone_sid() -> io::Result<SidBytes> {
    let mut size = 68u32; // SECURITY_MAX_SID_SIZE
    let mut bytes = vec![0u8; size as usize];
    if unsafe {
        CreateWellKnownSid(WinWorldSid, std::ptr::null_mut(), bytes.as_mut_ptr() as PSID, &mut size)
    } == 0
    {
        return Err(last_error("CreateWellKnownSid(Everyone)"));
    }
    bytes.truncate(size as usize);
    Ok(SidBytes(bytes))
}

/// Creates the restricted primary token for a sandboxed command.
pub(crate) fn create_restricted_token(capability_sid: &LocalSid) -> io::Result<OwnedHandle> {
    let base = current_process_token()?;
    let mut logon = logon_sid(&base)?;
    let mut everyone = everyone_sid()?;
    let restricting = [
        SID_AND_ATTRIBUTES { Sid: capability_sid.as_ptr(), Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: logon.as_ptr(), Attributes: 0 },
        SID_AND_ATTRIBUTES { Sid: everyone.as_ptr(), Attributes: 0 },
    ];
    let mut restricted: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        CreateRestrictedToken(
            base.as_raw_handle() as HANDLE,
            DISABLE_MAX_PRIVILEGE | LUA_TOKEN | WRITE_RESTRICTED,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            restricting.len() as u32,
            restricting.as_ptr(),
            &mut restricted,
        )
    };
    if ok == 0 {
        return Err(last_error("CreateRestrictedToken"));
    }
    let token = unsafe { OwnedHandle::from_raw_handle(restricted as _) };
    set_default_dacl(&token, logon.as_ptr())?;
    enable_privilege(&token, "SeChangeNotifyPrivilege")?;
    Ok(token)
}

/// Objects the sandboxed process creates (its own process/thread handles,
/// pipes, events) get a default DACL granting the logon session full
/// access, so both the child and the parent can use them.
fn set_default_dacl(token: &OwnedHandle, logon_sid: PSID) -> io::Result<()> {
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: logon_sid as *mut u16,
        },
    };
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let code = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut dacl) };
    if code != 0 {
        return Err(io::Error::from_raw_os_error(code as i32));
    }
    let info = TOKEN_DEFAULT_DACL { DefaultDacl: dacl };
    let ok = unsafe {
        SetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenDefaultDacl,
            &info as *const _ as *const c_void,
            std::mem::size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
    };
    let result = if ok == 0 { Err(last_error("SetTokenInformation(TokenDefaultDacl)")) } else { Ok(()) };
    unsafe { LocalFree(dacl as HLOCAL) };
    result
}

fn enable_privilege(token: &OwnedHandle, name: &str) -> io::Result<()> {
    let mut luid = LUID { LowPart: 0, HighPart: 0 };
    let wide = to_wide(name);
    if unsafe { LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut luid) } == 0 {
        return Err(last_error("LookupPrivilegeValueW"));
    }
    let mut privileges: TOKEN_PRIVILEGES = unsafe { std::mem::zeroed() };
    privileges.PrivilegeCount = 1;
    privileges.Privileges[0].Luid = luid;
    privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
    let ok = unsafe {
        AdjustTokenPrivileges(
            token.as_raw_handle() as HANDLE,
            0,
            &privileges,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(last_error("AdjustTokenPrivileges"));
    }
    Ok(())
}
