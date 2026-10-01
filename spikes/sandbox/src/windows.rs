// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Windows back end. Shared memory is an inheritable file-mapping handle; the worker is created
//! with `CreateProcess[AsUser]W` so the parent controls the token and the handle whitelist.
//! Modes: none (control), job (job object only), token+job (restricted token), appcontainer+job
//! (AppContainer with no capabilities). The worker puts itself into a job object (active-process
//! limit 1, 512 MB per-process memory); a real implementation would create the process suspended.

use super::Expect;
use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::{ExitStatus, Output};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{
    CloseHandle, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeleteAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{
    CreateRestrictedToken, DISABLE_MAX_PRIVILEGE, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES,
    SID_AND_ATTRIBUTES, TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, InitializeProcThreadAttributeList, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};

const ENV_SHM: &str = "AUTOCROP_SHM";
const ENV_SECRET: &str = "AUTOCROP_SECRET";
const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 0x0002_0009;
const APPC_NAME: &str = "autocrop.sandbox.spike";
const ERROR_ALREADY_EXISTS_HR: i32 = 0x8007_00B7_u32 as i32;

pub struct Shm(HANDLE);

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn modes() -> Vec<&'static str> {
    vec!["none", "job", "token+job", "appcontainer+job"]
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

pub fn shared_create(size: usize) -> io::Result<Shm> {
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    let h = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, &sa, PAGE_READWRITE, 0, size as u32, null()) };
    if h.is_null() { Err(io::Error::last_os_error()) } else { Ok(Shm(h)) }
}

fn map(h: HANDLE, len: usize) -> io::Result<*mut u8> {
    let p = unsafe { MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, len) };
    if p.Value.is_null() { Err(io::Error::last_os_error()) } else { Ok(p.Value.cast()) }
}

pub fn write_shared(data: &[u8]) -> io::Result<()> {
    let hex = std::env::var(ENV_SHM).map_err(|e| io::Error::other(e.to_string()))?;
    let h = usize::from_str_radix(&hex, 16).map_err(|e| io::Error::other(e.to_string()))? as HANDLE;
    let p = map(h, data.len())?;
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: p.cast() });
    }
    Ok(())
}

pub fn shared_read(shm: &Shm) -> Vec<u8> {
    let p = map(shm.0, super::PIXELS).expect("map shared memory");
    let v = unsafe { std::slice::from_raw_parts(p, super::PIXELS) }.to_vec();
    unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: p.cast() }) };
    v
}

/// The worker joins a job: active-process limit 1 (no child processes) and a 512 MB memory cap.
fn apply_job() -> Result<String, String> {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(format!("CreateJobObject: {}", io::Error::last_os_error()));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = 512 << 20;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            return Err(format!("SetInformationJobObject: {}", io::Error::last_os_error()));
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err(format!("AssignProcessToJobObject: {}", io::Error::last_os_error()));
        }
    }
    Ok("job(active-process=1,memory=512MB)".to_owned())
}

pub fn apply(mode: &str) -> Result<String, String> {
    if mode == "none" {
        return Ok("none".to_owned());
    }
    let job = apply_job()?;
    Ok(match mode {
        "token+job" => format!("restricted-token(restricting sids Everyone+Users+RESTRICTED, no privileges)+{job}"),
        "appcontainer+job" => format!("appcontainer(no capabilities)+{job}"),
        _ => job,
    })
}

pub fn expect(mode: &str, _level: &str) -> Expect {
    match mode {
        "none" => Expect { file: "allowed", tcp: "allowed", spawn: "allowed", alloc: "allowed" },
        // a job object limits processes and memory only
        "job" => Expect { file: "allowed", tcp: "allowed", spawn: "blocked", alloc: "blocked" },
        "token+job" => Expect { file: "blocked", tcp: "any", spawn: "blocked", alloc: "blocked" },
        _ => Expect { file: "blocked", tcp: "blocked", spawn: "blocked", alloc: "blocked" },
    }
}

// ---------------------------------------------------------------------------------------------
// launching

struct Handles(Vec<HANDLE>);

impl Drop for Handles {
    fn drop(&mut self) {
        for h in &self.0 {
            unsafe { CloseHandle(*h) };
        }
    }
}

fn icacls(dir: &std::path::Path, sid: &str) -> io::Result<()> {
    let st = std::process::Command::new("icacls").arg(dir).arg("/grant").arg(format!("*{sid}:(OI)(CI)(RX)")).arg("/T").arg("/Q").status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other("icacls failed")) }
}

/// A copy of this executable in a private directory whose ACL lets restricted principals read and
/// run it (they cannot reach the build directory).
fn worker_exe(sids: &[&str]) -> io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join("autocrop-sandbox-worker");
    std::fs::create_dir_all(&dir)?;
    let exe = dir.join("worker.exe");
    std::fs::copy(std::env::current_exe()?, &exe)?;
    for s in sids {
        icacls(&dir, s)?;
    }
    Ok(exe)
}

fn secret_file() -> io::Result<()> {
    let p = std::env::temp_dir().join(format!("autocrop-secret-{}.txt", std::process::id()));
    std::fs::write(&p, "top secret: only the owner may read this\n")?;
    // drop every inherited ACE so only the current user keeps access (AppContainers and
    // restricted tokens must not reach it)
    let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
    let st = std::process::Command::new("icacls").arg(&p).args(["/inheritance:r", "/grant:r"]).arg(format!("{user}:(F)")).arg("/Q").status()?;
    if !st.success() {
        return Err(io::Error::other("icacls on the secret file failed"));
    }
    if std::env::var_os("AUTOCROP_DEBUG").is_some() {
        let _ = std::process::Command::new("icacls").arg(&p).status();
    }
    unsafe { std::env::set_var(ENV_SECRET, &p) };
    Ok(())
}

fn restricted_token() -> io::Result<HANDLE> {
    unsafe {
        let mut tok: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_DEFAULT, &mut tok) == 0 {
            return Err(io::Error::last_os_error());
        }
        // restricting SIDs: a process needs access for BOTH its normal SIDs and these, so files
        // that only the owner can read (no Everyone/Users/RESTRICTED entry) become unreachable,
        // while the system DLLs the process needs to start stay readable.
        let mut sids: Vec<*mut c_void> = Vec::new();
        for s in ["S-1-1-0", "S-1-5-32-545", "S-1-5-12"] {
            let mut sid: *mut c_void = null_mut();
            if ConvertStringSidToSidW(wide(s).as_ptr(), &mut sid) == 0 {
                return Err(io::Error::last_os_error());
            }
            sids.push(sid);
        }
        let restrict: Vec<SID_AND_ATTRIBUTES> = sids.iter().map(|&sid| SID_AND_ATTRIBUTES { Sid: sid, Attributes: 0 }).collect();
        let mut out: HANDLE = null_mut();
        let ok = CreateRestrictedToken(tok, DISABLE_MAX_PRIVILEGE, 0, null(), 0, null(), restrict.len() as u32, restrict.as_ptr(), &mut out);
        CloseHandle(tok);
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(out) }
    }
}

fn appcontainer_sid() -> io::Result<*mut c_void> {
    unsafe {
        let name = wide(APPC_NAME);
        let mut sid: *mut c_void = null_mut();
        let hr = CreateAppContainerProfile(name.as_ptr(), name.as_ptr(), name.as_ptr(), null(), 0, &mut sid);
        if hr == 0 {
            return Ok(sid);
        }
        if hr == ERROR_ALREADY_EXISTS_HR {
            let hr2 = DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid);
            if hr2 == 0 {
                return Ok(sid);
            }
            return Err(io::Error::other(format!("DeriveAppContainerSid hr={hr2:#x}")));
        }
        Err(io::Error::other(format!("CreateAppContainerProfile hr={hr:#x}")))
    }
}

pub fn run(job: &super::Job, shm: &Shm, mode: &str) -> io::Result<Output> {
    if std::env::var_os(ENV_SECRET).is_none() {
        secret_file()?;
    }
    unsafe { std::env::set_var(ENV_SHM, format!("{:x}", shm.0 as usize)) };
    let appc = mode == "appcontainer+job";
    let token = mode == "token+job";
    let exe = if appc {
        worker_exe(&["S-1-15-2-1", "S-1-15-2-2"])?
    } else if token {
        worker_exe(&["S-1-5-12"])?
    } else {
        std::env::current_exe()?
    };
    let mut cmdline = String::new();
    cmdline.push('"');
    cmdline.push_str(&exe.to_string_lossy());
    cmdline.push('"');
    for a in &job.args {
        cmdline.push(' ');
        cmdline.push_str(a);
    }
    let mut cmdline_w = wide(&cmdline);
    let exe_w = wide(&exe.to_string_lossy());

    // stdin = the input file as an inherited handle (no path); stdout and stderr = pipes
    let input = File::open(job.input_path)?;
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: null_mut(), bInheritHandle: 1 };
    let mk_pipe = || -> io::Result<(HANDLE, HANDLE)> {
        let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if unsafe { CreatePipe(&mut r, &mut w, &sa, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe { SetHandleInformation(r, HANDLE_FLAG_INHERIT, 0) };
        Ok((r, w))
    };
    let (out_r, out_w) = mk_pipe()?;
    let (err_r, err_w) = mk_pipe()?;
    let stdin_h = input.as_raw_handle() as HANDLE;
    unsafe { SetHandleInformation(stdin_h, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
    let inherit = [stdin_h, out_w, err_w, shm.0];

    let appc_sid = if appc { Some(appcontainer_sid()?) } else { None };
    let tok = if token { Some(restricted_token()?) } else { None };

    let attr_count = 1 + u32::from(appc);
    let mut size = 0usize;
    unsafe { InitializeProcThreadAttributeList(null_mut(), attr_count, 0, &mut size) };
    let mut list_buf = vec![0u8; size];
    let list = list_buf.as_mut_ptr().cast::<c_void>();
    unsafe {
        if InitializeProcThreadAttributeList(list, attr_count, 0, &mut size) == 0 {
            return Err(io::Error::last_os_error());
        }
        if UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, inherit.as_ptr().cast(), std::mem::size_of_val(&inherit), null_mut(), null()) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let caps = appc_sid.map(|sid| SECURITY_CAPABILITIES { AppContainerSid: sid, Capabilities: null_mut(), CapabilityCount: 0, Reserved: 0 });
    if let Some(c) = &caps {
        unsafe {
            if UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, (c as *const SECURITY_CAPABILITIES).cast(), std::mem::size_of::<SECURITY_CAPABILITIES>(), null_mut(), null()) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }
    let mut si: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    si.StartupInfo.hStdInput = stdin_h;
    si.StartupInfo.hStdOutput = out_w;
    si.StartupInfo.hStdError = err_w;
    si.lpAttributeList = list;
    let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let created = unsafe {
        match tok {
            Some(t) => CreateProcessAsUserW(t, exe_w.as_ptr(), cmdline_w.as_mut_ptr(), null(), null(), 1, EXTENDED_STARTUPINFO_PRESENT, null(), null(), &si.StartupInfo, &mut pi),
            None => CreateProcessW(exe_w.as_ptr(), cmdline_w.as_mut_ptr(), null(), null(), 1, EXTENDED_STARTUPINFO_PRESENT, null(), null(), &si.StartupInfo, &mut pi),
        }
    };
    let create_err = io::Error::last_os_error();
    unsafe {
        DeleteProcThreadAttributeList(list);
        CloseHandle(out_w);
        CloseHandle(err_w);
        if let Some(t) = tok {
            CloseHandle(t);
        }
    }
    if created == 0 {
        unsafe {
            CloseHandle(out_r);
            CloseHandle(err_r);
        }
        return Err(io::Error::new(create_err.kind(), format!("CreateProcess ({mode}): {create_err}")));
    }
    let _guard = Handles(vec![pi.hProcess, pi.hThread]);
    let mut stdout_f = unsafe { File::from_raw_handle(out_r.cast()) };
    let mut stderr_f = unsafe { File::from_raw_handle(err_r.cast()) };
    let err_thread = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stderr_f.read_to_end(&mut v);
        v
    });
    let mut out = Vec::new();
    let _ = stdout_f.read_to_end(&mut out);
    let err = err_thread.join().unwrap_or_default();
    let mut code = 0u32;
    unsafe {
        if WaitForSingleObject(pi.hProcess, 60_000) != WAIT_OBJECT_0 {
            return Err(io::Error::other("worker timed out"));
        }
        GetExitCodeProcess(pi.hProcess, &mut code);
    }
    drop(input);
    Ok(Output { status: ExitStatus::from_raw(code), stdout: out, stderr: err })
}

/// Removes the AppContainer profile the spike created (nothing is left behind in the registry).
pub fn cleanup() {
    unsafe { DeleteAppContainerProfile(wide(APPC_NAME).as_ptr()) };
    if let Ok(p) = std::env::var(ENV_SECRET) {
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("autocrop-sandbox-worker"));
}
