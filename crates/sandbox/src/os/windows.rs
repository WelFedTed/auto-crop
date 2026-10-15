// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Windows: the worker is created already confined (ADR-0006, ADR-0013), in three attempts that
//! the pool tries in order and reports: AppContainer with no capabilities plus a job object,
//! restricted token plus job, job only. An AppContainer process cannot read the folder the
//! package was unzipped into (an ordinary user folder has no ACL entry for AppContainers), so for
//! the first two levels the worker, the DLLs beside it and the libheif plugin folder are copied
//! once into a private staging folder with read-and-execute grants for the AppContainer and
//! restricted SIDs (`%LOCALAPPDATA%\AutoCrop\worker-stage\<fingerprint>`); an installed copy under
//! `Program Files` would not need it but gets the same treatment, so there is one code path.

use super::ffi::windows::{self as w, Child, Level, Section, SpawnArgs};
use super::{Exit, LaunchSpec, Launched, Proc, Shm};
use std::ffi::OsString;
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub fn attempt_names() -> &'static [&'static str] {
    &["appcontainer+job", "restricted-token+job", "job"]
}

struct WinProc(Arc<Child>);

impl Proc for WinProc {
    fn pid(&self) -> u32 {
        self.0.id()
    }
    fn kill(&self) {
        self.0.kill();
    }
    fn try_wait(&self) -> Option<Exit> {
        self.0.try_wait().map(exit_of)
    }
    fn wait_timeout(&self, d: Duration) -> Option<Exit> {
        self.0
            .wait_timeout(u32::try_from(d.as_millis()).unwrap_or(u32::MAX))
            .map(exit_of)
    }
    fn resident_bytes(&self) -> Option<u64> {
        None
    }
}

fn exit_of(code: u32) -> Exit {
    Exit {
        code: Some(i64::from(code)),
        signal: None,
    }
}

struct WinShm {
    child: Arc<Child>,
    section: Option<Section>,
}

impl Shm for WinShm {
    fn prepare(&mut self, len: u64, _child: &dyn Proc) -> io::Result<u64> {
        let s = Section::create(len.max(1))?;
        let handle = s.duplicate_into(&self.child)?;
        self.section = Some(s);
        Ok(handle)
    }

    fn take(&mut self, len: usize) -> io::Result<Vec<u8>> {
        self.section
            .as_ref()
            .ok_or_else(|| io::Error::other("no section prepared"))?
            .copy_out(len)
    }

    fn release(&mut self) {
        self.section = None;
    }
}

// ---------------------------------------------------------------------------------------------
// staging

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

fn dlls_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dll")))
        .collect();
    v.sort();
    v
}

/// The folder that holds `heif.dll` when it is not beside the worker (a development build finds
/// its DLLs through `PATH`).
fn dll_dir_on_path() -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).find(|d| d.join("heif.dll").is_file())
}

fn stat_into(h: &mut u64, p: &Path) {
    fnv(h, p.file_name().unwrap_or_default().as_encoded_bytes());
    if let Ok(m) = std::fs::metadata(p) {
        fnv(h, &m.len().to_le_bytes());
        if let Ok(t) = m.modified()
            && let Ok(d) = t.duration_since(std::time::UNIX_EPOCH)
        {
            fnv(h, &d.as_nanos().to_le_bytes());
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_file() {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

fn icacls(dir: &Path) -> io::Result<()> {
    // ALL APPLICATION PACKAGES, ALL RESTRICTED APP PACKAGES (AppContainer) and RESTRICTED (the
    // restricted token's restricting SID): read and execute, inherited by the files.
    let st = std::process::Command::new("icacls")
        .arg(dir)
        .args([
            "/grant",
            "*S-1-15-2-1:(OI)(CI)(RX)",
            "/grant",
            "*S-1-15-2-2:(OI)(CI)(RX)",
            "/grant",
            "*S-1-5-12:(OI)(CI)(RX)",
            "/T",
            "/Q",
        ])
        .creation_flags(0x0800_0000)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if st.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("icacls failed: {st}")))
    }
}

/// Copies the worker, its DLLs and the plugin folder into the staging folder (once per
/// fingerprint) and returns the staged executable and plugin folder.
fn stage(spec: &LaunchSpec) -> io::Result<(PathBuf, Option<PathBuf>)> {
    // One staging at a time in this process (workers start on several threads).
    static STAGING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = STAGING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let exe_dir = spec
        .exe
        .parent()
        .ok_or_else(|| io::Error::other("worker path has no folder"))?;
    let mut dlls = dlls_in(exe_dir);
    if !exe_dir.join("heif.dll").is_file()
        && let Some(d) = dll_dir_on_path()
    {
        dlls.extend(dlls_in(&d));
    }
    let plugin_src = spec
        .plugin_dir
        .clone()
        .or_else(|| std::env::var_os("LIBHEIF_PLUGIN_PATH").map(PathBuf::from))
        .filter(|p| p.is_dir());

    let mut h = 0xcbf2_9ce4_8422_2325u64;
    stat_into(&mut h, &spec.exe);
    for d in &dlls {
        stat_into(&mut h, d);
    }
    if let Some(p) = &plugin_src {
        for d in dlls_in(p) {
            stat_into(&mut h, &d);
        }
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join("AutoCrop")
        .join("worker-stage");
    let name = format!("{h:016x}");
    let dir = base.join(&name);
    let exe_name = spec.exe.file_name().unwrap_or_default();
    let staged_exe = dir.join(exe_name);
    let staged_plugin = plugin_src.as_ref().map(|_| dir.join("libheif"));
    if dir.join(".complete").is_file() && staged_exe.is_file() {
        return Ok((staged_exe, staged_plugin));
    }
    std::fs::create_dir_all(&base)?;
    let tmp = base.join(format!("{name}.tmp{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let result = (|| -> io::Result<()> {
        std::fs::copy(&spec.exe, tmp.join(exe_name))?;
        for d in &dlls {
            std::fs::copy(d, tmp.join(d.file_name().unwrap_or_default()))?;
        }
        if let Some(p) = &plugin_src {
            copy_tree(p, &tmp.join("libheif"))?;
        }
        icacls(&tmp)?;
        std::fs::write(tmp.join(".complete"), b"ok")?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    if std::fs::rename(&tmp, &dir).is_err() {
        // Another process staged the same fingerprint first: use theirs.
        let _ = std::fs::remove_dir_all(&tmp);
        if !dir.join(".complete").is_file() {
            return Err(io::Error::other("could not stage the worker"));
        }
    }
    // Older fingerprints are stale; remove them best effort (one may be in use by another
    // running instance, in which case the removal of its locked files simply fails).
    if let Ok(rd) = std::fs::read_dir(&base) {
        for e in rd.flatten() {
            let fname = e.file_name().to_string_lossy().into_owned();
            let is_fingerprint = fname.len() == 16 && fname.bytes().all(|b| b.is_ascii_hexdigit());
            if is_fingerprint && fname != name && e.path().is_dir() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok((staged_exe, staged_plugin))
}

pub fn launch(spec: &LaunchSpec) -> io::Result<Launched> {
    let level = match spec.attempt {
        0 => Level::AppContainer,
        1 => Level::RestrictedToken,
        _ => Level::JobOnly,
    };
    let (exe, plugin) = if level == Level::JobOnly {
        (spec.exe.clone(), spec.plugin_dir.clone())
    } else {
        stage(spec)?
    };
    let mut args = spec.args.clone();
    if let Some(p) = &plugin {
        args.push("--plugin-dir".into());
        args.push(p.to_string_lossy().into_owned());
    }
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from("C:\\Windows"));
    let system32 = Path::new(&root).join("System32");
    let mut env: Vec<(OsString, OsString)> = vec![
        ("SystemRoot".into(), root.clone()),
        ("windir".into(), root.clone()),
    ];
    // Creating an AppContainer process rewrites these (it points them at the container's own
    // folders) and fails when the block lacks them.
    for k in [
        "TEMP",
        "TMP",
        "LOCALAPPDATA",
        "APPDATA",
        "USERPROFILE",
        "ProgramData",
    ] {
        if let Some(v) = std::env::var_os(k) {
            env.push((k.into(), v));
        }
    }
    if let Some(v) = std::env::var_os("USERPROFILE") {
        env.push(("AUTOCROP_PROBE_DIR".into(), v));
    }
    if level == Level::JobOnly {
        // Unconfined files anyway: keep the development PATH so the DLLs are found.
        env.push(("PATH".into(), std::env::var_os("PATH").unwrap_or_default()));
    } else {
        env.push(("PATH".into(), system32.into_os_string()));
    }
    env.extend(spec.env.iter().cloned());
    let sp = w::spawn(&SpawnArgs {
        exe: &exe,
        args: &args,
        env: &env,
        level,
        memory_limit: spec.memory_limit,
    })?;
    let child = Arc::new(sp.child);
    Ok(Launched {
        stdin: Box::new(sp.stdin),
        stdout: Box::new(sp.stdout),
        stderr: Box::new(sp.stderr),
        proc: Arc::new(WinProc(child.clone())),
        shm: Box::new(WinShm {
            child,
            section: None,
        }),
    })
}

/// The worker side: the process is already confined; report what it really runs under.
pub fn worker_confine(
    _cfg: &crate::worker_rt::ConfineConfig,
) -> Result<crate::worker_rt::Confinement, String> {
    let (level, _appcontainer) = w::confinement_report();
    Ok(crate::worker_rt::Confinement {
        level,
        probes: crate::worker_rt::probe_escapes(),
    })
}

/// Removes the AppContainer profile of the worker (tests; an uninstaller).
pub fn delete_profile() {
    w::delete_appcontainer_profile();
}
