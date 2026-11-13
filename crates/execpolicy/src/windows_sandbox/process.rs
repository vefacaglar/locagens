//! Spawning a command under the restricted token (port of Codex
//! `process.rs` + `spawn_prep.rs`, legacy path): apply the plan's ACLs,
//! build the token, then `CreateProcessAsUserW` with piped stdio, an
//! explicit inheritable-handle list and the job attached at creation.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

use super::acl;
use super::job::JobObject;
use super::plan::{build_plan, strip_verbatim_prefix, TokenMode, WindowsSandboxPlan};
use super::token::{create_restricted_token, to_wide, LocalSid};
use crate::sandbox::SandboxSpec;

/// What to run inside the sandbox.
pub struct SpawnRequest<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// The complete child environment (nothing is inherited implicitly).
    pub env: &'a HashMap<String, String>,
    /// Keep the stdin pipe open for interactive sessions.
    pub stdin_pipe: bool,
}

/// A running sandboxed process.
pub struct SandboxedChild {
    pub pid: u32,
    process: OwnedHandle,
    pub job: Arc<JobObject>,
    pub stdin: Option<File>,
    pub stdout: File,
    pub stderr: File,
}

impl SandboxedChild {
    /// A handle clone that can wait for the exit on another thread.
    pub fn waiter(&self) -> io::Result<ProcessWaiter> {
        Ok(ProcessWaiter(self.process.try_clone()?))
    }
}

/// Blocking exit waiter for a sandboxed process.
pub struct ProcessWaiter(OwnedHandle);

impl ProcessWaiter {
    /// Blocks until the process exits and returns its exit code.
    pub fn wait(&self) -> io::Result<i32> {
        let handle = self.0.as_raw_handle() as HANDLE;
        if unsafe { WaitForSingleObject(handle, INFINITE) } != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(code as i32)
    }
}

/// Serializes ACL preparation so parallel spawns do not race on one DACL.
static PREPARE_LOCK: Mutex<()> = Mutex::new(());

/// Applies the sandbox to `spec` and starts `request` inside it. Any error
/// means the command did not start (fail closed); the message explains why.
pub fn spawn_sandboxed(spec: &SandboxSpec, request: &SpawnRequest) -> Result<SandboxedChild, String> {
    let plan = build_plan(spec.mode, &spec.project_path, &std::env::temp_dir())?;
    let capability = LocalSid::parse(&plan.capability_sid)
        .map_err(|err| format!("Windows sandbox: invalid capability SID: {err}"))?;
    {
        let _guard = PREPARE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        apply_plan(&plan, &capability)?;
    }
    let token = create_restricted_token(&capability)
        .map_err(|err| format!("Windows sandbox: cannot create the restricted token: {err}"))?;
    let mut env = request.env.clone();
    if let Some(temp_dir) = &plan.temp_dir {
        for key in ["TEMP", "TMP"] {
            env.insert(key.to_string(), temp_dir.to_string_lossy().to_string());
        }
    }
    create_process(&token, request, &env)
        .map_err(|err| format!("Windows sandbox: cannot start the sandboxed process: {err}"))
}

/// Creates the plan's directories and applies its ACEs.
fn apply_plan(plan: &WindowsSandboxPlan, capability: &LocalSid) -> Result<(), String> {
    if plan.token_mode == TokenMode::ReadOnly {
        return Ok(());
    }
    for dir in &plan.create_dirs {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("Windows sandbox: cannot create {}: {err}", dir.display()))?;
    }
    for root in &plan.writable_roots {
        acl::grant_write(root, capability.as_ptr()).map_err(|err| {
            format!("Windows sandbox: cannot grant workspace write access ({err}); refusing to run the command unsandboxed")
        })?;
    }
    for path in &plan.deny_write_paths {
        acl::deny_write(path, capability.as_ptr()).map_err(|err| {
            format!("Windows sandbox: cannot protect {} ({err}); refusing to run the command", path.display())
        })?;
    }
    Ok(())
}

/// Removes every ACE the sandbox added for `project_path` (workspace root,
/// protected paths and the private temp dir). Returns the paths changed.
pub fn revoke_workspace_access(project_path: &Path) -> Result<Vec<PathBuf>, String> {
    let plan = build_plan(locagens_protocol::SandboxMode::WorkspaceWrite, project_path, &std::env::temp_dir())?;
    let capability = LocalSid::parse(&plan.capability_sid).map_err(|err| err.to_string())?;
    let _guard = PREPARE_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut changed = Vec::new();
    for path in plan.deny_write_paths.iter().chain(plan.writable_roots.iter()) {
        if !path.exists() {
            continue;
        }
        if acl::revoke(path, capability.as_ptr()).map_err(|err| err.to_string())? {
            changed.push(path.clone());
        }
    }
    Ok(changed)
}

/// Resolves bare `cmd` to `%SystemRoot%\System32\cmd.exe` so the lookup
/// never depends on Locagens' own working directory.
fn resolve_program(program: &str) -> String {
    let lower = program.to_ascii_lowercase();
    if lower == "cmd" || lower == "cmd.exe" {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        return format!(r"{root}\System32\cmd.exe");
    }
    program.to_string()
}

/// Builds the native command line. `cmd /C <script>` keeps the script
/// verbatim (`/s` strips exactly the outer quotes, Codex
/// `cmd_script_command_line`); other programs get MSVC quoting.
pub(crate) fn command_line(program: &str, args: &[String]) -> String {
    if let Some(script) = crate::sandbox::cmd_script(program, args) {
        return format!(r#""{program}" /s /c "{script}""#);
    }
    std::iter::once(program.to_string())
        .chain(args.iter().cloned())
        .map(|arg| quote_arg(&arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '"']) {
        return arg.to_string();
    }
    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(ch);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

/// `KEY=VALUE\0…\0\0`, sorted case-insensitively as Windows expects.
pub(crate) fn environment_block(env: &HashMap<String, String>) -> Vec<u16> {
    let mut entries: Vec<(&String, &String)> = env.iter().collect();
    entries.sort_by_key(|(key, _)| key.to_uppercase());
    entries.dedup_by(|a, b| a.0.eq_ignore_ascii_case(b.0));
    let mut block: Vec<u16> = Vec::new();
    for (key, value) in entries {
        block.extend(format!("{key}={value}").encode_utf16());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// An anonymous pipe; the child end is made inheritable.
struct Pipe {
    read: HANDLE,
    write: HANDLE,
}

fn create_pipe(child_end_is_read: bool) -> io::Result<Pipe> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let child_end = if child_end_is_read { read } else { write };
    if unsafe { SetHandleInformation(child_end, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) } == 0 {
        let err = io::Error::last_os_error();
        unsafe {
            CloseHandle(read);
            CloseHandle(write);
        }
        return Err(err);
    }
    Ok(Pipe { read, write })
}

/// A process/thread attribute list holding the handle list and job list.
struct AttributeList {
    buffer: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut size) };
        let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        if unsafe { InitializeProcThreadAttributeList(buffer.as_mut_ptr() as _, count, 0, &mut size) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST
    }

    /// `values` must outlive the `CreateProcessAsUserW` call.
    fn set(&mut self, attribute: u32, values: &[HANDLE]) -> io::Result<()> {
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                values.as_ptr() as *const c_void,
                std::mem::size_of_val(values),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

fn create_process(
    token: &OwnedHandle,
    request: &SpawnRequest,
    env: &HashMap<String, String>,
) -> io::Result<SandboxedChild> {
    let job = Arc::new(JobObject::create()?);
    let stdin = create_pipe(true)?;
    let stdout = create_pipe(false)?;
    let stderr = create_pipe(false)?;
    let child_ends = [stdin.read, stdout.write, stderr.write];
    let parent_ends = [stdin.write, stdout.read, stderr.read];
    let close_all = |handles: &[HANDLE]| {
        for handle in handles {
            unsafe { CloseHandle(*handle) };
        }
    };

    let result = launch(token, request, env, &job, child_ends);
    close_all(&child_ends);
    let process_info = match result {
        Ok(info) => info,
        Err(err) => {
            close_all(&parent_ends);
            return Err(err);
        }
    };
    unsafe { CloseHandle(process_info.hThread) };
    let stdin_file = unsafe { File::from_raw_handle(stdin.write as _) };
    Ok(SandboxedChild {
        pid: process_info.dwProcessId,
        process: unsafe { OwnedHandle::from_raw_handle(process_info.hProcess as _) },
        job,
        stdin: request.stdin_pipe.then_some(stdin_file),
        stdout: unsafe { File::from_raw_handle(stdout.read as _) },
        stderr: unsafe { File::from_raw_handle(stderr.read as _) },
    })
}

/// The `CreateProcessAsUserW` call itself.
fn launch(
    token: &OwnedHandle,
    request: &SpawnRequest,
    env: &HashMap<String, String>,
    job: &JobObject,
    child_ends: [HANDLE; 3],
) -> io::Result<PROCESS_INFORMATION> {
    let program = resolve_program(request.program);
    let mut command_line = to_wide(&command_line(&program, request.args));
    let env_block = environment_block(env);
    let cwd = to_wide(&strip_verbatim_prefix(request.cwd).to_string_lossy());
    let mut desktop = to_wide("Winsta0\\Default");
    let job_list = [job.raw()];

    let mut attributes = AttributeList::new(2)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &child_ends)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &job_list)?;

    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.lpDesktop = desktop.as_mut_ptr();
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = child_ends[0];
    startup.StartupInfo.hStdOutput = child_ends[1];
    startup.StartupInfo.hStdError = child_ends[2];
    startup.lpAttributeList = attributes.as_ptr();

    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe {
        CreateProcessAsUserW(
            token.as_raw_handle() as HANDLE,
            std::ptr::null(),
            command_line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
            env_block.as_ptr() as *const c_void,
            cwd.as_ptr(),
            &startup.StartupInfo,
            &mut info,
        )
    };
    if ok == 0 {
        let err = io::Error::last_os_error();
        return Err(io::Error::new(err.kind(), format!("CreateProcessAsUserW: {err}")));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_scripts_keep_their_quotes() {
        assert_eq!(
            command_line(r"C:\Windows\System32\cmd.exe", &["/C".into(), r#"echo "a b" > out.txt"#.into()]),
            r#""C:\Windows\System32\cmd.exe" /s /c "echo "a b" > out.txt""#
        );
        assert_eq!(
            command_line("git", &["commit".into(), "-m".into(), "say \"hi\"".into()]),
            r#"git commit -m "say \"hi\"""#
        );
    }

    #[test]
    fn environment_block_is_sorted_and_double_terminated() {
        let env = HashMap::from([
            ("b".to_string(), "2".to_string()),
            ("A".to_string(), "1".to_string()),
        ]);
        let block = String::from_utf16(&environment_block(&env)).unwrap();
        assert_eq!(block, "A=1\0b=2\0\0");
        assert_eq!(environment_block(&HashMap::new()), vec![0, 0]);
    }
}
