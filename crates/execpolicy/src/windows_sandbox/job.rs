//! Job Object wrapper: every sandboxed process tree lives in one job with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so killing the job (or dropping the
//! last handle, e.g. when Locagens exits) kills the whole tree.

use std::ffi::c_void;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct JobObject(OwnedHandle);

// SAFETY: a job handle is a process-wide kernel handle usable from any thread.
unsafe impl Send for JobObject {}
unsafe impl Sync for JobObject {}

impl JobObject {
    pub(crate) fn create() -> io::Result<Self> {
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(handle as _) });
        job.set_limit_flags(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
        Ok(job)
    }

    pub(crate) fn raw(&self) -> HANDLE {
        self.0.as_raw_handle() as HANDLE
    }

    fn set_limit_flags(&self, flags: JOB_OBJECT_LIMIT) -> io::Result<()> {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = flags;
        let ok = unsafe {
            SetInformationJobObject(
                self.raw(),
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Kills every process in the job.
    pub fn terminate(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.raw(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Lets background descendants outlive the job handle after the root
    /// command exited on its own (Codex `preserve_descendants`).
    pub fn preserve_descendants(&self) -> io::Result<()> {
        self.set_limit_flags(0)
    }
}
