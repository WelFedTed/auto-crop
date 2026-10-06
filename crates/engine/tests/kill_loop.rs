// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The randomised kill loop (ROADMAP M2.58; PLAN 2.7 "kill -9 and TerminateProcess runs"; gate
//! "fault injection"). A child process runs real engine saves (first saves of JPEG and PNG, a
//! re-save, a BMP to PNG conversion) and the parent kills it with `TerminateProcess` / `SIGKILL`
//! at a random instant, starts a new engine on the same data (the start-up recovery) and checks,
//! for every file the child had started on:
//!
//! * the file holds the original bytes, **or** the output is a verified result: it decodes, a
//!   `Saved` backup records exactly its hash, and that backup holds the original bytes;
//! * nothing is left over: no `.autocrop-*.tmp`, no journal, no backup that was never used;
//! * restoring every saved file gives the original bytes back, with its mtime.
//!
//! Each child widens its commit windows with a few random microseconds of sleep at every protocol
//! step (the fault hook), so a kill lands on every step with some probability. The default run is
//! 500 kills (about a minute in a debug build, four workers); `AC_KILL_COUNT` changes it and the
//! `#[ignore]`d nightly test runs 1,000. Power loss (data that was never fsynced) is not modelled
//! (docs/design/one-to-n-safety.md "Open").

use auto_crop_core::Pt;
use auto_crop_engine::group::{Fault, Step, pending_journals, process_started, stray_temps};
use auto_crop_engine::store::{BackupState, Store};
use auto_crop_engine::util::blake3_hex;
use auto_crop_engine::{
    AppPaths, DerivedAction, Edit, Engine, EngineOptions, ItemView, NoSplit, RestoreMode,
    SaveTarget,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

fn nop(_: ItemView) {}

fn old_mtime() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_500_000_000)
}

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

// ---------------------------------------------------------------------------------------------
// The child
// ---------------------------------------------------------------------------------------------

fn jpeg_bytes(seed: u64) -> Vec<u8> {
    let mut s = auto_crop_codecs::fixtures::JpegSpec::new(96, 72);
    s.sampling = (2, 2);
    s.quality = 70 + (seed % 25) as u8;
    s.icc = Some(auto_crop_codecs::fixtures::fake_icc(
        300 + (seed % 7) as usize,
    ));
    s.build()
}

fn png_bytes(seed: u64) -> Vec<u8> {
    auto_crop_codecs::fixtures::png_with_icc(
        64 + (seed % 9) as u32,
        48,
        &auto_crop_codecs::fixtures::fake_icc(200),
    )
}

fn bmp_bytes(seed: u64) -> Vec<u8> {
    let (w, h) = (40 + (seed % 5) as u32, 30);
    auto_crop_codecs::fixtures::bmp_rgb24(
        w,
        h,
        &auto_crop_codecs::fixtures::pattern(w, h),
        seed.is_multiple_of(2),
    )
}

fn set_mtime(p: &Path) {
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(old_mtime())
        .unwrap();
}

fn crop(engine: &Engine, id: u32, aligned: bool, variant: u32) {
    // Aligned: 16..80 x 16..64 of 96 x 72 on the MCU grid (the lossless path); otherwise a skew.
    let (x0, y0, x1, y1) = if variant == 0 {
        (16.0 / 96.0, 16.0 / 72.0, 80.0 / 96.0, 64.0 / 72.0)
    } else {
        (0.1, 0.1, 0.9, 0.9)
    };
    let edit = Edit {
        quad: [
            Pt::new(x0, y0),
            Pt::new(x1, y0),
            Pt::new(x1, y1),
            Pt::new(x0, y1),
        ],
        quarter_turns: 0,
        fine_deg: if aligned { 0.0 } else { 0.4 },
    };
    engine.set_edit(id, &edit, false, "crop").unwrap();
}

/// The worker the parent kills. Not a test of its own: it does nothing unless the parent set the
/// environment.
#[test]
fn child_kill_worker() {
    let Ok(root) = std::env::var("AC_KILL_ROOT") else {
        return;
    };
    let seed: u64 = std::env::var("AC_KILL_SEED").unwrap().parse().unwrap();
    let root = PathBuf::from(root);
    let dir = root.join("photos");
    let orig = root.join("orig");
    let marks = root.join("started");
    let paths = AppPaths::new(root.join("data"), root.join("config"));
    let engine = Engine::new(paths);
    engine.set_item_detector(Arc::new(NoSplit));
    engine.set_options(EngineOptions {
        lossless_jpeg: !seed.is_multiple_of(3),
        ..EngineOptions::default()
    });
    let mut rng = seed | 1;
    println!("AC-READY");
    std::io::stdout().flush().unwrap();
    // Random micro-delays at every protocol step widen the windows a kill can land in.
    let delay_state = std::cell::Cell::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let hook = |_: &Step| {
        let mut s = delay_state.get();
        let us = xorshift(&mut s) % 1500;
        delay_state.set(s);
        std::thread::sleep(Duration::from_micros(us));
        Fault::Pass
    };
    for round in 0u64.. {
        for kind in ["jpg", "png", "bmp"] {
            let name = format!("r{round}_{kind}.{kind}");
            let s = xorshift(&mut rng);
            let bytes = match kind {
                "jpg" => jpeg_bytes(s),
                "png" => png_bytes(s),
                _ => bmp_bytes(s),
            };
            let path = dir.join(&name);
            // Ground truth first (a copy the engine never sees), then the file, then the marker
            // that says the engine may touch it from here on.
            fs::write(orig.join(&name), &bytes).unwrap();
            fs::write(&path, &bytes).unwrap();
            set_mtime(&path);
            fs::write(marks.join(&name), b"").unwrap();
            let opened = engine.open_paths(std::slice::from_ref(&path), false);
            let id = opened.ids[0];
            if engine.analyse(id).unwrap().error.is_some() {
                continue;
            }
            if kind == "bmp" {
                engine.convert_items_with_faults(&[id], "kill", &hook, &nop);
                continue;
            }
            let aligned = kind == "jpg" && s.is_multiple_of(2);
            crop(&engine, id, aligned, 0);
            engine.save_items_with_faults(&[id], SaveTarget::Replace, "kill", &hook);
            if kind == "jpg" && s.is_multiple_of(3) {
                // A re-save: a second edit replaces the first output.
                crop(&engine, id, false, 1);
                engine.save_items_with_faults(&[id], SaveTarget::Replace, "kill", &hook);
            }
        }
        if round > 400 {
            break;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The parent
// ---------------------------------------------------------------------------------------------

#[derive(Default, Debug)]
struct Tally {
    kills: u32,
    alive_when_killed: u32,
    /// Kills that left a journal behind: the child died inside the commit protocol.
    in_protocol: u32,
    files_checked: u32,
    original_intact: u32,
    verified_output: u32,
    converted: u32,
    restored: u32,
    violations: Vec<String>,
}

fn started_names(marks: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(marks)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// One kill: run the child, kill it at a random instant, recover, check.
fn one_run(iter: u64, base: &Path, t: &mut Tally) {
    let root = base.join(format!("run{iter}"));
    for sub in ["photos", "orig", "started", "data", "config"] {
        fs::create_dir_all(root.join(sub)).unwrap();
    }
    let seed = iter.wrapping_mul(0x2545_F491_4F6C_DD1D) ^ 0xA5A5;
    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args([
            "--exact",
            "child_kill_worker",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("AC_KILL_ROOT", &root)
        .env("AC_KILL_SEED", seed.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        if out.read_line(&mut line).unwrap() == 0 {
            break; // the child ended before it was ready
        }
        if line.contains("AC-READY") {
            break;
        }
    }
    // The instant of the kill: anywhere in the first 0..180 ms of work (a debug-build child saves a
    // file in a few tens of milliseconds, so this covers several saves and every protocol step).
    let mut rng = seed | 1;
    let wait = xorshift(&mut rng) % 180_000;
    std::thread::sleep(Duration::from_micros(wait));
    let alive = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    let pid = child.id();
    let _ = child.wait();
    drop(out);
    drop(child);
    // An exited process stays queryable on Windows while a handle is open; wait until it is gone
    // before recovery decides its journal has no owner.
    for _ in 0..400 {
        if process_started(pid).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    t.kills += 1;
    t.alive_when_killed += u32::from(alive);

    // The next start: recovery, then the checks.
    let paths = AppPaths::new(root.join("data"), root.join("config"));
    let store = Store::new(paths.backups_dir());
    t.in_protocol += u32::from(pending_journals(&store) > 0);
    let engine = Engine::new(paths.clone());
    let dir = root.join("photos");
    let mut bad = |msg: String| {
        t.violations
            .push(format!("run {iter} (seed {seed}): {msg}"))
    };

    if !stray_temps(&dir).is_empty() {
        bad(format!("stray temps {:?}", stray_temps(&dir)));
    }
    if pending_journals(&store) != 0 {
        bad("a journal is left".to_owned());
    }
    let manifests = store.list();
    for m in &manifests {
        if m.state == BackupState::BackedUp {
            bad(format!("an unused backup is left ({})", m.original_name));
        }
    }
    let by_hash: BTreeMap<String, Vec<&auto_crop_engine::store::Manifest>> =
        manifests.iter().fold(BTreeMap::new(), |mut acc, m| {
            acc.entry(m.original_blake3.clone()).or_default().push(m);
            acc
        });

    for name in started_names(&root.join("started")) {
        let Ok(orig) = fs::read(root.join("orig").join(&name)) else {
            continue; // killed before the ground truth was complete: nothing started
        };
        t.files_checked += 1;
        let path = dir.join(&name);
        let hash = blake3_hex(&orig);
        let saved: Vec<_> = by_hash.get(&hash).cloned().unwrap_or_default();
        let cur = fs::read(&path).ok();
        let png_twin = path.with_extension("png");
        let is_bmp = name.ends_with(".bmp");
        let backup_holds_original = |m: &auto_crop_engine::store::Manifest| {
            store
                .original_path(m)
                .and_then(|p| fs::read(p).ok())
                .is_some_and(|b| b == orig)
        };
        match &cur {
            Some(c) if *c == orig => {
                t.original_intact += 1;
                // The original is in place: a conversion may also have finished its output
                // (the source could not be removed yet); that output must then be verified.
                if is_bmp && let Ok(png) = fs::read(&png_twin) {
                    let ok = auto_crop_codecs::decode(&png).is_ok()
                        && saved.iter().any(|m| {
                            m.state == BackupState::Saved
                                && m.outputs.iter().any(|o| o.blake3 == blake3_hex(&png))
                        });
                    if !ok {
                        bad(format!(
                            "{name}: the original is there but its PNG is not a verified output"
                        ));
                    }
                }
            }
            Some(c) => {
                // Not the original: it must be a verified output.
                let verified = auto_crop_codecs::decode(c).is_ok()
                    && saved.iter().any(|m| {
                        m.state == BackupState::Saved
                            && backup_holds_original(m)
                            && m.outputs
                                .iter()
                                .any(|o| o.blake3 == blake3_hex(c) && Path::new(&o.path) == path)
                    });
                if verified {
                    t.verified_output += 1;
                } else {
                    bad(format!(
                        "{name}: holds neither the original nor a verified output"
                    ));
                }
            }
            None if is_bmp => {
                // Converted: the PNG is the verified output and the backup holds the BMP.
                match fs::read(&png_twin) {
                    Ok(png) => {
                        let ok = auto_crop_codecs::decode(&png).is_ok()
                            && saved.iter().any(|m| {
                                m.state == BackupState::Saved
                                    && backup_holds_original(m)
                                    && m.outputs.iter().any(|o| o.blake3 == blake3_hex(&png))
                            });
                        if ok {
                            t.converted += 1;
                        } else {
                            bad(format!(
                                "{name}: converted but the PNG is not verified or the backup is wrong"
                            ));
                        }
                    }
                    Err(_) => bad(format!("{name}: neither the BMP nor its PNG exists")),
                }
            }
            None => bad(format!("{name}: the file is gone")),
        }
        // Whatever state it is in, restoring gives the original back, with its mtime.
        if saved.iter().any(|m| m.state == BackupState::Saved) {
            let target = if is_bmp {
                png_twin.clone()
            } else {
                path.clone()
            };
            let r = engine.restore_path(&target, RestoreMode::Auto, DerivedAction::Remove, &nop);
            let back = fs::read(&path).ok();
            let mtime_ok = fs::metadata(&path)
                .and_then(|m| m.modified())
                .is_ok_and(|m| m == old_mtime());
            if r.ok && back.as_deref() == Some(&orig[..]) && mtime_ok {
                t.restored += 1;
            } else {
                bad(format!(
                    "{name}: restore did not return the original: {r:?}"
                ));
            }
        }
    }
    let _ = fs::remove_dir_all(&root);
}

fn kill_loop(total: u64, workers: u64) -> Tally {
    let base = tempfile::tempdir().unwrap();
    let per = total.div_ceil(workers);
    let tallies: Vec<Tally> = std::thread::scope(|s| {
        (0..workers)
            .map(|w| {
                let base = base.path().to_path_buf();
                s.spawn(move || {
                    let mut t = Tally::default();
                    for i in 0..per {
                        one_run(w * per + i, &base, &mut t);
                    }
                    t
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect()
    });
    let mut sum = Tally::default();
    for t in tallies {
        sum.kills += t.kills;
        sum.alive_when_killed += t.alive_when_killed;
        sum.in_protocol += t.in_protocol;
        sum.files_checked += t.files_checked;
        sum.original_intact += t.original_intact;
        sum.verified_output += t.verified_output;
        sum.converted += t.converted;
        sum.restored += t.restored;
        sum.violations.extend(t.violations);
    }
    sum
}

fn count(default: u64) -> u64 {
    std::env::var("AC_KILL_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn report_and_assert(t: &Tally, wanted: u64) {
    eprintln!(
        "KILL LOOP: {} kills ({} with the child still running, {} inside the commit protocol),          {} files checked: {} original intact, {} verified output, {} converted,          {} restores byte-identical, {} violations",
        t.kills,
        t.alive_when_killed,
        t.in_protocol,
        t.files_checked,
        t.original_intact,
        t.verified_output,
        t.converted,
        t.restored,
        t.violations.len()
    );
    for v in t.violations.iter().take(20) {
        eprintln!("VIOLATION {v}");
    }
    assert!(t.violations.is_empty(), "{} violations", t.violations.len());
    assert!(u64::from(t.kills) >= wanted, "only {} kills", t.kills);
    assert!(
        u64::from(t.alive_when_killed) >= wanted * 9 / 10,
        "the child was already done in too many runs ({} of {})",
        t.kills - t.alive_when_killed,
        t.kills
    );
    // The loop must have exercised the interesting states, not only one.
    assert!(t.original_intact > 0 && t.verified_output > 0, "{t:?}");
}

#[test]
fn five_hundred_random_kills_never_lose_an_original_or_leave_a_bad_output() {
    let n = count(500);
    let t = kill_loop(n, 4);
    report_and_assert(&t, n);
}

#[test]
#[ignore = "nightly: 1,000 kills (set AC_KILL_COUNT to change)"]
fn a_thousand_random_kills_never_lose_an_original_or_leave_a_bad_output() {
    let n = count(1000);
    let t = kill_loop(n, 4);
    report_and_assert(&t, n);
}
