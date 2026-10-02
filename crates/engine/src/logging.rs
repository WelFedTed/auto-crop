// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Local logging (ROADMAP M1.11, PLAN 2.10): `tracing` events written to a daily file in the
//! app's data directory, kept for 7 days and 20 MB in total (PROVISIONAL). Nothing leaves the
//! machine: the only sink is that file (B18, C4).
//!
//! Paths are personal data. Events from `info` up (info, warn, error) never carry one: use
//! [`LogPath`], which renders `blake3(path)[..8]` plus the extension. Events at `debug` and
//! `trace` may use [`DebugPath`], which prints the raw path only while the active subscriber is
//! verbose enough to emit debug events at all. As a second line of defence the file writer
//! scrubs anything that still looks like an absolute path out of every line unless debug logging
//! is on.
//!
//! The daily file writer has an injectable [`Clock`] so rotation and retention are testable, and
//! feeds `tracing-appender`'s non-blocking worker so a slow disk never stalls an image job.

use crate::error::ErrKind;
use crate::paths::AppPaths;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::level_filters::LevelFilter;

/// Days of log files kept, today included (PROVISIONAL).
pub const RETENTION_DAYS: u32 = 7;
/// Total size of all log files (PROVISIONAL).
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

const FILE_PREFIX: &str = "auto-crop.";
const FILE_SUFFIX: &str = ".log";

// ---------------------------------------------------------------------------------------------
// Path redaction
// ---------------------------------------------------------------------------------------------

/// `blake3(path)[..8]` as 8 hex digits plus `.ext` (lower case, at most 8 alphanumerics) when the
/// path has an extension. Stable: the same path always gives the same token, so lines about one
/// file can be correlated without revealing where it lives.
pub fn redact_path(path: &Path) -> String {
    redact_str(&path.to_string_lossy())
}

fn redact_str(path: &str) -> String {
    let hash = blake3::hash(path.as_bytes()).to_hex();
    let mut out = hash.as_str()[..8].to_owned();
    // The extension is whatever follows the last dot of the last path component.
    let name = path.rsplit(['/', '\\']).next().unwrap_or("");
    if let Some((stem, ext)) = name.rsplit_once('.')
        && !stem.is_empty()
    {
        let ext: String = ext
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if !ext.is_empty() {
            out.push('.');
            out.push_str(&ext);
        }
    }
    out
}

/// A path for `info`, `warn` and `error` events: always redacted.
///
/// ```text
/// tracing::info!(file = %LogPath(path), "decoded");
/// ```
#[derive(Clone, Copy)]
pub struct LogPath<'a>(pub &'a Path);

impl fmt::Display for LogPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redact_path(self.0))
    }
}

impl fmt::Debug for LogPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// A path for `debug` and `trace` events: raw while debug logging is on, redacted otherwise. The
/// decision is taken in [`DebugPath::new`], at the call site, because a subscriber cannot be
/// asked anything while it is formatting an event.
#[derive(Clone, Copy)]
pub struct DebugPath<'a> {
    path: &'a Path,
    raw: bool,
}

impl<'a> DebugPath<'a> {
    pub fn new(path: &'a Path) -> Self {
        Self {
            path,
            raw: tracing::enabled!(target: "auto_crop::path", tracing::Level::DEBUG),
        }
    }
}

impl fmt::Display for DebugPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.raw {
            write!(f, "{}", self.path.display())
        } else {
            f.write_str(&redact_path(self.path))
        }
    }
}

impl fmt::Debug for DebugPath<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Replaces anything in `line` that looks like an absolute path (`C:\...`, `\\server\...`,
/// `/dir/...`) with its redacted token. A path runs to the closing quote if it starts after one,
/// else to the next ` key=` field or the end of the line, so names with spaces are covered.
pub fn scrub_line(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        let at_boundary = i == 0
            || matches!(
                bytes[i - 1],
                b' ' | b'"' | b'\'' | b'=' | b'(' | b'[' | b',' | b':' | b'\t'
            );
        if c == b'"' || c == b'\'' {
            quote = if quote == Some(c) { None } else { Some(c) };
        }
        if at_boundary && starts_absolute_path(&line[i..]) {
            let end = path_end(line, i, quote);
            let mut raw = &line[i..end];
            // A trailing separator or period belongs to the sentence, not the path.
            while let Some(stripped) = raw.strip_suffix(['.', ',', ';', ')', ']']) {
                if stripped.is_empty() {
                    break;
                }
                raw = stripped;
            }
            let end = i + raw.len();
            // Debug-format paths escape backslashes; undo that so the hash matches the real path.
            let unescaped = raw.replace("\\\\", "\\");
            out.push_str(&redact_str(&unescaped));
            i = end;
            continue;
        }
        // Copy one full char.
        let ch_len = line[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&line[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn starts_absolute_path(s: &str) -> bool {
    let b = s.as_bytes();
    // Drive letter: `C:\x` or `C:/x`.
    if b.len() >= 4 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
    {
        return true;
    }
    // UNC: `\\server\share`.
    if b.len() >= 4 && b[0] == b'\\' && b[1] == b'\\' && b[2] != b'\\' {
        return true;
    }
    // Unix: `/a/b` (at least two components, so a lone `/` or `a/b` ratio text is not touched).
    if b.len() >= 4 && b[0] == b'/' && b[1] != b'/' && b[1] != b' ' {
        return s[1..].contains('/');
    }
    false
}

/// File extensions that mark the end of an unquoted path.
const PATH_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "tif", "tiff", "webp", "heic", "heif", "avif", "gif", "bmp", "jxl",
    "tmp", "json", "toml", "db", "log", "bak", "pdf", "txt",
];

/// Where the path that starts at `start` ends.
fn path_end(line: &str, start: usize, quote: Option<u8>) -> usize {
    let rest = &line[start..];
    if let Some(q) = quote
        && let Some(p) = rest.find(q as char)
    {
        return start + p;
    }
    // The field runs to the next ` key=` or the end of the line ...
    let b = rest.as_bytes();
    let mut field_end = line.len();
    let mut j = 0;
    while j < b.len() {
        if b[j] == b' ' {
            let mut k = j + 1;
            while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_' || b[k] == b'.') {
                k += 1;
            }
            if k > j + 1 && k < b.len() && b[k] == b'=' {
                field_end = start + j;
                break;
            }
        }
        j += 1;
    }
    // ... but ends after the last known file extension inside it, so text that follows a file
    // name ("opened /a/b.jpg ok") is kept.
    let field = &line[start..field_end];
    let lower = field.to_ascii_lowercase();
    let mut best: Option<usize> = None;
    for ext in PATH_EXTS {
        let needle = format!(".{ext}");
        let mut from = 0;
        while let Some(p) = lower[from..].find(&needle) {
            let at = from + p;
            let after = at + needle.len();
            let boundary = lower
                .as_bytes()
                .get(after)
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            if boundary && best.is_none_or(|b| after > b) {
                best = Some(after);
            }
            from = at + 1;
        }
    }
    best.map_or(field_end, |e| start + e)
}

// ---------------------------------------------------------------------------------------------
// The daily file
// ---------------------------------------------------------------------------------------------

/// Source of the current time; tests substitute their own.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Days since 1970-01-01 (UTC) for a time.
fn day_number(t: SystemTime) -> i64 {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    secs.div_euclid(86_400)
}

/// `YYYY-MM-DD` for a day number (civil-from-days).
fn date_string(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let (y, m, d): (i64, i64, i64) = (
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
        it.next()?.parse().ok()?,
    );
    if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // days-from-civil
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

fn file_name_for(day: i64) -> String {
    format!("{FILE_PREFIX}{}{FILE_SUFFIX}", date_string(day))
}

fn day_of_file_name(name: &str) -> Option<i64> {
    let date = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    parse_date(date)
}

/// One log file per UTC day (`auto-crop.YYYY-MM-DD.log`) in `dir`. On each new day, and whenever
/// the total grows past the cap, files older than the retention window are deleted and then the
/// oldest ones until the total fits; the current day's file is never deleted, but if it alone
/// exceeds the cap it is truncated and starts again.
pub struct DailyFileWriter {
    dir: PathBuf,
    clock: Arc<dyn Clock>,
    retention_days: u32,
    max_total_bytes: u64,
    day: Option<i64>,
    file: Option<File>,
    /// Bytes in the current file.
    current_len: u64,
    /// Bytes in the other files, refreshed when pruning.
    others_len: u64,
    scrub: bool,
}

impl DailyFileWriter {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self::with_clock(dir, Arc::new(SystemClock))
    }

    pub fn with_clock(dir: impl Into<PathBuf>, clock: Arc<dyn Clock>) -> Self {
        Self {
            dir: dir.into(),
            clock,
            retention_days: RETENTION_DAYS,
            max_total_bytes: MAX_TOTAL_BYTES,
            day: None,
            file: None,
            current_len: 0,
            others_len: 0,
            scrub: true,
        }
    }

    pub fn retention_days(mut self, days: u32) -> Self {
        self.retention_days = days.max(1);
        self
    }

    pub fn max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Whether lines are scrubbed of absolute paths before they are written (default true).
    pub fn scrub(mut self, on: bool) -> Self {
        self.scrub = on;
        self
    }

    /// The log files in the directory, oldest first, as `(day, path, size)`.
    fn files(&self) -> Vec<(i64, PathBuf, u64)> {
        let mut v: Vec<(i64, PathBuf, u64)> = fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let day = day_of_file_name(&name)?;
                // Not `DirEntry::metadata`: on Windows its size of a file that is open for
                // writing is stale until the handle closes.
                let len = fs::metadata(e.path()).ok()?.len();
                Some((day, e.path(), len))
            })
            .collect();
        v.sort();
        v
    }

    /// Deletes files outside the retention window, then the oldest until the total fits.
    fn prune(&mut self, today: i64, reserve: u64) {
        let oldest_kept = today - i64::from(self.retention_days) + 1;
        let mut files = self.files();
        files.retain(|(day, path, _)| {
            if *day < oldest_kept {
                let _ = fs::remove_file(path);
                false
            } else {
                true
            }
        });
        let mut total: u64 = files.iter().map(|f| f.2).sum();
        for (day, path, len) in &files {
            if total + reserve <= self.max_total_bytes {
                break;
            }
            if *day == today {
                continue; // never delete the open file
            }
            if fs::remove_file(path).is_ok() {
                total -= len;
            }
        }
        self.others_len = files
            .iter()
            .filter(|f| f.0 != today && f.1.exists())
            .map(|f| f.2)
            .sum();
    }

    fn open_day(&mut self, day: i64) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join(file_name_for(day));
        let f = OpenOptions::new().create(true).append(true).open(&path)?;
        self.current_len = f.metadata()?.len();
        self.file = Some(f);
        self.day = Some(day);
        self.prune(day, 0);
        Ok(())
    }

    fn write_raw(&mut self, buf: &[u8]) -> io::Result<()> {
        let day = day_number(self.clock.now());
        if self.day != Some(day) || self.file.is_none() {
            self.open_day(day)?;
        }
        let incoming = buf.len() as u64;
        if self.others_len + self.current_len + incoming > self.max_total_bytes {
            self.prune(day, incoming);
            // Still too big with only today's file: start the file again.
            if self.others_len + self.current_len + incoming > self.max_total_bytes {
                let path = self.dir.join(file_name_for(day));
                let f = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(path)?;
                self.file = Some(f);
                self.current_len = 0;
            }
        }
        if let Some(f) = self.file.as_mut() {
            f.write_all(buf)?;
            self.current_len += incoming;
        }
        Ok(())
    }
}

impl Write for DailyFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.scrub {
            // Formatted events arrive as whole lines; scrub them as text.
            match std::str::from_utf8(buf) {
                Ok(text) => self.write_raw(scrub_lines(text).as_bytes())?,
                Err(_) => self.write_raw(String::from_utf8_lossy(buf).as_bytes())?,
            }
        } else {
            self.write_raw(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

fn scrub_lines(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for chunk in text.split_inclusive('\n') {
        let (body, nl) = match chunk.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (chunk, ""),
        };
        out.push_str(&scrub_line(body));
        out.push_str(nl);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Start-up
// ---------------------------------------------------------------------------------------------

impl AppPaths {
    /// `logs/` under the data directory.
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
}

/// Keeps the background log writer alive; drop it at exit to flush.
pub struct LogGuard {
    _worker: tracing_appender::non_blocking::WorkerGuard,
}

/// Parses `error`, `warn`, `info`, `debug`, `trace` or `off`.
pub fn parse_level(s: &str) -> Option<LevelFilter> {
    s.trim().to_ascii_lowercase().parse().ok()
}

/// Installs the global subscriber: an `info` (or `AUTOCROP_LOG`) file log under
/// `paths.logs_dir()`. A second call, or a subscriber installed by someone else, is not an error:
/// the guard is returned and logging simply stays as it was.
pub fn init(paths: &AppPaths) -> Result<LogGuard, ErrKind> {
    let level = std::env::var("AUTOCROP_LOG")
        .ok()
        .and_then(|v| parse_level(&v))
        .unwrap_or(LevelFilter::INFO);
    init_with(DailyFileWriter::new(paths.logs_dir()), level)
}

/// [`init`] with an explicit writer and level (tests, `--log-level`).
pub fn init_with(writer: DailyFileWriter, level: LevelFilter) -> Result<LogGuard, ErrKind> {
    // Raw paths are only ever written at debug and below; otherwise scrub every line.
    let writer = writer.scrub(level < LevelFilter::DEBUG);
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_ansi(false)
        .with_writer(non_blocking)
        .finish();
    // Ignore "already set": the first subscriber wins.
    let _ = tracing::subscriber::set_global_default(subscriber);
    Ok(LogGuard { _worker: guard })
}

// ---------------------------------------------------------------------------------------------
// Events the engine emits
// ---------------------------------------------------------------------------------------------

/// An image was decoded. `info` level: the path appears only redacted.
pub fn decode_done(path: &Path, format: &str, width: u32, height: u32) {
    tracing::info!(
        target: "auto_crop::decode",
        file = %LogPath(path),
        format,
        px = u64::from(width) * u64::from(height),
        "decoded"
    );
    tracing::debug!(
        target: "auto_crop::decode",
        path = %DebugPath::new(path),
        width,
        height,
        "decode detail"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use tracing_subscriber::fmt::MakeWriter;

    /// Captures formatted log lines.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
        }
    }

    impl Write for Capture {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Capture;
        fn make_writer(&'a self) -> Capture {
            self.clone()
        }
    }

    fn capture_at(level: LevelFilter, f: impl FnOnce()) -> String {
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .with_max_level(level)
            .with_ansi(false)
            .with_writer(cap.clone())
            .finish();
        tracing::subscriber::with_default(sub, f);
        cap.text()
    }

    struct FakeClock(Mutex<SystemTime>);

    impl FakeClock {
        fn at_day(day: i64) -> Arc<Self> {
            Arc::new(Self(Mutex::new(
                UNIX_EPOCH + Duration::from_secs(day as u64 * 86_400 + 3_600),
            )))
        }
        fn set_day(&self, day: i64) {
            *self.0.lock().unwrap() = UNIX_EPOCH + Duration::from_secs(day as u64 * 86_400 + 3_600);
        }
    }

    impl Clock for FakeClock {
        fn now(&self) -> SystemTime {
            *self.0.lock().unwrap()
        }
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn redaction_is_a_stable_hash_prefix_plus_extension() {
        let a = redact_path(Path::new(r"C:\Users\Jane Doe\Pictures\Receipt 12.JPG"));
        assert_eq!(a.len(), 8 + 4, "{a}");
        assert!(a.ends_with(".jpg"));
        assert!(a[..8].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(
            a,
            redact_path(Path::new(r"C:\Users\Jane Doe\Pictures\Receipt 12.JPG"))
        );
        assert_ne!(
            a,
            redact_path(Path::new(r"C:\Users\Jane Doe\Pictures\Receipt 13.JPG"))
        );
        assert!(!a.contains("Jane") && !a.contains("Receipt"));
        // No extension, a dot-file, a hostile extension.
        assert_eq!(redact_path(Path::new("/tmp/noext")).len(), 8);
        assert_eq!(redact_path(Path::new("/tmp/.hidden")).len(), 8);
        let weird = redact_path(Path::new("/tmp/x.j p/g;rm -rf"));
        assert_eq!(weird.len(), 8, "{weird}");
        let long = redact_path(Path::new("/tmp/a.verylongextension"));
        assert_eq!(long.len(), 8 + 1 + 8);
        // blake3 of the path text.
        let hex = blake3::hash(b"/tmp/noext").to_hex();
        assert_eq!(redact_path(Path::new("/tmp/noext")), &hex.as_str()[..8]);
    }

    #[test]
    fn an_info_level_decode_log_has_no_raw_path() {
        let path = Path::new(r"C:\Users\Jane Doe\SecretFolder\private_receipt.jpg");
        for level in [LevelFilter::INFO, LevelFilter::WARN] {
            let text = capture_at(level, || decode_done(path, "Jpeg", 4000, 3000));
            if level == LevelFilter::WARN {
                assert!(text.is_empty());
                continue;
            }
            assert!(text.contains("decoded"), "{text}");
            assert!(text.contains(&redact_path(path)), "{text}");
            for leak in [
                "Jane",
                "SecretFolder",
                "private_receipt",
                "Users",
                "C:\\",
                "C:/",
            ] {
                assert!(!text.contains(leak), "{leak} leaked: {text}");
            }
            // Dimensions and format are fine to log.
            assert!(text.contains("12000000"));
        }
    }

    #[test]
    fn debug_level_may_show_the_raw_path_but_info_events_still_do_not() {
        let path = Path::new("/home/jane/private_receipt.jpg");
        let text = capture_at(LevelFilter::DEBUG, || decode_done(path, "Jpeg", 10, 10));
        // The debug event shows the real path ...
        let debug_line = text.lines().find(|l| l.contains("decode detail")).unwrap();
        assert!(
            debug_line.contains("/home/jane/private_receipt.jpg"),
            "{text}"
        );
        // ... the info event of the same decode does not.
        let info_line = text.lines().find(|l| l.contains("decoded")).unwrap();
        assert!(!info_line.contains("jane"), "{info_line}");
        // DebugPath outside a verbose subscriber redacts too.
        let at_info = capture_at(LevelFilter::INFO, || {
            tracing::info!(p = %DebugPath::new(path), "x");
        });
        assert!(!at_info.contains("jane"), "{at_info}");
    }

    #[test]
    fn scrubbing_catches_paths_that_were_not_wrapped() {
        let win = r"loading C:\Users\Jane Doe\Pictures\a b.jpg size=3";
        let out = scrub_line(win);
        assert!(!out.contains("Jane") && !out.contains("Pictures"), "{out}");
        assert!(out.starts_with("loading "), "{out}");
        assert!(out.ends_with(" size=3"), "{out}");
        assert!(out.contains(".jpg"));

        let quoted = r#"error opening "C:\\Users\\Jane\\x.png": not found"#;
        let out = scrub_line(quoted);
        assert!(!out.contains("Jane"), "{out}");
        assert!(out.contains(": not found"), "{out}");

        let unix = "path=/home/jane/Documents/scan.tiff next=1";
        let out = scrub_line(unix);
        assert!(!out.contains("jane") && !out.contains("Documents"), "{out}");
        assert!(out.ends_with(" next=1"), "{out}");

        let unc = r"share \\nas\photos\2026\x.heic done";
        assert!(!scrub_line(unc).contains("nas"));

        // Text that is not a path is left alone.
        for keep in [
            "ratio 3/4 of width",
            "100% done / next",
            "a/b",
            "time 10:30:45",
            "cap=4294967296 bytes",
            "https://example.org/x is never logged but is not a file path",
        ] {
            // `https://...` has `//` after the colon: not treated as a path either.
            assert_eq!(scrub_line(keep), keep, "{keep}");
        }
        // Scrubbed and wrapped paths agree on the token for the same path.
        let p = r"C:\a\b.jpg";
        assert!(scrub_line(&format!("x {p}")).contains(&redact_str(p)));
    }

    #[test]
    fn the_file_writer_scrubs_unless_debug() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at_day(20_000);
        let mut w = DailyFileWriter::with_clock(dir.path(), clock.clone());
        w.write_all(b"opened /home/jane/pic/a.jpg ok\n").unwrap();
        let mut raw = DailyFileWriter::with_clock(dir.path().join("raw"), clock).scrub(false);
        raw.write_all(b"opened /home/jane/pic/a.jpg ok\n").unwrap();
        let text = fs::read_to_string(dir.path().join(file_name_for(20_000))).unwrap();
        assert!(!text.contains("jane") && text.ends_with(" ok\n"), "{text}");
        let text = fs::read_to_string(dir.path().join("raw").join(file_name_for(20_000))).unwrap();
        assert!(text.contains("/home/jane/pic/a.jpg"));
    }

    #[test]
    fn dates_round_trip_and_names_parse() {
        for day in [-1, 0, 1, 59, 60, 365, 11_016, 20_000, 25_000, 47_482] {
            assert_eq!(parse_date(&date_string(day)), Some(day), "{day}");
            assert_eq!(day_of_file_name(&file_name_for(day)), Some(day));
        }
        assert_eq!(date_string(0), "1970-01-01");
        assert_eq!(date_string(20_000), "2024-10-04");
        assert_eq!(day_of_file_name("auto-crop.log"), None);
        assert_eq!(day_of_file_name("auto-crop.2026-13-01.log"), None);
        assert_eq!(day_of_file_name("other.2026-01-01.log"), None);
    }

    #[test]
    fn one_file_per_day_and_old_days_are_deleted_under_a_fake_clock() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at_day(20_000);
        let mut w = DailyFileWriter::with_clock(dir.path(), clock.clone());
        // Ten days of logging, two lines a day.
        for day in 20_000..20_010 {
            clock.set_day(day);
            writeln!(w, "day {day} first").unwrap();
            writeln!(w, "day {day} second").unwrap();
            // Today's file exists; it holds both lines of its day.
            let today = fs::read_to_string(dir.path().join(file_name_for(day))).unwrap();
            assert_eq!(today, format!("day {day} first\nday {day} second\n"));
        }
        // 7 days kept, today included: days 20_003..=20_009.
        let want: Vec<String> = (20_003..20_010).map(file_name_for).collect();
        assert_eq!(names(dir.path()), want);
        // A stray unrelated file is never touched.
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        clock.set_day(20_020);
        writeln!(w, "much later").unwrap();
        assert_eq!(
            names(dir.path()),
            vec![file_name_for(20_020), "notes.txt".to_owned()]
        );
    }

    #[test]
    fn a_restart_appends_to_the_same_days_file() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at_day(30_000);
        {
            let mut w = DailyFileWriter::with_clock(dir.path(), clock.clone());
            writeln!(w, "before restart").unwrap();
        }
        let mut w = DailyFileWriter::with_clock(dir.path(), clock);
        writeln!(w, "after restart").unwrap();
        let text = fs::read_to_string(dir.path().join(file_name_for(30_000))).unwrap();
        assert_eq!(text, "before restart\nafter restart\n");
    }

    #[test]
    fn the_total_size_cap_deletes_the_oldest_and_truncates_a_runaway_day() {
        let dir = tempfile::tempdir().unwrap();
        let clock = FakeClock::at_day(40_000);
        let mut w = DailyFileWriter::with_clock(dir.path(), clock.clone())
            .max_total_bytes(1_000)
            .scrub(false);
        let line = "x".repeat(99) + "\n"; // 100 bytes
        for day in 40_000..40_004 {
            clock.set_day(day);
            for _ in 0..4 {
                w.write_all(line.as_bytes()).unwrap();
            }
        }
        let total = |d: &Path| -> u64 {
            fs::read_dir(d)
                .unwrap()
                .map(|e| fs::metadata(e.unwrap().path()).unwrap().len())
                .sum()
        };
        assert!(total(dir.path()) <= 1_000, "{}", total(dir.path()));
        assert!(
            names(dir.path()).contains(&file_name_for(40_003)),
            "today stays"
        );
        // Pour 5 KB into one day: the day's file is truncated, never the cap exceeded.
        for _ in 0..50 {
            w.write_all(line.as_bytes()).unwrap();
        }
        assert!(total(dir.path()) <= 1_000, "{}", total(dir.path()));
        assert_eq!(names(dir.path()), vec![file_name_for(40_003)]);
    }

    #[test]
    fn init_writes_a_redacted_line_to_the_daily_file() {
        // The global subscriber can be set once per process, so this test drives the same
        // writer through a scoped subscriber instead of `init`.
        let dir = tempfile::tempdir().unwrap();
        let (nb, guard) = tracing_appender::non_blocking(DailyFileWriter::new(dir.path()));
        let sub = tracing_subscriber::fmt()
            .with_max_level(LevelFilter::INFO)
            .with_ansi(false)
            .with_writer(nb)
            .finish();
        let secret = Path::new("/home/jane/SecretFolder/scan.png");
        tracing::subscriber::with_default(sub, || {
            decode_done(secret, "Png", 8, 8);
            // An unwrapped path is caught by the writer's scrub.
            tracing::warn!("could not open {}", secret.display());
        });
        drop(guard); // flush
        let file = fs::read_dir(dir.path())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = fs::read_to_string(file).unwrap();
        assert!(
            text.contains("decoded") && text.contains("could not open"),
            "{text}"
        );
        assert!(
            !text.contains("jane") && !text.contains("SecretFolder"),
            "{text}"
        );
    }

    #[test]
    fn levels_parse_and_the_logs_dir_is_under_the_data_dir() {
        assert_eq!(parse_level("DEBUG"), Some(LevelFilter::DEBUG));
        assert_eq!(parse_level(" warn "), Some(LevelFilter::WARN));
        assert_eq!(parse_level("off"), Some(LevelFilter::OFF));
        assert_eq!(parse_level("loud"), None);
        let p = AppPaths::under(Path::new("root"));
        assert_eq!(p.logs_dir(), Path::new("root").join("data").join("logs"));
    }

    #[test]
    fn there_is_no_network_sink_among_the_engine_dependencies() {
        // B18 and C4: logs go to a local file only. (M1.72 adds the workspace-wide guard.)
        let manifest = include_str!("../Cargo.toml");
        for banned in [
            "reqwest",
            "hyper",
            "ureq",
            "tokio",
            "opentelemetry",
            "tracing-gelf",
            "tracing-loki",
            "tracing-bunyan",
            "sentry",
            "rustls",
            "native-tls",
            "openssl",
            "isahc",
            "surf",
        ] {
            assert!(!manifest.contains(banned), "{banned} in engine/Cargo.toml");
        }
    }
}
