// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `auto-crop-eval`: the accuracy harness command line. See `--help`.

use auto_crop_eval::compare::{CompareConfig, Verdict, compare, render_text};
use auto_crop_eval::detector::{DetectorPredictor, ItemsDetectorPredictor};
use auto_crop_eval::multi::{self, MultiOracle, MultiPredictor, MultiRunConfig};
use auto_crop_eval::predictor::{FullFrame, Jittered, JsonLines, Oracle, Predictor};
use auto_crop_eval::publish::{PublishableMetrics, check_no_leak};
use auto_crop_eval::publish_multi::{
    PublishableMultiMetrics, check_no_leak as check_no_leak_multi,
};
use auto_crop_eval::report::summary_text;
use auto_crop_eval::run::{RunConfig, from_json, run, to_json};
use auto_crop_eval::stats::MIN_GATE_N;
use auto_crop_eval::{manifest, noise, selfcheck, splits, synth, variants};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

const HELP: &str = "\
auto-crop-eval <command> [options]

Commands:
  synth --suite smoke|full --out DIR [--seed N] [--count N] [--max-edge N]
        Write a STAND-IN synthetic suite (images + manifest.jsonl) for the harness. Never commit it.
  run --manifest FILE --predictor SPEC --out FILE [--split dev|test|all] [--threads N]
      [--commit SHA] [--tier T] [--suite NAME] [--multi]
        Score a predictor. SPEC is one of: full-frame | oracle | detector[:GOOD_THRESHOLD] |
        jitter:SHIFT[:SEED] | jsonl:PATH. Writes results JSON (local; per-image rows) and prints
        an aggregate summary. Wall time goes to <out>.timings.json, never into the results.
        --multi scores several items per image (manifest `items`): item count, recall and precision
        at IoU 0.9, silent wrong splits, routing of touching/overlapping scans. SPEC is then
        items[:GOOD_CUTOFF] (the multi-item detector) or oracle.
  compare --base FILE --head FILE [--waiver] [--out FILE] [--min-gate-n N]
        Paired regression gate. Exit 1 when mean IoU falls 0.3 pt or the failure rate rises 0.5 pt
        (or a slice with n >= gate floor does), unless --waiver (the accuracy-waiver label).
  noise-floor --a FILE --b FILE [--out FILE]
        Disagreement between two annotators' label files (JSON lines: id, width, height, quad).
  publish --results FILE --out FILE [--multi]
        Write the publishable aggregate view (no per-image rows; slices n >= 30 only) and leak-check it.
        --multi reads a `run --multi` result and writes the multi-item aggregate view.
  validate-manifest FILE
        Check a manifest: valid quads, unique ids, relative paths, scene-disjoint splits.
  check-splits FILE...
        Fail when a scene_id, scene_seed, group_id, document_id or background_seed appears in two
        splits, in one manifest or across several (an item without `split` counts as its file).
  check-variants --dir DIR
        Decode every format x EXIF orientation x colour-space variant written by
        `python -m synth variants --out DIR` and require the upright reference back.
  self-check
        Harness self-validation with no dataset (oracle, analytic jitter curve, area fraction,
        crash counting, determinism, gate mutation, calibration).
";

struct Args {
    rest: Vec<String>,
}

impl Args {
    fn value(&self, name: &str) -> Result<Option<String>, String> {
        match self.rest.iter().position(|a| a == name) {
            None => Ok(None),
            Some(i) => self
                .rest
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("{name} needs a value")),
        }
    }

    fn required(&self, name: &str) -> Result<String, String> {
        self.value(name)?.ok_or_else(|| format!("missing {name}"))
    }

    fn flag(&self, name: &str) -> bool {
        self.rest.iter().any(|a| a == name)
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, String> {
        self.value(name)?
            .map(|v| {
                v.parse::<T>()
                    .map_err(|_| format!("{name}: not a valid number: {v}"))
            })
            .transpose()
    }
}

fn git_commit() -> String {
    if let Ok(c) = std::env::var("AUTO_CROP_COMMIT")
        && !c.is_empty()
    {
        return c;
    }
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(|| "unknown".to_owned(), |s| s.trim().to_owned())
}

fn predictor_from_spec(spec: &str, m: &manifest::Manifest) -> Result<Box<dyn Predictor>, String> {
    let (kind, arg) = spec
        .split_once(':')
        .map_or((spec, None), |(k, a)| (k, Some(a)));
    match (kind, arg) {
        ("full-frame", None) => Ok(Box::new(FullFrame)),
        ("oracle", None) => Ok(Box::new(Oracle::from_manifest(m))),
        ("detector", None) => Ok(Box::new(DetectorPredictor::default())),
        ("detector", Some(t)) => Ok(Box::new(DetectorPredictor {
            good_threshold: t
                .parse()
                .map_err(|_| format!("bad detector threshold {t}"))?,
        })),
        ("jitter", Some(a)) => {
            let (shift, seed) = a.split_once(':').map_or((a, "1"), |(s, d)| (s, d));
            Ok(Box::new(Jittered::from_manifest(
                m,
                shift
                    .parse()
                    .map_err(|_| format!("bad jitter shift {shift}"))?,
                seed.parse()
                    .map_err(|_| format!("bad jitter seed {seed}"))?,
            )))
        }
        ("jsonl", Some(path)) => Ok(Box::new(JsonLines::load(Path::new(path))?)),
        _ => Err(format!("unknown predictor spec `{spec}`")),
    }
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

fn cmd_synth(a: &Args) -> Result<ExitCode, String> {
    let name = a.required("--suite")?;
    let mut spec = synth::SuiteSpec::named(&name)
        .ok_or_else(|| format!("unknown suite `{name}` (smoke|full)"))?;
    if let Some(s) = a.number::<u64>("--seed")? {
        spec.seed = s;
    }
    if let Some(c) = a.number::<usize>("--count")? {
        spec.count = c;
    }
    if let Some(e) = a.number::<u32>("--max-edge")? {
        spec.max_edge = e;
    }
    let out = PathBuf::from(a.required("--out")?);
    let started = Instant::now();
    let g = synth::generate(&spec, &out)?;
    println!(
        "wrote {} images ({:.1} MB) and manifest.jsonl ({:.0} KB) to {} in {:.1}s [{}; STAND-IN generator, never commit]",
        g.n,
        g.image_bytes as f64 / 1e6,
        g.manifest_bytes as f64 / 1e3,
        out.display(),
        started.elapsed().as_secs_f64(),
        synth::GENERATOR
    );
    Ok(ExitCode::SUCCESS)
}

fn multi_predictor_from_spec(
    spec: &str,
    m: &manifest::Manifest,
) -> Result<Box<dyn MultiPredictor>, String> {
    let (kind, arg) = spec
        .split_once(':')
        .map_or((spec, None), |(k, a)| (k, Some(a)));
    match (kind, arg) {
        ("oracle", None) => Ok(Box::new(MultiOracle::from_manifest(m))),
        ("items", cutoff) => {
            let mut p = ItemsDetectorPredictor::default();
            if let Some(c) = cutoff {
                p.opts.good_cutoff = c.parse().map_err(|_| format!("bad cutoff {c}"))?;
            }
            Ok(Box::new(p))
        }
        _ => Err(format!(
            "unknown multi predictor spec `{spec}` (items[:CUTOFF] | oracle)"
        )),
    }
}

fn cmd_run_multi(a: &Args) -> Result<ExitCode, String> {
    let split = a.value("--split")?.unwrap_or_else(|| "all".to_owned());
    let m = manifest::load(Path::new(&a.required("--manifest")?))?.filter_split(&split);
    if m.items.is_empty() {
        return Err(format!("no images in split `{split}`"));
    }
    let predictor = multi_predictor_from_spec(&a.required("--predictor")?, &m)?;
    let cfg = MultiRunConfig {
        threads: a.number::<usize>("--threads")?.unwrap_or(0),
        commit: a.value("--commit")?.unwrap_or_else(git_commit),
        suite: a.value("--suite")?.unwrap_or_else(|| "adhoc".to_owned()),
        split,
    };
    let started = Instant::now();
    let results = multi::run(&m, predictor.as_ref(), &cfg)?;
    let wall = started.elapsed();
    let out = PathBuf::from(a.required("--out")?);
    write(&out, &multi::to_json(&results))?;
    let mut sidecar = out.clone().into_os_string();
    sidecar.push(".timings.json");
    write(
        Path::new(&sidecar),
        &format!(
            "{{\"wall_ms\":{},\"scans\":{},\"ms_per_scan_wall\":{:.3}}}
",
            wall.as_millis(),
            results.summary.scans,
            wall.as_secs_f64() * 1000.0 / results.summary.scans as f64
        ),
    )?;
    print!("{}", multi::summary_text(&results));
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(a: &Args) -> Result<ExitCode, String> {
    if a.flag("--multi") {
        return cmd_run_multi(a);
    }
    let split = a.value("--split")?.unwrap_or_else(|| "all".to_owned());
    let m = manifest::load(Path::new(&a.required("--manifest")?))?.filter_split(&split);
    if m.items.is_empty() {
        return Err(format!("no images in split `{split}`"));
    }
    let predictor = predictor_from_spec(&a.required("--predictor")?, &m)?;
    let cfg = RunConfig {
        threads: a.number::<usize>("--threads")?.unwrap_or(0),
        commit: a.value("--commit")?.unwrap_or_else(git_commit),
        suite: a.value("--suite")?.unwrap_or_else(|| "adhoc".to_owned()),
        split,
        tier: a.value("--tier")?,
    };
    let started = Instant::now();
    let results = run(&m, predictor.as_ref(), &cfg)?;
    let wall = started.elapsed();
    let out = PathBuf::from(a.required("--out")?);
    write(&out, &to_json(&results))?;
    let mut sidecar = out.clone().into_os_string();
    sidecar.push(".timings.json");
    write(
        Path::new(&sidecar),
        &format!(
            "{{\"wall_ms\":{},\"images\":{},\"ms_per_image_wall\":{:.3}}}\n",
            wall.as_millis(),
            results.summary.n,
            wall.as_secs_f64() * 1000.0 / results.summary.n as f64
        ),
    )?;
    print!("{}", summary_text(&results));
    Ok(ExitCode::SUCCESS)
}

fn cmd_compare(a: &Args) -> Result<ExitCode, String> {
    let base = from_json(&read(&a.required("--base")?)?)?;
    let head = from_json(&read(&a.required("--head")?)?)?;
    let cfg = CompareConfig {
        waiver: a.flag("--waiver"),
        min_gate_n: a.number::<usize>("--min-gate-n")?.unwrap_or(MIN_GATE_N),
        ..CompareConfig::default()
    };
    let c = compare(&base, &head, &cfg)?;
    print!("{}", render_text(&c));
    if let Some(out) = a.value("--out")? {
        write(
            Path::new(&out),
            &(serde_json::to_string_pretty(&c).map_err(|e| e.to_string())? + "\n"),
        )?;
    }
    Ok(if c.verdict == Verdict::Fail {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn cmd_noise_floor(a: &Args) -> Result<ExitCode, String> {
    let (la, lb) = (
        noise::parse_labels(&read(&a.required("--a")?)?)?,
        noise::parse_labels(&read(&a.required("--b")?)?)?,
    );
    let nf = noise::noise_floor(&la, &lb);
    let json = serde_json::to_string_pretty(&nf).map_err(|e| e.to_string())? + "\n";
    print!("{json}");
    if let Some(out) = a.value("--out")? {
        write(Path::new(&out), &json)?;
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_publish_multi(a: &Args) -> Result<ExitCode, String> {
    let results: multi::MultiResults = serde_json::from_str(&read(&a.required("--results")?)?)
        .map_err(|e| format!("not a multi-item results file: {e}"))?;
    let json = PublishableMultiMetrics::from_results(&results).to_json();
    check_no_leak_multi(&json, &results)?;
    write(Path::new(&a.required("--out")?), &json)?;
    println!("wrote publishable multi-item aggregates (no per-scan rows, slices n >= 30 only)");
    Ok(ExitCode::SUCCESS)
}

fn cmd_publish(a: &Args) -> Result<ExitCode, String> {
    if a.flag("--multi") {
        return cmd_publish_multi(a);
    }
    let results = from_json(&read(&a.required("--results")?)?)?;
    let json = PublishableMetrics::from_results(&results).to_json();
    check_no_leak(&json, &results)?;
    write(Path::new(&a.required("--out")?), &json)?;
    println!("wrote publishable aggregates (no per-image rows, slices n >= 30 only)");
    Ok(ExitCode::SUCCESS)
}

fn cmd_validate(a: &Args) -> Result<ExitCode, String> {
    let path = a.rest.first().ok_or("validate-manifest needs a file")?;
    let m = manifest::load(Path::new(path))?;
    println!("manifest ok: {} images, sha256 {}", m.items.len(), m.sha256);
    Ok(ExitCode::SUCCESS)
}

fn cmd_check_splits(a: &Args) -> Result<ExitCode, String> {
    if a.rest.is_empty() {
        return Err("check-splits needs at least one manifest".to_owned());
    }
    let paths: Vec<&Path> = a.rest.iter().map(Path::new).collect();
    let violations = splits::check_files(&paths)?;
    if violations.is_empty() {
        println!(
            "splits ok: {} manifest(s), no scene or seed spans two splits",
            paths.len()
        );
        Ok(ExitCode::SUCCESS)
    } else {
        for v in &violations {
            eprintln!("SPLIT LEAK {v}");
        }
        eprintln!("{} leak(s)", violations.len());
        Ok(ExitCode::from(1))
    }
}

fn cmd_check_variants(a: &Args) -> Result<ExitCode, String> {
    let dir = PathBuf::from(a.required("--dir")?);
    let r = variants::check_dir(&dir)?;
    println!(
        "variants: {} decoded ({} lossless exact, {} lossy within bound; worst lossy mean abs error {:.3}), {} failure(s)",
        r.checked,
        r.lossless,
        r.checked - r.lossless,
        r.worst_lossy_error,
        r.failures.len()
    );
    for f in &r.failures {
        eprintln!("VARIANT {f}");
    }
    Ok(if r.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn cmd_self_check() -> Result<ExitCode, String> {
    // The crash-counting check plants panics on purpose; keep their reports out of the log.
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let planted = info
            .payload()
            .downcast_ref::<&str>()
            .is_some_and(|m| *m == "planted predictor crash");
        if !planted {
            default(info);
        }
    }));
    let mut failed = 0;
    for c in selfcheck::run_all() {
        match c.outcome {
            Ok(detail) => println!("ok    {}: {detail}", c.name),
            Err(e) => {
                failed += 1;
                println!("FAIL  {}: {e}", c.name);
            }
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_default();
    let a = Args {
        rest: argv.collect(),
    };
    let result = match cmd.as_str() {
        "synth" => cmd_synth(&a),
        "run" => cmd_run(&a),
        "compare" => cmd_compare(&a),
        "noise-floor" => cmd_noise_floor(&a),
        "publish" => cmd_publish(&a),
        "validate-manifest" => cmd_validate(&a),
        "check-splits" => cmd_check_splits(&a),
        "check-variants" => cmd_check_variants(&a),
        "self-check" => cmd_self_check(),
        "" | "help" | "--help" | "-h" => {
            print!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command `{other}`\n\n{HELP}")),
    };
    result.unwrap_or_else(|e| {
        eprintln!("error: {e}");
        ExitCode::from(2)
    })
}
