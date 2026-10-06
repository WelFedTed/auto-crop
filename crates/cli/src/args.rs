// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The command line: a small hand-written parser (decision recorded in
//! `docs/adr/0011-cli-argument-parser.md`) driven by one flag table per command. The same tables
//! produce `--help` and are checked against `docs/cli.md`, so the reference cannot drift.
//!
//! Rules: `--flag value` and `--flag=value`; `-x` short forms, clustered switches (`-rq`) and
//! `-j4`; `--` ends the flags; a value flag takes the next argument whatever it looks like; an
//! unknown flag is a usage error with a suggestion. Parsing never touches the file system.

use auto_crop_core::{SplitPolicy, SplitProfile};
use auto_crop_engine::{EngineOptions, QualityPreset, QualitySetting};
use std::path::PathBuf;

/// One flag of a command.
#[derive(Debug, Clone, Copy)]
pub struct Flag {
    pub name: &'static str,
    pub short: Option<char>,
    /// The value's name in the help, `None` for a switch.
    pub value: Option<&'static str>,
    pub help: &'static str,
}

const fn sw(name: &'static str, short: Option<char>, help: &'static str) -> Flag {
    Flag {
        name,
        short,
        value: None,
        help,
    }
}

const fn val(
    name: &'static str,
    short: Option<char>,
    value: &'static str,
    help: &'static str,
) -> Flag {
    Flag {
        name,
        short,
        value: Some(value),
        help,
    }
}

/// A usage error: printed as `error: ...` with a pointer to `--help`, exit code 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usage(pub String);

impl std::fmt::Display for Usage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

type Res<T> = Result<T, Usage>;

fn usage<T>(msg: impl Into<String>) -> Res<T> {
    Err(Usage(msg.into()))
}

// ---------------------------------------------------------------- flag tables

pub fn global_flags() -> Vec<Flag> {
    vec![
        sw("quiet", Some('q'), "Print nothing on stderr except errors"),
        sw(
            "verbose",
            Some('v'),
            "Print one line for every item, not only held and failed ones",
        ),
        sw(
            "json",
            None,
            "Print one JSON document on stdout (the run manifest, or the command's result)",
        ),
        sw(
            "ndjson",
            None,
            "Stream one JSON event per line on stdout (start, item..., end)",
        ),
        val(
            "home",
            None,
            "DIR",
            "Keep backups and settings under DIR instead of the per-user folders (also AUTO_CROP_HOME)",
        ),
        sw(
            "no-config",
            None,
            "Ignore settings.toml (default retention, no one-time notice record)",
        ),
        sw("help", Some('h'), "Show help for the command"),
    ]
}

fn input_flags() -> Vec<Flag> {
    vec![
        sw(
            "recursive",
            Some('r'),
            "Also take the images in sub-folders (links are never followed)",
        ),
        val(
            "max-depth",
            None,
            "N",
            "Deepest sub-folder level taken with -r (default 64)",
        ),
        val(
            "max-files",
            None,
            "N",
            "Stop collecting after N files (default 50000)",
        ),
        val(
            "include",
            None,
            "GLOB",
            "Only files whose name matches GLOB (repeatable)",
        ),
        val(
            "exclude",
            None,
            "GLOB",
            "Skip files whose name matches GLOB (repeatable)",
        ),
    ]
}

fn detect_flags() -> Vec<Flag> {
    vec![
        val(
            "triage",
            None,
            "strict|balanced|aggressive",
            "How sure a result must be to be written (default strict; balanced is experimental)",
        ),
        val(
            "min-confidence",
            None,
            "SCORE",
            "Use this score as the cut-off instead of the triage preset (0.60 to 1.0)",
        ),
        val(
            "margin",
            None,
            "PERCENT",
            "Grow (or, if negative, trim) every crop by PERCENT of its size on each side (default 0)",
        ),
        val(
            "split",
            None,
            "auto|always|never",
            "Look for several items on one scan (default auto)",
        ),
        val(
            "profile",
            None,
            "photos|receipts",
            "What the items on a scan are (default photos)",
        ),
    ]
}

fn pool_flags() -> Vec<Flag> {
    vec![
        val(
            "jobs",
            Some('j'),
            "N",
            "Images processed at once (default: half the cores, at most 4)",
        ),
        val(
            "mem-limit",
            None,
            "MB",
            "Cap on the memory the decoded images of the running jobs may use",
        ),
    ]
}

pub fn process_flags() -> Vec<Flag> {
    let mut f = input_flags();
    f.extend([
        sw(
            "in-place",
            None,
            "Overwrite the originals after a verified backup (the default)",
        ),
        val(
            "output",
            Some('o'),
            "DIR",
            "Write the results to DIR (mirrors the folder tree) and leave the originals alone",
        ),
        val(
            "suffix",
            None,
            "TEXT",
            "Write the results beside the originals as <name>TEXT.<ext>",
        ),
        sw(
            "copy",
            None,
            "Write the results to an AutoCrop folder beside each original",
        ),
        val(
            "name-template",
            None,
            "TEMPLATE",
            "Name of a written file: {name}, {n} (item number of a split), {ext}",
        ),
        val(
            "if-exists",
            None,
            "keep-both|skip",
            "When a copy's name is taken: number it (default) or skip the image",
        ),
        val(
            "format",
            None,
            "jpg|png|keep",
            "Output format (default keep; a change needs --output, --suffix or --copy)",
        ),
        val(
            "quality",
            None,
            "small|balanced|best|1-100",
            "JPEG quality of written files: a preset taken from the source's own quality (default balanced) or a fixed number",
        ),
        sw(
            "strip-location",
            None,
            "Drop GPS, XMP and IPTC metadata from written files (other EXIF is kept)",
        ),
        val(
            "max-pixels",
            None,
            "N",
            "Refuse images larger than N pixels (default 100 million, at most 500 million)",
        ),
    ]);
    f.extend(detect_flags());
    f.extend([
        sw(
            "accept-splits",
            None,
            "Replace a scan with several items in place when every item is good (otherwise it is held)",
        ),
        sw(
            "reprocess",
            None,
            "Process files that are outputs of an earlier run again",
        ),
        sw(
            "dry-run",
            Some('n'),
            "Analyse and print the plan; write nothing at all",
        ),
    ]);
    f.extend(pool_flags());
    f.extend([
        val(
            "manifest",
            None,
            "FILE",
            "Also write the run manifest (JSON, see docs/cli.md) to FILE",
        ),
        sw(
            "hold-exit-zero",
            None,
            "Exit 0 when the only problem is held items",
        ),
        val(
            "progress",
            None,
            "auto|always|never",
            "Progress line on stderr (default auto: only on a terminal)",
        ),
    ]);
    f.extend(global_flags());
    f
}

pub fn analyze_flags() -> Vec<Flag> {
    let mut f = input_flags();
    f.extend(detect_flags());
    f.extend([
        val(
            "emit-edit",
            None,
            "DIR",
            "Write each image's edit state as DIR/<name>.edit.json (for `render --edit`)",
        ),
        sw("timings", None, "Include per-stage times in the report"),
        val(
            "max-pixels",
            None,
            "N",
            "Refuse images larger than N pixels (default 100 million, at most 500 million)",
        ),
    ]);
    f.extend(pool_flags());
    f.extend(global_flags());
    f
}

pub fn render_flags() -> Vec<Flag> {
    let mut f = vec![
        val(
            "edit",
            None,
            "FILE",
            "Apply this edit state (from `analyze --emit-edit`) instead of detecting",
        ),
        val(
            "output",
            Some('o'),
            "FILE",
            "Where to write the result (required; a split writes FILE_01, FILE_02, ...)",
        ),
        val(
            "format",
            None,
            "jpg|png|keep",
            "Output format (default keep)",
        ),
        val(
            "quality",
            None,
            "small|balanced|best|1-100",
            "JPEG quality: a preset taken from the source's own quality (default balanced) or a fixed number",
        ),
        sw(
            "strip-location",
            None,
            "Drop GPS, XMP and IPTC metadata from the output",
        ),
        val(
            "max-pixels",
            None,
            "N",
            "Refuse images larger than N pixels (default 100 million, at most 500 million)",
        ),
        sw("force", None, "Overwrite an existing output file"),
    ];
    f.extend(
        detect_flags()
            .into_iter()
            .filter(|x| x.name != "triage" && x.name != "min-confidence"),
    );
    f.extend(global_flags());
    f
}

pub fn restore_flags() -> Vec<Flag> {
    let mut f = vec![
        val(
            "run",
            None,
            "ID",
            "Restore every file of a run (the id is in the manifest and `backups list`)",
        ),
        val(
            "if-modified",
            None,
            "fail|backup|copy",
            "When the file changed since it was saved: stop (default), restore anyway keeping the changed file, or restore as a copy",
        ),
        val(
            "derived",
            None,
            "keep|remove",
            "For a split scan: keep the derived files (default) or move the unchanged ones into the backup",
        ),
        sw(
            "dry-run",
            Some('n'),
            "Say what would be restored; change nothing",
        ),
    ];
    f.extend(global_flags());
    f
}

pub fn backups_flags() -> Vec<Flag> {
    let mut f = vec![
        sw("expired", None, "purge: the backups past their retention"),
        val(
            "older-than",
            None,
            "DAYS",
            "purge: backups made more than DAYS days ago",
        ),
        val("id", None, "ID", "purge: this backup (repeatable)"),
        sw("all", None, "purge: every saved or restored backup"),
        sw(
            "yes",
            Some('y'),
            "purge: do not ask; required when stdin is not a terminal",
        ),
        sw(
            "dry-run",
            Some('n'),
            "purge: list what would be deleted; delete nothing",
        ),
    ];
    f.extend(global_flags());
    f
}

pub fn doctor_flags() -> Vec<Flag> {
    global_flags()
}

// ---------------------------------------------------------------- parsing

/// What `parse` found: flags in order (repeatable ones appear several times) and positionals.
#[derive(Debug, Default, Clone)]
pub struct Parsed {
    pub flags: Vec<(&'static str, Option<String>)>,
    pub positionals: Vec<String>,
}

impl Parsed {
    pub fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| *n == name)
    }

    /// The last value given for `name`.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(n, _)| *n == name)
            .and_then(|(_, v)| v.as_deref())
    }

    pub fn values(&self, name: &str) -> Vec<&str> {
        self.flags
            .iter()
            .filter(|(n, _)| *n == name)
            .filter_map(|(_, v)| v.as_deref())
            .collect()
    }
}

fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

fn suggest(flags: &[Flag], got: &str) -> String {
    flags
        .iter()
        .map(|f| (distance(f.name, got), f.name))
        .filter(|(d, _)| *d <= 2)
        .min()
        .map_or_else(String::new, |(_, n)| format!(" (did you mean `--{n}`?)"))
}

/// Parses `args` (everything after the command word) against `flags`.
pub fn parse(flags: &[Flag], args: &[String]) -> Res<Parsed> {
    let mut out = Parsed::default();
    let mut i = 0;
    let mut only_positionals = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if only_positionals || arg == "-" || !arg.starts_with('-') {
            out.positionals.push(arg.clone());
            continue;
        }
        if arg == "--" {
            only_positionals = true;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_owned())),
                None => (long, None),
            };
            let Some(flag) = flags.iter().find(|f| f.name == name) else {
                return usage(format!("unknown option `--{name}`{}", suggest(flags, name)));
            };
            match (flag.value, inline) {
                (None, None) => out.flags.push((flag.name, None)),
                (None, Some(_)) => return usage(format!("`--{name}` takes no value")),
                (Some(_), Some(v)) => out.flags.push((flag.name, Some(v))),
                (Some(vn), None) => {
                    let Some(v) = args.get(i) else {
                        return usage(format!("`--{name}` needs a value ({vn})"));
                    };
                    i += 1;
                    out.flags.push((flag.name, Some(v.clone())));
                }
            }
            continue;
        }
        // Short flags: a cluster of switches, the last of which may take a value.
        let shorts: Vec<char> = arg[1..].chars().collect();
        let mut k = 0;
        while k < shorts.len() {
            let c = shorts[k];
            k += 1;
            let Some(flag) = flags.iter().find(|f| f.short == Some(c)) else {
                return usage(format!("unknown option `-{c}`"));
            };
            match flag.value {
                None => out.flags.push((flag.name, None)),
                Some(vn) => {
                    let rest: String = shorts[k..].iter().collect();
                    let v = if !rest.is_empty() {
                        rest.trim_start_matches('=').to_owned()
                    } else if let Some(v) = args.get(i) {
                        i += 1;
                        v.clone()
                    } else {
                        return usage(format!("`-{c}` needs a value ({vn})"));
                    };
                    out.flags.push((flag.name, Some(v)));
                    break;
                }
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- typed commands

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Global {
    pub quiet: bool,
    pub verbose: bool,
    pub json: bool,
    pub ndjson: bool,
    pub home: Option<PathBuf>,
    pub no_config: bool,
}

fn global(p: &Parsed) -> Res<Global> {
    let g = Global {
        quiet: p.has("quiet"),
        verbose: p.has("verbose"),
        json: p.has("json"),
        ndjson: p.has("ndjson"),
        home: p.value("home").map(PathBuf::from),
        no_config: p.has("no-config"),
    };
    if g.quiet && g.verbose {
        return usage("`--quiet` and `--verbose` cannot be used together");
    }
    if g.json && g.ndjson {
        return usage("`--json` and `--ndjson` cannot be used together");
    }
    Ok(g)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputArgs {
    pub paths: Vec<String>,
    pub recursive: bool,
    pub max_depth: usize,
    pub max_files: usize,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

pub const DEFAULT_MAX_DEPTH: usize = 64;
pub const DEFAULT_MAX_FILES: usize = 50_000;

fn number<T: std::str::FromStr + PartialOrd + std::fmt::Display + Copy>(
    p: &Parsed,
    name: &str,
    lo: T,
    hi: T,
) -> Res<Option<T>> {
    let Some(v) = p.value(name) else {
        return Ok(None);
    };
    match v.trim().parse::<T>() {
        Ok(n) if n >= lo && n <= hi => Ok(Some(n)),
        _ => usage(format!(
            "`--{name}` needs a number from {lo} to {hi}, not `{v}`"
        )),
    }
}

fn inputs(p: &Parsed, what: &str) -> Res<InputArgs> {
    if p.positionals.is_empty() {
        return usage(format!(
            "{what}: no input given (a file, a folder or a pattern)"
        ));
    }
    Ok(InputArgs {
        paths: p.positionals.clone(),
        recursive: p.has("recursive"),
        max_depth: number(p, "max-depth", 0usize, 256)?.unwrap_or(DEFAULT_MAX_DEPTH),
        max_files: number(p, "max-files", 1usize, 10_000_000)?.unwrap_or(DEFAULT_MAX_FILES),
        include: p.values("include").into_iter().map(str::to_owned).collect(),
        exclude: p.values("exclude").into_iter().map(str::to_owned).collect(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Triage {
    Strict,
    Balanced,
    Aggressive,
}

impl Triage {
    pub fn name(self) -> &'static str {
        match self {
            Triage::Strict => "strict",
            Triage::Balanced => "balanced",
            Triage::Aggressive => "aggressive",
        }
    }

    /// The interim cut-offs on the uncalibrated v0 score (PLAN 4.9).
    pub fn cutoff(self) -> f32 {
        match self {
            Triage::Strict => auto_crop_core::STRICT_CUTOFF,
            Triage::Balanced => 0.90,
            Triage::Aggressive => 0.80,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectArgs {
    pub triage: Triage,
    /// The effective cut-off: `--min-confidence`, else the triage preset's.
    pub cutoff: f32,
    pub margin: f32,
    pub split: SplitPolicy,
    pub profile: SplitProfile,
}

fn detect(p: &Parsed) -> Res<DetectArgs> {
    let triage = match p.value("triage") {
        None | Some("strict") => Triage::Strict,
        Some("balanced") => Triage::Balanced,
        Some("aggressive") => Triage::Aggressive,
        Some(o) => {
            return usage(format!(
                "unknown --triage `{o}` (strict, balanced or aggressive)"
            ));
        }
    };
    let min = number(p, "min-confidence", 0.60f32, 1.0)?;
    if p.has("min-confidence")
        && p.value("min-confidence")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("nan"))
    {
        return usage("`--min-confidence` needs a number from 0.6 to 1");
    }
    let margin = number(p, "margin", -40.0f32, 100.0)?.unwrap_or(0.0);
    let split = match p.value("split") {
        None | Some("auto") => SplitPolicy::Auto,
        Some("always") => SplitPolicy::Always,
        Some("never") => SplitPolicy::Never,
        Some(o) => return usage(format!("unknown --split `{o}` (auto, always or never)")),
    };
    let profile = match p.value("profile") {
        None | Some("photos") => SplitProfile::Photos,
        Some("receipts") => SplitProfile::Receipts,
        Some(o) => return usage(format!("unknown --profile `{o}` (photos or receipts)")),
    };
    Ok(DetectArgs {
        triage,
        cutoff: min.unwrap_or_else(|| triage.cutoff()),
        margin,
        split,
        profile,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatArg {
    Keep,
    Jpg,
    Png,
}

impl FormatArg {
    pub fn name(self) -> &'static str {
        match self {
            FormatArg::Keep => "keep",
            FormatArg::Jpg => "jpg",
            FormatArg::Png => "png",
        }
    }
}

/// The output knobs that are the engine's own options (quality, metadata, pixel cap).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Knobs {
    pub quality: Option<QualitySetting>,
    pub strip_location: bool,
    pub max_pixels: Option<u64>,
}

impl Knobs {
    /// The engine's options for a run: its defaults with what the command line changed.
    pub fn engine_options(&self) -> EngineOptions {
        let mut o = EngineOptions::default();
        if let Some(q) = self.quality {
            o.quality = q;
        }
        o.strip_location = self.strip_location;
        if let Some(m) = self.max_pixels {
            o.max_pixels = m;
        }
        o
    }

    /// The quality as written in the manifest: `small`, `balanced`, `best` or the number.
    pub fn quality_name(&self) -> Option<String> {
        self.quality.map(|q| match q {
            QualitySetting::Fixed { value } => value.to_string(),
            QualitySetting::Preset { preset } => match preset {
                QualityPreset::Small => "small",
                QualityPreset::Balanced => "balanced",
                QualityPreset::Best => "best",
            }
            .to_owned(),
        })
    }
}

fn knobs(p: &Parsed) -> Res<Knobs> {
    let quality = match p.value("quality") {
        None => None,
        Some("small") => Some(QualitySetting::Preset {
            preset: QualityPreset::Small,
        }),
        Some("balanced") => Some(QualitySetting::Preset {
            preset: QualityPreset::Balanced,
        }),
        Some("best") => Some(QualitySetting::Preset {
            preset: QualityPreset::Best,
        }),
        Some(_) => number(p, "quality", 1u8, 100)
            .map_err(|_| {
                Usage(
                    "`--quality` needs small, balanced, best or a number from 1 to 100".to_owned(),
                )
            })?
            .map(|value| QualitySetting::Fixed { value }),
    };
    Ok(Knobs {
        quality,
        strip_location: p.has("strip-location"),
        max_pixels: number(p, "max-pixels", 1_000_000u64, 500_000_000)?,
    })
}

fn format_arg(p: &Parsed) -> Res<FormatArg> {
    match p.value("format") {
        None | Some("keep") => Ok(FormatArg::Keep),
        Some("jpg" | "jpeg") => Ok(FormatArg::Jpg),
        Some("png") => Ok(FormatArg::Png),
        Some(o) => usage(format!("unknown --format `{o}` (jpg, png or keep)")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMode {
    /// Overwrite the original after a verified backup. `explicit` is `--in-place` (no notice
    /// difference, only recorded).
    InPlace { explicit: bool },
    /// `--output DIR`.
    Dir(PathBuf),
    /// `--suffix TEXT`.
    Suffix(String),
    /// `--copy`: `<folder>/AutoCrop/`.
    Copy,
}

impl OutputMode {
    pub fn name(&self) -> &'static str {
        match self {
            OutputMode::InPlace { .. } => "in_place",
            OutputMode::Dir(_) => "output",
            OutputMode::Suffix(_) => "suffix",
            OutputMode::Copy => "copy",
        }
    }

    pub fn replaces_originals(&self) -> bool {
        matches!(self, OutputMode::InPlace { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfExists {
    KeepBoth,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone)]
pub struct ProcessArgs {
    pub input: InputArgs,
    pub detect: DetectArgs,
    pub mode: OutputMode,
    pub name_template: Option<String>,
    pub if_exists: IfExists,
    pub format: FormatArg,
    pub knobs: Knobs,
    pub accept_splits: bool,
    pub reprocess: bool,
    pub dry_run: bool,
    pub jobs: Option<usize>,
    pub mem_limit_mb: Option<u64>,
    pub manifest: Option<PathBuf>,
    pub hold_exit_zero: bool,
    pub progress: Progress,
}

fn progress(p: &Parsed) -> Res<Progress> {
    match p.value("progress") {
        None | Some("auto") => Ok(Progress::Auto),
        Some("always") => Ok(Progress::Always),
        Some("never") => Ok(Progress::Never),
        Some(o) => usage(format!("unknown --progress `{o}` (auto, always or never)")),
    }
}

fn pool(p: &Parsed) -> Res<(Option<usize>, Option<u64>)> {
    Ok((
        number(p, "jobs", 1usize, 256)?,
        number(p, "mem-limit", 64u64, 1_048_576)?,
    ))
}

fn process(p: &Parsed) -> Res<ProcessArgs> {
    let modes = ["in-place", "output", "suffix", "copy"]
        .iter()
        .filter(|m| p.has(m))
        .count();
    if modes > 1 {
        return usage("give at most one of --in-place, --output, --suffix and --copy");
    }
    let mode = if let Some(d) = p.value("output") {
        if d.is_empty() {
            return usage("`--output` needs a folder");
        }
        OutputMode::Dir(PathBuf::from(d))
    } else if let Some(s) = p.value("suffix") {
        if s.is_empty() || s.contains(['{', '}', '/', '\\']) {
            return usage("`--suffix` needs plain text without braces or path separators");
        }
        OutputMode::Suffix(s.to_owned())
    } else if p.has("copy") {
        OutputMode::Copy
    } else {
        OutputMode::InPlace {
            explicit: p.has("in-place"),
        }
    };
    let format = format_arg(p)?;
    if mode.replaces_originals() && format != FormatArg::Keep {
        return usage(
            "changing the format needs --output, --suffix or --copy: replacing an original with \
             another format is not available yet",
        );
    }
    let name_template = p.value("name-template").map(str::to_owned);
    if name_template.is_some() && mode.replaces_originals() {
        return usage("`--name-template` needs --output, --suffix or --copy");
    }
    if let Some(t) = &name_template {
        // The same grammar and sanitiser as every other output name.
        auto_crop_engine::fsplan::expand_name(t, "name", None, "jpg")
            .map_err(|e| Usage(format!("`--name-template`: {e}")))?;
        if t.contains(['/', '\\']) {
            return usage("`--name-template` must not contain path separators");
        }
    }
    let if_exists = match p.value("if-exists") {
        None | Some("keep-both") => IfExists::KeepBoth,
        Some("skip") => IfExists::Skip,
        Some("replace") => {
            return usage("`--if-exists replace` is not available: copies never overwrite a file");
        }
        Some(o) => return usage(format!("unknown --if-exists `{o}` (keep-both or skip)")),
    };
    if p.has("if-exists") && mode.replaces_originals() {
        return usage("`--if-exists` needs --output, --suffix or --copy");
    }
    let (jobs, mem_limit_mb) = pool(p)?;
    Ok(ProcessArgs {
        input: inputs(p, "process")?,
        detect: detect(p)?,
        mode,
        name_template,
        if_exists,
        format,
        knobs: knobs(p)?,
        accept_splits: p.has("accept-splits"),
        reprocess: p.has("reprocess"),
        dry_run: p.has("dry-run"),
        jobs,
        mem_limit_mb,
        manifest: p.value("manifest").map(PathBuf::from),
        hold_exit_zero: p.has("hold-exit-zero"),
        progress: progress(p)?,
    })
}

#[derive(Debug, Clone)]
pub struct AnalyzeArgs {
    pub input: InputArgs,
    pub detect: DetectArgs,
    pub emit_edit: Option<PathBuf>,
    pub timings: bool,
    pub knobs: Knobs,
    pub jobs: Option<usize>,
    pub mem_limit_mb: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RenderArgs {
    pub input: String,
    pub output: PathBuf,
    pub edit: Option<PathBuf>,
    pub format: FormatArg,
    pub knobs: Knobs,
    pub force: bool,
    pub margin: f32,
    pub split: SplitPolicy,
    pub profile: SplitProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfModified {
    Fail,
    Backup,
    Copy,
}

#[derive(Debug, Clone)]
pub struct RestoreArgs {
    pub targets: Vec<String>,
    pub run: Option<String>,
    pub if_modified: IfModified,
    pub remove_derived: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeSelect {
    Expired,
    OlderThan(u32),
    Ids(Vec<String>),
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackupsCmd {
    List,
    Show(String),
    Purge {
        select: PurgeSelect,
        yes: bool,
        dry_run: bool,
    },
}

#[derive(Debug, Clone)]
pub enum Command {
    Help(Option<&'static str>),
    Version,
    Process(Box<ProcessArgs>),
    Analyze(Box<AnalyzeArgs>),
    Render(Box<RenderArgs>),
    Restore(RestoreArgs),
    Backups(BackupsCmd),
    Doctor,
    /// The developer pipeline of M1.54: the raw argument list, parsed by its own module.
    DevPipeline(Vec<String>),
}

/// A parsed command line.
#[derive(Debug, Clone)]
pub struct Cli {
    pub global: Global,
    pub command: Command,
}

pub const COMMANDS: [(&str, &str); 6] = [
    (
        "process",
        "Detect, crop and straighten images (overwrites originals after a verified backup unless told otherwise)",
    ),
    (
        "analyze",
        "Detect only: print the crop and the confidence of each image; writes no images",
    ),
    (
        "render",
        "Write one crop to a chosen file without touching the source",
    ),
    ("restore", "Put originals back from the backup store"),
    (
        "backups",
        "List, show or purge the backups (backups list | show ID | purge ...)",
    ),
    (
        "doctor",
        "Check this machine: CPU floor, memory, decoders, the backup store",
    ),
];

/// The name of a command as written on the command line, to find its flag table.
pub fn flags_for(command: &str) -> Option<Vec<Flag>> {
    Some(match command {
        "process" => process_flags(),
        "analyze" => analyze_flags(),
        "render" => render_flags(),
        "restore" => restore_flags(),
        "backups" => backups_flags(),
        "doctor" => doctor_flags(),
        _ => return None,
    })
}

fn static_command(name: &str) -> Option<&'static str> {
    COMMANDS.iter().find(|(n, _)| *n == name).map(|(n, _)| *n)
}

/// Parses `args` (without the program name).
pub fn parse_cli(args: &[String]) -> Res<Cli> {
    // Global flags may come before the command word; find it first.
    let gl = global_flags();
    let mut pre: Vec<String> = Vec::new();
    let mut idx = 0;
    let mut command_word: Option<&str> = None;
    while idx < args.len() {
        let a = args[idx].as_str();
        if a == "-V" || a == "--version" {
            return Ok(Cli {
                global: Global::default(),
                command: Command::Version,
            });
        }
        if a == "--" || !a.starts_with('-') || a == "-" {
            command_word = Some(a);
            idx += 1;
            break;
        }
        pre.push(args[idx].clone());
        // A global flag with a value consumes the next argument too.
        let name = a.trim_start_matches('-');
        let takes = a.starts_with("--")
            && !a.contains('=')
            && gl.iter().any(|f| f.name == name && f.value.is_some());
        if takes && idx + 1 < args.len() {
            idx += 1;
            pre.push(args[idx].clone());
        }
        idx += 1;
    }
    let Some(word) = command_word else {
        // Only flags (or nothing): help or a usage error.
        let p = parse(&gl, &pre)?;
        let g = global(&p)?;
        if p.has("help") || pre.is_empty() {
            return Ok(Cli {
                global: g,
                command: Command::Help(None),
            });
        }
        return usage("no command given (try `auto-crop --help`)");
    };
    if word == "help" {
        let topic = args.get(idx).and_then(|a| static_command(a));
        return Ok(Cli {
            global: Global::default(),
            command: Command::Help(topic),
        });
    }
    if word == "version" {
        return Ok(Cli {
            global: Global::default(),
            command: Command::Version,
        });
    }
    if word == "dev-pipeline" {
        return Ok(Cli {
            global: Global::default(),
            command: Command::DevPipeline(args[idx..].to_vec()),
        });
    }
    let Some(flags) = flags_for(word) else {
        let near = COMMANDS
            .iter()
            .map(|(n, _)| (distance(n, word), *n))
            .filter(|(d, _)| *d <= 2)
            .min()
            .map_or_else(String::new, |(_, n)| format!(" (did you mean `{n}`?)"));
        return usage(format!("unknown command `{word}`{near}"));
    };
    let mut rest = pre;
    rest.extend(args[idx..].iter().cloned());
    let p = parse(&flags, &rest)?;
    let g = global(&p)?;
    let name = static_command(word).expect("flags_for matched");
    if p.has("help") {
        return Ok(Cli {
            global: g,
            command: Command::Help(Some(name)),
        });
    }
    let command = match word {
        "process" => Command::Process(Box::new(process(&p)?)),
        "analyze" => {
            let (jobs, mem_limit_mb) = pool(&p)?;
            Command::Analyze(Box::new(AnalyzeArgs {
                input: inputs(&p, "analyze")?,
                detect: detect(&p)?,
                emit_edit: p.value("emit-edit").map(PathBuf::from),
                knobs: knobs(&p)?,
                timings: p.has("timings"),
                jobs,
                mem_limit_mb,
            }))
        }
        "render" => {
            let [input] = p.positionals.as_slice() else {
                return usage("render: give exactly one input image");
            };
            let Some(out) = p.value("output") else {
                return usage("render: `--output FILE` is required");
            };
            let split = match p.value("split") {
                None | Some("auto") => SplitPolicy::Auto,
                Some("always") => SplitPolicy::Always,
                Some("never") => SplitPolicy::Never,
                Some(o) => return usage(format!("unknown --split `{o}` (auto, always or never)")),
            };
            let d = detect_for_render(&p)?;
            Command::Render(Box::new(RenderArgs {
                input: input.clone(),
                output: PathBuf::from(out),
                edit: p.value("edit").map(PathBuf::from),
                format: format_arg(&p)?,
                knobs: knobs(&p)?,
                force: p.has("force"),
                margin: d.0,
                split,
                profile: d.1,
            }))
        }
        "restore" => {
            let if_modified = match p.value("if-modified") {
                None | Some("fail") => IfModified::Fail,
                Some("backup") => IfModified::Backup,
                Some("copy") => IfModified::Copy,
                Some(o) => {
                    return usage(format!(
                        "unknown --if-modified `{o}` (fail, backup or copy)"
                    ));
                }
            };
            let remove_derived = match p.value("derived") {
                None | Some("keep") => false,
                Some("remove") => true,
                Some(o) => return usage(format!("unknown --derived `{o}` (keep or remove)")),
            };
            let run = p.value("run").map(str::to_owned);
            if run.is_none() && p.positionals.is_empty() {
                return usage("restore: give a file path, a backup id or --run ID");
            }
            if run.is_some() && !p.positionals.is_empty() {
                return usage("restore: give either targets or --run, not both");
            }
            Command::Restore(RestoreArgs {
                targets: p.positionals.clone(),
                run,
                if_modified,
                remove_derived,
                dry_run: p.has("dry-run"),
            })
        }
        "backups" => Command::Backups(backups(&p)?),
        "doctor" => {
            if !p.positionals.is_empty() {
                return usage("doctor takes no arguments");
            }
            Command::Doctor
        }
        _ => unreachable!("flags_for matched"),
    };
    Ok(Cli { global: g, command })
}

fn detect_for_render(p: &Parsed) -> Res<(f32, SplitProfile)> {
    let margin = number(p, "margin", -40.0f32, 100.0)?.unwrap_or(0.0);
    let profile = match p.value("profile") {
        None | Some("photos") => SplitProfile::Photos,
        Some("receipts") => SplitProfile::Receipts,
        Some(o) => return usage(format!("unknown --profile `{o}` (photos or receipts)")),
    };
    Ok((margin, profile))
}

fn backups(p: &Parsed) -> Res<BackupsCmd> {
    let Some(sub) = p.positionals.first() else {
        return usage("backups: say what to do (list, show ID or purge)");
    };
    match sub.as_str() {
        "list" => {
            if p.positionals.len() != 1 {
                return usage("backups list takes no arguments");
            }
            Ok(BackupsCmd::List)
        }
        "show" => match p.positionals.as_slice() {
            [_, id] => Ok(BackupsCmd::Show(id.clone())),
            _ => usage("backups show: give one backup id"),
        },
        "purge" => {
            if p.positionals.len() != 1 {
                return usage(
                    "backups purge takes no arguments; use --expired, --older-than, --id or --all",
                );
            }
            let mut sel: Vec<PurgeSelect> = Vec::new();
            if p.has("expired") {
                sel.push(PurgeSelect::Expired);
            }
            if let Some(d) = number(p, "older-than", 0u32, 36_500)? {
                sel.push(PurgeSelect::OlderThan(d));
            }
            let ids: Vec<String> = p.values("id").into_iter().map(str::to_owned).collect();
            if !ids.is_empty() {
                sel.push(PurgeSelect::Ids(ids));
            }
            if p.has("all") {
                sel.push(PurgeSelect::All);
            }
            match sel.len() {
                0 => usage(
                    "backups purge: choose one of --expired, --older-than DAYS, --id ID or --all",
                ),
                1 => Ok(BackupsCmd::Purge {
                    select: sel.remove(0),
                    yes: p.has("yes"),
                    dry_run: p.has("dry-run"),
                }),
                _ => usage(
                    "backups purge: choose only one of --expired, --older-than, --id and --all",
                ),
            }
        }
        other => usage(format!(
            "unknown backups command `{other}` (list, show or purge)"
        )),
    }
}

// ---------------------------------------------------------------- help

fn flag_line(f: &Flag) -> String {
    let short = f
        .short
        .map_or_else(|| "    ".to_owned(), |c| format!("-{c}, "));
    let long = match f.value {
        Some(v) => format!("--{} <{v}>", f.name),
        None => format!("--{}", f.name),
    };
    format!("  {short}{long}")
}

fn wrap(text: &str, indent: usize, width: usize) -> String {
    let mut out = String::new();
    let mut line_len = indent;
    for word in text.split_whitespace() {
        if line_len + word.len() + 1 > width && line_len > indent {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            line_len = indent;
        } else if line_len > indent {
            out.push(' ');
            line_len += 1;
        }
        out.push_str(word);
        line_len += word.len();
    }
    out
}

/// The `--help` text of one command, or the overview.
pub fn help(topic: Option<&str>) -> String {
    let mut s = String::new();
    match topic {
        None => {
            s.push_str("auto-crop: crop, rotate and straighten photos of documents, receipts and scans.\n\n");
            s.push_str("usage: auto-crop <command> [options]\n\ncommands:\n");
            for (n, about) in COMMANDS {
                s.push_str(&format!("  {n:<9} {}\n", wrap(about, 12, 90)));
            }
            s.push_str("\nglobal options:\n");
            for f in global_flags() {
                s.push_str(&format!(
                    "{}\n      {}\n",
                    flag_line(&f),
                    wrap(f.help, 6, 90)
                ));
            }
            s.push_str("  -V, --version\n      Print the version, the build and what this build can decode\n\n");
            s.push_str("Run `auto-crop <command> --help` for the options of a command, and see\ndocs/cli.md for the reference, the exit codes and the run manifest.\n");
        }
        Some(cmd) => {
            let about = COMMANDS
                .iter()
                .find(|(n, _)| *n == cmd)
                .map_or("", |(_, a)| *a);
            let line = match cmd {
                "process" => "auto-crop process [options] <file|folder|pattern>...",
                "analyze" => "auto-crop analyze [options] <file|folder|pattern>...",
                "render" => "auto-crop render [options] --output <file> <image>",
                "restore" => "auto-crop restore [options] <file|backup-id>...   |   --run <id>",
                "backups" => {
                    "auto-crop backups list | show <id> | purge (--expired | --older-than N | --id ID | --all)"
                }
                _ => "auto-crop doctor [options]",
            };
            s.push_str(&format!("{about}\n\nusage: {line}\n\noptions:\n"));
            let flags = flags_for(cmd).unwrap_or_default();
            for f in &flags {
                s.push_str(&format!(
                    "{}\n      {}\n",
                    flag_line(f),
                    wrap(f.help, 6, 90)
                ));
            }
            s.push_str("\nExit codes: 0 ok, 1 internal error, 2 usage, 3 some failed, 4 held items, 5 no supported input, 6 precondition failed, 130 interrupted.\n");
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    fn process_of(a: &[&str]) -> Res<ProcessArgs> {
        match parse_cli(&v(a))?.command {
            Command::Process(p) => Ok(*p),
            other => panic!("not process: {other:?}"),
        }
    }

    #[test]
    fn defaults_are_strict_in_place_auto_split() {
        let p = process_of(&["process", "a.jpg"]).unwrap();
        assert_eq!(p.detect.triage, Triage::Strict);
        assert_eq!(p.detect.cutoff, 0.95);
        assert_eq!(p.mode, OutputMode::InPlace { explicit: false });
        assert_eq!(p.detect.split, SplitPolicy::Auto);
        assert_eq!(p.detect.profile, SplitProfile::Photos);
        assert_eq!(p.format, FormatArg::Keep);
        assert!(!p.dry_run && !p.accept_splits && !p.reprocess);
        assert_eq!(p.input.max_depth, 64);
        assert_eq!(p.input.max_files, 50_000);
    }

    #[test]
    fn every_documented_option_parses() {
        let p = process_of(&[
            "process",
            "-r",
            "--max-depth=3",
            "--max-files",
            "10",
            "--include",
            "*.jpg",
            "--exclude",
            "x*",
            "--suffix",
            "_c",
            "--format",
            "png",
            "--quality",
            "80",
            "--triage",
            "balanced",
            "--margin",
            "-2.5",
            "--split",
            "never",
            "--profile",
            "receipts",
            "--reprocess",
            "-n",
            "-j",
            "2",
            "--mem-limit",
            "512",
            "--manifest",
            "m.json",
            "--hold-exit-zero",
            "--progress",
            "never",
            "--if-exists",
            "skip",
            "--name-template",
            "{name}-{n}",
            "dir",
            "b.png",
        ])
        .unwrap();
        assert!(p.input.recursive && p.dry_run && p.reprocess && p.hold_exit_zero);
        assert_eq!(p.input.paths, ["dir", "b.png"]);
        assert_eq!(p.detect.cutoff, 0.90);
        assert_eq!(p.detect.margin, -2.5);
        assert_eq!(p.detect.split, SplitPolicy::Never);
        assert_eq!(p.mode, OutputMode::Suffix("_c".to_owned()));
        assert_eq!(
            (p.jobs, p.mem_limit_mb, p.knobs.quality),
            (
                Some(2),
                Some(512),
                Some(QualitySetting::Fixed { value: 80 })
            )
        );
        assert_eq!(p.if_exists, IfExists::Skip);
        assert_eq!(p.progress, Progress::Never);
    }

    #[test]
    fn min_confidence_overrides_the_preset_and_has_a_floor() {
        let p = process_of(&["process", "a.jpg", "--min-confidence", "0.7"]).unwrap();
        assert_eq!(p.detect.cutoff, 0.7);
        assert!(process_of(&["process", "a.jpg", "--min-confidence", "0.5"]).is_err());
        assert!(process_of(&["process", "a.jpg", "--min-confidence", "1.5"]).is_err());
        assert!(process_of(&["process", "a.jpg", "--min-confidence", "nan"]).is_err());
    }

    #[test]
    fn bad_command_lines_are_usage_errors() {
        for bad in [
            &["process"][..],
            &["process", "a.jpg", "--nope"],
            &["process", "a.jpg", "--quality", "0"],
            &["process", "a.jpg", "--quality", "101"],
            &["process", "a.jpg", "--quality", "x"],
            &["process", "a.jpg", "--quality"],
            &["process", "a.jpg", "--jobs", "0"],
            &["process", "a.jpg", "--triage", "wild"],
            &["process", "a.jpg", "--split", "maybe"],
            &["process", "a.jpg", "--profile", "x"],
            &["process", "a.jpg", "--format", "gif"],
            &["process", "a.jpg", "--margin", "500"],
            &["process", "a.jpg", "--output", "o", "--suffix", "s"],
            &["process", "a.jpg", "--copy", "--in-place"],
            &["process", "a.jpg", "--suffix", "{x}"],
            &["process", "a.jpg", "--format", "png"],
            &["process", "a.jpg", "--name-template", "{name}"],
            &[
                "process",
                "a.jpg",
                "--output",
                "o",
                "--name-template",
                "{bogus}",
            ],
            &[
                "process",
                "a.jpg",
                "--output",
                "o",
                "--if-exists",
                "replace",
            ],
            &["process", "a.jpg", "--dry-run=yes"],
            &["process", "a.jpg", "-q", "-v"],
            &["process", "a.jpg", "--json", "--ndjson"],
            &["frobnicate"],
            &["render", "a.jpg"],
            &["render", "-o", "x.jpg"],
            &["restore"],
            &["restore", "x", "--run", "y"],
            &["backups"],
            &["backups", "purge"],
            &["backups", "purge", "--all", "--expired"],
            &["backups", "show"],
            &["backups", "list", "extra"],
            &["doctor", "extra"],
        ] {
            assert!(parse_cli(&v(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn errors_name_the_problem_and_suggest() {
        let e = parse_cli(&v(&["process", "a.jpg", "--recursiv"])).unwrap_err();
        assert!(
            e.0.contains("--recursiv") && e.0.contains("did you mean `--recursive`"),
            "{e}"
        );
        let e = parse_cli(&v(&["proces", "a.jpg"])).unwrap_err();
        assert!(e.0.contains("did you mean `process`"), "{e}");
    }

    #[test]
    fn clusters_attached_values_and_the_double_dash() {
        let p = process_of(&["process", "-rn", "-j4", "--", "-weird.jpg"]).unwrap();
        assert!(p.input.recursive && p.dry_run);
        assert_eq!(p.jobs, Some(4));
        assert_eq!(p.input.paths, ["-weird.jpg"]);
        // A value may look like a flag.
        let p = process_of(&["process", "a.jpg", "--output", "--odd-folder"]).unwrap();
        assert_eq!(p.mode, OutputMode::Dir(PathBuf::from("--odd-folder")));
    }

    #[test]
    fn global_flags_may_come_before_or_after_the_command() {
        let c = parse_cli(&v(&["--quiet", "--home", "h", "process", "a.jpg"])).unwrap();
        assert!(c.global.quiet);
        assert_eq!(c.global.home, Some(PathBuf::from("h")));
        let c = parse_cli(&v(&["process", "a.jpg", "--json", "--no-config"])).unwrap();
        assert!(c.global.json && c.global.no_config);
    }

    #[test]
    fn help_and_version() {
        assert!(matches!(
            parse_cli(&v(&[])).unwrap().command,
            Command::Help(None)
        ));
        assert!(matches!(
            parse_cli(&v(&["--help"])).unwrap().command,
            Command::Help(None)
        ));
        assert!(matches!(
            parse_cli(&v(&["help", "process"])).unwrap().command,
            Command::Help(Some("process"))
        ));
        assert!(matches!(
            parse_cli(&v(&["process", "--help"])).unwrap().command,
            Command::Help(Some("process"))
        ));
        assert!(matches!(
            parse_cli(&v(&["-V"])).unwrap().command,
            Command::Version
        ));
        assert!(parse_cli(&v(&["process", "a", "--version"])).is_err());
        let h = help(Some("process"));
        assert!(h.contains("--dry-run") && h.contains("Exit codes"));
        assert!(help(None).contains("restore"));
    }

    #[test]
    fn restore_and_backups_commands() {
        match parse_cli(&v(&[
            "restore",
            "a.jpg",
            "--if-modified",
            "copy",
            "--derived",
            "remove",
            "-n",
        ]))
        .unwrap()
        .command
        {
            Command::Restore(r) => {
                assert_eq!(r.if_modified, IfModified::Copy);
                assert!(r.remove_derived && r.dry_run);
            }
            other => panic!("{other:?}"),
        }
        match parse_cli(&v(&["backups", "purge", "--older-than", "10", "--yes"]))
            .unwrap()
            .command
        {
            Command::Backups(BackupsCmd::Purge { select, yes, .. }) => {
                assert_eq!(select, PurgeSelect::OlderThan(10));
                assert!(yes);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A fuzz-ish pass: arbitrary argument vectors never panic and are either a command or a
    /// usage error (the parser never touches the file system).
    #[test]
    fn arbitrary_argument_lists_never_panic() {
        let atoms = [
            "process",
            "analyze",
            "render",
            "restore",
            "backups",
            "doctor",
            "list",
            "purge",
            "show",
            "--",
            "-",
            "--=",
            "--x=",
            "-=",
            "---",
            "--recursive",
            "-r",
            "-rr",
            "-j",
            "-j0",
            "--jobs",
            "--jobs=",
            "--output",
            "-o",
            "--quality=",
            "--quality",
            "999999999999999999999",
            "--margin",
            "NaN",
            "inf",
            "-inf",
            "--min-confidence",
            "--triage",
            "strict",
            "",
            " ",
            "\u{202e}",
            "ä",
            "日本語",
            "--format",
            "png",
            "--home",
            "--json",
            "--ndjson",
            "-q",
            "-v",
            "--help",
            "-h",
            "-V",
            "--version",
            "--dry-run",
            "-n",
            "a.jpg",
            "--suffix",
            "_x",
            "--name-template",
            "{n}",
            "{",
            "}",
            "{{}}",
            "--if-exists",
            "skip",
            "replace",
        ];
        // A small deterministic generator: no dependency, reproducible.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut parsed = 0usize;
        for _ in 0..20_000 {
            let n = (next() % 8) as usize;
            let args: Vec<String> = (0..n)
                .map(|_| atoms[(next() % atoms.len() as u64) as usize].to_owned())
                .collect();
            if parse_cli(&args).is_ok() {
                parsed += 1;
            }
        }
        assert!(parsed > 0, "some random lists are valid");
    }
}
