// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Headless command-line tool. Pre-alpha: the only command is the developer pipeline of
//! ROADMAP M1.54.

use auto_crop_engine::ErrKind;
use auto_crop_engine::skeleton::{self, Enhance, Input, Options};
use std::path::PathBuf;
use std::process::ExitCode;

fn banner() -> String {
    format!(
        "auto-crop {} (pre-alpha, one developer command: dev-pipeline)",
        env!("CARGO_PKG_VERSION")
    )
}

const USAGE: &str = "\
usage: auto-crop dev-pipeline <file> [options]

Runs one image through the benchmark skeleton (read_probe, decode, proxy, analyse, rectify,
enhance, encode) and reports per-stage timings. Developer tool: the analyse stage is a STAND-IN
(classical detector) and enhance a PROTOTYPE (grey + threshold); output is not a product result.

options:
  --timings            print the per-stage table with the PROVISIONAL Table A budgets
  --json               print the report as one JSON line instead of the table
  --trace              also print `stage_done` tracing events to stderr
  --out <file>         write the encoded JPEG there (default: nothing is written)
  --threads <n>        run the parallel kernels on exactly n threads (default: all cores)
  --enhance <mode>     off | otsu | sauvola (default otsu)
  --quality <1-100>    JPEG quality (default 90)
";

#[derive(Debug, PartialEq)]
struct Args {
    file: PathBuf,
    timings: bool,
    json: bool,
    trace: bool,
    out: Option<PathBuf>,
    threads: Option<usize>,
    enhance: Enhance,
    quality: u8,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut it = args.iter();
    let mut a = Args {
        file: PathBuf::new(),
        timings: false,
        json: false,
        trace: false,
        out: None,
        threads: None,
        enhance: Enhance::default(),
        quality: skeleton::BENCH_JPEG_QUALITY,
    };
    let mut file = None;
    let value = |it: &mut std::slice::Iter<'_, String>, flag: &str| {
        it.next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--timings" => a.timings = true,
            "--json" => a.json = true,
            "--trace" => a.trace = true,
            "--out" => a.out = Some(PathBuf::from(value(&mut it, "--out")?)),
            "--threads" => {
                let n: usize = value(&mut it, "--threads")?
                    .parse()
                    .map_err(|_| "--threads needs a whole number".to_owned())?;
                if n == 0 {
                    return Err("--threads must be at least 1".to_owned());
                }
                a.threads = Some(n);
            }
            "--enhance" => {
                a.enhance = match value(&mut it, "--enhance")?.as_str() {
                    "off" => Enhance::Off,
                    "otsu" => Enhance::Otsu,
                    "sauvola" => Enhance::Sauvola,
                    other => return Err(format!("unknown --enhance mode `{other}`")),
                }
            }
            "--quality" => {
                a.quality = value(&mut it, "--quality")?
                    .parse()
                    .ok()
                    .filter(|q| (1..=100).contains(q))
                    .ok_or_else(|| "--quality needs a number from 1 to 100".to_owned())?;
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option `{flag}`")),
            path => {
                if file.replace(PathBuf::from(path)).is_some() {
                    return Err("only one input file is accepted".to_owned());
                }
            }
        }
    }
    a.file = file.ok_or_else(|| "no input file given".to_owned())?;
    Ok(a)
}

fn dev_pipeline(args: &[String]) -> Result<(), String> {
    let a = parse(args)?;
    let _trace = a.trace.then(skeleton::trace_to_stderr);
    let pool = match a.threads {
        Some(n) => Some(skeleton::thread_pool(n).map_err(|e| e.to_string())?),
        None => None,
    };
    let opts = Options {
        quality: a.quality,
        enhance: a.enhance,
        pool,
        ..Options::default()
    };
    let token = auto_crop_core::CancelToken::never();
    let out = skeleton::run(Input::Path(&a.file), &opts, &token).map_err(|f| {
        let done: Vec<_> = f.done.iter().map(|s| s.stage.name()).collect();
        let hint = if f.kind == ErrKind::Unreadable {
            " (is the path right?)"
        } else {
            ""
        };
        format!(
            "{f} [{:?}]{hint}; finished before it: {}",
            f.kind,
            if done.is_empty() {
                "nothing".to_owned()
            } else {
                done.join(", ")
            }
        )
    })?;
    if let Some(path) = &a.out {
        std::fs::write(path, &out.bytes)
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    }
    if a.json {
        println!(
            "{}",
            serde_json::to_string(&out.report).map_err(|e| e.to_string())?
        );
    } else if a.timings {
        println!("{}", skeleton::format_timings(&out.report));
    } else {
        println!(
            "{}x{} -> {}x{}, {} bytes, {:.0} ms",
            out.report.source.0,
            out.report.source.1,
            out.report.output.0,
            out.report.output.1,
            out.report.output_bytes,
            out.report.total_ms
        );
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dev-pipeline") => match dev_pipeline(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}\n\n{USAGE}");
                ExitCode::FAILURE
            }
        },
        Some("--help" | "-h" | "help") => {
            println!("{}\n\n{USAGE}", banner());
            ExitCode::SUCCESS
        }
        _ => {
            println!("{}", banner());
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn banner_names_the_tool() {
        assert!(super::banner().starts_with("auto-crop "));
    }

    #[test]
    fn options_parse() {
        let a = parse(&v(&[
            "p.jpg",
            "--timings",
            "--threads",
            "8",
            "--enhance",
            "sauvola",
            "--out",
            "o.jpg",
            "--quality",
            "75",
        ]))
        .unwrap();
        assert_eq!(a.file, PathBuf::from("p.jpg"));
        assert!(a.timings && !a.json);
        assert_eq!(a.threads, Some(8));
        assert_eq!(a.enhance, Enhance::Sauvola);
        assert_eq!(a.out, Some(PathBuf::from("o.jpg")));
        assert_eq!(a.quality, 75);
    }

    #[test]
    fn bad_options_are_refused() {
        for bad in [
            &["--timings"][..],
            &["a.jpg", "b.jpg"],
            &["a.jpg", "--threads", "0"],
            &["a.jpg", "--threads", "x"],
            &["a.jpg", "--quality", "0"],
            &["a.jpg", "--quality", "101"],
            &["a.jpg", "--enhance", "magic"],
            &["a.jpg", "--out"],
            &["a.jpg", "--nope"],
        ] {
            assert!(parse(&v(bad)).is_err(), "{bad:?}");
        }
    }
}
