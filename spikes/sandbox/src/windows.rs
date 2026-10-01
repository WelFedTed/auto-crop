// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Windows back end: inheritable file-mapping handle as shared memory and a job object
//! (active-process limit 1, per-process memory cap). Modes: none (control), job.
//! The job object alone does NOT block file or network access; that needs a restricted token
//! or an AppContainer (added in the next step of M0.21).

use super::Expect;
use std::io;
use std::os::windows::process::CommandExt;
use std::process::{Command, Output};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, SetInformationJobObject,
};
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const ENV: &str = "AUTOCROP_SHM";

pub struct Shm(HANDLE);

impl Drop for Shm {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn modes() -> Vec<&'static str> {
    vec!["none", "job"]
}

pub fn shared_create(size: usize) -> io::Result<Shm> {
    let sa = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: std::ptr::null_mut(), bInheritHandle: 1 };
    let h = unsafe { CreateFileMappingW(INVALID_HANDLE_VALUE, &sa, PAGE_READWRITE, 0, size as u32, std::ptr::null()) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(Shm(h))
}

pub fn prepare(cmd: &mut Command, shm: &Shm, _mode: &str) {
    // std spawns with handle inheritance on, so an inheritable handle keeps its value in the child.
    cmd.env(ENV, format!("{:x}", shm.0 as usize));
    cmd.creation_flags(0);
}

pub fn run(mut cmd: Command, _mode: &str) -> io::Result<Output> {
    cmd.spawn()?.wait_with_output()
}

fn map(h: HANDLE, len: usize) -> io::Result<*mut u8> {
    let p = unsafe { MapViewOfFile(h, FILE_MAP_ALL_ACCESS, 0, 0, len) };
    if p.Value.is_null() { Err(io::Error::last_os_error()) } else { Ok(p.Value.cast()) }
}

pub fn write_shared(data: &[u8]) -> io::Result<()> {
    let h = usize::from_str_radix(&std::env::var(ENV).map_err(|e| io::Error::other(e.to_string()))?, 16).map_err(|e| io::Error::other(e.to_string()))? as HANDLE;
    let p = map(h, data.len())?;
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: p.cast() });
    }
    Ok(())
}

pub fn shared_read(shm: &Shm) -> Vec<u8> {
    let p = map(shm.0, super::PIXELS).expect("map shared memory");
    let v = unsafe { std::slice::from_raw_parts(p, super::PIXELS) }.to_vec();
    unsafe { UnmapViewOfFile(windows_sys::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: p.cast() }) };
    v
}

/// The worker puts itself into a job (the real implementation creates the process suspended and
/// assigns it before it runs; a process may also join a job itself).
fn apply_job() -> Result<String, String> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(format!("CreateJobObject: {}", io::Error::last_os_error()));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        info.ProcessMemoryLimit = 512 << 20;
        let ok = SetInformationJobObject(job, JobObjectExtendedLimitInformation, (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(), std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
        if ok == 0 {
            return Err(format!("SetInformationJobObject: {}", io::Error::last_os_error()));
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err(format!("AssignProcessToJobObject: {}", io::Error::last_os_error()));
        }
    }
    Ok("job(active-process=1, memory=512MB)".to_owned())
}

pub fn apply(mode: &str) -> Result<String, String> {
    if mode == "job" { apply_job() } else { Ok("none".to_owned()) }
}

pub fn expect(mode: &str, _level: &str) -> Expect {
    if mode == "job" {
        // a job object limits processes and memory only: file and network access remain open
        Expect { file: "allowed", tcp: "allowed", spawn: "blocked", alloc: "blocked" }
    } else {
        Expect { file: "allowed", tcp: "allowed", spawn: "allowed", alloc: "allowed" }
    }
}
