// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! ROADMAP M6.86: worker faults through the *engine*, not the pool alone. The real
//! `auto-crop-worker` (built with the test-only fault hooks) sits behind the real `WorkerPool`,
//! which is installed as the remote decoder of `auto-crop-codecs`; the engine's Convert run and its
//! crop flow (open, analyse, save) then meet files that crash the worker, hang it, or make it
//! allocate past its limit. Each such file fails alone with the typed code (`DECODER_CRASHED`,
//! `DECODE_TIMEOUT`, `MEMORY_LIMIT`), the other files of the same run succeed, and the failed
//! sources are byte-identical afterwards.
//!
//! No engine change is needed for this: the engine only ever sees `auto_crop_codecs`, so the test
//! installs a thin `RemoteDecoder` over a `WorkerPool` with short test limits (a 4 s deadline, a
//! 512 MiB memory limit) instead of the shipped `auto_crop_sandbox::install`. The fault is chosen by
//! a marker appended to the file (`AUTOCROP-FAULT:<kind>`), which every decoder ignores after the
//! end of the image.

#![cfg(feature = "escape-tests")]

use auto_crop_codecs::{
    CodecError, Decoded, Format, RemoteDecoder, RemoteRequest, fixtures, set_remote_decoder,
};
use auto_crop_engine::convert::{ConvertOptions, ConvertTarget, Placement};
use auto_crop_engine::convert_batch::{ConvertFileResult, ConvertStatus, ConvertSummary};
use auto_crop_engine::{AppPaths, Engine, ErrKind, ItemStatus, SaveTarget};
use auto_crop_sandbox::{PoolConfig, PoolFailure, WorkerPool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The pool as the engine reaches it: every decodable format goes to the worker.
struct TestRemote(Arc<WorkerPool>);

impl RemoteDecoder for TestRemote {
    fn handles(&self, format: Format) -> bool {
        format.is_decodable()
    }

    fn decode(&self, req: RemoteRequest<'_>) -> Result<Decoded, CodecError> {
        match self.0.decode(
            req.bytes,
            req.limits,
            req.apply_orientation,
            req.flatten_alpha,
        ) {
            Ok(d) => Ok(d),
            Err(PoolFailure::Codec(e)) => Err(e),
            Err(PoolFailure::Unavailable(why)) => Err(CodecError::WorkerCrashed(why)),
        }
    }
}

/// The remote decoder is process-wide: one test at a time owns it.
static EXCLUSIVE: Mutex<()> = Mutex::new(());

struct Installed {
    pool: Arc<WorkerPool>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Installed {
    fn drop(&mut self) {
        set_remote_decoder(None);
    }
}

fn install_pool() -> Installed {
    let guard = EXCLUSIVE.lock().unwrap_or_else(|e| e.into_inner());
    let mut cfg = PoolConfig::new(PathBuf::from(env!("CARGO_BIN_EXE_auto-crop-worker")));
    cfg.workers = 2;
    cfg.memory_limit = Some(512 << 20);
    cfg.startup_timeout = Duration::from_secs(60);
    // A job with no deadline of its own gets this one (the engine passes none).
    cfg.timeout_base = Duration::from_secs(4);
    let pool = Arc::new(WorkerPool::new(cfg));
    set_remote_decoder(Some(Arc::new(TestRemote(pool.clone()))));
    Installed {
        pool,
        _guard: guard,
    }
}

fn with_fault(mut bytes: Vec<u8>, kind: &str) -> Vec<u8> {
    bytes.extend_from_slice(format!("AUTOCROP-FAULT:{kind}").as_bytes());
    bytes
}

/// What each file of a run is: a good picture or one of the three faults, with the code the
/// engine must give the faulty ones.
const FAULTS: [(&str, &str, &str, ErrKind); 3] = [
    ("crash", "abort", "DECODER_CRASHED", ErrKind::DecoderCrashed),
    ("hang", "hang", "DECODE_TIMEOUT", ErrKind::DecodeTimeout),
    ("bomb", "alloc", "MEMORY_LIMIT", ErrKind::MemoryLimit),
];

/// Writes the run's folder: good files and faulty files interleaved, so a fault in the middle
/// must not take its neighbours down. Returns every file with its bytes.
fn write_run_folder(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::create_dir_all(dir).unwrap();
    let good = [
        ("a_good.png", fixtures::png_rgb(64, 48)),
        ("c_good.jpg", fixtures::jpeg_baseline(80, 60)),
        ("e_good.png", fixtures::png_rgba(32, 32)),
        ("g_good.png", fixtures::png_gray(33, 21)),
    ];
    let mut files: BTreeMap<String, Vec<u8>> =
        good.into_iter().map(|(n, b)| (n.to_owned(), b)).collect();
    for (i, (stem, kind, _, _)) in FAULTS.iter().enumerate() {
        let base = if i % 2 == 0 {
            fixtures::png_rgb(40, 40)
        } else {
            fixtures::jpeg_baseline(48, 48)
        };
        let ext = if i % 2 == 0 { "png" } else { "jpg" };
        files.insert(
            format!("{}_{stem}.{ext}", ["b", "d", "f"][i]),
            with_fault(base, kind),
        );
    }
    for (name, bytes) in &files {
        std::fs::write(dir.join(name), bytes).unwrap();
    }
    files
}

fn engine_in(root: &Path) -> Engine {
    Engine::new(AppPaths::under(&root.join("app")))
}

fn read_all(dir: &Path, files: &BTreeMap<String, Vec<u8>>) -> BTreeMap<String, Option<Vec<u8>>> {
    files
        .keys()
        .map(|n| (n.clone(), std::fs::read(dir.join(n)).ok()))
        .collect()
}

fn expected_code(name: &str) -> Option<&'static str> {
    FAULTS
        .iter()
        .find(|(stem, ..)| name.contains(&format!("_{stem}.")))
        .map(|(_, _, code, _)| *code)
}

fn by_name(summary: &ConvertSummary) -> BTreeMap<&str, &ConvertFileResult> {
    summary
        .results
        .iter()
        .map(|r| (r.name.as_str(), r))
        .collect()
}

fn run_convert(engine: &Engine, dir: &Path, placement: Placement) -> ConvertSummary {
    let added = engine.convert_add(&[dir.to_path_buf()], false);
    assert_eq!(added.added, 7, "{added:?}");
    let opts = ConvertOptions {
        target: ConvertTarget::Jpg,
        placement,
        // The PNGs are converted too (a same-format file would be skipped before it is decoded).
        reencode: true,
        ..ConvertOptions::default()
    };
    engine.convert_run(&opts, "worker faults", 2, &|_| {})
}

fn check_results(summary: &ConvertSummary) {
    assert!(!summary.cancelled);
    assert_eq!(summary.results.len(), 7, "{summary:?}");
    for (name, r) in by_name(summary) {
        match expected_code(name) {
            Some(code) => {
                assert_eq!(r.status, ConvertStatus::Failed, "{name}: {r:?}");
                assert_eq!(r.code.as_deref(), Some(code), "{name}: {r:?}");
                assert!(r.output.is_none(), "{name} wrote an output: {r:?}");
            }
            None => {
                // Every good file is converted, whatever its neighbours did to their workers.
                assert_eq!(r.status, ConvertStatus::Converted, "{name}: {r:?}");
            }
        }
    }
    assert_eq!(summary.failed, 3, "{summary:?}");
    assert_eq!((summary.converted, summary.skipped), (4, 0), "{summary:?}");
}

fn check_pool(pool: &WorkerPool) {
    let st = pool.status();
    assert!(st.crashes >= 1, "{st:?}");
    assert_eq!(st.timeouts, 1, "{st:?}");
    assert!(st.live <= 2, "{st:?}");
    // Workers were replaced, and the pool still decodes after all three faults.
    assert!(st.spawned >= 3, "{st:?}");
    let ok = pool.decode(
        &fixtures::png_rgb(8, 8),
        &auto_crop_codecs::DecodeLimits::default(),
        true,
        false,
    );
    assert!(ok.is_ok(), "{ok:?}");
}

#[test]
fn a_convert_run_that_keeps_originals_fails_only_the_faulty_files() {
    let inst = install_pool();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("photos");
    let files = write_run_folder(&dir);
    let engine = engine_in(root.path());

    let summary = run_convert(&engine, &dir, Placement::Keep);
    check_results(&summary);
    // Every source, faulty or not, is exactly as it was written.
    for (name, now) in read_all(&dir, &files) {
        assert_eq!(now.as_ref(), Some(&files[&name]), "{name} changed");
    }
    // The converted pictures exist beside their sources; the faulty ones left nothing behind.
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    for n in &names {
        assert!(!n.contains(".tmp") && !n.contains(".partial"), "{n}");
    }
    for r in &summary.results {
        if let Some(out) = &r.output {
            assert!(dir.join(out).is_file(), "{out}");
        }
    }
    assert_eq!(names.len(), files.len() + summary.converted, "{names:?}");
    check_pool(&inst.pool);
}

#[test]
fn a_convert_run_that_replaces_backs_up_the_good_files_and_leaves_the_faulty_ones_alone() {
    let inst = install_pool();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("photos");
    let files = write_run_folder(&dir);
    let engine = engine_in(root.path());

    let summary = run_convert(&engine, &dir, Placement::Replace);
    check_results(&summary);
    for (name, now) in read_all(&dir, &files) {
        if expected_code(&name).is_some() {
            assert_eq!(now.as_ref(), Some(&files[&name]), "{name} changed");
        }
    }
    // Only the converted files have a backup; the failed ones never entered the run's backups.
    let backups = engine.list_backups();
    let backed: usize = backups.runs.iter().map(|r| r.file_count).sum();
    assert_eq!(backed, summary.converted, "{backups:?}");
    check_pool(&inst.pool);
}

#[test]
fn the_crop_flow_fails_only_the_faulty_items_and_saves_the_rest() {
    let inst = install_pool();
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("photos");
    let samples = auto_crop_engine::samples::write_samples(&dir).unwrap();
    // Faulty copies of a real sample, one per fault.
    let sample_bytes = std::fs::read(&samples[0]).unwrap();
    let mut faulty: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for (stem, kind, ..) in FAULTS {
        let p = dir.join(format!("zz_{stem}.jpg"));
        let b = with_fault(sample_bytes.clone(), kind);
        std::fs::write(&p, &b).unwrap();
        faulty.push((p, b));
    }
    let engine = engine_in(root.path());
    let opened = engine.open_paths(std::slice::from_ref(&dir), false);
    assert_eq!(opened.ids.len(), samples.len() + 3, "{opened:?}");

    let (mut good, mut ready) = (Vec::new(), 0);
    for id in opened.ids.clone() {
        let view = engine.analyse(id).unwrap();
        let name = view.name.clone();
        let want = FAULTS
            .iter()
            .find(|(stem, ..)| name == format!("zz_{stem}.jpg"))
            .map(|(_, _, _, kind)| *kind);
        match want {
            Some(kind) => {
                assert_eq!(view.status, ItemStatus::Error, "{name}: {view:?}");
                assert_eq!(view.error, Some(kind), "{name}");
                assert!(view.edit.is_none(), "{name}");
            }
            None => {
                assert_eq!(view.status, ItemStatus::Ready, "{name}: {view:?}");
                assert_eq!(view.error, None, "{name}");
                ready += 1;
                // A sample the detector finds no crop in has nothing to save.
                if view.edit.is_some() {
                    good.push(id);
                }
            }
        }
    }
    assert_eq!(ready, samples.len(), "every sample was analysed");
    assert!(good.len() >= 2, "{} samples with a crop", good.len());

    // The good items save (replace, with a backup) unless the engine itself holds them for
    // review, which has nothing to do with the workers; the failed ones refuse and change nothing.
    let mut saved = 0;
    for id in &good {
        let out = engine.save_in_run(*id, SaveTarget::Replace, "faults", "worker faults");
        if out.ok {
            saved += 1;
        } else {
            assert_eq!(out.error, Some(ErrKind::HeldForReview), "{out:?}");
        }
    }
    assert!(saved >= 2, "{saved} samples saved");
    for id in opened.ids {
        let view = engine.item_view(id).unwrap();
        if view.status == ItemStatus::Error {
            let out = engine.save_in_run(id, SaveTarget::Replace, "faults", "worker faults");
            assert!(!out.ok, "{out:?}");
        }
    }
    for (path, bytes) in &faulty {
        assert_eq!(&std::fs::read(path).unwrap(), bytes, "{}", path.display());
    }
    assert_eq!(
        engine
            .list_backups()
            .runs
            .iter()
            .map(|r| r.file_count)
            .sum::<usize>(),
        saved
    );
    check_pool(&inst.pool);
}
