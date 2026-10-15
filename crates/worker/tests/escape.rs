// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The escape tests (ROADMAP S.15, M6.06, M6.10, ADR-0006): the REAL worker binary, built with
//! the test-only `escape-tests` feature, attempts four things AFTER its sandbox is applied (open a
//! file by path, connect to a TCP port, start a process, allocate 1 GiB) and reports each. What
//! must be blocked depends on the level the worker reached, and the expectations are those of the
//! spike (`spikes/sandbox`, copied into ADR-0006):
//!
//! | OS and level | file | tcp | spawn | alloc |
//! |---|---|---|---|---|
//! | Windows AppContainer + job | blocked | blocked | blocked | blocked |
//! | Windows restricted token + job | blocked | any | blocked | blocked |
//! | Windows job only | allowed | allowed | blocked | blocked |
//! | Linux rlimit + Landlock + seccomp | blocked | blocked | blocked | blocked |
//! | Linux, Landlock absent (seccomp + rlimit) | blocked | blocked | blocked | blocked |
//! | macOS Seatbelt deny-default | blocked | blocked | blocked | not checked (no RLIMIT_AS) |
//!
//! The control row is the same worker with no confinement where the OS has a way to ask for it
//! (Windows job only: file and tcp are allowed, proving the tests can pass when nothing blocks).
#![cfg(feature = "escape-tests")]

use auto_crop_sandbox::{PoolConfig, Tier, WorkerPool};
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

struct Report {
    file: String,
    tcp: String,
    spawn: String,
    alloc: String,
}

fn parse(text: &str) -> Report {
    let get = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k).map(|s| s.trim().to_owned()))
            .unwrap_or_else(|| "missing".into())
    };
    Report {
        file: get("file "),
        tcp: get("tcp "),
        spawn: get("spawn "),
        alloc: get("alloc "),
    }
}

/// A file only the current user can read, so a world-readable system file cannot make a blocked
/// level look unblocked (or the reverse).
fn secret_file(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("autocrop-escape-secret.txt");
    std::fs::write(&p, "top secret: only the owner may read this\n").unwrap();
    #[cfg(windows)]
    {
        let user = format!(
            "{}\\{}",
            std::env::var("USERDOMAIN").unwrap_or_default(),
            std::env::var("USERNAME").unwrap_or_default()
        );
        let st = std::process::Command::new("icacls")
            .arg(&p)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("{user}:(F)"))
            .arg("/Q")
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "icacls on the secret file failed");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    p
}

fn run_level(attempt: Option<usize>, extra_args: &[&str]) -> (Report, String, Tier) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            drop(s);
        }
    });
    let dir = tempfile::tempdir().unwrap();
    let secret = secret_file(dir.path());
    let mut cfg = PoolConfig::new(PathBuf::from(env!("CARGO_BIN_EXE_auto-crop-worker")));
    cfg.memory_limit = Some(512 << 20);
    cfg.await_escape = true;
    cfg.force_attempt = attempt;
    cfg.startup_timeout = Duration::from_secs(60);
    cfg.args = extra_args.iter().map(|s| (*s).to_owned()).collect();
    cfg.env.push((
        "AUTOCROP_ESCAPE".into(),
        format!("{},{port}", secret.display()).into(),
    ));
    let pool = WorkerPool::new(cfg);
    let (status, text) = pool.probe_escape().expect("a worker starts");
    let level = status.level.clone().unwrap();
    println!("ESCAPE attempt={:?}", status.attempt);
    println!("ESCAPE level={level}");
    println!(
        "ESCAPE probes={}",
        status.probes.clone().unwrap_or_default()
    );
    for l in text.lines() {
        println!("ESCAPE {l}");
    }
    for f in &status.attempt_failures {
        println!("ESCAPE (stronger attempt failed: {f})");
    }
    (parse(&text), level, status.tier.unwrap())
}

fn blocked(what: &str, got: &str) {
    assert!(
        got.starts_with("blocked"),
        "{what} should be blocked, got {got}"
    );
}

#[allow(dead_code)]
fn allowed(what: &str, got: &str) {
    assert!(
        got.starts_with("allowed"),
        "{what} should be allowed, got {got}"
    );
}

#[cfg(windows)]
mod windows {
    use super::*;

    fn attempt(name: &str) -> usize {
        auto_crop_sandbox::os::attempt_names()
            .iter()
            .position(|n| *n == name)
            .unwrap()
    }

    #[test]
    fn appcontainer_blocks_file_tcp_spawn_and_alloc() {
        let (r, level, tier) = run_level(Some(attempt("appcontainer+job")), &[]);
        assert!(level.starts_with("appcontainer(capabilities=0)"), "{level}");
        assert_eq!(tier, Tier::Full);
        blocked("file", &r.file);
        blocked("tcp", &r.tcp);
        blocked("spawn", &r.spawn);
        blocked("alloc", &r.alloc);
    }

    #[test]
    fn restricted_token_blocks_files_spawn_and_alloc_but_not_the_network() {
        let (r, level, tier) = run_level(Some(attempt("restricted-token+job")), &[]);
        assert!(level.starts_with("restricted-token"), "{level}");
        assert_eq!(tier, Tier::Reduced);
        blocked("file", &r.file);
        blocked("spawn", &r.spawn);
        blocked("alloc", &r.alloc);
        // the network is left open by a restricted token: recorded, not claimed
        println!("ESCAPE restricted token tcp = {}", r.tcp);
    }

    #[test]
    fn job_only_is_the_control_files_and_network_open_processes_and_memory_limited() {
        let (r, level, tier) = run_level(Some(attempt("job")), &[]);
        assert!(level.contains("job(active-process=1"), "{level}");
        assert_eq!(tier, Tier::ProcessOnly);
        allowed("file", &r.file);
        allowed("tcp", &r.tcp);
        blocked("spawn", &r.spawn);
        blocked("alloc", &r.alloc);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;

    #[test]
    fn full_level_blocks_file_tcp_spawn_and_alloc() {
        let (r, level, tier) = run_level(None, &[]);
        assert!(level.contains("seccomp("), "{level}");
        assert!(tier >= Tier::Reduced, "{level}");
        blocked("file", &r.file);
        blocked("tcp", &r.tcp);
        blocked("spawn", &r.spawn);
        blocked("alloc", &r.alloc);
        if level.contains("landlock=FullyEnforced") {
            assert_eq!(tier, Tier::Full);
        }
    }

    #[test]
    fn with_landlock_absent_the_level_says_so_and_seccomp_still_blocks_everything() {
        let (r, level, tier) = run_level(None, &["--no-landlock"]);
        assert!(level.contains("landlock=unavailable("), "{level}");
        assert_eq!(tier, Tier::Reduced);
        blocked("file", &r.file);
        blocked("tcp", &r.tcp);
        blocked("spawn", &r.spawn);
        blocked("alloc", &r.alloc);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;

    #[test]
    fn seatbelt_blocks_file_tcp_and_spawn() {
        let (r, level, tier) = run_level(None, &[]);
        assert!(level.starts_with("sandbox_init("), "{level}");
        assert_eq!(tier, Tier::Full);
        blocked("file", &r.file);
        blocked("tcp", &r.tcp);
        blocked("spawn", &r.spawn);
        // RLIMIT_AS is not enforced by the macOS kernel: the memory limit is the parent's
        // watchdog, shown by the pool test `running_out_of_memory_...`, not by this probe.
        println!("ESCAPE macOS alloc (not claimed as blocked) = {}", r.alloc);
    }
}
