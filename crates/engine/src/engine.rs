// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The engine: an item registry with analysis, parametric edits and history, preview rendering,
//! and the safe save and restore paths. UI-agnostic: a front end drives it through [`Engine`] and
//! receives plain serialisable views.

use crate::api::*;
use crate::commit::{free_name, swap, verify_temp, write_temp};
use crate::enumerate;
use crate::error::{ErrKind, Result, codec_err};
use crate::fsplan::ReservedKeys;
use crate::items_detect::{ClassicalItemDetector, ItemDetector, worst_confidence};
use crate::paths::AppPaths;
use crate::scan::{GroupOut, crop_views};
use crate::settings::Settings;
use crate::store::{BackupKind, BackupState, Manifest, NewBackup, OutputRec, Store};
use crate::util::{blake3_hex, display_name, new_id, now_secs, rfc3339, unix_ms};
use auto_crop_codecs::{Format, MAX_PIXELS, decode, encode, probe};
use auto_crop_core::{
    Confidence, EditState, Forced, History, Origin, QuadWarp, SplitPolicy, SplitState,
};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::detect::detect;
use auto_crop_imgproc::render::{Limits, render_quad};
use auto_crop_imgproc::scale::resize_to_fit;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

/// Long edge of the display proxy served as `src` and used for previews.
pub(crate) const DISPLAY_EDGE: u32 = 2048;
/// Long edge of the always-resident thumbnail source.
const THUMB_SRC_EDGE: u32 = 640;
pub(crate) const THUMB_EDGE: u32 = 256;
pub(crate) const RESULT_EDGE: u32 = 1400;
pub(crate) const JPEG_PREVIEW_QUALITY: u8 = 86;
/// Quality for JPEG outputs (PLAN: a real quality estimate arrives with the codec work in M2).
pub(crate) const JPEG_SAVE_QUALITY: u8 = 92;
const WORKERS: usize = 3;
const PROXY_CACHE: usize = 6;
const RENDER_CACHE_BYTES: usize = 192 * 1024 * 1024;
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

/// Called with the new view of an item whenever one changes in the background.
pub type Notify = Arc<dyn Fn(ItemView) + Send + Sync>;

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) size: u64,
    pub(crate) mtime_ms: i64,
    pub(crate) blake3: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SavedRec {
    pub(crate) backup_id: Option<String>,
    pub(crate) output_path: PathBuf,
    pub(crate) copy: bool,
    pub(crate) state: EditState,
    /// The written file as it stood after the swap, to notice later outside edits.
    pub(crate) out: Snapshot,
    /// The N outputs of a split scan, in output order (empty for a one-to-one save).
    pub(crate) group: Vec<GroupOut>,
}

/// One opened image (the registry entry; not a crop: an image has `EditState.items` crops).
pub(crate) struct Item {
    pub(crate) id: u32,
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) status: ItemStatus,
    pub(crate) error: Option<ErrKind>,
    pub(crate) format: Format,
    pub(crate) snapshot: Snapshot,
    /// mtime of the very first source, applied to every output (kept by default).
    pub(crate) orig_mtime_ms: i64,
    /// Where pixels come from: the file itself until the first in-place save, then the backup.
    pub(crate) original_path: PathBuf,
    pub(crate) dims: (u32, u32),
    pub(crate) thumb_src: Option<Arc<Raster>>,
    pub(crate) icc: Option<Arc<Vec<u8>>>,
    pub(crate) auto: Option<EditState>,
    pub(crate) confidence: Option<Confidence>,
    pub(crate) history: Option<History<EditState>>,
    pub(crate) generation: u64,
    pub(crate) saved: Option<SavedRec>,
    /// Frames, pages or IFDs the source declares (a multi-page TIFF is never replaced).
    pub(crate) frames: u32,
    /// The `render_hash` of the state the user accepted for saving (M10.29): a held split scan is
    /// written only while its current state still has this hash.
    pub(crate) accepted: Option<u64>,
}

impl Item {
    pub(crate) fn view(&self) -> ItemView {
        let current = self.history.as_ref().map(|h| h.current());
        let edited = match (current, &self.auto) {
            (Some(c), Some(a)) => c != a,
            _ => false,
        };
        let (crops, split) = current
            .map(|c| crop_views(self, c))
            .unwrap_or((Vec::new(), None));
        ItemView {
            id: self.id,
            name: self.name.clone(),
            width: self.dims.0,
            height: self.dims.1,
            status: self.status,
            error: self.error,
            edit: current.and_then(edit_of),
            auto_edit: self.auto.as_ref().and_then(edit_of),
            confidence: self.confidence.clone(),
            generation: self.generation,
            edited,
            saved: self.saved.as_ref().map(|s| SavedInfo {
                backup_id: s.backup_id.clone(),
                output: s
                    .output_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                copy: s.copy,
                outputs: s
                    .group
                    .iter()
                    .filter_map(|g| g.path.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .collect(),
            }),
            dirty_since_save: self
                .saved
                .as_ref()
                .is_some_and(|s| Some(&s.state) != current),
            can_undo: self.history.as_ref().is_some_and(|h| h.can_undo()),
            can_redo: self.history.as_ref().is_some_and(|h| h.can_redo()),
            undo_label: self
                .history
                .as_ref()
                .and_then(|h| h.undo_label().map(str::to_owned)),
            redo_label: self
                .history
                .as_ref()
                .and_then(|h| h.redo_label().map(str::to_owned)),
            crops,
            split,
            history_position: self.history.as_ref().map_or(0, |h| h.position()),
        }
    }

    fn geometry(&self) -> Option<QuadWarp> {
        self.history
            .as_ref()
            .and_then(|h| h.current().quad().cloned())
    }

    /// The current state differs from what the detector proposed.
    fn is_edited(&self) -> bool {
        match (self.history.as_ref().map(|h| h.current()), &self.auto) {
            (Some(c), Some(a)) => c != a,
            _ => false,
        }
    }

    /// The image would be saved as several files: the thumbnail then shows the whole scan.
    pub(crate) fn is_split(&self) -> bool {
        self.history.as_ref().is_some_and(|h| {
            h.current()
                .included()
                .filter(|i| i.geometry.quad().is_some())
                .count()
                >= 2
        })
    }
}

#[derive(Default)]
pub(crate) struct ProxyCache {
    /// Most recently used last.
    entries: Vec<(u32, Arc<Raster>)>,
}

impl ProxyCache {
    pub(crate) fn get(&mut self, id: u32) -> Option<Arc<Raster>> {
        let i = self.entries.iter().position(|(k, _)| *k == id)?;
        let e = self.entries.remove(i);
        let r = e.1.clone();
        self.entries.push(e);
        Some(r)
    }

    pub(crate) fn put(&mut self, id: u32, r: Arc<Raster>) {
        self.entries.retain(|(k, _)| *k != id);
        self.entries.push((id, r));
        while self.entries.len() > PROXY_CACHE {
            self.entries.remove(0);
        }
    }

    fn drop_item(&mut self, id: u32) {
        self.entries.retain(|(k, _)| *k != id);
    }
}

#[derive(Default)]
pub(crate) struct RenderCache {
    map: HashMap<(u32, u8, u64), Arc<Vec<u8>>>,
    order: VecDeque<(u32, u8, u64)>,
    bytes: usize,
}

impl RenderCache {
    pub(crate) fn get(&self, k: &(u32, u8, u64)) -> Option<Arc<Vec<u8>>> {
        self.map.get(k).cloned()
    }

    pub(crate) fn put(&mut self, k: (u32, u8, u64), v: Arc<Vec<u8>>) {
        self.bytes += v.len();
        if self.map.insert(k, v).is_none() {
            self.order.push_back(k);
        }
        while self.bytes > RENDER_CACHE_BYTES {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(v) = self.map.remove(&old) {
                self.bytes = self.bytes.saturating_sub(v.len());
            }
        }
    }

    fn drop_item(&mut self, id: u32) {
        self.order.retain(|k| k.0 != id);
        let dead: Vec<_> = self.map.keys().filter(|k| k.0 == id).copied().collect();
        for k in dead {
            if let Some(v) = self.map.remove(&k) {
                self.bytes = self.bytes.saturating_sub(v.len());
            }
        }
    }
}

pub(crate) struct Inner {
    pub(crate) paths: AppPaths,
    pub(crate) store: Store,
    pub(crate) settings: Mutex<Settings>,
    pub(crate) items: Mutex<BTreeMap<u32, Arc<Mutex<Item>>>>,
    next_id: AtomicU32,
    pub(crate) proxies: Mutex<ProxyCache>,
    pub(crate) renders: Mutex<RenderCache>,
    queue: Mutex<VecDeque<u32>>,
    workers: AtomicUsize,
    /// The multi-item detector (M10): the classical one of `imgproc::items` unless one is installed.
    pub(crate) detector: Mutex<Arc<dyn ItemDetector>>,
    /// Output names of split saves in flight, so two scans never plan the same file (M10.22).
    pub(crate) reserved: Mutex<ReservedKeys>,
    /// Commands that touched several images at once (a split preset on 20 scans): one undo.
    pub(crate) session: Mutex<auto_crop_core::SessionHistory<u32>>,
}

/// A cheap-to-clone handle on the engine.
#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) fn snapshot_of(path: &Path) -> Result<(Snapshot, Vec<u8>)> {
    let meta = fs::metadata(path).map_err(|e| ErrKind::from_io(&e))?;
    if meta.len() > MAX_SOURCE_BYTES {
        return Err(ErrKind::TooLarge);
    }
    let bytes = fs::read(path).map_err(|e| ErrKind::from_io(&e))?;
    let mtime_ms = meta.modified().map(unix_ms).unwrap_or(0);
    Ok((
        Snapshot {
            size: bytes.len() as u64,
            mtime_ms,
            blake3: blake3_hex(&bytes),
        },
        bytes,
    ))
}

pub(crate) fn stat_of(path: &Path) -> Option<(u64, i64)> {
    let m = fs::metadata(path).ok()?;
    Some((m.len(), m.modified().map(unix_ms).unwrap_or(0)))
}

pub(crate) fn jpeg(r: &Raster, quality: u8) -> Result<Vec<u8>> {
    encode(r, Format::Jpeg, quality, None).map_err(codec_err)
}

impl Engine {
    pub fn new(paths: AppPaths) -> Self {
        let settings = Settings::load(&paths);
        let store = Store::new(paths.backups_dir());
        let engine = Self {
            inner: Arc::new(Inner {
                paths,
                store,
                settings: Mutex::new(settings),
                items: Mutex::new(BTreeMap::new()),
                next_id: AtomicU32::new(1),
                proxies: Mutex::new(ProxyCache::default()),
                renders: Mutex::new(RenderCache::default()),
                queue: Mutex::new(VecDeque::new()),
                workers: AtomicUsize::new(0),
                detector: Mutex::new(Arc::new(ClassicalItemDetector)),
                reserved: Mutex::new(ReservedKeys::default()),
                session: Mutex::new(auto_crop_core::SessionHistory::new()),
            }),
        };
        // A crash may have interrupted a split save: finish or undo it before anything is opened
        // (M10.24), then purge.
        crate::group::recover(&engine.inner.store, &crate::group::NoFaults);
        engine.inner.store.purge(now_secs());
        engine
    }

    /// Installs the multi-item detector (the classical one in the app, a stub in tests).
    pub fn set_item_detector(&self, d: Arc<dyn ItemDetector>) {
        *lock(&self.inner.detector) = d;
    }

    pub fn paths(&self) -> &AppPaths {
        &self.inner.paths
    }

    // ------------------------------------------------------------------ settings

    pub fn settings(&self) -> Settings {
        lock(&self.inner.settings).clone()
    }

    pub fn set_settings(&self, s: Settings) -> Settings {
        let s = s.sanitised();
        // A failed write keeps the in-memory value: the app stays usable, the next change retries.
        let _ = s.save(&self.inner.paths);
        *lock(&self.inner.settings) = s.clone();
        s
    }

    // ------------------------------------------------------------------ items

    pub(crate) fn item(&self, id: u32) -> Option<Arc<Mutex<Item>>> {
        lock(&self.inner.items).get(&id).cloned()
    }

    pub fn list_items(&self) -> Vec<ItemView> {
        let items: Vec<_> = lock(&self.inner.items).values().cloned().collect();
        items.iter().map(|i| lock(i).view()).collect()
    }

    pub fn item_view(&self, id: u32) -> Option<ItemView> {
        self.item(id).map(|i| lock(&i).view())
    }

    /// Registers files and folders as items (status `analysing`); analysis is a separate step so a
    /// front end can show them at once. Files already open are skipped.
    pub fn open_paths(&self, roots: &[PathBuf], include_subfolders: bool) -> OpenSummary {
        let found = enumerate::collect(roots, include_subfolders);
        let mut summary = OpenSummary::default();
        summary.skipped += found.links_skipped;
        let mut known: std::collections::HashSet<PathBuf> = lock(&self.inner.items)
            .values()
            .map(|i| lock(i).path.clone())
            .collect();
        for path in found.files {
            if !known.insert(path.clone()) {
                summary.skipped += 1;
                continue;
            }
            let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            let item = Item {
                id,
                name: display_name(&path),
                original_path: path.clone(),
                path,
                status: ItemStatus::Analysing,
                error: None,
                format: Format::Jpeg,
                snapshot: Snapshot {
                    size: 0,
                    mtime_ms: 0,
                    blake3: String::new(),
                },
                orig_mtime_ms: 0,
                dims: (0, 0),
                thumb_src: None,
                icc: None,
                auto: None,
                confidence: None,
                history: None,
                generation: 0,
                saved: None,
                frames: 1,
                accepted: None,
            };
            lock(&self.inner.items).insert(id, Arc::new(Mutex::new(item)));
            summary.ids.push(id);
        }
        summary.added = summary.ids.len();
        summary
    }

    /// Writes (or reuses) the synthetic samples and opens them.
    pub fn add_samples(&self) -> Result<OpenSummary> {
        let files = crate::samples::write_samples(&self.inner.paths.samples_dir())
            .map_err(|e| ErrKind::from_io(&e))?;
        Ok(self.open_paths(&files, false))
    }

    pub fn remove_items(&self, ids: &[u32]) {
        for id in ids {
            lock(&self.inner.items).remove(id);
            lock(&self.inner.proxies).drop_item(*id);
            lock(&self.inner.renders).drop_item(*id);
        }
    }

    // ------------------------------------------------------------------ analysis

    /// Queues `ids` for background analysis on a small worker pool; `notify` hears every result.
    pub fn spawn_analysis(&self, ids: Vec<u32>, notify: Notify) {
        lock(&self.inner.queue).extend(ids);
        while self.inner.workers.load(Ordering::SeqCst) < WORKERS {
            self.inner.workers.fetch_add(1, Ordering::SeqCst);
            let engine = self.clone();
            let notify = notify.clone();
            std::thread::spawn(move || {
                loop {
                    let next = lock(&engine.inner.queue).pop_front();
                    let Some(id) = next else { break };
                    let view = engine.analyse(id);
                    if let Some(v) = view {
                        notify(v);
                    }
                }
                engine.inner.workers.fetch_sub(1, Ordering::SeqCst);
                // Work queued between the last pop and the decrement is picked up by a new pool.
                if !lock(&engine.inner.queue).is_empty()
                    && engine.inner.workers.load(Ordering::SeqCst) == 0
                {
                    engine.spawn_analysis(Vec::new(), notify);
                }
            });
        }
    }

    /// Decodes, detects and records the first auto result. Never panics the caller.
    pub fn analyse(&self, id: u32) -> Option<ItemView> {
        let item = self.item(id)?;
        let path = lock(&item).path.clone();
        let result =
            crate::run_isolated(std::panic::AssertUnwindSafe(|| self.analyse_inner(&path)))
                .unwrap_or(Err(ErrKind::Internal));
        let view = {
            let mut it = lock(&item);
            match result {
                Ok(a) => {
                    it.format = a.format;
                    it.snapshot = a.snapshot.clone();
                    it.orig_mtime_ms = a.snapshot.mtime_ms;
                    it.dims = a.raster_dims;
                    it.frames = a.frames;
                    it.accepted = None;
                    it.thumb_src = Some(Arc::new(a.thumb_src));
                    it.icc = a.icc.map(Arc::new);
                    it.confidence = Some(a.confidence);
                    it.auto = Some(a.state.clone());
                    it.history = Some(History::for_edit(a.state));
                    it.status = ItemStatus::Ready;
                    it.error = None;
                    it.generation = 1;
                    let view = it.view();
                    drop(it);
                    lock(&self.inner.proxies).put(id, Arc::new(a.display));
                    view
                }
                Err(e) => {
                    it.status = ItemStatus::Error;
                    it.error = Some(e);
                    it.view()
                }
            }
        };
        Some(view)
    }

    fn analyse_inner(&self, path: &Path) -> Result<Analysis> {
        let (snapshot, bytes) = snapshot_of(path)?;
        let probe = probe(&bytes).map_err(codec_err)?;
        if u64::from(probe.width) * u64::from(probe.height) > MAX_PIXELS {
            return Err(ErrKind::TooLarge);
        }
        let decoded = decode(&bytes).map_err(codec_err)?;
        let raster = decoded.raster;
        crate::logging::decode_done(
            path,
            &format!("{:?}", decoded.format),
            raster.width,
            raster.height,
        );
        // Several items on one scan (M10): the detector is asked first, and only two or more
        // items make a split; anything else is the ordinary single-item route.
        let (policy, profile) = {
            let s = lock(&self.inner.settings);
            (s.split_policy, s.split_profile)
        };
        let detector = lock(&self.inner.detector).clone();
        let split = (policy != SplitPolicy::Never)
            .then(|| detector.detect(&raster, policy, profile))
            .flatten()
            .filter(|d| d.items.len() >= 2);
        let (mut state, confidence) = match split {
            Some(d) => {
                let confidence = worst_confidence(&d.items);
                let items = d
                    .items
                    .into_iter()
                    .map(|i| {
                        let mut q = QuadWarp::new(i.quad);
                        q.quarter_turns = i.quarter_turns % 4;
                        auto_crop_core::items::auto_item(q, 1, Some(i.confidence))
                    })
                    .collect();
                let mut st = EditState::default();
                st.split.policy = policy;
                st.split.profile = profile;
                st.redetect(items, (raster.width, raster.height))
                    .map_err(|e| e.kind())?;
                (st, confidence)
            }
            None => {
                let detection = detect(&raster);
                let state = detection
                    .quad
                    .map(QuadWarp::new)
                    .map_or_else(EditState::default, |q| {
                        EditState::single_with(q, Origin::Auto { pipeline_ver: 1 }, None)
                    });
                (state, detection.confidence)
            }
        };
        state.split = SplitState {
            policy,
            profile,
            ..state.split
        };
        Ok(Analysis {
            format: decoded.format,
            snapshot,
            raster_dims: (raster.width, raster.height),
            thumb_src: resize_to_fit(&raster, THUMB_SRC_EDGE),
            display: resize_to_fit(&raster, DISPLAY_EDGE),
            icc: decoded.icc,
            confidence,
            state,
            frames: decoded.frames.max(1),
        })
    }

    // ------------------------------------------------------------------ edits

    pub(crate) fn with_history<R>(&self, id: u32, f: impl FnOnce(&mut Item) -> R) -> Result<R> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let mut it = lock(&item);
        if it.history.is_none() {
            return Err(ErrKind::Internal);
        }
        Ok(f(&mut it))
    }

    /// `live` returns the view the UI would show for a drag in progress without recording
    /// anything; `end` commits one history entry and bumps the generation if the state changed.
    pub fn set_edit(&self, id: u32, edit: &Edit, live: bool, label: &str) -> Result<ItemView> {
        self.with_history(id, |it| {
            // The edit is for the first included crop; every other crop, the order and the split
            // settings stay as they are. An image with no crop yet gets a fresh single crop.
            let current = it.history.as_ref().expect("checked").current().clone();
            let state = match current
                .included()
                .find_map(|c| c.geometry.quad().map(|q| (c.id, q)))
            {
                Some((cid, q)) => {
                    let mut st = current.clone();
                    match st.edit_item_quad(cid, edit.apply_to(q)) {
                        Ok(()) => st,
                        Err(_) => edit.to_state(),
                    }
                }
                None => EditState {
                    split: current.split,
                    ..edit.to_state()
                },
            };
            if live {
                let mut v = it.view();
                v.edit = edit_of(&state);
                return v;
            }
            let h = it.history.as_mut().expect("checked");
            if h.commit(label.chars().take(60).collect::<String>(), state) {
                it.generation += 1;
            }
            it.view()
        })
    }

    pub fn undo(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            if it.history.as_mut().expect("checked").undo().is_some() {
                it.generation += 1;
            }
            it.view()
        })
    }

    pub fn redo(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            if it.history.as_mut().expect("checked").redo().is_some() {
                it.generation += 1;
            }
            it.view()
        })
    }

    pub fn reset_to_auto(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            let auto = it.auto.clone().unwrap_or_default();
            if it
                .history
                .as_mut()
                .expect("checked")
                .commit("Reset to auto", auto)
            {
                it.generation += 1;
            }
            it.view()
        })
    }

    /// For items with no crop: an editable quad inset about 5% from the frame.
    pub fn draw_crop(&self, id: u32) -> Result<ItemView> {
        self.with_history(id, |it| {
            let split = it.history.as_ref().expect("checked").current().split;
            let state = EditState {
                split,
                ..EditState::single(QuadWarp::inset_frame(0.05))
            };
            if it
                .history
                .as_mut()
                .expect("checked")
                .commit("Draw crop", state)
            {
                it.generation += 1;
            }
            it.view()
        })
    }

    // ------------------------------------------------------------------ pixels for the UI

    pub(crate) fn proxy(&self, id: u32) -> Result<Arc<Raster>> {
        if let Some(p) = lock(&self.inner.proxies).get(id) {
            return Ok(p);
        }
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let path = lock(&item).original_path.clone();
        let bytes = fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ErrKind::OriginalExpired
            } else {
                ErrKind::from_io(&e)
            }
        })?;
        let decoded = decode(&bytes).map_err(codec_err)?;
        let p = Arc::new(resize_to_fit(&decoded.raster, DISPLAY_EDGE));
        lock(&self.inner.proxies).put(id, p.clone());
        Ok(p)
    }

    /// Encoded bytes for the `acimg` scheme. `generation` only busts caches; the current state is served.
    pub fn image_bytes(&self, id: u32, kind: ImageKind) -> Result<(Arc<Vec<u8>>, &'static str)> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let (generation, geometry, failed_unedited, thumb_src, ready) = {
            let it = lock(&item);
            let failed = it
                .confidence
                .as_ref()
                .is_some_and(|c| c.forced == Some(Forced::Failed));
            (
                it.generation,
                // A split scan's thumbnail is the whole scan; its crops have their own images.
                if kind == ImageKind::Thumb && it.is_split() {
                    None
                } else {
                    it.geometry()
                },
                failed && !it.is_edited(),
                it.thumb_src.clone(),
                it.status == ItemStatus::Ready,
            )
        };
        if !ready {
            return Err(ErrKind::Internal);
        }
        let key = (id, kind as u8, generation);
        if let Some(b) = lock(&self.inner.renders).get(&key) {
            return Ok((b, "image/jpeg"));
        }
        let bytes = match kind {
            ImageKind::Src => jpeg(self.proxy(id)?.as_ref(), JPEG_PREVIEW_QUALITY)?,
            ImageKind::Result => {
                let q = geometry.ok_or(ErrKind::NoCrop)?;
                let proxy = self.proxy(id)?;
                let out = render_quad(
                    &proxy,
                    &q,
                    Limits {
                        max_pixels: u64::MAX,
                        max_edge: RESULT_EDGE,
                    },
                )
                .map_err(|_| ErrKind::NoCrop)?;
                jpeg(&out, JPEG_PREVIEW_QUALITY)?
            }
            ImageKind::Thumb => {
                let src = thumb_src.ok_or(ErrKind::Internal)?;
                let out = match (&geometry, failed_unedited) {
                    (Some(q), false) => {
                        // Render a little larger, then area-average down: much cleaner than
                        // point-sampling a large reduction.
                        let big = render_quad(
                            &src,
                            q,
                            Limits {
                                max_pixels: u64::MAX,
                                max_edge: THUMB_EDGE * 2,
                            },
                        )
                        .map_err(|_| ErrKind::NoCrop)?;
                        resize_to_fit(&big, THUMB_EDGE)
                    }
                    _ => resize_to_fit(&src, THUMB_EDGE),
                };
                jpeg(&out, 80)?
            }
        };
        let bytes = Arc::new(bytes);
        lock(&self.inner.renders).put(key, bytes.clone());
        Ok((bytes, "image/jpeg"))
    }

    // ------------------------------------------------------------------ saving

    /// Saves each item in turn. `notify` hears the new view of every item that changed.
    pub fn save_items(
        &self,
        ids: &[u32],
        target: SaveTarget,
        run_name: &str,
        notify: &dyn Fn(ItemView),
    ) -> Vec<SaveOutcome> {
        let run_id = new_id();
        let run_name: String = run_name
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        ids.iter()
            .map(|id| {
                let outcome = self.save_dispatch(*id, target, &run_id, &run_name);
                if let Some(v) = self.item_view(*id) {
                    notify(v);
                }
                outcome
            })
            .collect()
    }

    pub(crate) fn save_one(
        &self,
        id: u32,
        target: SaveTarget,
        run_id: &str,
        run_name: &str,
    ) -> Result<SavedInfo> {
        let item = self.item(id).ok_or(ErrKind::Internal)?;
        let (path, original_path, state, fmt, snap, orig_mtime_ms, saved, icc) = {
            let it = lock(&item);
            if it.status != ItemStatus::Ready {
                return Err(ErrKind::Internal);
            }
            let state = it
                .history
                .as_ref()
                .ok_or(ErrKind::Internal)?
                .current()
                .clone();
            if state.quad().is_none() {
                return Err(ErrKind::NoCrop);
            }
            (
                it.path.clone(),
                it.original_path.clone(),
                state,
                it.format,
                it.snapshot.clone(),
                it.orig_mtime_ms,
                it.saved.clone(),
                it.icc.clone(),
            )
        };
        let geometry = state.quad().cloned().ok_or(ErrKind::NoCrop)?;

        // Writers exist for JPEG and PNG only (PLAN 3.2.3). A source in any other format that this
        // build can open (WebP, TIFF, HEIC, AVIF) is never replaced in place: nothing can write it
        // back without dropping content, so the source stays byte-identical. A copy is written in a
        // format the build can write: JPEG for HEIC (the conversion target of PLAN 3.5), PNG for
        // the rest (lossless, carries the ICC profile).
        let (fmt, copy_ext) = if fmt.is_encodable() {
            (fmt, None)
        } else if target == SaveTarget::Replace {
            return Err(ErrKind::UnsupportedOutput);
        } else {
            let out = if fmt == Format::Heic {
                Format::Jpeg
            } else {
                Format::Png
            };
            (out, Some(out.extension()))
        };

        // Read the pixels' source: the file before the first save, the backup afterwards.
        let bytes = fs::read(&original_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                if saved.is_some() {
                    ErrKind::OriginalExpired
                } else {
                    ErrKind::SourceChanged
                }
            } else {
                ErrKind::from_io(&e)
            }
        })?;
        // Reading the source file itself: it must still be what was opened.
        if original_path == path && blake3_hex(&bytes) != snap.blake3 {
            return Err(ErrKind::SourceChanged);
        }
        let decoded = decode(&bytes).map_err(codec_err)?;
        drop(bytes);
        let out = render_quad(&decoded.raster, &geometry, Limits::pixels(MAX_PIXELS))
            .map_err(|_| ErrKind::NoCrop)?;
        drop(decoded);
        let quality = JPEG_SAVE_QUALITY;
        let encoded =
            encode(&out, fmt, quality, icc.as_deref().map(|v| v.as_slice())).map_err(codec_err)?;
        let dims = (out.width, out.height);
        drop(out);
        let mtime = UNIX_EPOCH + Duration::from_millis(orig_mtime_ms.max(0) as u64);

        match target {
            SaveTarget::Copy => {
                let dir = path.parent().ok_or(ErrKind::Internal)?.join("AutoCrop");
                fs::create_dir_all(&dir).map_err(|e| ErrKind::from_io(&e))?;
                // Saving the same item as a copy again overwrites its own copy.
                let dest = match &saved {
                    Some(s) if s.copy && s.output_path.exists() => s.output_path.clone(),
                    _ => {
                        let mut name = PathBuf::from(path.file_name().ok_or(ErrKind::Internal)?);
                        if let Some(ext) = copy_ext {
                            name.set_extension(ext);
                        }
                        free_name(&dir.join(name))
                    }
                };
                let tmp = write_temp(&dir, &encoded, Some(mtime))?;
                if let Err(e) = verify_temp(&tmp, dims, fmt).and_then(|()| swap(&tmp.path, &dest)) {
                    tmp.discard();
                    return Err(e);
                }
                let out_snap = stat_of(&dest)
                    .map(|(size, mtime_ms)| Snapshot {
                        size,
                        mtime_ms,
                        blake3: tmp.blake3.clone(),
                    })
                    .ok_or(ErrKind::Internal)?;
                let mut it = lock(&item);
                it.saved = Some(SavedRec {
                    backup_id: saved.as_ref().and_then(|s| s.backup_id.clone()),
                    output_path: dest.clone(),
                    copy: true,
                    state,
                    out: out_snap,
                    group: Vec::new(),
                });
                // The first copy leaves the source where it is; later edits still start from it.
                Ok(SavedInfo {
                    backup_id: it.saved.as_ref().and_then(|s| s.backup_id.clone()),
                    output: dest
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    copy: true,
                    outputs: Vec::new(),
                })
            }
            SaveTarget::Replace => {
                let dir = path.parent().ok_or(ErrKind::Internal)?;
                let tmp = write_temp(dir, &encoded, Some(mtime))?;
                let fail = |tmp: &crate::commit::TempWrite, e: ErrKind| -> ErrKind {
                    tmp.discard();
                    e
                };
                verify_temp(&tmp, dims, fmt).map_err(|e| fail(&tmp, e))?;

                // Back the original up (first save) or find the existing backup (re-save).
                let retention = lock(&self.inner.settings).retention_days;
                let mut manifest: Manifest = match saved.as_ref().and_then(|s| s.backup_id.clone())
                {
                    Some(bid) => self
                        .inner
                        .store
                        .read(&bid)
                        .ok_or_else(|| fail(&tmp, ErrKind::OriginalExpired))?,
                    None => self
                        .inner
                        .store
                        .create(&NewBackup {
                            source: &path,
                            source_blake3: &snap.blake3,
                            source_size: snap.size,
                            source_mtime_ms: snap.mtime_ms,
                            format_ext: fmt.extension(),
                            run_id,
                            run_name,
                            retention_days: retention,
                            edit: Some(state.clone()),
                        })
                        .map_err(|e| fail(&tmp, e))?,
                };

                // Re-stat the target: if anything else touched it since we looked, stop.
                let expected = match &saved {
                    Some(s) if !s.copy => (s.out.size, s.out.mtime_ms),
                    _ => (snap.size, snap.mtime_ms),
                };
                if stat_of(&path) != Some(expected) {
                    return Err(fail(&tmp, ErrKind::SourceChanged));
                }

                // Record what is about to be written, so a crash after the swap still restores.
                let expected_mtime = unix_ms(mtime);
                manifest.outputs = vec![OutputRec::plain(
                    path.to_string_lossy().into_owned(),
                    tmp.blake3.clone(),
                    tmp.size,
                    expected_mtime,
                )];
                manifest.edit = Some(state.clone());
                self.inner
                    .store
                    .write(&manifest)
                    .map_err(|e| fail(&tmp, e))?;

                swap(&tmp.path, &path).map_err(|e| fail(&tmp, e))?;
                manifest.state = BackupState::Saved;
                // The file is already replaced and the backup exists; a failed state write here is
                // reported but cannot lose data.
                self.inner
                    .store
                    .write(&manifest)
                    .map_err(|_| ErrKind::Internal)?;

                let out_snap = stat_of(&path)
                    .map(|(size, mtime_ms)| Snapshot {
                        size,
                        mtime_ms,
                        blake3: tmp.blake3.clone(),
                    })
                    .ok_or(ErrKind::Internal)?;
                let backup_original = self
                    .inner
                    .store
                    .original_path(&manifest)
                    .ok_or(ErrKind::Internal)?;
                let mut it = lock(&item);
                it.saved = Some(SavedRec {
                    backup_id: Some(manifest.id.clone()),
                    output_path: path.clone(),
                    copy: false,
                    state,
                    out: out_snap.clone(),
                    group: Vec::new(),
                });
                it.original_path = backup_original;
                it.snapshot = out_snap;
                Ok(SavedInfo {
                    backup_id: Some(manifest.id),
                    output: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    copy: false,
                    outputs: Vec::new(),
                })
            }
        }
    }

    // ------------------------------------------------------------------ backups and restore

    pub fn list_backups(&self) -> BackupsView {
        let mut runs: Vec<(String, Vec<Manifest>)> = Vec::new();
        for m in self.inner.store.list() {
            match runs.iter_mut().find(|(r, _)| *r == m.run_id) {
                Some((_, v)) => v.push(m),
                None => runs.push((m.run_id.clone(), vec![m])),
            }
        }
        let runs = runs
            .into_iter()
            .map(|(id, ms)| {
                let files: Vec<BackupFile> = ms.iter().rev().map(|m| self.backup_file(m)).collect();
                BackupRun {
                    id,
                    name: ms[0].run_name.clone(),
                    created_at: rfc3339(ms.iter().map(|m| m.created_at).min().unwrap_or(0)),
                    expires_at: ms.iter().filter_map(|m| m.purge_after).min().map(rfc3339),
                    file_count: ms.len(),
                    total_bytes: ms.iter().map(|m| m.original_size).sum(),
                    pinned: ms.iter().all(|m| m.pinned),
                    files,
                }
            })
            .collect();
        BackupsView {
            location: self.inner.store.dir().to_string_lossy().into_owned(),
            used_bytes: self.inner.store.used_bytes(),
            free_bytes: None,
            runs,
        }
    }

    fn changed_since_saved(&self, m: &Manifest) -> bool {
        if m.kind == BackupKind::OneToN {
            return crate::restore::derived_files(m)
                .iter()
                .any(|d| d.state == DerivedState::Changed);
        }
        let Some(out) = m.outputs.first() else {
            return false;
        };
        if m.state != BackupState::Saved {
            return false;
        }
        let path = PathBuf::from(&m.original_path);
        match fs::read(&path) {
            Ok(b) => blake3_hex(&b) != out.blake3,
            Err(_) => true,
        }
    }

    fn backup_file(&self, m: &Manifest) -> BackupFile {
        let one_to_n = m.kind == BackupKind::OneToN;
        BackupFile {
            id: format!("{}/0", m.id),
            name: m.original_name.clone(),
            display_path: m.original_path.clone(),
            original_bytes: m.original_size,
            output_bytes: if one_to_n {
                Some(m.outputs.iter().map(|o| o.size).sum())
            } else {
                m.outputs.first().map(|o| o.size)
            },
            changed_since_saved: self.changed_since_saved(m),
            restored: m.state == BackupState::Restored,
            kind: m.kind,
            derived: if one_to_n {
                crate::restore::derived_files(m)
            } else {
                Vec::new()
            },
        }
    }

    pub fn pin_run(&self, run_id: &str, pinned: bool) {
        for mut m in self.inner.store.list() {
            if m.run_id == run_id {
                m.pinned = pinned;
                let _ = self.inner.store.write(&m);
            }
        }
    }

    pub fn purge_now(&self) -> usize {
        self.inner.store.purge(now_secs())
    }

    pub fn restore_run(&self, run_id: &str, notify: &dyn Fn(ItemView)) -> Vec<RestoreOutcome> {
        let mut ms: Vec<Manifest> = self
            .inner
            .store
            .list()
            .into_iter()
            .filter(|m| m.run_id == run_id && m.state == BackupState::Saved)
            .collect();
        ms.reverse();
        ms.iter()
            .map(|m| self.restore_manifest(m.clone(), RestoreMode::Auto, notify))
            .collect()
    }

    pub fn restore_file(
        &self,
        file_id: &str,
        mode: RestoreMode,
        notify: &dyn Fn(ItemView),
    ) -> RestoreOutcome {
        let backup_id = file_id.split('/').next().unwrap_or("");
        match self.inner.store.read(backup_id) {
            Some(m) => self.restore_manifest(m, mode, notify),
            None => RestoreOutcome::failed(ErrKind::OriginalExpired),
        }
    }

    pub(crate) fn restore_manifest(
        &self,
        mut m: Manifest,
        mode: RestoreMode,
        notify: &dyn Fn(ItemView),
    ) -> RestoreOutcome {
        if m.kind == BackupKind::OneToN {
            // The old entry points never touch derived files: Keep.
            return self.restore_group(m, mode, DerivedAction::Keep, notify);
        }
        let result = (|| -> Result<RestoreOutcome> {
            let orig_file = self
                .inner
                .store
                .original_path(&m)
                .ok_or(ErrKind::OriginalExpired)?;
            let orig_bytes = fs::read(&orig_file).map_err(|_| ErrKind::OriginalExpired)?;
            if blake3_hex(&orig_bytes) != m.original_blake3 {
                return Err(ErrKind::VerifyFailed);
            }
            let target = PathBuf::from(&m.original_path);
            let dir = target.parent().ok_or(ErrKind::Internal)?.to_path_buf();
            let mtime = UNIX_EPOCH + Duration::from_millis(m.original_mtime_ms.max(0) as u64);
            let current = fs::read(&target).ok().map(|b| blake3_hex(&b));
            let output_hash = m.outputs.first().map(|o| o.blake3.clone());
            let already_original = current.as_deref() == Some(m.original_blake3.as_str());
            let changed = match (&current, &output_hash) {
                (Some(c), Some(o)) => c != o && !already_original,
                (Some(_), None) => !already_original,
                _ => false,
            };
            let name = target
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("image");

            if mode == RestoreMode::AsCopy {
                let stem = Path::new(name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("image");
                let dest = free_name(&dir.join(format!("{stem} (restored).{}", m.format)));
                let tmp = write_temp(&dir, &orig_bytes, Some(mtime))?;
                swap(&tmp.path, &dest).inspect_err(|_| tmp.discard())?;
                return Ok(RestoreOutcome {
                    ok: true,
                    needs_choice: false,
                    error: None,
                    restored: dest.file_name().map(|n| n.to_string_lossy().into_owned()),
                    derived: Vec::new(),
                });
            }
            if changed && mode == RestoreMode::Auto {
                return Ok(RestoreOutcome {
                    ok: false,
                    needs_choice: true,
                    error: None,
                    restored: None,
                    derived: Vec::new(),
                });
            }
            if !already_original {
                // Keep what is there: restore is itself reversible.
                if current.is_some()
                    && let Some(pre) = self.inner.store.pre_restore_path(&m)
                {
                    fs::copy(&target, &pre).map_err(|e| ErrKind::from_io(&e))?;
                }
                let tmp = write_temp(&dir, &orig_bytes, Some(mtime))?;
                swap(&tmp.path, &target).inspect_err(|_| tmp.discard())?;
            }
            m.state = BackupState::Restored;
            m.restored_at = Some(now_secs());
            self.inner.store.write(&m)?;
            // Any open item that was saved from this backup is back to an unsaved original.
            let items: Vec<_> = lock(&self.inner.items).values().cloned().collect();
            let snapshot = Snapshot {
                size: orig_bytes.len() as u64,
                mtime_ms: stat_of(&target).map(|s| s.1).unwrap_or(0),
                blake3: m.original_blake3.clone(),
            };
            for item in items {
                let mut it = lock(&item);
                if it.saved.as_ref().and_then(|s| s.backup_id.as_deref()) == Some(m.id.as_str()) {
                    it.saved = None;
                    it.original_path = target.clone();
                    it.snapshot = snapshot.clone();
                    it.generation += 1;
                    notify(it.view());
                }
            }
            Ok(RestoreOutcome {
                ok: true,
                needs_choice: false,
                error: None,
                restored: Some(name.to_owned()),
                derived: Vec::new(),
            })
        })();
        result.unwrap_or_else(RestoreOutcome::failed)
    }
}

struct Analysis {
    format: Format,
    snapshot: Snapshot,
    raster_dims: (u32, u32),
    thumb_src: Raster,
    display: Raster,
    icc: Option<Vec<u8>>,
    confidence: Confidence,
    state: EditState,
    frames: u32,
}
