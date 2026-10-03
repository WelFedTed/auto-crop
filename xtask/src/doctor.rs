// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask doctor [--strict]` (ROADMAP M0.11, M1.79).
//!
//! Checks the native build toolchain and prints the install command for each
//! missing tool (winget, brew or apt). "No C toolchain needed" is false for this
//! project: libheif, libde265, libjpeg-turbo and others are built with CMake.
//!
//! From M1 it also checks the *dev tools* the oracles and memory profiling use: Valgrind (Linux
//! only), Python 3.12+ (the hashed oracle lock needs it), Tesseract 5.x, ImageMagick and
//! unpaper. A missing build tool fails `doctor`; a missing dev tool is reported with its install
//! command and fails only under `--strict` (the devcontainer and Linux CI use that). `doctor`
//! never installs anything.

use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Linux,
}

impl Os {
    pub fn current() -> Self {
        match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::Mac,
            _ => Os::Linux,
        }
    }
}

/// Whether a missing tool fails `doctor` or is only reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
    /// The native build toolchain: missing means the workspace cannot be built.
    Build,
    /// Dev tools for oracles, profiling and tests (M1.79): reported, fatal only with `--strict`.
    Dev,
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    /// Command and arguments that print a version when the tool is installed.
    pub probe: &'static [&'static str],
    /// Further commands to try when `probe` fails (other spellings of the same tool).
    pub alt_probes: &'static [&'static [&'static str]],
    /// Only needed on x86 / x86_64 hosts (NASM).
    pub x86_only: bool,
    /// Only exists on Linux (Valgrind).
    pub linux_only: bool,
    pub need: Need,
    /// Minimum (major, minor) version parsed from the first output line.
    pub min_version: Option<(u32, u32)>,
}

const fn build_tool(name: &'static str, probe: &'static [&'static str], x86_only: bool) -> Tool {
    Tool {
        name,
        probe,
        alt_probes: &[],
        x86_only,
        linux_only: false,
        need: Need::Build,
        min_version: None,
    }
}

const fn dev_tool(
    name: &'static str,
    probe: &'static [&'static str],
    alt_probes: &'static [&'static [&'static str]],
    min_version: Option<(u32, u32)>,
) -> Tool {
    Tool {
        name,
        probe,
        alt_probes,
        x86_only: false,
        linux_only: false,
        need: Need::Dev,
        min_version,
    }
}

pub const TOOLS: &[Tool] = &[
    build_tool("git", &["git", "--version"], false),
    build_tool("cmake", &["cmake", "--version"], false),
    build_tool("ninja", &["ninja", "--version"], false),
    // Builds dav1d (AVIF) in `cargo xtask build-native`.
    build_tool("meson", &["meson", "--version"], false),
    build_tool("nasm", &["nasm", "-v"], true),
    build_tool("node", &["node", "--version"], false),
    // Dev tools (M1.79).
    Tool {
        linux_only: true,
        ..dev_tool("valgrind", &["valgrind", "--version"], &[], None)
    },
    dev_tool(
        "python3",
        &["python3", "--version"],
        &[&["python", "--version"], &["py", "-3", "--version"]],
        Some((3, 12)),
    ),
    dev_tool("tesseract", &["tesseract", "--version"], &[], Some((5, 0))),
    // `magick` is ImageMagick 7; `convert` is ImageMagick 6 on Linux. On Windows `convert` is an
    // unrelated system tool, so it is never probed there (see `probes_for`).
    dev_tool(
        "imagemagick",
        &["magick", "-version"],
        &[&["convert", "-version"]],
        None,
    ),
    dev_tool("unpaper", &["unpaper", "--version"], &[], None),
];

/// The probe commands to try for `tool` on `os`, in order.
pub fn probes_for(tool: &Tool, os: Os) -> Vec<&'static [&'static str]> {
    let mut v = vec![tool.probe];
    for alt in tool.alt_probes {
        // Windows ships an unrelated `convert.exe` (a disk converter): never run it as ImageMagick.
        if os == Os::Windows && alt[0] == "convert" {
            continue;
        }
        v.push(*alt);
    }
    v
}

/// First `major.minor` found in a version line ("tesseract v5.4.0", "Python 3.12.3",
/// "Version: ImageMagick 7.1.2-8").
pub fn parse_version(line: &str) -> Option<(u32, u32)> {
    for tok in line.split(|c: char| c.is_whitespace() || c == ',' || c == '(') {
        let tok = tok.trim_start_matches(['v', 'V']);
        let mut parts = tok.split('.');
        let (Some(a), Some(b)) = (parts.next(), parts.next()) else {
            continue;
        };
        let digits = |s: &str| {
            let d: String = s.chars().take_while(char::is_ascii_digit).collect();
            d.parse::<u32>().ok()
        };
        if let (Some(a), Some(b)) = (digits(a), digits(b)) {
            return Some((a, b));
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Found(String),
    Missing,
    /// Found, but older than the minimum version (or the version could not be read).
    TooOld(String),
    NotApplicable(String),
}

impl Status {
    pub fn is_ok(&self) -> bool {
        matches!(self, Status::Found(_) | Status::NotApplicable(_))
    }
}

/// Judges the first output line of a probe against the tool's minimum version.
pub fn judge(tool: &Tool, line: Option<String>) -> Status {
    let Some(line) = line else {
        return Status::Missing;
    };
    if let Some(min) = tool.min_version {
        match parse_version(&line) {
            Some(v) if v >= min => {}
            _ => return Status::TooOld(line),
        }
    }
    Status::Found(line)
}

/// Install command for a tool on an OS (empty if unknown).
pub fn hint(os: Os, tool: &str) -> &'static str {
    match (os, tool) {
        (Os::Windows, "git") => "winget install --id Git.Git -e",
        (Os::Windows, "cmake") => "winget install --id Kitware.CMake -e",
        (Os::Windows, "ninja") => "winget install --id Ninja-build.Ninja -e",
        (Os::Windows, "meson") => "python -m pip install meson ninja",
        (Os::Windows, "nasm") => "winget install --id NASM.NASM -e",
        (Os::Windows, "node") => "winget install --id OpenJS.NodeJS.LTS -e",
        (Os::Windows, "c-toolchain") => {
            "winget install --id Microsoft.VisualStudio.2022.BuildTools -e (select the C++ build tools workload)"
        }
        (Os::Windows, "python3") => "winget install --id Python.Python.3.12 -e",
        (Os::Windows, "tesseract") => "winget install --id UB-Mannheim.TesseractOCR -e",
        (Os::Windows, "imagemagick") => "winget install --id ImageMagick.ImageMagick -e",
        (Os::Windows, "unpaper") => {
            "no Windows build: run the unpaper comparisons in the devcontainer or WSL (sudo apt install unpaper)"
        }
        (Os::Windows | Os::Mac, "valgrind") => {
            "Linux only: use the devcontainer or WSL (sudo apt install valgrind)"
        }
        (Os::Mac, "c-toolchain") => "xcode-select --install",
        (Os::Mac, "git") => "brew install git",
        (Os::Mac, "cmake") => "brew install cmake",
        (Os::Mac, "ninja") => "brew install ninja",
        (Os::Mac, "meson") => "brew install meson",
        (Os::Mac, "nasm") => "brew install nasm",
        (Os::Mac, "node") => "brew install node",
        (Os::Mac, "python3") => "brew install python@3.12",
        (Os::Mac, "tesseract") => "brew install tesseract",
        (Os::Mac, "imagemagick") => "brew install imagemagick",
        (Os::Mac, "unpaper") => "brew install unpaper",
        (Os::Linux, "c-toolchain") => "sudo apt install build-essential",
        (Os::Linux, "git") => "sudo apt install git",
        (Os::Linux, "cmake") => "sudo apt install cmake",
        (Os::Linux, "ninja") => "sudo apt install ninja-build",
        (Os::Linux, "meson") => "sudo apt install meson",
        (Os::Linux, "nasm") => "sudo apt install nasm",
        (Os::Linux, "node") => {
            "install Node.js LTS (https://nodejs.org or your package manager's current LTS)"
        }
        (Os::Linux, "valgrind") => "sudo apt install valgrind",
        (Os::Linux, "python3") => {
            "sudo apt install python3 python3-pip python3-venv (3.12 or newer is needed: Ubuntu 22.04 ships 3.10, so use the devcontainer, deadsnakes or `uv python install 3.12`)"
        }
        (Os::Linux, "tesseract") => {
            "sudo apt install tesseract-ocr tesseract-ocr-eng (5.x is needed: Ubuntu 22.04 ships 4.1, so add ppa:alex-p/tesseract-ocr5 first, or use the devcontainer)"
        }
        (Os::Linux, "imagemagick") => "sudo apt install imagemagick",
        (Os::Linux, "unpaper") => "sudo apt install unpaper",
        _ => "",
    }
}

fn probe(cmd: &[&str]) -> Option<String> {
    let out = Command::new(cmd[0])
        .args(&cmd[1..])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let first = s.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        // A few tools print their version on stderr (older Tesseract, some Python builds).
        let e = String::from_utf8_lossy(&out.stderr);
        let first = e.lines().next().unwrap_or("").trim();
        return if first.is_empty() {
            None
        } else {
            Some(first.to_owned())
        };
    }
    Some(first.to_owned())
}

fn c_toolchain(os: Os) -> Option<String> {
    match os {
        Os::Windows => {
            let vswhere =
                "C:\\Program Files (x86)\\Microsoft Visual Studio\\Installer\\vswhere.exe";
            let out = Command::new(vswhere)
                .args([
                    "-products",
                    "*",
                    "-requires",
                    "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
                    "-property",
                    "installationPath",
                ])
                .output()
                .ok()?;
            let path = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            if path.is_empty() {
                None
            } else {
                Some(format!("MSVC build tools at {path}"))
            }
        }
        Os::Mac => probe(&["xcode-select", "-p"]).map(|p| format!("Xcode CLT at {p}")),
        Os::Linux => probe(&["cc", "--version"]),
    }
}

/// Names of missing tools given probe results (pure, for tests).
pub fn missing<'a>(results: &[(&'a str, bool)]) -> Vec<&'a str> {
    results
        .iter()
        .filter(|(_, ok)| !ok)
        .map(|(n, _)| *n)
        .collect()
}

/// Build tools must all be present; dev tools only matter under `--strict`.
pub fn passes(miss_build: &[&str], miss_dev: &[&str], strict: bool) -> bool {
    miss_build.is_empty() && (miss_dev.is_empty() || !strict)
}

fn check(tool: &Tool, os: Os, x86: bool) -> Status {
    if tool.x86_only && !x86 {
        return Status::NotApplicable(format!("not needed on {}", std::env::consts::ARCH));
    }
    if tool.linux_only && os != Os::Linux {
        return Status::NotApplicable("Linux only".to_owned());
    }
    let mut best: Option<Status> = None;
    for p in probes_for(tool, os) {
        let st = judge(tool, probe(p));
        match st {
            Status::Found(_) => return st,
            Status::TooOld(_) => best = Some(st),
            _ => {}
        }
    }
    best.unwrap_or(Status::Missing)
}

pub fn run(args: &[String]) -> Result<(), String> {
    let strict = args.iter().any(|a| a == "--strict");
    if let Some(bad) = args.iter().find(|a| a.as_str() != "--strict") {
        return Err(format!(
            "unknown doctor argument: {bad}\nusage: cargo xtask doctor [--strict]"
        ));
    }
    let os = Os::current();
    let x86 = matches!(std::env::consts::ARCH, "x86" | "x86_64");
    let mut build_results: Vec<(&str, bool)> = Vec::new();
    let mut dev_results: Vec<(&str, bool)> = Vec::new();
    let tc = c_toolchain(os);
    println!(
        "{:<12} {}",
        "c-toolchain",
        tc.as_deref().unwrap_or("MISSING")
    );
    build_results.push(("c-toolchain", tc.is_some()));
    for t in TOOLS {
        let st = check(t, os, x86);
        let label = if t.need == Need::Dev {
            " (dev tool)"
        } else {
            ""
        };
        match &st {
            Status::Found(v) => println!("{:<12} {v}{label}", t.name),
            Status::Missing => println!("{:<12} MISSING{label}", t.name),
            Status::TooOld(v) => {
                let (a, b) = t.min_version.unwrap_or((0, 0));
                println!(
                    "{:<12} TOO OLD or unreadable ({v}); need {a}.{b} or newer{label}",
                    t.name
                );
            }
            Status::NotApplicable(why) => println!("{:<12} {why}{label}", t.name),
        }
        let list = if t.need == Need::Dev {
            &mut dev_results
        } else {
            &mut build_results
        };
        list.push((t.name, st.is_ok()));
    }
    let miss_build = missing(&build_results);
    let miss_dev = missing(&dev_results);
    let mut msg = String::new();
    for (title, list) in [("build tools", &miss_build), ("dev tools", &miss_dev)] {
        if !list.is_empty() {
            msg.push_str(&format!(
                "missing or too old ({title}): {}\nInstall with:",
                list.join(", ")
            ));
            for m in list {
                msg.push_str(&format!("\n  {m}: {}", hint(os, m)));
            }
            msg.push('\n');
        }
    }
    if passes(&miss_build, &miss_dev, strict) {
        if miss_dev.is_empty() {
            println!("doctor: toolchain complete");
        } else {
            println!(
                "doctor: build toolchain complete; some dev tools are missing (not fatal without --strict)\n{msg}"
            );
        }
        Ok(())
    } else {
        Err(msg.trim_end().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str) -> &'static Tool {
        TOOLS.iter().find(|t| t.name == name).unwrap()
    }

    #[test]
    fn missing_lists_only_failures() {
        assert_eq!(
            missing(&[("a", true), ("b", false), ("c", false)]),
            vec!["b", "c"]
        );
    }

    #[test]
    fn every_tool_has_a_hint_on_every_os() {
        for os in [Os::Windows, Os::Mac, Os::Linux] {
            for t in TOOLS {
                assert!(!hint(os, t.name).is_empty(), "{os:?} {}", t.name);
            }
            assert!(!hint(os, "c-toolchain").is_empty());
        }
    }

    #[test]
    fn git_is_always_probeable_in_dev_environments() {
        // This repository is a git checkout, so git must be present when tests run.
        assert!(probe(&["git", "--version"]).is_some());
    }

    #[test]
    fn the_m1_dev_tools_are_listed() {
        for n in ["valgrind", "python3", "tesseract", "imagemagick", "unpaper"] {
            assert_eq!(tool(n).need, Need::Dev, "{n}");
        }
        assert!(tool("valgrind").linux_only);
        assert_eq!(tool("tesseract").min_version, Some((5, 0)));
        assert_eq!(tool("python3").min_version, Some((3, 12)));
        for n in ["git", "cmake", "ninja", "meson", "nasm", "node"] {
            assert_eq!(tool(n).need, Need::Build, "{n}");
        }
    }

    #[test]
    fn versions_are_parsed_from_real_banners() {
        assert_eq!(parse_version("tesseract 5.3.4"), Some((5, 3)));
        assert_eq!(parse_version("tesseract v5.4.0.20240606"), Some((5, 4)));
        assert_eq!(parse_version("tesseract 4.1.1"), Some((4, 1)));
        assert_eq!(parse_version("Python 3.12.3"), Some((3, 12)));
        assert_eq!(
            parse_version("Version: ImageMagick 7.1.2-8 Q16-HDRI x64"),
            Some((7, 1))
        );
        assert_eq!(parse_version("valgrind-3.18.1"), None);
        assert_eq!(parse_version("no digits here"), None);
    }

    #[test]
    fn tesseract_4_is_too_old_and_5_passes() {
        let t = tool("tesseract");
        assert_eq!(judge(t, None), Status::Missing);
        assert!(matches!(
            judge(t, Some("tesseract 4.1.1".into())),
            Status::TooOld(_)
        ));
        assert!(matches!(
            judge(t, Some("tesseract 5.3.4".into())),
            Status::Found(_)
        ));
        assert!(matches!(
            judge(t, Some("tesseract".into())),
            Status::TooOld(_)
        ));
    }

    #[test]
    fn python_3_10_is_too_old_for_the_oracle_lock() {
        let t = tool("python3");
        assert!(matches!(
            judge(t, Some("Python 3.10.12".into())),
            Status::TooOld(_)
        ));
        assert!(matches!(
            judge(t, Some("Python 3.14.7".into())),
            Status::Found(_)
        ));
    }

    #[test]
    fn tools_without_a_minimum_accept_any_output() {
        assert!(matches!(
            judge(tool("unpaper"), Some("7.0.0".into())),
            Status::Found(_)
        ));
    }

    #[test]
    fn windows_never_probes_the_system_convert() {
        let im = tool("imagemagick");
        let win = probes_for(im, Os::Windows);
        assert!(win.iter().all(|p| p[0] != "convert"), "{win:?}");
        let lin = probes_for(im, Os::Linux);
        assert!(lin.iter().any(|p| p[0] == "convert"), "{lin:?}");
    }

    #[test]
    fn valgrind_is_not_applicable_off_linux() {
        let st = check(tool("valgrind"), Os::Windows, true);
        assert!(matches!(st, Status::NotApplicable(_)), "{st:?}");
    }

    #[test]
    fn missing_dev_tools_fail_only_under_strict() {
        assert!(passes(&[], &[], true));
        assert!(passes(&[], &["tesseract"], false));
        assert!(!passes(&[], &["tesseract"], true));
        assert!(!passes(&["cmake"], &[], false));
        assert!(!passes(&["cmake"], &["unpaper"], false));
    }

    #[test]
    fn unknown_argument_is_rejected() {
        assert!(run(&["--bogus".to_owned()]).is_err());
    }
}
