// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The worker pool client: starts sandboxed workers on demand (up to a limit), reuses them,
//! enforces the per-job wall-clock timeout and the memory limits, replaces a worker that crashed
//! or timed out (the job gets a typed error, the batch continues), applies back-pressure on the
//! bytes in flight and honours cancellation by killing the worker.

use crate::os::{self, Exit, ExitClass, LaunchSpec, Launched, Proc, Shm};
use crate::proto::{self, Hello, JobHeader, Kind, ProtoError, Reply};
use auto_crop_codecs::{CodecError, DecodeLimits, Decoded, Format};
use auto_crop_core::CancelToken;
use auto_crop_imgproc::Raster;
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const MIB: u64 = 1 << 20;
const POLL: Duration = Duration::from_millis(20);

thread_local! {
    static CANCEL: RefCell<Option<CancelToken>> = const { RefCell::new(None) };
}

/// Runs `f` with `token` as the cancellation token of every worker decode this thread starts:
/// a cancelled token kills the worker and the decode returns [`CodecError::Cancelled`].
pub fn with_cancel<R>(token: &CancelToken, f: impl FnOnce() -> R) -> R {
    let prev = CANCEL.with(|c| c.replace(Some(token.clone())));
    let r = f();
    CANCEL.with(|c| *c.borrow_mut() = prev);
    r
}

fn cancelled() -> bool {
    CANCEL.with(|c| c.borrow().as_ref().is_some_and(CancelToken::is_cancelled))
}

/// How protective a worker's confinement is, strongest last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// No operating-system confinement was applied (a separate process only).
    None,
    /// Memory and process limits only (job object, rlimits): crash isolation, no file or
    /// network confinement.
    ProcessOnly,
    /// Files are blocked but something else is not (restricted token: the network; seccomp
    /// without Landlock).
    Reduced,
    /// The strongest level of the OS: AppContainer + job; Landlock + seccomp; Seatbelt.
    Full,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::None => "none",
            Tier::ProcessOnly => "process-only",
            Tier::Reduced => "reduced",
            Tier::Full => "full",
        }
    }
}

/// The tier a level text (as the worker reported it) amounts to.
pub fn tier_of(level: &str) -> Tier {
    if level.starts_with("appcontainer(") {
        Tier::Full
    } else if level.starts_with("restricted-token") {
        Tier::Reduced
    } else if level.contains("landlock=FullyEnforced") && level.contains("seccomp") {
        Tier::Full
    } else if level.contains("seccomp") {
        // seccomp without a fully enforced Landlock (absent, partial): files and processes are
        // still blocked by the allow-list, the network by its missing `socket`.
        Tier::Reduced
    } else if level.starts_with("sandbox_init(") {
        Tier::Full
    } else if level.contains("job(") || level.contains("rlimit(") {
        Tier::ProcessOnly
    } else {
        Tier::None
    }
}

/// Settings of a pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub worker_exe: PathBuf,
    pub plugin_dir: Option<PathBuf>,
    /// Most workers alive at once.
    pub workers: usize,
    /// Pixels a worker is sized for (its memory limit); a job with a higher pixel cap gets a
    /// fresh, larger worker.
    pub capacity_pixels: u64,
    /// Overrides the computed memory limit (tests).
    pub memory_limit: Option<u64>,
    /// Refuse a worker whose confinement is below this tier.
    pub min_tier: Tier,
    /// Which formats go to the pool.
    pub formats: fn(Format) -> bool,
    /// How long a new worker may take to confine itself and say hello.
    pub startup_timeout: Duration,
    /// Wall-clock cap of one job, when the job's limits give none: `base + per_mp * megapixels`.
    pub timeout_base: Duration,
    pub timeout_per_mp: Duration,
    /// Cap on the bytes (input plus pixel memory) of all jobs in flight at once.
    pub inflight_cap: u64,
    /// A worker is retired after this many jobs (its memory is fresh again).
    pub recycle_after: u32,
    /// Extra environment for the worker (tests).
    pub env: Vec<(OsString, OsString)>,
    /// Extra worker arguments (tests).
    pub args: Vec<String>,
    /// Wait for the test-only `Escape` frame after `Hello`.
    pub await_escape: bool,
    /// Try only this confinement attempt (an index into `os::attempt_names()`): tests of the
    /// fallback levels.
    pub force_attempt: Option<usize>,
}

impl PoolConfig {
    pub fn new(worker_exe: PathBuf) -> PoolConfig {
        let cores = std::thread::available_parallelism().map_or(2, usize::from);
        PoolConfig {
            worker_exe,
            plugin_dir: None,
            workers: (cores / 2).clamp(1, 4),
            capacity_pixels: auto_crop_codecs::DEFAULT_MAX_PIXELS,
            memory_limit: None,
            min_tier: Tier::None,
            formats: |f| matches!(f, Format::Heic | Format::Avif),
            startup_timeout: Duration::from_secs(30),
            timeout_base: Duration::from_secs(10),
            timeout_per_mp: Duration::from_secs(1),
            inflight_cap: 4096 * MIB,
            recycle_after: 64,
            env: Vec::new(),
            args: Vec::new(),
            await_escape: false,
            force_attempt: None,
        }
    }

    fn memory_limit_for(&self, capacity_pixels: u64) -> u64 {
        self.memory_limit.unwrap_or_else(|| {
            // About 16 bytes per pixel (libheif planes, the RGB raster, the shared copy, codec
            // frame buffers) plus a gigabyte for code, thread stacks and arenas.
            capacity_pixels
                .saturating_mul(16)
                .saturating_add(1024 * MIB)
        })
    }
}

/// What a worker said about itself and the pool's counters.
#[derive(Debug, Clone, Default)]
pub struct PoolStatus {
    pub level: Option<String>,
    pub probes: Option<String>,
    pub attempt: Option<String>,
    pub tier: Option<Tier>,
    pub libheif: Option<String>,
    pub hevc: Option<bool>,
    pub av1: Option<bool>,
    pub live: usize,
    pub spawned: u64,
    pub jobs: u64,
    pub crashes: u64,
    pub timeouts: u64,
    pub memory_kills: u64,
    pub cancelled: u64,
    /// Why no worker could be started, when none could.
    pub unavailable: Option<String>,
    /// Why the stronger confinement attempts before the one in use failed (empty when the first
    /// one worked).
    pub attempt_failures: Vec<String>,
}

/// Why a pool decode did not produce an answer about the file.
#[derive(Debug)]
pub enum PoolFailure {
    /// A typed outcome for this file (corrupt, too large, worker crashed, timeout, ...).
    Codec(CodecError),
    /// No worker could be started at all (not the file's fault).
    Unavailable(String),
}

enum Event {
    Frame(Kind, Vec<u8>),
    End(String),
}

struct Worker {
    proc: Arc<dyn Proc>,
    stdin: Box<dyn Write + Send>,
    rx: Receiver<Event>,
    shm: Box<dyn Shm>,
    hello: Hello,
    escape: Option<String>,
    stderr: Arc<Mutex<Vec<u8>>>,
    result_cap: Arc<AtomicU64>,
    capacity_pixels: u64,
    jobs: u32,
}

impl Worker {
    fn kill(&self) {
        self.proc.kill();
    }

    fn stderr_tail(&self) -> String {
        let g = self.stderr.lock().map(|g| g.clone()).unwrap_or_default();
        let s = String::from_utf8_lossy(&g);
        let line = s
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_owned();
        line.chars().filter(|c| !c.is_control()).take(200).collect()
    }

    fn stderr_has(&self, needle: &str) -> bool {
        self.stderr
            .lock()
            .map(|g| String::from_utf8_lossy(&g).contains(needle))
            .unwrap_or(false)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // Ask politely, then end it: a dropped worker never lingers.
        let _ = proto::write_frame(&mut self.stdin, Kind::Shutdown, &[], &[]);
        self.proc.kill();
    }
}

#[derive(Default)]
struct State {
    idle: Vec<Worker>,
    /// Idle plus busy plus starting.
    live: usize,
    inflight: u64,
    closed: bool,
}

struct Inner {
    cfg: PoolConfig,
    state: Mutex<State>,
    cv: Condvar,
    next_job: AtomicU64,
    /// The first launch attempt to try (a failed stronger attempt is not retried every time).
    first_attempt: AtomicUsize,
    status: Mutex<PoolStatus>,
}

/// The pool. Cloning is cheap (shared state).
#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<Inner>,
}

fn drain_stderr(mut r: Box<dyn Read + Send>, tail: Arc<Mutex<Vec<u8>>>) {
    let mut buf = [0u8; 4096];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        if let Ok(mut t) = tail.lock() {
            t.extend_from_slice(&buf[..n]);
            let len = t.len();
            if len > 4096 {
                t.drain(..len - 4096);
            }
        }
    }
}

fn read_frames(mut r: Box<dyn Read + Send>, tx: Sender<Event>, result_cap: Arc<AtomicU64>) {
    loop {
        let f = proto::read_frame(&mut r, |k| match k {
            Kind::Hello | Kind::Escape => proto::MAX_HELLO,
            Kind::Result => proto::MAX_REPLY_BASE + result_cap.load(Ordering::Relaxed),
            _ => 0,
        });
        match f {
            Ok((k, p)) => {
                if tx.send(Event::Frame(k, p)).is_err() {
                    return;
                }
            }
            Err(ProtoError::Closed) => {
                let _ = tx.send(Event::End("the worker closed its output".into()));
                return;
            }
            Err(e) => {
                let _ = tx.send(Event::End(format!("protocol error: {e}")));
                return;
            }
        }
    }
}

impl WorkerPool {
    pub fn new(cfg: PoolConfig) -> WorkerPool {
        WorkerPool {
            inner: Arc::new(Inner {
                cfg,
                state: Mutex::new(State::default()),
                cv: Condvar::new(),
                next_job: AtomicU64::new(1),
                first_attempt: AtomicUsize::new(0),
                status: Mutex::new(PoolStatus::default()),
            }),
        }
    }

    pub fn config(&self) -> &PoolConfig {
        &self.inner.cfg
    }

    pub fn status(&self) -> PoolStatus {
        let mut s = self
            .inner
            .status
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        s.live = self.inner.state.lock().map_or(0, |g| g.live);
        s
    }

    fn with_status(&self, f: impl FnOnce(&mut PoolStatus)) {
        if let Ok(mut s) = self.inner.status.lock() {
            f(&mut s);
        }
    }

    /// Starts one worker (trying the confinement attempts in order) and waits for its `Hello`.
    fn start(&self, capacity_pixels: u64) -> Result<Worker, String> {
        let cfg = &self.inner.cfg;
        let memory_limit = cfg.memory_limit_for(capacity_pixels);
        let names = os::attempt_names();
        let (first, last) = match cfg.force_attempt {
            Some(a) => (a.min(names.len() - 1), a.min(names.len() - 1) + 1),
            None => (
                self.inner.first_attempt.load(Ordering::Relaxed),
                names.len(),
            ),
        };
        let mut failures: Vec<String> = Vec::new();
        for (attempt, name) in names.iter().enumerate().take(last).skip(first) {
            let mut args = vec![
                "--mem-limit".to_owned(),
                memory_limit.to_string(),
                "--cpu-secs".to_owned(),
                "3600".to_owned(),
            ];
            args.extend(cfg.args.iter().cloned());
            let spec = LaunchSpec {
                exe: cfg.worker_exe.clone(),
                args,
                env: cfg.env.clone(),
                memory_limit,
                plugin_dir: cfg.plugin_dir.clone(),
                attempt,
            };
            match self.start_attempt(&spec, capacity_pixels) {
                Ok(w) => {
                    self.inner.first_attempt.store(attempt, Ordering::Relaxed);
                    self.with_status(|s| {
                        s.spawned += 1;
                        s.attempt = Some(names[attempt].to_owned());
                        s.level = Some(w.hello.level.clone());
                        s.probes = Some(w.hello.probes.clone());
                        s.tier = Some(tier_of(&w.hello.level));
                        s.libheif = Some(w.hello.libheif.clone());
                        s.hevc = Some(w.hello.hevc);
                        s.av1 = Some(w.hello.av1);
                        s.unavailable = None;
                        s.attempt_failures = failures.clone();
                    });
                    return Ok(w);
                }
                Err(e) => failures.push(format!("{name}: {e}")),
            }
        }
        let msg = failures.join("; ");
        self.with_status(|s| s.unavailable = Some(msg.clone()));
        Err(msg)
    }

    fn start_attempt(&self, spec: &LaunchSpec, capacity_pixels: u64) -> Result<Worker, String> {
        let cfg = &self.inner.cfg;
        let Launched {
            stdin,
            stdout,
            stderr,
            proc,
            shm,
        } = os::launch(spec).map_err(|e| format!("could not start: {e}"))?;
        let tail = Arc::new(Mutex::new(Vec::new()));
        let t2 = tail.clone();
        std::thread::Builder::new()
            .name("auto-crop-worker-stderr".into())
            .spawn(move || drain_stderr(stderr, t2))
            .map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        let result_cap = Arc::new(AtomicU64::new(0));
        let rc = result_cap.clone();
        std::thread::Builder::new()
            .name("auto-crop-worker-reader".into())
            .spawn(move || read_frames(stdout, tx, rc))
            .map_err(|e| e.to_string())?;
        let mut worker = Worker {
            proc,
            stdin,
            rx,
            shm,
            hello: Hello {
                pid: 0,
                level: String::new(),
                probes: String::new(),
                libheif: String::new(),
                hevc: false,
                av1: false,
            },
            escape: None,
            stderr: tail,
            result_cap,
            capacity_pixels,
            jobs: 0,
        };
        let fail = |w: &Worker, why: String| -> String {
            w.kill();
            let _ = w.proc.wait_timeout(Duration::from_secs(1));
            let tail = w.stderr_tail();
            if tail.is_empty() {
                why
            } else {
                format!("{why} (worker said: {tail})")
            }
        };
        match worker.rx.recv_timeout(cfg.startup_timeout) {
            Ok(Event::Frame(Kind::Hello, p)) => match Hello::decode(&p) {
                Ok(h) => worker.hello = h,
                Err(e) => return Err(fail(&worker, format!("bad hello: {e}"))),
            },
            Ok(Event::Frame(k, _)) => {
                return Err(fail(&worker, format!("expected hello, got {k:?}")));
            }
            Ok(Event::End(m)) => {
                let how = worker
                    .proc
                    .wait_timeout(Duration::from_secs(2))
                    .map(describe_exit)
                    .unwrap_or_default();
                return Err(fail(&worker, format!("{m} {how}")));
            }
            Err(_) => return Err(fail(&worker, "no hello in time".to_owned())),
        }
        if cfg.await_escape {
            match worker.rx.recv_timeout(Duration::from_secs(20)) {
                Ok(Event::Frame(Kind::Escape, p)) => {
                    worker.escape = Some(String::from_utf8_lossy(&p).into_owned());
                }
                _ => return Err(fail(&worker, "no escape report".to_owned())),
            }
        }
        let tier = tier_of(&worker.hello.level);
        if tier < cfg.min_tier {
            return Err(fail(
                &worker,
                format!(
                    "the sandbox level '{}' ({}) is below the required {}",
                    worker.hello.level,
                    tier.name(),
                    cfg.min_tier.name()
                ),
            ));
        }
        Ok(worker)
    }

    /// Probes the pool: starts a worker if none is idle, returns what it reported. Used by
    /// `doctor` and the packaging smoke tests.
    pub fn probe(&self) -> Result<PoolStatus, String> {
        let cap = self.inner.cfg.capacity_pixels;
        let w = {
            let mut g = self.inner.state.lock().map_err(|e| e.to_string())?;
            if g.idle.is_empty() {
                g.live += 1;
                drop(g);
                match self.start(cap) {
                    Ok(w) => w,
                    Err(e) => {
                        if let Ok(mut g) = self.inner.state.lock() {
                            g.live -= 1;
                        }
                        return Err(e);
                    }
                }
            } else {
                g.idle.pop().expect("not empty")
            }
        };
        let status = self.status();
        self.give_back(w);
        Ok(status)
    }

    /// The `Escape` report of a freshly started worker (test builds of the worker only).
    pub fn probe_escape(&self) -> Result<(PoolStatus, String), String> {
        let cap = self.inner.cfg.capacity_pixels;
        let w = self.start(cap)?;
        let text = w.escape.clone().unwrap_or_default();
        let status = self.status();
        drop(w);
        Ok((status, text))
    }

    fn give_back(&self, w: Worker) {
        let mut g = match self.inner.state.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.closed || w.jobs >= self.inner.cfg.recycle_after {
            g.live -= 1;
            drop(g);
            drop(w);
        } else {
            g.idle.push(w);
            drop(g);
        }
        self.inner.cv.notify_all();
    }

    fn discard(&self, w: Worker) {
        w.kill();
        drop(w);
        if let Ok(mut g) = self.inner.state.lock() {
            g.live -= 1;
        }
        self.inner.cv.notify_all();
    }

    fn release_bytes(&self, n: u64) {
        if let Ok(mut g) = self.inner.state.lock() {
            g.inflight = g.inflight.saturating_sub(n);
        }
        self.inner.cv.notify_all();
    }

    /// Waits for a slot: admission of `weight` bytes and a worker (idle, or room to start one).
    /// Returns the worker, or `None` when a new one must be started (its slot is counted).
    fn acquire(
        &self,
        weight: u64,
        capacity_pixels: u64,
        deadline: Instant,
    ) -> Result<Option<Worker>, CodecError> {
        let cfg = &self.inner.cfg;
        let mut g = self.inner.state.lock().map_err(|_| poisoned())?;
        loop {
            if g.closed {
                return Err(CodecError::WorkerCrashed(
                    "the decoder pool is closed".into(),
                ));
            }
            if cancelled() {
                return Err(CodecError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(CodecError::DecodeTimeout);
            }
            // A job heavier than the whole cap is admitted alone, so it cannot starve.
            let admitted = g.inflight == 0 || g.inflight + weight <= cfg.inflight_cap;
            if admitted {
                // An idle worker big enough for the job.
                if let Some(i) = g.idle.iter().position(|w| {
                    w.capacity_pixels >= capacity_pixels && w.proc.try_wait().is_none()
                }) {
                    g.inflight += weight;
                    return Ok(Some(g.idle.swap_remove(i)));
                }
                // An idle worker that is dead or too small is replaced by a fresh one.
                if let Some(i) = g.idle.iter().position(|w| {
                    w.capacity_pixels < capacity_pixels || w.proc.try_wait().is_some()
                }) {
                    let old = g.idle.swap_remove(i);
                    old.kill();
                    g.inflight += weight;
                    drop(g);
                    drop(old);
                    return Ok(None);
                }
                if g.live < cfg.workers {
                    g.live += 1;
                    g.inflight += weight;
                    return Ok(None);
                }
            }
            let (ng, _) = self
                .inner
                .cv
                .wait_timeout(g, POLL)
                .map_err(|_| poisoned())?;
            g = ng;
        }
    }

    /// Decodes one file in a worker.
    pub fn decode(
        &self,
        bytes: &[u8],
        limits: &DecodeLimits,
        apply_orientation: bool,
        flatten_alpha: bool,
    ) -> Result<Decoded, PoolFailure> {
        let cfg = &self.inner.cfg;
        // The pixel count the header declares sizes the shared memory and the time allowance.
        let probe = auto_crop_codecs::probe_with(bytes, limits).map_err(PoolFailure::Codec)?;
        let pixels = u64::from(probe.width) * u64::from(probe.height);
        let shm_len = pixels.saturating_mul(3).max(3);
        let mp = pixels.div_ceil(1_000_000);
        let wall = limits.max_decode_ms.map_or_else(
            || cfg.timeout_base + cfg.timeout_per_mp * u32::try_from(mp).unwrap_or(u32::MAX),
            Duration::from_millis,
        );
        let started = Instant::now();
        let deadline = started + wall;
        let weight = shm_len + bytes.len() as u64;
        let capacity = limits.max_pixels.max(cfg.capacity_pixels);

        let mut worker = match self.acquire(weight, capacity, deadline) {
            Ok(Some(w)) => w,
            Ok(None) => match self.start(capacity) {
                Ok(w) => w,
                Err(e) => {
                    if let Ok(mut g) = self.inner.state.lock() {
                        g.live -= 1;
                    }
                    self.release_bytes(weight);
                    self.inner.cv.notify_all();
                    return Err(PoolFailure::Unavailable(e));
                }
            },
            Err(e) => return Err(PoolFailure::Codec(e)),
        };

        let id = self.inner.next_job.fetch_add(1, Ordering::Relaxed);
        let result = self.run(
            &mut worker,
            id,
            bytes,
            limits,
            apply_orientation,
            flatten_alpha,
            shm_len,
            deadline,
            wall,
        );
        self.release_bytes(weight);
        worker.jobs += 1;
        worker.shm.release();
        self.with_status(|s| s.jobs += 1);
        match result {
            Ok(d) => {
                self.give_back(worker);
                Ok(d)
            }
            Err((e, reusable)) => {
                if reusable {
                    self.give_back(worker);
                } else {
                    self.discard(worker);
                }
                Err(PoolFailure::Codec(e))
            }
        }
    }

    /// One job on one worker. `Err((error, worker_still_good))`.
    #[allow(clippy::too_many_arguments)]
    fn run(
        &self,
        worker: &mut Worker,
        id: u64,
        bytes: &[u8],
        limits: &DecodeLimits,
        apply_orientation: bool,
        flatten_alpha: bool,
        shm_len: u64,
        deadline: Instant,
        wall: Duration,
    ) -> Result<Decoded, (CodecError, bool)> {
        // The worker cancels itself a little before the pool's backstop kills it.
        let mut job_limits = limits.clone();
        job_limits.max_decode_ms = Some(wall.as_millis().saturating_mul(9).div_ceil(10) as u64);
        worker
            .result_cap
            .store(limits.max_metadata_bytes, Ordering::Relaxed);
        let handle = worker
            .shm
            .prepare(shm_len, worker.proc.as_ref())
            .map_err(|e| {
                (
                    CodecError::WorkerCrashed(format!("shared memory: {e}")),
                    false,
                )
            })?;
        let head = JobHeader {
            job_id: id,
            flags: if apply_orientation {
                proto::FLAG_APPLY_ORIENTATION
            } else {
                0
            } | if flatten_alpha {
                proto::FLAG_FLATTEN_ALPHA
            } else {
                0
            },
            limits: job_limits,
            shm_len,
            shm_handle: handle,
        }
        .encode();

        let Worker {
            stdin, rx, proc, ..
        } = &mut *worker;
        let proc = proc.clone();
        let outcome: Result<Reply, (CodecError, bool)> = std::thread::scope(|scope| {
            let writer = scope.spawn(|| proto::write_frame(stdin, Kind::Job, &head, bytes));
            let outcome = loop {
                if cancelled() {
                    proc.kill();
                    self.with_status(|s| s.cancelled += 1);
                    break Err((CodecError::Cancelled, false));
                }
                let now = Instant::now();
                if now >= deadline {
                    proc.kill();
                    self.with_status(|s| s.timeouts += 1);
                    break Err((CodecError::DecodeTimeout, false));
                }
                if let Some(rss) = proc.resident_bytes()
                    && rss > worker_limit(worker_limit_of(limits, &self.inner.cfg))
                {
                    proc.kill();
                    self.with_status(|s| s.memory_kills += 1);
                    break Err((CodecError::MemoryLimit, false));
                }
                match rx.recv_timeout(POLL.min(deadline - now)) {
                    Ok(Event::Frame(Kind::Result, p)) => {
                        match Reply::decode(&p, limits.max_metadata_bytes) {
                            Ok(r) if r.job_id() == id => break Ok(r),
                            Ok(_) => {
                                proc.kill();
                                break Err((
                                    CodecError::WorkerCrashed("a reply for another job".into()),
                                    false,
                                ));
                            }
                            Err(e) => {
                                proc.kill();
                                break Err((
                                    CodecError::WorkerCrashed(format!("bad reply: {e}")),
                                    false,
                                ));
                            }
                        }
                    }
                    Ok(Event::Frame(k, _)) => {
                        proc.kill();
                        break Err((
                            CodecError::WorkerCrashed(format!("unexpected {k:?} frame")),
                            false,
                        ));
                    }
                    Ok(Event::End(m)) => break Err((CodecError::WorkerCrashed(m), false)),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        break Err((
                            CodecError::WorkerCrashed("the worker's reader ended".into()),
                            false,
                        ));
                    }
                }
            };
            if outcome.is_err() {
                // Unblocks a writer stuck on a full pipe.
                proc.kill();
            }
            let _ = writer.join();
            outcome
        });

        let reply = match outcome {
            Ok(r) => r,
            Err((CodecError::WorkerCrashed(m), _)) => {
                return Err((self.classify_crash(worker, &m), false));
            }
            Err(other) => return Err(other),
        };
        match reply {
            Reply::Err(e) => Err((proto::err_from_wire(&e), true)),
            Reply::Ok(ok) => {
                let px = u64::from(ok.width) * u64::from(ok.height);
                if px == 0 || px.saturating_mul(3) > shm_len {
                    return Err((
                        CodecError::WorkerCrashed(
                            "the worker reported a size that does not fit its memory".into(),
                        ),
                        false,
                    ));
                }
                let data = worker.shm.take((px * 3) as usize).map_err(|e| {
                    (
                        CodecError::WorkerCrashed(format!("shared memory: {e}")),
                        false,
                    )
                })?;
                let raster = Raster::from_raw(ok.width, ok.height, data).ok_or((
                    CodecError::Corrupt("pixel buffer size mismatch".into()),
                    true,
                ))?;
                Ok(Decoded {
                    raster,
                    format: probe_format(bytes),
                    exif_orientation: ok.exif_orientation,
                    icc: ok.icc,
                    source_bit_depth: ok.source_bit_depth,
                    frames: ok.frames,
                    notices: ok.notices.iter().filter_map(|n| intern_notice(n)).collect(),
                })
            }
        }
    }

    /// Turns "the pipe ended" into the right typed error by asking how the process ended.
    fn classify_crash(&self, w: &Worker, how: &str) -> CodecError {
        let exit: Option<Exit> = w.proc.wait_timeout(Duration::from_millis(1500));
        if exit.is_none() {
            w.kill();
        }
        let mem_text = w.stderr_has("memory allocation of");
        match exit.map(os::classify) {
            Some(ExitClass::MemoryLimit) => {
                self.with_status(|s| s.memory_kills += 1);
                CodecError::MemoryLimit
            }
            _ if mem_text => {
                self.with_status(|s| s.memory_kills += 1);
                CodecError::MemoryLimit
            }
            Some(ExitClass::CpuLimit) => {
                self.with_status(|s| s.timeouts += 1);
                CodecError::DecodeTimeout
            }
            Some(ExitClass::Crash(c)) => {
                self.with_status(|s| s.crashes += 1);
                let tail = w.stderr_tail();
                CodecError::WorkerCrashed(if tail.is_empty() {
                    c
                } else {
                    format!("{c}; {tail}")
                })
            }
            None => {
                self.with_status(|s| s.crashes += 1);
                CodecError::WorkerCrashed(how.to_owned())
            }
        }
    }

    /// Ends every idle worker; the pool starts new ones on demand afterwards.
    pub fn shutdown_idle(&self) {
        let idle = match self.inner.state.lock() {
            Ok(mut g) => {
                let v = std::mem::take(&mut g.idle);
                g.live -= v.len();
                v
            }
            Err(_) => return,
        };
        drop(idle);
        self.inner.cv.notify_all();
    }
}

fn poisoned() -> CodecError {
    CodecError::WorkerCrashed("the decoder pool lock is poisoned".into())
}

fn describe_exit(e: Exit) -> String {
    match os::classify(e) {
        ExitClass::MemoryLimit => "(memory limit)".into(),
        ExitClass::CpuLimit => "(cpu limit)".into(),
        ExitClass::Crash(c) => format!("({c})"),
    }
}

/// The memory limit used by the parent's resident-size watchdog (macOS): the pool's configured
/// limit, or the one computed for the job's pixel cap.
fn worker_limit_of(limits: &DecodeLimits, cfg: &PoolConfig) -> u64 {
    cfg.memory_limit_for(limits.max_pixels.max(cfg.capacity_pixels))
}

fn worker_limit(limit: u64) -> u64 {
    // The kernel does not enforce a limit here, so the watchdog allows a quarter more than the
    // figure the other platforms enforce.
    limit.saturating_add(limit / 4)
}

fn probe_format(bytes: &[u8]) -> Format {
    auto_crop_codecs::sniff(bytes).unwrap_or(Format::Jpeg)
}

/// The notice codes the pool has seen, as `&'static str` (the codec API names them statically).
/// A worker is untrusted: only plausible codes are taken and at most 512 distinct ones kept.
fn intern_notice(code: &str) -> Option<&'static str> {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    static TABLE: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    if code.is_empty()
        || code.len() > proto::MAX_NOTICE_LEN
        || !code
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.')
    {
        return None;
    }
    let mut t = TABLE.get_or_init(Default::default).lock().ok()?;
    if let Some(s) = t.get(code) {
        return Some(s);
    }
    if t.len() >= 512 {
        return None;
    }
    let leaked: &'static str = Box::leak(code.to_owned().into_boxed_str());
    t.insert(code.to_owned(), leaked);
    Some(leaked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_the_reported_level() {
        assert_eq!(
            tier_of(
                "appcontainer(capabilities=0)+job(active-process=1,memory=2048MiB,kill-on-close=yes)"
            ),
            Tier::Full
        );
        assert_eq!(
            tier_of("restricted-token+job(active-process=1)"),
            Tier::Reduced
        );
        assert_eq!(
            tier_of("token=unrestricted+job(active-process=1,memory=1MiB,kill-on-close=yes)"),
            Tier::ProcessOnly
        );
        assert_eq!(
            tier_of("rlimit(as=1MiB,cpu=1s,core=0)+landlock=FullyEnforced+seccomp(40 syscalls)"),
            Tier::Full
        );
        assert_eq!(
            tier_of("rlimit(as=1MiB,cpu=1s,core=0)+landlock=unavailable(x)+seccomp(40 syscalls)"),
            Tier::Reduced
        );
        assert_eq!(tier_of("rlimit(as=1MiB,cpu=1s,core=0)"), Tier::ProcessOnly);
        assert_eq!(
            tier_of("sandbox_init(deny default)+rlimit(cpu=1s,core=0)"),
            Tier::Full
        );
        assert_eq!(tier_of("none"), Tier::None);
        assert!(Tier::Full > Tier::Reduced && Tier::Reduced > Tier::ProcessOnly);
    }

    #[test]
    fn notices_are_interned_once_and_hostile_ones_refused() {
        let a = intern_notice("depth.reduced_to_8").unwrap();
        let b = intern_notice("depth.reduced_to_8").unwrap();
        assert!(std::ptr::eq(a, b));
        assert!(intern_notice("Not A Code").is_none());
        assert!(intern_notice("").is_none());
        assert!(intern_notice(&"a".repeat(100)).is_none());
    }

    #[test]
    fn a_cancelled_scope_is_visible_to_the_pool_only_inside_it() {
        let t = CancelToken::new_batch();
        assert!(!cancelled());
        with_cancel(&t, || {
            assert!(!cancelled());
            t.cancel();
            assert!(cancelled());
        });
        assert!(!cancelled());
    }
}
