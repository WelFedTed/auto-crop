// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask synth`, `synth-setup`, `synth-check` and `check-splits` (ROADMAP M1.30-M1.35).
//!
//! * `synth --suite smoke|full [--generator python|rust] ...` writes a suite under
//!   `target/synth/<suite>` (never committed). The default generator is the Python tool in
//!   `tools/synth` (Augraphy behind a seam, known-text pages, pinhole camera, EXIF and colour-space
//!   variants); `--generator rust` selects the old STAND-IN writer in `auto-crop-eval`, kept as a
//!   fallback for machines without Python and for the accuracy-smoke gate until it moves.
//! * `synth-setup` creates `target/synth-venv` and installs the hash-locked requirements.
//! * `synth-check [tests|geometry|variants|ocr|all]` runs the generator's own acceptance checks.
//! * `check-splits [MANIFEST...]` fails when a scene or seed spans two splits.
//!
//! The Python interpreter is `$AUTO_CROP_SYNTH_PYTHON`, else the virtual environment made by
//! `synth-setup`. Nothing is installed implicitly.

use std::path::{Path, PathBuf};
use std::process::Command;

const VENV: &str = "target/synth-venv";
const LOCK: &str = "tools/synth/requirements.lock";

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Generator {
    Python,
    Rust,
}

/// Splits `--generator X` out of the arguments.
pub fn split_generator(args: &[String]) -> Result<(Generator, Vec<String>), String> {
    let mut generator = Generator::Python;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--generator" {
            generator = match it.next().map(String::as_str) {
                Some("python") => Generator::Python,
                Some("rust") => Generator::Rust,
                Some(other) => return Err(format!("unknown generator `{other}` (python|rust)")),
                None => return Err("--generator needs python or rust".to_owned()),
            };
        } else {
            rest.push(a.clone());
        }
    }
    Ok((generator, rest))
}

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
}

/// Arguments for the chosen generator: `--suite` is required, `--out` defaults to
/// `target/synth/<suite>` (the Rust stand-in gets `<suite>-rust` so the two never mix).
pub fn generator_args(generator: Generator, args: &[String]) -> Result<Vec<String>, String> {
    let suite = value_of(args, "--suite").ok_or("synth needs --suite smoke|full")?;
    let mut out: Vec<String> = args.to_vec();
    if value_of(args, "--out").is_none() {
        out.push("--out".to_owned());
        out.push(match generator {
            Generator::Python => format!("target/synth/{suite}"),
            Generator::Rust => format!("target/synth/{suite}-rust"),
        });
    }
    Ok(out)
}

fn venv_python() -> PathBuf {
    let rel = if cfg!(windows) {
        "Scripts/python.exe"
    } else {
        "bin/python"
    };
    Path::new(VENV).join(rel)
}

/// The interpreter that runs the generator.
pub fn find_python() -> Result<String, String> {
    if let Ok(p) = std::env::var("AUTO_CROP_SYNTH_PYTHON")
        && !p.is_empty()
    {
        return Ok(p);
    }
    let v = venv_python();
    if v.exists() {
        // Absolute, so a command that also changes directory (the unit tests run in tools/synth)
        // still finds it: a relative program path is resolved against the new directory on Unix.
        let abs = std::env::current_dir()
            .map_err(|e| format!("cannot read the current directory: {e}"))?
            .join(v);
        return Ok(abs.to_string_lossy().into_owned());
    }
    Err(format!(
        "no Python environment for tools/synth: run `cargo xtask synth-setup` (creates {VENV} from {LOCK}) or set AUTO_CROP_SYNTH_PYTHON"
    ))
}

/// A system Python 3.12 or newer to build the environment from.
fn system_python() -> Result<String, String> {
    for cand in ["python3", "python", "py"] {
        let Ok(o) = Command::new(cand).arg("--version").output() else {
            continue;
        };
        let text =
            String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
        if let Some(v) = text.trim().strip_prefix("Python ") {
            let mut parts = v.split('.').filter_map(|p| p.parse::<u32>().ok());
            if let (Some(3), Some(minor)) = (parts.next(), parts.next())
                && minor >= 12
            {
                return Ok(cand.to_owned());
            }
        }
    }
    Err("Python 3.12 or newer not found (tools/synth needs it: numpy 2.5)".to_owned())
}

fn run(cmd: &mut Command, what: &str) -> Result<(), String> {
    let status = cmd
        .status()
        .map_err(|e| format!("cannot run {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} exited with {status}"))
    }
}

fn python_module(args: &[&str]) -> Result<(), String> {
    let py = find_python()?;
    let mut c = Command::new(&py);
    c.env("PYTHONPATH", "tools/synth")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .args(["-m", "synth"])
        .args(args);
    run(&mut c, &format!("{py} -m synth {}", args.join(" ")))
}

/// `cargo xtask synth-setup`: builds the virtual environment from the hashed lock.
pub fn run_setup(_args: &[String]) -> Result<(), String> {
    if !Path::new(LOCK).exists() {
        return Err(format!("{LOCK} not found (run from the repository root)"));
    }
    if !venv_python().exists() {
        let sys = system_python()?;
        run(
            Command::new(&sys).args(["-m", "venv", VENV]),
            "python -m venv",
        )?;
    }
    let py = venv_python();
    run(
        Command::new(&py).args([
            "-m",
            "pip",
            "install",
            "--require-hashes",
            "--no-deps",
            "-r",
            LOCK,
        ]),
        "pip install --require-hashes",
    )?;
    println!("tools/synth environment ready in {VENV} (hashes verified)");
    Ok(())
}

/// `cargo xtask synth --suite smoke|full [--generator python|rust] ...`.
pub fn run_synth(args: &[String]) -> Result<(), String> {
    let (generator, rest) = split_generator(args)?;
    let full = generator_args(generator, &rest)?;
    match generator {
        Generator::Rust => {
            let mut a = vec!["synth".to_owned()];
            a.extend(full);
            crate::eval::harness(&a)
        }
        Generator::Python => {
            let refs: Vec<&str> = full.iter().map(String::as_str).collect();
            python_module(&refs)
        }
    }
}

/// `cargo xtask check-splits [MANIFEST...]`.
pub fn run_check_splits(args: &[String]) -> Result<(), String> {
    let mut paths: Vec<String> = args.to_vec();
    if paths.is_empty() {
        if let Ok(rd) = std::fs::read_dir("target/synth") {
            let mut found: Vec<_> = rd
                .filter_map(Result::ok)
                .map(|d| d.path().join("manifest.jsonl"))
                .filter(|p| p.exists())
                .collect();
            found.sort();
            paths = found
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect();
        }
        if paths.is_empty() {
            return Err("no manifests: pass paths, or generate suites first (cargo xtask synth --suite smoke)".to_owned());
        }
    }
    let refs: Vec<&Path> = paths.iter().map(Path::new).collect();
    let violations = auto_crop_eval::splits::check_files(&refs)?;
    if violations.is_empty() {
        println!(
            "check-splits: {} manifest(s), no scene or seed spans two splits",
            paths.len()
        );
        Ok(())
    } else {
        Err(violations
            .into_iter()
            .map(|v| format!("split leak: {v}"))
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// `cargo xtask synth-check [tests|geometry|variants|ocr|all]`.
pub fn run_check(args: &[String]) -> Result<(), String> {
    let which = args.first().map_or("all", String::as_str);
    let extra: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let run_tests = || -> Result<(), String> {
        let py = find_python()?;
        let mut c = Command::new(&py);
        c.current_dir("tools/synth")
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .args(["-m", "unittest", "discover", "-s", "tests", "-t", ".", "-v"]);
        run(&mut c, "tools/synth unit tests")
    };
    let geometry = || {
        let mut a = vec!["check-geometry"];
        a.extend(extra.iter().copied());
        python_module(&a)
    };
    let variants = || -> Result<(), String> {
        let dir = "target/synth/variants";
        python_module(&["variants", "--out", dir])?;
        crate::eval::harness(&[
            "check-variants".to_owned(),
            "--dir".to_owned(),
            dir.to_owned(),
        ])
    };
    let ocr = || {
        let mut a = vec!["check-ocr"];
        a.extend(extra.iter().copied());
        python_module(&a)
    };
    match which {
        "tests" => run_tests(),
        "geometry" => geometry(),
        "variants" => variants(),
        "ocr" => ocr(),
        // `all` skips OCR when no tesseract is installed (it says so); CI runs `ocr --require`.
        "all" => {
            run_tests()?;
            geometry()?;
            variants()?;
            python_module(&["check-ocr"])
        }
        other => Err(format!(
            "unknown synth-check `{other}` (tests|geometry|variants|ocr|all)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn the_generator_defaults_to_python_and_is_removed_from_the_arguments() {
        let (g, rest) = split_generator(&s(&["--suite", "smoke"])).expect("valid");
        assert_eq!((g, rest), (Generator::Python, s(&["--suite", "smoke"])));
        let (g, rest) =
            split_generator(&s(&["--generator", "rust", "--suite", "smoke"])).expect("valid");
        assert_eq!((g, rest), (Generator::Rust, s(&["--suite", "smoke"])));
        assert!(split_generator(&s(&["--generator", "go"])).is_err());
        assert!(split_generator(&s(&["--generator"])).is_err());
    }

    #[test]
    fn each_generator_gets_its_own_default_output_directory() {
        let a = generator_args(Generator::Python, &s(&["--suite", "smoke"])).expect("valid");
        assert_eq!(a, s(&["--suite", "smoke", "--out", "target/synth/smoke"]));
        let a = generator_args(Generator::Rust, &s(&["--suite", "full"])).expect("valid");
        assert_eq!(
            a,
            s(&["--suite", "full", "--out", "target/synth/full-rust"])
        );
    }

    #[test]
    fn an_explicit_output_is_kept_and_a_suite_is_required() {
        let a = generator_args(
            Generator::Python,
            &s(&["--suite", "full", "--out", "x", "--jobs", "4"]),
        )
        .expect("valid");
        assert_eq!(a, s(&["--suite", "full", "--out", "x", "--jobs", "4"]));
        assert!(generator_args(Generator::Rust, &s(&["--out", "x"])).is_err());
    }
}
