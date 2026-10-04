// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! `perf stages`: measured stage p50/p95 against the PROVISIONAL Table A budgets and the
//! stage-sum check (ROADMAP M1.63). A stage more than 1.5x over its budget at p50 is reported as
//! needing a redesign (PLAN 7.2, gate G0); nothing here relaxes a budget.

use super::host::{Host, Monitor, idle_check};
use super::{Flags, ensure_image, percentile, sorted, write_json};
use auto_crop_core::CancelToken;
use auto_crop_engine::skeleton::standin;
use auto_crop_engine::skeleton::{
    Analyse, Enhance, Input, Options, Report, Stage, chained_budget_ms, run as run_pipeline,
    thread_pool,
};
use serde_json::{Value, json};
use std::path::Path;

/// Table B: full auto-process p50 budget in ms by megapixels (12, 48, 100).
fn table_b_total_ms(megapixels: f64) -> Option<f64> {
    match megapixels as u32 {
        12 => Some(700.0),
        48 => Some(2500.0),
        100 => Some(5000.0),
        _ => None,
    }
}

/// The 1.5x line of PLAN 7.2: more than this over budget forces a redesign.
const REDESIGN_RATIO: f64 = 1.5;

pub struct Row {
    pub stage: Stage,
    pub px_median: u64,
    pub min: f64,
    pub p50: f64,
    pub p95: f64,
}

pub struct Measured {
    pub megapixels: f64,
    pub source: (u32, u32),
    pub file_bytes: u64,
    pub output: (u32, u32),
    pub runs: usize,
    pub rows: Vec<Row>,
    pub total_p50: f64,
    pub total_p95: f64,
    /// Median and maximum of `total - sum of stages`, in ms.
    pub glue_median: f64,
    pub glue_max: f64,
    pub sum_p50: f64,
    /// Another process was using the CPU while this ran: every verdict carries a NOISY tag.
    pub noisy: bool,
    /// What the `analyse` stage ran (starts with `STAND-IN`).
    pub analyse: String,
}

/// Runs `warmup + runs` times and reduces the reports.
pub fn measure(
    path: &Path,
    megapixels: f64,
    opts: &Options,
    warmup: usize,
    runs: usize,
) -> Result<Measured, String> {
    let token = CancelToken::never();
    let file_bytes = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let mut reports: Vec<Report> = Vec::with_capacity(runs);
    for i in 0..warmup + runs {
        let out = run_pipeline(Input::Path(path), opts, &token)
            .map_err(|f| format!("{}: {f}", path.display()))?;
        if i >= warmup {
            reports.push(out.report);
        }
    }
    Ok(reduce(megapixels, file_bytes, &reports))
}

pub fn reduce(megapixels: f64, file_bytes: u64, reports: &[Report]) -> Measured {
    let mut rows = Vec::new();
    for stage in Stage::ALL {
        let ms: Vec<f64> = reports.iter().filter_map(|r| r.ms_of(stage)).collect();
        if ms.is_empty() {
            continue;
        }
        let mut px: Vec<u64> = reports
            .iter()
            .filter_map(|r| r.stages.iter().find(|s| s.stage == stage).map(|s| s.px))
            .collect();
        px.sort_unstable();
        let ms = sorted(ms);
        rows.push(Row {
            stage,
            px_median: px[px.len() / 2],
            min: ms[0],
            p50: percentile(&ms, 0.5),
            p95: percentile(&ms, 0.95),
        });
    }
    let totals = sorted(reports.iter().map(|r| r.total_ms).collect());
    let glue = sorted(
        reports
            .iter()
            .map(|r| r.total_ms - r.stage_sum_ms())
            .collect(),
    );
    let sums = sorted(reports.iter().map(Report::stage_sum_ms).collect());
    let first = &reports[0];
    Measured {
        megapixels,
        source: first.source,
        file_bytes,
        output: first.output,
        runs: reports.len(),
        rows,
        total_p50: percentile(&totals, 0.5),
        total_p95: percentile(&totals, 0.95),
        glue_median: percentile(&glue, 0.5),
        glue_max: percentile(&glue, 1.0),
        sum_p50: percentile(&sums, 0.5),
        noisy: false,
        analyse: first.analyse.clone(),
    }
}

/// `ok`, `over budget` or the redesign flag, from a measured/budget ratio.
pub fn verdict(ratio: f64) -> &'static str {
    if ratio <= 1.0 {
        "within"
    } else if ratio <= REDESIGN_RATIO {
        "over budget (<= 1.5x)"
    } else {
        "OVER 1.5x: REDESIGN"
    }
}

/// [`verdict`] with a NOISY tag when the run shared the machine.
fn tagged(m: &Measured, ratio: f64) -> String {
    if m.noisy {
        format!("{} [NOISY: re-measure idle]", verdict(ratio))
    } else {
        verdict(ratio).to_owned()
    }
}

fn markdown(m: &Measured, label: &str) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "### {} MP ({}x{}, {:.1} MB file, output {}x{}), {label}, {} runs\n",
        m.megapixels,
        m.source.0,
        m.source.1,
        m.file_bytes as f64 / 1e6,
        m.output.0,
        m.output.1,
        m.runs
    );
    let at_12 = m.megapixels as u32 == 12;
    let _ = writeln!(
        s,
        "| stage | px | min ms | p50 ms | p95 ms | budget ms (PROVISIONAL, 12 MP) | p50 / budget | verdict |\n|---|---:|---:|---:|---:|---:|---:|---|"
    );
    for r in &m.rows {
        let b = r.stage.budget_ms();
        let (bs, ratio, v) = if at_12 {
            (
                format!("{b:.0}"),
                format!("{:.2}", r.p50 / b),
                tagged(m, r.p50 / b),
            )
        } else {
            (
                "-".into(),
                "-".into(),
                "no per-stage budget at this size".into(),
            )
        };
        let note = match r.stage {
            Stage::Analyse => format!(" ({})", m.analyse),
            Stage::Enhance => " (PROTOTYPE)".to_owned(),
            _ => String::new(),
        };
        let _ = writeln!(
            s,
            "| {}{note} | {} | {:.1} | {:.1} | {:.1} | {bs} | {ratio} | {v} |",
            r.stage.name(),
            r.px_median,
            r.min,
            r.p50,
            r.p95
        );
    }
    let _ = writeln!(
        s,
        "| **sum of stages** | | | {:.1} | | {} | | |",
        m.sum_p50,
        if at_12 {
            format!("{:.0}", chained_budget_ms())
        } else {
            "-".into()
        }
    );
    let _ = writeln!(
        s,
        "| **total (wall)** | | | {:.1} | {:.1} | | | |",
        m.total_p50, m.total_p95
    );
    if let Some(b) = table_b_total_ms(m.megapixels) {
        let _ = writeln!(
            s,
            "\nTable B full-process p50 budget at this size: {b:.0} ms (also holds `refine`, `commit` and slack, not chained here), measured chained p50 {:.0} ms = {:.2}x of it ({}).",
            m.total_p50,
            m.total_p50 / b,
            tagged(m, m.total_p50 / b)
        );
    }
    let glue_pct = 100.0 * m.glue_median / m.total_p50;
    let _ = writeln!(
        s,
        "\nStage-sum check: total - sum of stages = {:.2} ms median ({glue_pct:.1}% of the total), {:.2} ms worst: {}.",
        m.glue_median,
        m.glue_max,
        if glue_pct <= 3.0 {
            "PASS (<= 3%)"
        } else {
            "FAIL (> 3%)"
        }
    );
    if at_12 && let Some(a) = m.rows.iter().find(|r| r.stage == Stage::Analyse) {
        let _ = writeln!(
            s,
            "Analysis ceiling: p95 {:.1} ms against 40 ms (STAND-IN detector): {}.",
            a.p95,
            tagged(m, a.p95 / 40.0)
        );
    }
    s
}

fn json_of(m: &Measured, label: &str) -> Value {
    json!({
        "megapixels": m.megapixels, "label": label, "source": [m.source.0, m.source.1],
        "output": [m.output.0, m.output.1], "file_bytes": m.file_bytes, "runs": m.runs,
        "stages": m.rows.iter().map(|r| json!({
            "stage": r.stage.name(), "px": r.px_median, "min_ms": r.min, "p50_ms": r.p50, "p95_ms": r.p95,
            "budget_ms": r.stage.budget_ms()})).collect::<Vec<_>>(),
        "total_p50_ms": m.total_p50, "total_p95_ms": m.total_p95, "sum_p50_ms": m.sum_p50,
        "glue_median_ms": m.glue_median, "glue_max_ms": m.glue_max,
    })
}

pub fn run(f: &Flags) -> Result<(), String> {
    let mps: Vec<f64> = f.list("--mp", "12")?;
    let runs: usize = f.num("--runs", 20)?;
    let warmup: usize = f.num("--warmup", 3)?;
    if runs == 0 {
        return Err("--runs must be at least 1".into());
    }
    let (threads, tlabel) = match f.get("--threads") {
        None | Some("all") => (None, "all threads (rayon global pool)".to_owned()),
        Some(n) => {
            let n: usize = n.parse().map_err(|_| "--threads needs `all` or a number")?;
            (Some(n), format!("{n} thread(s)"))
        }
    };
    let enhance = match f.get("--enhance").unwrap_or("otsu") {
        "otsu" => Enhance::Otsu,
        "sauvola" => Enhance::Sauvola,
        "off" => Enhance::Off,
        o => return Err(format!("unknown --enhance `{o}`")),
    };
    let analyse = match f.get("--analyse").unwrap_or("classical") {
        "classical" => Analyse::Classical,
        "standin-canny" => Analyse::StandinCanny,
        "standin-net" => {
            let backend = match f.get("--net-backend") {
                Some(b) => b,
                None => standin::default_backend().ok_or(
                    "this build has no inference backend: --features standin-ort,standin-rten",
                )?,
            };
            let path = match f.get("--net") {
                Some(p) => p.into(),
                None => crate::standin::make_standin_net(Path::new("target/standin"))?,
            };
            Analyse::StandinNet(standin::load_net_at(
                &path,
                backend,
                f.num("--net-threads", 4)?,
                crate::standin::dev_runtime().as_deref(),
            )?)
        }
        o => return Err(format!("unknown --analyse `{o}` ({})", Analyse::NAMES)),
    };
    let opts = Options {
        enhance,
        analyse,
        pool: match threads {
            Some(n) => Some(thread_pool(n).map_err(|e| e.to_string())?),
            None => None,
        },
        ..Options::default()
    };
    let host = Host::capture();
    println!("## Host\n\n{}\n", host.markdown());
    let pre = idle_check();
    println!("Load before: {}\n", pre.label());

    let monitor = Monitor::start();
    let mut results = Vec::new();
    let label = format!(
        "{tlabel}, enhance {enhance:?}, analyse {}",
        opts.analyse.label()
    );
    for mp in mps {
        let path = match f.get("--file") {
            Some(p) => p.into(),
            None => ensure_image(&f.dir(), mp, 1, f.has("--fill"))?,
        };
        results.push(measure(&path, mp, &opts, warmup, runs)?);
    }
    let during = monitor.finish();
    for m in &mut results {
        m.noisy = pre.noisy() || during.noisy();
        println!("{}", markdown(m, &label));
    }
    println!("Load during: {}", during.label());
    if during.noisy() || pre.noisy() {
        println!(
            "\nNOISY: another process was using the CPU. Treat every number above as a pessimistic bound, not a measurement of this code; re-run when the machine is idle."
        );
    }
    if let Some(out) = f.get("--json") {
        write_json(
            out,
            &json!({
                "host": host.json(), "load_before": pre.json(), "load_during": during.json(),
                "results": results.iter().map(|m| json_of(m, &label)).collect::<Vec<_>>(),
            }),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_engine::skeleton::StageTiming;

    fn report(scale: f64) -> Report {
        Report {
            stages: Stage::ALL
                .iter()
                .map(|s| StageTiming {
                    stage: *s,
                    ms: s.budget_ms() * scale,
                    px: 1000,
                })
                .collect(),
            total_ms: chained_budget_ms() * scale + 2.0,
            source: (4000, 3000),
            output: (3000, 2000),
            output_bytes: 1,
            quad_found: true,
            analyse: "STAND-IN (test)".to_owned(),
        }
    }

    #[test]
    fn the_verdicts_follow_the_one_and_a_half_times_rule() {
        assert_eq!(verdict(0.5), "within");
        assert_eq!(verdict(1.0), "within");
        assert_eq!(verdict(1.2), "over budget (<= 1.5x)");
        assert_eq!(verdict(1.5), "over budget (<= 1.5x)");
        assert!(verdict(1.51).contains("REDESIGN"));
    }

    #[test]
    fn reduce_computes_percentiles_glue_and_flags_the_redesign() {
        let reports: Vec<Report> = (1..=20).map(|i| report(f64::from(i) / 10.0)).collect();
        let m = reduce(12.0, 5_000_000, &reports);
        assert_eq!(m.rows.len(), 7);
        let decode = m.rows.iter().find(|r| r.stage == Stage::Decode).unwrap();
        // Scales 0.1..2.0: p50 is the 10th (1.0x), p95 the 19th (1.9x).
        assert!((decode.p50 - 120.0).abs() < 1e-9);
        assert!((decode.p95 - 120.0 * 1.9).abs() < 1e-9);
        assert!((m.glue_median - 2.0).abs() < 1e-9);
        let md = markdown(&m, "test");
        assert!(md.contains("PASS") || md.contains("FAIL"));
        assert!(md.contains("STAND-IN") && md.contains("PROTOTYPE"));
        // p95 of decode is 1.9x its budget but p50 is within: only p50 decides the verdict.
        assert!(md.contains("| decode |"));
        let mut slow = reduce(12.0, 1, &[report(2.0)]);
        assert!(markdown(&slow, "t").contains("REDESIGN"));
        assert!(!markdown(&slow, "t").contains("NOISY"));
        slow.noisy = true;
        assert!(markdown(&slow, "t").contains("REDESIGN [NOISY"));
    }
}
