// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `cargo xtask make-hostile` (ROADMAP M1.69): writes the hostile-file corpus of
//! `auto_crop_codecs::hostile` (bombs, oversize headers, IFD floods, truncations, cyclic EXIF) and
//! decodes every file in its own subprocess with the default `DecodeLimits`. A file passes when the
//! child exits normally, reports a typed error or a bounded decode, stays inside its time and heap
//! budget, and never reports a caught panic. A hang, an abort or a budget miss fails the run.
//!
//! The child side is `xtask hostile-run <file>`: it prints one `HOSTILE ...` line with timings and
//! the peak heap (from the counting allocator) of the probe and of the decode.

use crate::alloc_count;
use auto_crop_codecs::hostile::{self, Expect};
use auto_crop_codecs::{DecodeLimits, decode_guarded, probe_with};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIB: usize = 1 << 20;
/// A probe never allocates anything proportional to the image or the file (M1.12).
const PROBE_PEAK_MAX: usize = MIB;
/// Reject cases: typed error within 1 s and 64 MiB (PLAN 3.10.1 / 7.7).
const REJECT_MS: u64 = 1000;
const REJECT_MIB: usize = 64;
/// A child that runs this long is a hang.
const HANG: Duration = Duration::from_secs(120);

#[derive(Debug, Default)]
struct Outcome {
    ok: bool,
    code: String,
    probe_ms: u64,
    probe_peak: usize,
    decode_ms: u64,
    decode_peak: usize,
}

/// The child process: decode one file and report.
pub fn run_child(args: &[String]) -> Result<(), String> {
    let path = args.first().ok_or("usage: xtask hostile-run <file>")?;
    let bytes: Arc<[u8]> = std::fs::read(path)
        .map_err(|e| format!("{path}: {e}"))?
        .into();
    let limits = DecodeLimits::default();

    alloc_count::reset_peak();
    let base = alloc_count::live();
    let t = Instant::now();
    let _ = probe_with(&bytes, &limits);
    let probe_ms = t.elapsed().as_millis();
    let probe_peak = alloc_count::peak().saturating_sub(base);

    alloc_count::reset_peak();
    let base = alloc_count::live();
    let t = Instant::now();
    let r = decode_guarded(bytes.clone(), &limits);
    let decode_ms = t.elapsed().as_millis();
    let decode_peak = alloc_count::peak().saturating_sub(base);
    let (ok, code) = match &r {
        Ok(_) => (true, "ok".to_owned()),
        Err(e) => (false, e.code().to_owned()),
    };
    println!(
        "HOSTILE ok={ok} code={code} probe_ms={probe_ms} probe_peak={probe_peak} \
         decode_ms={decode_ms} decode_peak={decode_peak}"
    );
    Ok(())
}

fn parse_outcome(stdout: &str) -> Option<Outcome> {
    let line = stdout.lines().find(|l| l.starts_with("HOSTILE "))?;
    let mut o = Outcome::default();
    for kv in line["HOSTILE ".len()..].split_whitespace() {
        let (k, v) = kv.split_once('=')?;
        match k {
            "ok" => o.ok = v == "true",
            "code" => o.code = v.to_owned(),
            "probe_ms" => o.probe_ms = v.parse().ok()?,
            "probe_peak" => o.probe_peak = v.parse().ok()?,
            "decode_ms" => o.decode_ms = v.parse().ok()?,
            "decode_peak" => o.decode_peak = v.parse().ok()?,
            _ => {}
        }
    }
    Some(o)
}

/// Runs the child on `file`; `Err` describes an abort, crash or hang.
fn run_in_subprocess(file: &Path) -> Result<Outcome, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut child = Command::new(exe)
        .arg("hostile-run")
        .arg(file)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start the child: {e}"))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(s) => break s,
            None if started.elapsed() > HANG => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("HANG: still running after {}s", HANG.as_secs()));
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        use std::io::Read;
        let _ = so.read_to_string(&mut out);
    }
    if !status.success() {
        return Err(format!("ABORT or crash: child exited with {status}"));
    }
    parse_outcome(&out).ok_or_else(|| format!("no result line from the child (stdout {out:?})"))
}

fn check(expect: Expect, o: &Outcome) -> Vec<String> {
    let mut bad = Vec::new();
    if o.code == "internal_panic" {
        bad.push("a decoder panicked (caught as InternalPanic)".to_owned());
    }
    if o.code == "decode_timeout" {
        bad.push("unexpected decode timeout".to_owned());
    }
    if o.probe_peak > PROBE_PEAK_MAX {
        bad.push(format!(
            "probe allocated {} KiB (> {} KiB)",
            o.probe_peak / 1024,
            PROBE_PEAK_MAX / 1024
        ));
    }
    match expect {
        Expect::Reject => {
            if o.ok {
                bad.push("was meant to be rejected but decoded".to_owned());
            }
            if o.decode_ms > REJECT_MS {
                bad.push(format!("rejection took {} ms (> {REJECT_MS})", o.decode_ms));
            }
            if o.decode_peak > REJECT_MIB * MIB {
                bad.push(format!(
                    "rejection allocated {} MiB (> {REJECT_MIB})",
                    o.decode_peak / MIB
                ));
            }
        }
        Expect::Bounded { max_ms, max_mib } => {
            if o.decode_ms > max_ms {
                bad.push(format!("took {} ms (> {max_ms})", o.decode_ms));
            }
            if o.decode_peak > max_mib as usize * MIB {
                bad.push(format!(
                    "allocated {} MiB (> {max_mib})",
                    o.decode_peak / MIB
                ));
            }
        }
    }
    bad
}

fn default_out_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.join("target").join("hostile"))
        .unwrap_or_else(|| PathBuf::from("target/hostile"))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut out_dir = default_out_dir();
    let mut only: Option<String> = None;
    let mut generate_only = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = PathBuf::from(it.next().ok_or("--out needs a directory")?),
            "--only" => only = Some(it.next().ok_or("--only needs a substring")?.clone()),
            "--no-run" => generate_only = true,
            other => return Err(format!("unknown make-hostile option: {other}")),
        }
    }
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let t0 = Instant::now();
    let corpus = hostile::corpus();
    println!(
        "make-hostile: {} files generated in {:.1} s into {}",
        corpus.len(),
        t0.elapsed().as_secs_f64(),
        out_dir.display()
    );
    let mut failures = 0usize;
    let mut ran = 0usize;
    println!(
        "{:<44} {:>10} {:<16} {:>7} {:>8}",
        "case", "bytes", "outcome", "ms", "peak MiB"
    );
    for case in &corpus {
        if only
            .as_ref()
            .is_some_and(|f| !case.name.contains(f.as_str()))
        {
            continue;
        }
        let file = out_dir.join(format!("{}.bin", case.name));
        std::fs::write(&file, &case.bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        if generate_only {
            continue;
        }
        ran += 1;
        match run_in_subprocess(&file) {
            Err(why) => {
                failures += 1;
                println!("{:<44} {:>10} FAIL {why}", case.name, case.bytes.len());
            }
            Ok(o) => {
                let bad = check(case.expect, &o);
                println!(
                    "{:<44} {:>10} {:<16} {:>7} {:>8.1}{}",
                    case.name,
                    case.bytes.len(),
                    o.code,
                    o.decode_ms,
                    o.decode_peak as f64 / MIB as f64,
                    if bad.is_empty() { "" } else { "  FAIL" }
                );
                for b in &bad {
                    println!("    -> {b}");
                }
                failures += usize::from(!bad.is_empty());
            }
        }
    }
    if !generate_only {
        println!(
            "make-hostile: {ran} files run, {failures} failed (gate: 0 panics, aborts, hangs, budget misses)"
        );
    }
    if failures > 0 {
        Err(format!("{failures} hostile file(s) failed"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_line_is_parsed() {
        let o = parse_outcome(
            "noise\nHOSTILE ok=false code=too_large probe_ms=0 probe_peak=1200 decode_ms=3 decode_peak=4096\n",
        )
        .unwrap();
        assert!(!o.ok);
        assert_eq!(o.code, "too_large");
        assert_eq!(o.decode_peak, 4096);
        assert!(parse_outcome("nothing").is_none());
    }

    #[test]
    fn the_budget_check_flags_panics_overruns_and_wrong_outcomes() {
        let fine = Outcome {
            ok: false,
            code: "too_large".into(),
            decode_ms: 5,
            decode_peak: 1024,
            ..Outcome::default()
        };
        assert!(check(Expect::Reject, &fine).is_empty());
        let panicked = Outcome {
            code: "internal_panic".into(),
            ..Outcome::default()
        };
        assert!(!check(Expect::Reject, &panicked).is_empty());
        let big = Outcome {
            ok: true,
            code: "ok".into(),
            decode_peak: 80 * MIB,
            ..Outcome::default()
        };
        // Decoded when it should have been rejected, and over the 64 MiB rejection budget.
        assert_eq!(check(Expect::Reject, &big).len(), 2);
        assert!(
            check(
                Expect::Bounded {
                    max_ms: 10,
                    max_mib: 100
                },
                &big
            )
            .is_empty()
        );
        assert!(
            !check(
                Expect::Bounded {
                    max_ms: 10,
                    max_mib: 64
                },
                &big
            )
            .is_empty()
        );
    }
}
