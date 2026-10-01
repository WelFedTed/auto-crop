// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Sandbox spike (ROADMAP M0.21-M0.23).
//!
//! The parent opens an input file and a shared-memory region, then spawns itself as a worker
//! ("decode helper") that receives the input through an INHERITED handle (its stdin: no path)
//! and returns RGB8 pixels through the shared memory. The worker first applies the sandbox
//! `mode`, then attempts four "escapes" (open a file by path, connect to a TCP port, spawn a
//! process, allocate 1 GiB) and finally does its real work. The parent checks that blocked
//! things were blocked, that allowed things were allowed (the `none` mode is the control that
//! proves the tests mean something), and that the pixels came back correct.

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod os;
#[cfg(windows)]
#[path = "windows.rs"]
mod os;

/// Bytes of "decoded pixels" the worker writes to shared memory.
pub const PIXELS: usize = 4096;

/// What the harness asks a back end to run: the worker's arguments and its input file (which the
/// worker receives as an inherited handle, never as a path).
pub struct Job<'a> {
    pub args: Vec<String>,
    pub input_path: &'a str,
}

/// What a mode is expected to block ("blocked") or allow ("allowed"); "any" is not checked.
#[derive(Clone, Copy)]
pub struct Expect {
    pub file: &'static str,
    pub tcp: &'static str,
    pub spawn: &'static str,
    pub alloc: &'static str,
}

/// The "decoded image" both sides derive from the input bytes.
pub fn expected_pixels(input: &[u8]) -> Vec<u8> {
    (0..PIXELS).map(|i| input[i % input.len().max(1)] ^ 0x5A).collect()
}

fn t_file() -> String {
    // Windows: a file only the current user can read (created by the parent); elsewhere /etc/passwd
    let path = std::env::var("AUTOCROP_SECRET").unwrap_or_else(|_| "/etc/passwd".to_owned());
    match std::fs::File::open(&path) {
        Ok(_) => format!("allowed ({path})"),
        Err(e) => format!("blocked({:?})", e.kind()),
    }
}

fn t_tcp(port: u16) -> String {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    match TcpStream::connect_timeout(&addr, Duration::from_millis(1500)) {
        Ok(_) => "allowed".into(),
        Err(e) => format!("blocked({:?})", e.kind()),
    }
}

fn t_spawn() -> String {
    let (prog, args): (&str, &[&str]) = if cfg!(windows) {
        ("C:\\Windows\\System32\\cmd.exe", &["/C", "exit", "0"])
    } else if cfg!(target_os = "macos") {
        ("/usr/bin/true", &[])
    } else {
        ("/bin/true", &[])
    };
    match Command::new(prog).args(args).stdin(Stdio::null()).stdout(Stdio::null()).status() {
        Ok(_) => "allowed".into(),
        Err(e) => format!("blocked({:?})", e.kind()),
    }
}

fn t_alloc() -> String {
    let mut v: Vec<u8> = Vec::new();
    match v.try_reserve_exact(1 << 30) {
        Ok(()) => {
            // touch it so a commit limit (not just address space) is exercised
            v.resize(1 << 30, 1);
            "allowed".into()
        }
        Err(_) => "blocked(alloc)".into(),
    }
}

fn worker(mode: &str, port: u16) {
    let info = match os::apply(mode) {
        Ok(i) => i,
        Err(e) => {
            println!("APPLY_FAILED {e}");
            std::process::exit(3);
        }
    };
    println!("LEVEL {info}");
    println!("TEST file {}", t_file());
    println!("TEST tcp {}", t_tcp(port));
    println!("TEST spawn {}", t_spawn());
    println!("TEST alloc {}", t_alloc());
    // the real work: input via the inherited handle, pixels out via shared memory
    let mut input = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut input) {
        println!("WORK fail read-input {e}");
        std::process::exit(4);
    }
    match os::write_shared(&expected_pixels(&input)) {
        Ok(()) => println!("WORK ok input={}", input.len()),
        Err(e) => println!("WORK fail shared-memory {e}"),
    }
}

fn check(name: &str, got: &str, want: &str, problems: &mut Vec<String>) {
    let ok = want == "any" || got.starts_with(want);
    if !ok {
        problems.push(format!("{name}: expected {want}, got {got}"));
    }
}

fn parent(input_path: &str) -> i32 {
    let input = std::fs::read(input_path).expect("input file");
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            drop(s);
        }
    });
    let want_pixels = expected_pixels(&input);
    let mut failures = 0;
    println!("os={} arch={}", std::env::consts::OS, std::env::consts::ARCH);
    for mode in os::modes() {
        let shm = os::shared_create(PIXELS).expect("shared memory");
        let job = Job { args: vec!["--worker".into(), mode.to_string(), port.to_string()], input_path };
        let out = match os::run(&job, &shm, mode) {
            Ok(o) => o,
            Err(e) => {
                println!("[{mode}] FAILED to run worker: {e}");
                failures += 1;
                continue;
            }
        };
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let get = |key: &str| text.lines().find_map(|l| l.strip_prefix(key).map(|s| s.trim().to_owned())).unwrap_or_else(|| "missing".into());
        let (level, file, tcp, spawn, alloc, work) = (get("LEVEL "), get("TEST file "), get("TEST tcp "), get("TEST spawn "), get("TEST alloc "), get("WORK "));
        let exp = os::expect(mode, &level);
        let mut problems = Vec::new();
        check("file", &file, exp.file, &mut problems);
        check("tcp", &tcp, exp.tcp, &mut problems);
        check("spawn", &spawn, exp.spawn, &mut problems);
        check("alloc", &alloc, exp.alloc, &mut problems);
        if !work.starts_with("ok") {
            problems.push(format!("work: {work} (status {:?}, stderr: {})", out.status.code(), String::from_utf8_lossy(&out.stderr).trim()));
        } else if os::shared_read(&shm) != want_pixels {
            problems.push("work: pixels from shared memory are wrong".into());
        }
        let verdict = if problems.is_empty() { "PASS" } else { "FAIL" };
        println!("[{mode}] {verdict}  level={level} | file={file} tcp={tcp} spawn={spawn} alloc={alloc} work={work}");
        for p in &problems {
            println!("    - {p}");
        }
        if !problems.is_empty() {
            failures += 1;
        }
    }
    os::cleanup();
    failures
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--worker") {
        let port = args.get(3).and_then(|p| p.parse().ok()).unwrap_or(0);
        worker(&args[2], port);
    } else {
        let input = args.get(1).cloned().unwrap_or_else(|| "Cargo.toml".to_owned());
        let failures = parent(&input);
        std::process::exit(if failures == 0 { 0 } else { 1 });
    }
}
