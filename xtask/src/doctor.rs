// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask doctor` (ROADMAP M0.11).
//!
//! Checks the native build toolchain and prints the install command for each
//! missing tool (winget, brew or apt). "No C toolchain needed" is false for this
//! project: libheif, libde265, libjpeg-turbo and others are built with CMake.

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

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    /// Command and arguments that print a version when the tool is installed.
    pub probe: &'static [&'static str],
    /// Only needed on x86 / x86_64 hosts (NASM).
    pub x86_only: bool,
}

pub const TOOLS: &[Tool] = &[
    Tool {
        name: "git",
        probe: &["git", "--version"],
        x86_only: false,
    },
    Tool {
        name: "cmake",
        probe: &["cmake", "--version"],
        x86_only: false,
    },
    Tool {
        name: "ninja",
        probe: &["ninja", "--version"],
        x86_only: false,
    },
    Tool {
        name: "nasm",
        probe: &["nasm", "-v"],
        x86_only: true,
    },
    Tool {
        name: "node",
        probe: &["node", "--version"],
        x86_only: false,
    },
];

/// Install command for a tool on an OS (empty if unknown).
pub fn hint(os: Os, tool: &str) -> &'static str {
    match (os, tool) {
        (Os::Windows, "git") => "winget install --id Git.Git -e",
        (Os::Windows, "cmake") => "winget install --id Kitware.CMake -e",
        (Os::Windows, "ninja") => "winget install --id Ninja-build.Ninja -e",
        (Os::Windows, "nasm") => "winget install --id NASM.NASM -e",
        (Os::Windows, "node") => "winget install --id OpenJS.NodeJS.LTS -e",
        (Os::Windows, "c-toolchain") => {
            "winget install --id Microsoft.VisualStudio.2022.BuildTools -e (select the C++ build tools workload)"
        }
        (Os::Mac, "c-toolchain") => "xcode-select --install",
        (Os::Mac, t) if ["git", "cmake", "ninja", "nasm", "node"].contains(&t) => match t {
            "git" => "brew install git",
            "cmake" => "brew install cmake",
            "ninja" => "brew install ninja",
            "nasm" => "brew install nasm",
            _ => "brew install node",
        },
        (Os::Linux, "c-toolchain") => "sudo apt install build-essential",
        (Os::Linux, "git") => "sudo apt install git",
        (Os::Linux, "cmake") => "sudo apt install cmake",
        (Os::Linux, "ninja") => "sudo apt install ninja-build",
        (Os::Linux, "nasm") => "sudo apt install nasm",
        (Os::Linux, "node") => {
            "install Node.js LTS (https://nodejs.org or your package manager's current LTS)"
        }
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
    Some(s.lines().next().unwrap_or("").trim().to_owned())
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

pub fn run(_args: &[String]) -> Result<(), String> {
    let os = Os::current();
    let x86 = matches!(std::env::consts::ARCH, "x86" | "x86_64");
    let mut results: Vec<(&str, bool)> = Vec::new();
    let tc = c_toolchain(os);
    println!(
        "{:<12} {}",
        "c-toolchain",
        tc.as_deref().unwrap_or("MISSING")
    );
    results.push(("c-toolchain", tc.is_some()));
    for t in TOOLS {
        if t.x86_only && !x86 {
            println!("{:<12} not needed on {}", t.name, std::env::consts::ARCH);
            continue;
        }
        let v = probe(t.probe);
        println!("{:<12} {}", t.name, v.as_deref().unwrap_or("MISSING"));
        results.push((t.name, v.is_some()));
    }
    let miss = missing(&results);
    if miss.is_empty() {
        println!("doctor: toolchain complete");
        Ok(())
    } else {
        let mut msg = format!("missing: {}\nInstall with:", miss.join(", "));
        for m in &miss {
            msg.push_str(&format!("\n  {m}: {}", hint(os, m)));
        }
        Err(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
