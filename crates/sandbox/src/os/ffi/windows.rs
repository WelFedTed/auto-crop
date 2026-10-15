// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Windows glue: creating the worker process confined (AppContainer, restricted token, job
//! object), pipes and shared-memory sections, and the worker's own report of what it is confined
//! by. Ported from `spikes/sandbox/src/windows.rs` (ADR-0006), with the differences that matter
//! for a long-lived pool: the job object is created by the parent and the process is created
//! suspended and assigned before it runs (no window in which it is unconfined), the pixel section
//! is created per job and duplicated into the worker by handle value, and handles are passed
//! through an explicit `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` whitelist (never blanket inheritance).
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{FromRawHandle, RawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, FreeSid, GetTokenInformation, IsTokenRestricted,
    SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT,
    TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_QUERY, TokenCapabilities,
    TokenIsAppContainer, TokenRestrictedSids,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, QueryInformationJobObject,
    SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW,
    DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;
const APPC_NAME: &str = "autocrop.decode.worker";
const ERROR_ALREADY_EXISTS_HR: i32 = 0x8007_00B7_u32 as i32;
/// Exit code the worker's allocator uses when an allocation fails (the job's memory limit).
pub const EXIT_MEMORY_LIMIT: u32 = 0x4D45_4D01;

/// How strongly the worker process is confined at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// AppContainer with no capabilities (no network, no user files) plus the job object.
    AppContainer,
    /// Restricted token (restricting SIDs, no privileges) plus the job object: files are blocked,
    /// the network is not.
    RestrictedToken,
    /// The job object only: no child processes, memory cap, no UI access.
    JobOnly,
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn wide_str(s: &str) -> Vec<u16> {
    wide(OsStr::new(s))
}

/// A handle that is closed when dropped.
pub struct OwnedHandle(HANDLE);

// SAFETY: a kernel handle value may be used from any thread; the wrapper only closes it once.
unsafe impl Send for OwnedHandle {}
// SAFETY: as above; all the operations on it are thread-safe kernel calls.
unsafe impl Sync for OwnedHandle {}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: the handle is owned by this value and closed exactly once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// The worker process and the job object that holds it. Dropping it closes the job, which kills
/// the process (`KILL_ON_JOB_CLOSE`): the parent can never leave an orphan behind.
pub struct Child {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid
    }

    /// Ends the worker now (the whole job).
    pub fn kill(&self) {
        // SAFETY: both handles are valid for the life of `self`; failures (already gone) are
        // irrelevant.
        unsafe {
            TerminateJobObject(self.job.0, 1);
            TerminateProcess(self.process.0, 1);
        }
    }

    /// The exit code once the process has ended, waiting at most `ms` milliseconds.
    pub fn wait_timeout(&self, ms: u32) -> Option<u32> {
        let mut code = 0u32;
        // SAFETY: the process handle is valid; `code` is a valid out-pointer.
        unsafe {
            if WaitForSingleObject(self.process.0, ms) != WAIT_OBJECT_0 {
                return None;
            }
            if GetExitCodeProcess(self.process.0, &mut code) == 0 {
                return Some(1);
            }
        }
        Some(code)
    }

    pub fn try_wait(&self) -> Option<u32> {
        self.wait_timeout(0)
    }
}

/// A started worker with the parent's ends of its three pipes.
pub struct Spawned {
    pub child: Child,
    pub stdin: File,
    pub stdout: File,
    pub stderr: File,
}

pub struct SpawnArgs<'a> {
    pub exe: &'a Path,
    pub args: &'a [String],
    pub env: &'a [(OsString, OsString)],
    pub level: Level,
    /// Per-process commit limit of the job object, in bytes.
    pub memory_limit: u64,
}

/// Quotes one argument for a Windows command line.
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_owned();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in a.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                out.push(c);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

fn environment_block(env: &[(OsString, OsString)]) -> Vec<u16> {
    let mut sorted: Vec<&(OsString, OsString)> = env.iter().collect();
    sorted.sort_by_key(|(k, _)| k.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (k, v) in sorted {
        block.extend(k.encode_wide());
        block.push(u16::from(b'='));
        block.extend(v.encode_wide());
        block.push(0);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// A pipe whose read end is inheritable (`child_reads`) or whose write end is.
fn pipe(child_reads: bool) -> io::Result<(HANDLE, HANDLE)> {
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // SAFETY: out-pointers are valid locals; `sa` outlives the call.
    unsafe {
        if CreatePipe(&mut r, &mut w, &sa, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        // The parent's end must not be inherited.
        let parent_end = if child_reads { w } else { r };
        SetHandleInformation(parent_end, HANDLE_FLAG_INHERIT, 0);
    }
    Ok((r, w))
}

fn restricted_token() -> io::Result<OwnedHandle> {
    // SAFETY: every pointer passed is to a valid local; the SIDs are freed at the end; handles are
    // closed or wrapped.
    unsafe {
        let mut tok: HANDLE = null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT,
            &mut tok,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        // Restricting SIDs: a process needs access for BOTH its normal SIDs and these, so files
        // that only the owner can read (no Everyone/Users/RESTRICTED entry) become unreachable
        // while the system DLLs the process needs to start stay readable.
        let mut sids: Vec<*mut c_void> = Vec::new();
        for s in ["S-1-1-0", "S-1-5-32-545", "S-1-5-12"] {
            let mut sid: *mut c_void = null_mut();
            if ConvertStringSidToSidW(wide_str(s).as_ptr(), &mut sid) == 0 {
                let e = io::Error::last_os_error();
                CloseHandle(tok);
                return Err(e);
            }
            sids.push(sid);
        }
        let restrict: Vec<SID_AND_ATTRIBUTES> = sids
            .iter()
            .map(|&sid| SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: 0,
            })
            .collect();
        let mut out: HANDLE = null_mut();
        let ok = CreateRestrictedToken(
            tok,
            DISABLE_MAX_PRIVILEGE,
            0,
            null(),
            0,
            null(),
            restrict.len() as u32,
            restrict.as_ptr(),
            &mut out,
        );
        let err = io::Error::last_os_error();
        CloseHandle(tok);
        // The SIDs were allocated by ConvertStringSidToSidW (LocalAlloc); a few bytes per worker
        // start are not worth a LocalFree binding.
        if ok == 0 {
            Err(err)
        } else {
            Ok(OwnedHandle(out))
        }
    }
}

/// The SID of the worker's AppContainer profile, creating the profile when it is missing.
fn appcontainer_sid() -> io::Result<*mut c_void> {
    let name = wide_str(APPC_NAME);
    // SAFETY: `name` is NUL-terminated and outlives the calls; `sid` is a valid out-pointer.
    unsafe {
        let mut sid: *mut c_void = null_mut();
        let hr = CreateAppContainerProfile(
            name.as_ptr(),
            name.as_ptr(),
            name.as_ptr(),
            null(),
            0,
            &mut sid,
        );
        if hr == 0 {
            return Ok(sid);
        }
        if hr == ERROR_ALREADY_EXISTS_HR {
            let hr2 = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
            if hr2 == 0 {
                return Ok(sid);
            }
            return Err(io::Error::other(format!(
                "DeriveAppContainerSidFromAppContainerName hr={hr2:#x}"
            )));
        }
        Err(io::Error::other(format!(
            "CreateAppContainerProfile hr={hr:#x}"
        )))
    }
}

/// Removes the AppContainer profile (tests and uninstall; a running app leaves it in place).
pub fn delete_appcontainer_profile() {
    let name = wide_str(APPC_NAME);
    // SAFETY: `name` is NUL-terminated and outlives the call.
    unsafe { DeleteAppContainerProfile(name.as_ptr()) };
}

/// The job object of one worker: one active process, a commit limit, no UI, kill on close.
fn create_job(memory_limit: u64) -> io::Result<OwnedHandle> {
    // SAFETY: the structs are zero-initialised plain data of the documented size; the handle is
    // wrapped at once so it is closed on every error path.
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle(job);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = usize::try_from(memory_limit).unwrap_or(usize::MAX);
        if SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_EXITWINDOWS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        };
        if SetInformationJobObject(
            job.0,
            JobObjectBasicUIRestrictions,
            (&raw const ui).cast(),
            std::mem::size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

/// Starts the worker at `args.level`: pipes for stdin, stdout and stderr (the only handles the
/// child inherits), a job object, the confined token, and a suspended start that is resumed only
/// after the process is inside the job.
pub fn spawn(args: &SpawnArgs<'_>) -> io::Result<Spawned> {
    let mut cmdline = quote_arg(&args.exe.to_string_lossy());
    for a in args.args {
        cmdline.push(' ');
        cmdline.push_str(&quote_arg(a));
    }
    let mut cmdline_w = wide_str(&cmdline);
    let exe_w = wide(args.exe.as_os_str());
    let env_w = environment_block(args.env);

    let (in_r, in_w) = pipe(true)?;
    let (out_r, out_w) = pipe(false)?;
    let (err_r, err_w) = pipe(false)?;
    // Parent ends become `File`s at once so every error path closes them.
    // SAFETY: the handles are valid, owned pipe ends and are not used elsewhere.
    let (stdin, stdout, stderr) = unsafe {
        (
            File::from_raw_handle(in_w as RawHandle),
            File::from_raw_handle(out_r as RawHandle),
            File::from_raw_handle(err_r as RawHandle),
        )
    };
    let child_ends = [OwnedHandle(in_r), OwnedHandle(out_w), OwnedHandle(err_w)];

    let job = create_job(args.memory_limit)?;
    let appc_sid = if args.level == Level::AppContainer {
        Some(appcontainer_sid()?)
    } else {
        None
    };
    let token = if args.level == Level::RestrictedToken {
        Some(restricted_token()?)
    } else {
        None
    };

    let inherit: [HANDLE; 3] = [child_ends[0].0, child_ends[1].0, child_ends[2].0];
    let attr_count = 1 + u32::from(appc_sid.is_some());
    let caps = appc_sid.map(|sid| SECURITY_CAPABILITIES {
        AppContainerSid: sid,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    });
    let mut pi: PROCESS_INFORMATION = PROCESS_INFORMATION {
        hProcess: null_mut(),
        hThread: null_mut(),
        dwProcessId: 0,
        dwThreadId: 0,
    };
    // SAFETY: `list_buf` is sized by the first call and outlives the attribute list; `inherit`,
    // `caps`, the strings and `si` all outlive the CreateProcess call; the attribute list is
    // deleted before the buffers go away; the SID is freed after the call.
    let created = unsafe {
        let mut size = 0usize;
        InitializeProcThreadAttributeList(null_mut(), attr_count, 0, &mut size);
        let mut list_buf = vec![0u8; size];
        let list = list_buf.as_mut_ptr().cast::<c_void>();
        if InitializeProcThreadAttributeList(list, attr_count, 0, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ok = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherit.as_ptr().cast(),
            std::mem::size_of_val(&inherit),
            null_mut(),
            null(),
        ) != 0;
        if ok && let Some(c) = &caps {
            ok = UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
                (c as *const SECURITY_CAPABILITIES).cast(),
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                null_mut(),
                null(),
            ) != 0;
        }
        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.StartupInfo.hStdInput = child_ends[0].0;
        si.StartupInfo.hStdOutput = child_ends[1].0;
        si.StartupInfo.hStdError = child_ends[2].0;
        si.lpAttributeList = list;
        let flags = EXTENDED_STARTUPINFO_PRESENT
            | CREATE_SUSPENDED
            | CREATE_UNICODE_ENVIRONMENT
            // No console at all: the worker talks through pipes, and a console process under a
            // restricted token or AppContainer cannot always attach to a new console.
            | DETACHED_PROCESS;
        let r = if !ok {
            0
        } else if let Some(t) = &token {
            CreateProcessAsUserW(
                t.0,
                exe_w.as_ptr(),
                cmdline_w.as_mut_ptr(),
                null(),
                null(),
                1,
                flags,
                env_w.as_ptr().cast(),
                null(),
                &raw const si.StartupInfo,
                &mut pi,
            )
        } else {
            CreateProcessW(
                exe_w.as_ptr(),
                cmdline_w.as_mut_ptr(),
                null(),
                null(),
                1,
                flags,
                env_w.as_ptr().cast(),
                null(),
                &raw const si.StartupInfo,
                &mut pi,
            )
        };
        let err = io::Error::last_os_error();
        DeleteProcThreadAttributeList(list);
        if let Some(sid) = appc_sid {
            FreeSid(sid);
        }
        if r == 0 { Err(err) } else { Ok(()) }
    };
    created.map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("CreateProcess at level {:?}: {e}", args.level),
        )
    })?;
    let process = OwnedHandle(pi.hProcess);
    let thread = OwnedHandle(pi.hThread);
    drop(child_ends);
    // SAFETY: valid process and job handles; the thread is suspended, so the process has run no
    // code yet when it is assigned.
    unsafe {
        if AssignProcessToJobObject(job.0, process.0) == 0 {
            let e = io::Error::last_os_error();
            TerminateProcess(process.0, 1);
            return Err(io::Error::new(
                e.kind(),
                format!("AssignProcessToJobObject: {e}"),
            ));
        }
        if ResumeThread(thread.0) == u32::MAX {
            let e = io::Error::last_os_error();
            TerminateProcess(process.0, 1);
            return Err(e);
        }
    }
    drop(thread);
    Ok(Spawned {
        child: Child {
            process,
            job,
            pid: pi.dwProcessId,
        },
        stdin,
        stdout,
        stderr,
    })
}

// -------------------------------------------------------------------------------------------
// shared memory

/// The parent's pixel section for one job.
pub struct Section {
    handle: OwnedHandle,
}

impl Section {
    /// A pagefile-backed section of `len` bytes, not inheritable.
    pub fn create(len: u64) -> io::Result<Section> {
        // SAFETY: plain call with a null name and null security attributes.
        let h = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                null(),
                PAGE_READWRITE,
                (len >> 32) as u32,
                len as u32,
                null(),
            )
        };
        if h.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Section {
            handle: OwnedHandle(h),
        })
    }

    /// Duplicates the section into the worker (read and write access only) and returns the
    /// handle value as the worker sees it.
    pub fn duplicate_into(&self, child: &Child) -> io::Result<u64> {
        let mut out: HANDLE = null_mut();
        // SAFETY: valid source handle and process handles; `out` is a valid out-pointer.
        let ok = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                self.handle.0,
                child.process.0,
                &mut out,
                FILE_MAP_READ | FILE_MAP_WRITE,
                0,
                0,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(out as usize as u64)
    }

    /// Copies the first `len` bytes out of the section.
    pub fn copy_out(&self, len: usize) -> io::Result<Vec<u8>> {
        let mut v: Vec<u8> = Vec::new();
        v.try_reserve_exact(len)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        // SAFETY: the view is mapped for `len` bytes read-only, copied from, then unmapped; the
        // section was created with at least `len` bytes (the caller sizes both from one number).
        unsafe {
            let p = MapViewOfFile(self.handle.0, FILE_MAP_READ, 0, 0, len);
            if p.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            v.extend_from_slice(std::slice::from_raw_parts(p.Value.cast::<u8>(), len));
            UnmapViewOfFile(p);
        }
        Ok(v)
    }
}

/// Closes a handle value the parent duplicated into this worker, when the job ended before it was
/// mapped.
pub fn release_handle(handle: u64) {
    // SAFETY: the value was duplicated into this process by the parent for this job and has not
    // been closed; closing a stale value only fails.
    unsafe { CloseHandle(handle as usize as HANDLE) };
}

/// The worker's writable view of the pixel section. Closing the handle and unmapping happen on
/// drop.
pub struct Mapping {
    base: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
    handle: OwnedHandle,
}

impl Mapping {
    /// Maps `len` bytes of the section whose handle value (valid in this process) is `handle`.
    pub fn map(handle: u64, len: usize) -> io::Result<Mapping> {
        let h = handle as usize as HANDLE;
        // SAFETY: the handle value was duplicated into this process by the parent for exactly this
        // purpose; a bad value makes MapViewOfFile fail, which is reported.
        let base = unsafe { MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, len) };
        if base.Value.is_null() {
            let e = io::Error::last_os_error();
            // SAFETY: closing a handle value we were given; failure is irrelevant.
            unsafe { CloseHandle(h) };
            return Err(e);
        }
        Ok(Mapping {
            base,
            len,
            handle: OwnedHandle(h),
        })
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: `base` points to a live view of `len` bytes owned by this value; the &mut self
        // borrow makes the slice unique in this process.
        unsafe { std::slice::from_raw_parts_mut(self.base.Value.cast::<u8>(), self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: the view was returned by MapViewOfFile and is unmapped once; the handle field
        // closes itself afterwards.
        unsafe { UnmapViewOfFile(self.base) };
        let _ = &self.handle;
    }
}

// -------------------------------------------------------------------------------------------
// the worker's own view

/// The worker's control channel: its stdin and a private copy of its stdout. Stray output of a
/// library to stdout is not part of the frames only as far as Rust's own handles go (the stdout
/// handle of the process is pointed at stderr).
pub fn control_channel() -> io::Result<(File, File)> {
    // SAFETY: GetStdHandle has no preconditions; the handles are duplicated before they are
    // wrapped, so the originals stay valid for std.
    unsafe {
        let hin = GetStdHandle(STD_INPUT_HANDLE);
        let hout = GetStdHandle(STD_OUTPUT_HANDLE);
        if hin.is_null()
            || hin == INVALID_HANDLE_VALUE
            || hout.is_null()
            || hout == INVALID_HANDLE_VALUE
        {
            return Err(io::Error::other("the worker has no standard handles"));
        }
        let own = |h: HANDLE| -> io::Result<File> {
            let mut dup: HANDLE = null_mut();
            if DuplicateHandle(
                GetCurrentProcess(),
                h,
                GetCurrentProcess(),
                &mut dup,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(File::from_raw_handle(dup as RawHandle))
        };
        let (fin, fout) = (own(hin)?, own(hout)?);
        // Point STD_OUTPUT_HANDLE at stderr so Rust-side prints cannot corrupt the frames.
        let herr = GetStdHandle(windows_sys::Win32::System::Console::STD_ERROR_HANDLE);
        windows_sys::Win32::System::Console::SetStdHandle(STD_OUTPUT_HANDLE, herr);
        Ok((fin, fout))
    }
}

/// What the worker process is confined by, from the token and job it actually runs in (not from
/// what the parent intended). Returns the level text and whether it is an AppContainer.
pub fn confinement_report() -> (String, bool) {
    // SAFETY: all out-pointers are valid locals or buffers sized by the query that precedes the
    // read; the token is the process pseudo handle, which needs no closing.
    unsafe {
        // The pseudo handle of the process token: opening the token by `OpenProcessToken` is
        // refused by a restricted token's own access check.
        let tok: HANDLE = -4isize as HANDLE; // GetCurrentProcessToken()
        let mut appcontainer = false;
        let mut caps = u32::MAX;
        let restricted;
        {
            let mut v = 0u32;
            let mut n = 0u32;
            if GetTokenInformation(
                tok,
                TokenIsAppContainer,
                (&raw mut v).cast(),
                std::mem::size_of::<u32>() as u32,
                &mut n,
            ) != 0
            {
                appcontainer = v != 0;
            }
            let mut is_restricted = IsTokenRestricted(tok) != 0;
            if !is_restricted {
                // Belt and braces: count the restricting SIDs directly.
                let mut need = 0u32;
                GetTokenInformation(tok, TokenRestrictedSids, null_mut(), 0, &mut need);
                if (4..1 << 20).contains(&need) {
                    let mut buf = vec![0u64; (need as usize).div_ceil(8).max(4)];
                    if GetTokenInformation(
                        tok,
                        TokenRestrictedSids,
                        buf.as_mut_ptr().cast(),
                        need,
                        &mut need,
                    ) != 0
                    {
                        is_restricted = (*buf.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount > 0;
                    }
                }
            }
            restricted = is_restricted;
            // TokenCapabilities: a TOKEN_GROUPS; only the count matters.
            let mut need = 0u32;
            GetTokenInformation(tok, TokenCapabilities, null_mut(), 0, &mut need);
            if (4..1 << 20).contains(&need) {
                let mut buf = vec![0u64; (need as usize).div_ceil(8).max(4)];
                if GetTokenInformation(
                    tok,
                    TokenCapabilities,
                    buf.as_mut_ptr().cast(),
                    need,
                    &mut need,
                ) != 0
                {
                    caps = (*buf.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount;
                }
            }
        }
        let mut in_job = 0i32;
        let mut job_text = String::from("job=none");
        if IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) != 0 && in_job != 0 {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            let mut ret = 0u32;
            if QueryInformationJobObject(
                null_mut(),
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                &mut ret,
            ) != 0
            {
                let f = info.BasicLimitInformation.LimitFlags;
                let active = if f & JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0 {
                    info.BasicLimitInformation.ActiveProcessLimit
                } else {
                    0
                };
                let mem = if f & JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0 {
                    (info.ProcessMemoryLimit as u64) >> 20
                } else {
                    0
                };
                let kill = f & JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE != 0;
                job_text = format!(
                    "job(active-process={active},memory={mem}MiB,kill-on-close={})",
                    if kill { "yes" } else { "no" }
                );
            } else {
                job_text = "job(limits unreadable)".into();
            }
        }
        let token = if appcontainer {
            format!(
                "appcontainer(capabilities={})",
                if caps == u32::MAX {
                    "?".to_owned()
                } else {
                    caps.to_string()
                }
            )
        } else if restricted {
            "restricted-token".to_owned()
        } else {
            "token=unrestricted".to_owned()
        };
        (format!("{token}+{job_text}"), appcontainer)
    }
}
