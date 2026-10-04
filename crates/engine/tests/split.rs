// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Multi-item scans end to end (ROADMAP M10.17-M10.31): a synthetic scene of several items on a
//! bed goes through the engine with a stub detector that returns the known quads (the real
//! detector is `imgproc::items`), is edited, held or accepted, saved as N files, restored, saved
//! again with a different N, and crashed at every step of the save.

use auto_crop_codecs::{Format, decode, encode};
use auto_crop_core::{Confidence, Cut, CutAxis, Forced, Pt, Reason, ReasonCode, ScanTriage};
use auto_crop_engine::group::{Fault, Step};
use auto_crop_engine::{
    AppPaths, CropImage, DerivedAction, DerivedState, DetectedItem, Engine, ErrKind, ImageKind,
    ItemDetector, ItemView, RestoreMode, RevertTo, SaveTarget, Settings, SplitDetection,
    SplitPatch,
};
use auto_crop_imgproc::Raster;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------------ the scene and the stub

const W: u32 = 1600;
const H: u32 = 1200;
const COLOURS: [[u8; 3]; 6] = [
    [200, 40, 40],
    [40, 190, 40],
    [40, 40, 200],
    [200, 190, 40],
    [190, 40, 190],
    [40, 190, 190],
];

type Quad = [Pt; 4];

fn q(p: [(f64, f64); 4]) -> Quad {
    p.map(|(x, y)| Pt::new(x, y))
}

/// Four items in a 2 by 2 grid, each slightly turned; the order here is NOT reading order.
fn grid4() -> Vec<Quad> {
    vec![
        q([(0.56, 0.56), (0.95, 0.55), (0.96, 0.94), (0.57, 0.95)]), // bottom right
        q([(0.05, 0.06), (0.46, 0.05), (0.47, 0.45), (0.06, 0.46)]), // top left
        q([(0.55, 0.05), (0.95, 0.07), (0.94, 0.46), (0.54, 0.44)]), // top right
        q([(0.06, 0.55), (0.45, 0.56), (0.44, 0.95), (0.05, 0.94)]), // bottom left
    ]
}

fn inside(poly: &[(f64, f64); 4], x: f64, y: f64) -> bool {
    let mut sign = 0.0f64;
    for i in 0..4 {
        let (a, b) = (poly[i], poly[(i + 1) % 4]);
        let c = (b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0);
        if c != 0.0 {
            if sign != 0.0 && c.signum() != sign.signum() {
                return false;
            }
            sign = c;
        }
    }
    true
}

/// A scanner bed with one colour per item painted inside its quad. `colours[i]` belongs to
/// `quads[i]`. The bed is a light grey.
fn scene(quads: &[Quad], colours: &[[u8; 3]]) -> Raster {
    scene_sized(quads, colours, W, H)
}

fn scene_sized(quads: &[Quad], colours: &[[u8; 3]], w: u32, h: u32) -> Raster {
    let (fw, fh) = (f64::from(w), f64::from(h));
    let mut r = Raster::filled(w, h, [236, 236, 232]);
    for (qd, col) in quads.iter().zip(colours) {
        let poly: [(f64, f64); 4] = std::array::from_fn(|i| (qd[i].x * fw, qd[i].y * fh));
        let (x0, x1) = poly
            .iter()
            .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = poly
            .iter()
            .fold((f64::MAX, f64::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        for y in (y0.floor() as u32)..(y1.ceil() as u32).min(h) {
            for x in (x0.floor() as u32)..(x1.ceil() as u32).min(w) {
                if inside(&poly, f64::from(x) + 0.5, f64::from(y) + 0.5) {
                    // A light texture so the pixels are not flat.
                    let t = ((x / 9 + y / 9) % 2) as u8 * 6;
                    r.set_pixel(
                        x,
                        y,
                        [
                            col[0].saturating_sub(t),
                            col[1].saturating_sub(t),
                            col[2].saturating_sub(t),
                        ],
                    );
                }
            }
        }
    }
    r
}

fn good() -> Confidence {
    Confidence {
        score: 0.98,
        forced: None,
        reasons: vec![],
    }
}

fn check() -> Confidence {
    Confidence {
        score: 0.8,
        forced: Some(Forced::Check),
        reasons: vec![Reason {
            code: ReasonCode::TouchingItems,
            side: None,
        }],
    }
}

/// Returns fixed quads for every scan.
struct Stub {
    items: Mutex<Vec<(Quad, Confidence)>>,
}

impl Stub {
    fn new(quads: &[Quad]) -> Arc<Self> {
        Arc::new(Self {
            items: Mutex::new(quads.iter().map(|qd| (*qd, good())).collect()),
        })
    }
}

impl ItemDetector for Stub {
    fn detect(
        &self,
        _: &Raster,
        _: auto_crop_core::SplitPolicy,
        _: auto_crop_core::SplitProfile,
    ) -> Option<SplitDetection> {
        Some(SplitDetection {
            items: self
                .items
                .lock()
                .unwrap()
                .iter()
                .map(|(quad, c)| DetectedItem {
                    quad: *quad,
                    confidence: c.clone(),
                    quarter_turns: 0,
                })
                .collect(),
            rejected: vec![],
        })
    }
}

struct Env {
    root: tempfile::TempDir,
    engine: Engine,
    dir: PathBuf,
}

fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let engine = Engine::new(AppPaths::under(root.path()));
    let dir = root.path().join("scans");
    fs::create_dir_all(&dir).unwrap();
    Env { root, engine, dir }
}

fn nop(_: ItemView) {}

impl Env {
    fn paths(&self) -> AppPaths {
        AppPaths::under(self.root.path())
    }

    /// Writes a scene as `name` (JPEG or PNG by extension), opens and analyses it.
    fn open(&self, name: &str, quads: &[Quad]) -> ItemView {
        self.open_sized(name, quads, W, H)
    }

    fn open_sized(&self, name: &str, quads: &[Quad], w: u32, h: u32) -> ItemView {
        let colours: Vec<[u8; 3]> = (0..quads.len()).map(|i| COLOURS[i % 6]).collect();
        let raster = scene_sized(quads, &colours, w, h);
        let fmt = if name.ends_with(".png") {
            Format::Png
        } else {
            Format::Jpeg
        };
        let bytes = encode(&raster, fmt, 92, None).unwrap();
        let path = self.dir.join(name);
        fs::write(&path, bytes).unwrap();
        let s = self.engine.open_paths(std::slice::from_ref(&path), false);
        assert_eq!(s.added, 1);
        self.engine.analyse(s.ids[0]).unwrap()
    }

    fn files(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(&self.dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn save(&self, id: u32, target: SaveTarget) -> auto_crop_engine::SaveOutcome {
        self.engine
            .save_items(&[id], target, "Split run", &nop)
            .remove(0)
    }
}

fn mean_colour(bytes: &[u8]) -> [f64; 3] {
    let d = decode(bytes).unwrap();
    let mut sum = [0.0f64; 3];
    let n = f64::from(d.raster.width * d.raster.height);
    for px in d.raster.data.as_chunks::<3>().0 {
        for c in 0..3 {
            sum[c] += f64::from(px[c]);
        }
    }
    sum.map(|s| s / n)
}

fn near(got: [f64; 3], want: [u8; 3], tol: f64) -> bool {
    (0..3).all(|c| (got[c] - f64::from(want[c])).abs() <= tol)
}

fn sha(p: &Path) -> String {
    auto_crop_engine::util::blake3_hex(&fs::read(p).unwrap())
}

// ------------------------------------------------------------------ analysis and views

#[test]
fn a_scan_with_several_items_becomes_ordered_crops_with_planned_names() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(v.crops.len(), 4);
    // Reading order whatever order the detector reported: top row, then bottom row.
    let tl: Vec<(f64, f64)> = v
        .crops
        .iter()
        .map(|c| {
            let q0 = c.edit.as_ref().unwrap().quad[0];
            ((q0.x * 20.0).round() / 20.0, (q0.y * 20.0).round() / 20.0)
        })
        .collect();
    assert_eq!(tl, [(0.05, 0.05), (0.55, 0.05), (0.05, 0.55), (0.55, 0.55)]);
    let names: Vec<_> = v
        .crops
        .iter()
        .map(|c| c.output_name.clone().unwrap())
        .collect();
    assert_eq!(
        names,
        ["scan_01.jpg", "scan_02.jpg", "scan_03.jpg", "scan_04.jpg"]
    );
    assert_eq!(
        v.crops.iter().map(|c| c.order).collect::<Vec<_>>(),
        [1, 2, 3, 4]
    );
    let ids: std::collections::BTreeSet<u32> = v.crops.iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 4, "ids are unique");
    let s = v.split.as_ref().unwrap();
    assert!(s.is_split && s.included == 4 && !s.accepted);
    assert_eq!(s.triage, ScanTriage::Approved);
    assert!(
        v.crops
            .iter()
            .all(|c| c.band == Some(auto_crop_core::Band::Good) && !c.edited)
    );
    // `edit` is the first included crop, as the single-item UI expects.
    assert_eq!(v.edit, v.crops[0].edit);
    // The grid thumbnail is the whole scan; the crops have their own images.
    let (thumb, _) = e.engine.image_bytes(v.id, ImageKind::Thumb).unwrap();
    let t = decode(&thumb).unwrap();
    assert!(
        t.raster.width > t.raster.height,
        "the whole 4:3 scan, not one crop"
    );
}

#[test]
fn per_crop_images_are_cached_per_crop_and_editing_one_leaves_the_others() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    // The colour of item i is COLOURS[position in the detector's list]; reading order maps the
    // detector's list [BR, TL, TR, BL] to crops [TL, TR, BL, BR].
    let colour_of_crop = [COLOURS[1], COLOURS[2], COLOURS[3], COLOURS[0]];
    let mut first: Vec<Arc<Vec<u8>>> = Vec::new();
    for (c, want) in v.crops.iter().zip(colour_of_crop) {
        let (b, mime) = e
            .engine
            .crop_image_bytes(v.id, c.id, CropImage::Result)
            .unwrap();
        assert_eq!(mime, "image/jpeg");
        assert!(
            near(mean_colour(&b), want, 14.0),
            "crop {} is the wrong item",
            c.id
        );
        let (t, _) = e
            .engine
            .crop_image_bytes(v.id, c.id, CropImage::Thumb)
            .unwrap();
        assert!(
            decode(&t)
                .unwrap()
                .raster
                .width
                .max(decode(&t).unwrap().raster.height)
                <= 256
        );
        first.push(b);
    }
    // Same crop again: the very same cached bytes.
    let (again, _) = e
        .engine
        .crop_image_bytes(v.id, v.crops[0].id, CropImage::Result)
        .unwrap();
    assert!(Arc::ptr_eq(&again, &first[0]));

    // Edit crop 2: its key changes, the others keep theirs and their cache entries.
    let mut edit = v.crops[1].edit.clone().unwrap();
    edit.fine_deg = 2.0;
    let v2 = e
        .engine
        .set_crop_edit(v.id, v.crops[1].id, &edit, false, "Straighten", Some(1))
        .unwrap();
    for k in [0, 2, 3] {
        assert_eq!(v2.crops[k].render_key, v.crops[k].render_key, "crop {k}");
        let (b, _) = e
            .engine
            .crop_image_bytes(v.id, v.crops[k].id, CropImage::Result)
            .unwrap();
        assert!(Arc::ptr_eq(&b, &first[k]), "crop {k} was re-rendered");
    }
    assert_ne!(v2.crops[1].render_key, v.crops[1].render_key);
    assert_eq!(v2.undo_label.as_deref(), Some("Straighten (item 2)"));
    assert!(
        v2.crops[1].edited && v2.crops[1].origin == auto_crop_engine::CropOrigin::AutoThenEdited
    );
    // A crop that does not exist, or no image: typed errors.
    assert_eq!(
        e.engine
            .crop_image_bytes(v.id, 999, CropImage::Result)
            .unwrap_err(),
        ErrKind::NoCrop
    );
}

// ------------------------------------------------------------------ the hold rule

#[test]
fn by_default_a_split_scan_is_held_and_nothing_is_written() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let before = e.files();
    let sha_before = sha(&e.dir.join("scan.jpg"));
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(!out.ok);
    assert_eq!(out.error, Some(ErrKind::HeldForReview));
    assert_eq!(out.notices, ["split.held"]);
    assert_eq!(e.files(), before);
    assert_eq!(sha(&e.dir.join("scan.jpg")), sha_before);
    assert!(e.engine.list_backups().runs.is_empty());
    // A replacement stays held on every retry; only a copy is allowed without acceptance (next test).
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
    assert_eq!(e.files(), before);
}

#[test]
fn a_copy_of_an_unaccepted_split_is_written_and_destroys_nothing() {
    // Owner confirmation 2026-10-04: a copy needs no acceptance because it removes and overwrites
    // nothing; replace-in-place stays held until the split is accepted.
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    assert!(!v.split.as_ref().unwrap().accepted);
    let scan_path = e.dir.join("scan.jpg");
    let scan = fs::read(&scan_path).unwrap();
    let mtime = fs::metadata(&scan_path).unwrap().modified().unwrap();
    // Files that already sit where the copies would go, under the first names and one other.
    let out_dir = e.dir.join("AutoCrop");
    fs::create_dir_all(&out_dir).unwrap();
    let foreign = [
        ("scan_01.jpg", &b"mine 1"[..]),
        ("scan_03.jpg", &b"mine 3"[..]),
    ];
    for (n, b) in foreign {
        fs::write(out_dir.join(n), b).unwrap();
    }
    let out = e.save(v.id, SaveTarget::Copy);
    assert!(out.ok, "{out:?}");
    let saved = out.saved.unwrap();
    assert!(saved.copy);
    assert_eq!(saved.outputs.len(), 4);
    // The scan is byte-identical, still where it was, with its modification time, and no backup
    // was needed.
    assert_eq!(fs::read(&scan_path).unwrap(), scan);
    assert_eq!(fs::metadata(&scan_path).unwrap().modified().unwrap(), mtime);
    assert_eq!(e.files(), ["scan.jpg"]);
    assert!(e.engine.list_backups().runs.is_empty());
    // Nothing that was there before was overwritten: the whole set moved to another base name.
    for (n, b) in foreign {
        assert_eq!(fs::read(out_dir.join(n)).unwrap(), b, "{n}");
    }
    assert!(
        saved.outputs.iter().all(|n| n.starts_with("scan (2)_")),
        "{:?}",
        saved.outputs
    );
    for n in &saved.outputs {
        assert!(decode(&fs::read(out_dir.join(n)).unwrap()).is_ok(), "{n}");
    }
    assert_eq!(fs::read_dir(&out_dir).unwrap().count(), 6);
    // The copy did not accept the split: replacing is still held, and still writes nothing.
    assert!(
        !e.engine
            .item_view(v.id)
            .unwrap()
            .split
            .as_ref()
            .unwrap()
            .accepted
    );
    let before = e.files();
    let out = e.save(v.id, SaveTarget::Replace);
    assert_eq!(out.error, Some(ErrKind::HeldForReview));
    assert_eq!(e.files(), before);
    assert_eq!(fs::read(&scan_path).unwrap(), scan);
    assert!(e.engine.list_backups().runs.is_empty());
}

#[test]
fn a_copy_of_a_split_that_needs_review_is_written_too() {
    let e = env();
    let stub = Stub::new(&grid4());
    stub.items.lock().unwrap()[1].1 = check();
    e.engine.set_item_detector(stub);
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(
        v.split.as_ref().unwrap().triage,
        ScanTriage::HeldForReview {
            items_need_check: 1
        }
    );
    let scan = sha(&e.dir.join("scan.jpg"));
    let out = e.save(v.id, SaveTarget::Copy);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs.len(), 4);
    assert_eq!(sha(&e.dir.join("scan.jpg")), scan);
    assert_eq!(e.files(), ["scan.jpg"]);
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
}

#[test]
fn auto_save_of_splits_is_experimental_and_needs_every_crop_good() {
    let e = env();
    e.engine.set_settings(Settings {
        auto_save_splits: true,
        ..Settings::default()
    });
    // All good at the Strict cutoff: saved without a review.
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("a.jpg", &grid4());
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs.len(), 4);
    // One doubtful crop holds the whole scan, whatever the setting says.
    let stub = Stub::new(&grid4());
    stub.items.lock().unwrap()[2].1 = check();
    e.engine.set_item_detector(stub);
    let v2 = e.open("b.jpg", &grid4());
    assert_eq!(
        v2.split.as_ref().unwrap().triage,
        ScanTriage::HeldForReview {
            items_need_check: 1
        }
    );
    let before = e.files();
    let out = e.save(v2.id, SaveTarget::Replace);
    assert_eq!(out.error, Some(ErrKind::HeldForReview));
    assert_eq!(e.files(), before, "held means not written");
}

#[test]
fn an_acceptance_covers_exactly_the_state_that_was_accepted() {
    let e = env();
    let stub = Stub::new(&grid4());
    stub.items.lock().unwrap()[0].1 = check();
    e.engine.set_item_detector(stub);
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
    let accepted = e.engine.accept_scan(v.id).unwrap();
    assert!(accepted.split.as_ref().unwrap().accepted);
    // Any edit withdraws it.
    let v2 = e.engine.remove_crop(v.id, v.crops[3].id).unwrap();
    assert!(!v2.split.as_ref().unwrap().accepted);
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
    // Undo returns to the accepted state, which is accepted again.
    let v3 = e.engine.undo(v.id).unwrap();
    assert!(v3.split.as_ref().unwrap().accepted);
    assert!(e.save(v.id, SaveTarget::Replace).ok);
}

// ------------------------------------------------------------------ the save

#[test]
fn an_accepted_split_becomes_n_files_a_backup_and_the_scan_leaves() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let scan = fs::read(e.dir.join("scan.jpg")).unwrap();
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert!(out.notes.is_empty());
    let saved = out.saved.unwrap();
    assert_eq!(
        saved.outputs,
        ["scan_01.jpg", "scan_02.jpg", "scan_03.jpg", "scan_04.jpg"]
    );
    assert_eq!(
        e.files(),
        saved.outputs,
        "the scan is gone, four files took its place"
    );
    // Each output is the right item, upright and valid.
    let want = [COLOURS[1], COLOURS[2], COLOURS[3], COLOURS[0]];
    for (n, w) in saved.outputs.iter().zip(want) {
        let b = fs::read(e.dir.join(n)).unwrap();
        let d = auto_crop_codecs::decode(&b).unwrap();
        assert!(
            d.raster.width > 400 && d.raster.height > 300,
            "{n}: {}x{}",
            d.raster.width,
            d.raster.height
        );
        assert!(near(mean_colour(&b), w, 14.0), "{n}");
    }
    // The backup is the scan, byte for byte, and lists the derived files.
    let runs = e.engine.list_backups().runs;
    assert_eq!(runs.len(), 1);
    let f = &runs[0].files[0];
    assert_eq!(f.kind, auto_crop_engine::store::BackupKind::OneToN);
    assert_eq!(f.derived.len(), 4);
    assert!(f.derived.iter().all(|d| d.state == DerivedState::Unchanged));
    assert!(!f.changed_since_saved);
    let bid = f.id.split('/').next().unwrap();
    let stored = e
        .engine
        .paths()
        .backups_dir()
        .join(bid)
        .join("original.jpg");
    assert_eq!(fs::read(stored).unwrap(), scan);
    // The image now reads its pixels from the backup and remembers its outputs.
    let after = e.engine.item_view(v.id).unwrap();
    assert_eq!(after.saved.unwrap().outputs.len(), 4);
    assert!(!after.dirty_since_save);
    // The idempotency guard sees every one of the N outputs (M10.25).
    for (i, n) in saved.outputs.iter().enumerate() {
        let p = e.engine.processed_by(&e.dir.join(n)).expect("recorded");
        assert_eq!((p.output_index, p.output_count), (i, 4));
    }
    assert!(
        e.engine
            .processed_by(&e.dir.join("nonexistent.jpg"))
            .is_none()
    );
}

#[test]
fn one_included_crop_keeps_the_single_item_name_and_flow() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let mut v = v;
    for c in v.crops.clone().iter().skip(1) {
        v = e.engine.remove_crop(v.id, c.id).unwrap();
    }
    assert_eq!(v.split.as_ref().unwrap().included, 1);
    assert_eq!(v.crops[0].output_name.as_deref(), Some("scan.jpg"));
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    // The ordinary single-item save: the same name, replaced in place after a backup.
    assert_eq!(e.files(), ["scan.jpg"]);
    let f = e.engine.list_backups().runs[0].files[0].clone();
    assert_eq!(f.kind, auto_crop_engine::store::BackupKind::OneToOne);
}

#[test]
fn a_scan_changed_after_opening_is_left_alone() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    e.engine.accept_scan(v.id).unwrap();
    fs::write(e.dir.join("scan.jpg"), b"someone else edited this").unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert_eq!(out.error, Some(ErrKind::SourceChanged));
    assert_eq!(e.files(), ["scan.jpg"]);
    assert_eq!(
        fs::read(e.dir.join("scan.jpg")).unwrap(),
        b"someone else edited this"
    );
    assert!(e.engine.list_backups().runs.is_empty());
}

#[test]
fn a_copy_leaves_the_scan_and_never_overwrites() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    e.engine.accept_scan(v.id).unwrap();
    let scan = sha(&e.dir.join("scan.jpg"));
    fs::create_dir_all(e.dir.join("AutoCrop")).unwrap();
    fs::write(e.dir.join("AutoCrop").join("scan_02.jpg"), b"unrelated").unwrap();
    let out = e.save(v.id, SaveTarget::Copy);
    assert!(out.ok, "{out:?}");
    assert_eq!(sha(&e.dir.join("scan.jpg")), scan);
    // One taken name moves the whole group to another base: no mixed numbering.
    let names = out.saved.unwrap().outputs;
    assert_eq!(names[0], "scan (2)_01.jpg");
    assert_eq!(names.len(), 4);
    assert_eq!(
        fs::read(e.dir.join("AutoCrop").join("scan_02.jpg")).unwrap(),
        b"unrelated"
    );
    assert!(
        e.engine.list_backups().runs.is_empty(),
        "a copy needs no backup"
    );
}

#[test]
fn names_of_other_open_images_are_never_planned() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    // Another image in the batch is literally called scan_01.jpg.
    let other = e.dir.join("scan_01.jpg");
    fs::write(
        &other,
        encode(&Raster::filled(40, 30, [9, 9, 9]), Format::Jpeg, 80, None).unwrap(),
    )
    .unwrap();
    e.engine.open_paths(std::slice::from_ref(&other), false);
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(
        v.crops[0].output_name.as_deref(),
        Some("scan_01.jpg"),
        "the view is a preview"
    );
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs[0], "scan (2)_01.jpg");
    assert!(other.exists());
}

// ------------------------------------------------------------------ never-replaced sources

/// A tiny two-page uncompressed RGB TIFF (little endian), built by hand.
fn two_page_tiff(w: u32, h: u32, shade: [u8; 2]) -> Vec<u8> {
    fn le16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn le32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    let pix = (w * h * 3) as usize;
    let mut out = vec![b'I', b'I', 42, 0];
    le32(&mut out, 8);
    let mut pages = Vec::new();
    // Layout: header(8) | ifd0 | bps0 | pixels0 | ifd1 | bps1 | pixels1
    let ifd_len = 2 + 9 * 12 + 4;
    let mut pos = 8usize;
    for (k, shade) in shade.iter().enumerate() {
        let ifd = pos;
        let bps = ifd + ifd_len;
        let data = bps + 6;
        let next = if k == 0 { (data + pix) as u32 } else { 0 };
        let mut p = Vec::new();
        le16(&mut p, 9);
        let entry = |p: &mut Vec<u8>, tag: u16, ty: u16, count: u32, val: u32| {
            le16(p, tag);
            le16(p, ty);
            le32(p, count);
            le32(p, val);
        };
        entry(&mut p, 256, 4, 1, w);
        entry(&mut p, 257, 4, 1, h);
        entry(&mut p, 258, 3, 3, bps as u32);
        entry(&mut p, 259, 3, 1, 1);
        entry(&mut p, 262, 3, 1, 2);
        entry(&mut p, 273, 4, 1, data as u32);
        entry(&mut p, 277, 3, 1, 3);
        entry(&mut p, 278, 4, 1, h);
        entry(&mut p, 279, 4, 1, pix as u32);
        le32(&mut p, next);
        for _ in 0..3 {
            le16(&mut p, 8);
        }
        p.extend(std::iter::repeat_n(*shade, pix));
        pages.extend(p);
        pos = data + pix;
    }
    out.extend(pages);
    out
}

#[test]
fn a_multi_page_tiff_is_never_replaced_and_copies_are_png() {
    let e = env();
    let bytes = two_page_tiff(64, 48, [90, 200]);
    let path = e.dir.join("pages.tif");
    fs::write(&path, &bytes).unwrap();
    assert_eq!(auto_crop_codecs::probe(&bytes).unwrap().frames, 2);
    e.engine.set_item_detector(Stub::new(&[
        q([(0.05, 0.05), (0.45, 0.05), (0.45, 0.9), (0.05, 0.9)]),
        q([(0.55, 0.05), (0.95, 0.05), (0.95, 0.9), (0.55, 0.9)]),
    ]));
    let s = e.engine.open_paths(std::slice::from_ref(&path), false);
    let v = e.engine.analyse(s.ids[0]).unwrap();
    assert_eq!(v.crops.len(), 2);
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert_eq!(out.error, Some(ErrKind::NotReplaceable));
    assert_eq!(out.notices, ["tiff.multi_page"]);
    assert_eq!(fs::read(&path).unwrap(), bytes, "byte-identical");
    assert!(e.engine.list_backups().runs.is_empty());
    assert_eq!(e.files(), ["pages.tif"]);
    // Copies on opt-in: PNG, the source untouched.
    let out = e.save(v.id, SaveTarget::Copy);
    assert!(out.ok, "{out:?}");
    // Copies of a TIFF are PNG; the folder is AutoCrop/ (the pages.png name is taken by the
    // source's own folder, not this one).
    assert_eq!(out.saved.unwrap().outputs, ["pages_01.png", "pages_02.png"]);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

// ------------------------------------------------------------------ restore

fn saved_split(e: &Env) -> (ItemView, Vec<String>, String) {
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let scan = sha(&e.dir.join("scan.jpg"));
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    (v, out.saved.unwrap().outputs, scan)
}

fn backup_file_id(e: &Env) -> String {
    e.engine.list_backups().runs[0].files[0].id.clone()
}

#[test]
fn restore_keep_returns_the_scan_and_leaves_the_derived_files() {
    let e = env();
    let (v, outputs, scan) = saved_split(&e);
    let r = e.engine.restore_file_derived(
        &backup_file_id(&e),
        RestoreMode::Auto,
        DerivedAction::Keep,
        &nop,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(sha(&e.dir.join("scan.jpg")), scan, "byte-identical");
    for n in &outputs {
        assert!(e.dir.join(n).exists());
    }
    assert!(r.derived.iter().all(|d| d.state == DerivedState::Unchanged));
    assert!(e.engine.list_backups().runs[0].files[0].restored);
    // The open image is an unsaved original again.
    assert!(e.engine.item_view(v.id).unwrap().saved.is_none());
    // The old entry point never touches derived files either.
    let again = e
        .engine
        .restore_file(&backup_file_id(&e), RestoreMode::Auto, &nop);
    assert!(again.ok);
    assert_eq!(e.files().len(), 5);
}

#[test]
fn restore_remove_moves_unchanged_derived_files_into_the_store_and_keeps_edited_ones() {
    let e = env();
    let (_, outputs, scan) = saved_split(&e);
    let edited = e.dir.join(&outputs[1]);
    fs::write(&edited, b"touched up by hand").unwrap();
    let f = e.engine.list_backups().runs[0].files[0].clone();
    assert!(f.changed_since_saved);
    assert_eq!(f.derived[1].state, DerivedState::Changed);
    let r = e
        .engine
        .restore_file_derived(&f.id, RestoreMode::Auto, DerivedAction::Remove, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(sha(&e.dir.join("scan.jpg")), scan);
    // The edited one stays; the three unchanged ones moved into the store, none deleted.
    assert_eq!(e.files(), ["scan.jpg", outputs[1].as_str()]);
    assert_eq!(fs::read(&edited).unwrap(), b"touched up by hand");
    let states: Vec<_> = r.derived.iter().map(|d| d.state).collect();
    assert_eq!(
        states,
        [
            DerivedState::Removed,
            DerivedState::Changed,
            DerivedState::Removed,
            DerivedState::Removed
        ]
    );
    let bid = f.id.split('/').next().unwrap();
    let kept = e
        .engine
        .paths()
        .backups_dir()
        .join(bid)
        .join("derived-by-restore");
    let n = fs::read_dir(&kept).unwrap().count();
    assert_eq!(n, 3, "reversible: the bytes are in the store");
}

#[test]
fn restore_after_a_restart_works_and_is_repeatable() {
    let e = env();
    let (_, outputs, scan) = saved_split(&e);
    // A new engine on the same data: nothing is open, the backup is all there is.
    let e2 = Engine::new(e.paths());
    let runs = e2.list_backups().runs;
    assert_eq!(runs[0].files[0].derived.len(), 4);
    let id = runs[0].files[0].id.clone();
    // Occupy the scan's path first: the scan comes back beside it, nothing is replaced.
    fs::write(e.dir.join("scan.jpg"), b"a different file now").unwrap();
    let r = e2.restore_file_derived(&id, RestoreMode::Auto, DerivedAction::Remove, &nop);
    assert!(r.ok, "{r:?}");
    assert_eq!(r.restored.as_deref(), Some("scan (restored).jpg"));
    assert_eq!(
        fs::read(e.dir.join("scan.jpg")).unwrap(),
        b"a different file now"
    );
    assert_eq!(sha(&e.dir.join("scan (restored).jpg")), scan);
    for n in &outputs {
        assert!(!e.dir.join(n).exists());
    }
    // Restoring again is harmless.
    let again = e2.restore_file_derived(&id, RestoreMode::Auto, DerivedAction::Remove, &nop);
    assert!(again.ok, "{again:?}");
    assert_eq!(
        again
            .derived
            .iter()
            .filter(|d| d.state == DerivedState::Removed)
            .count(),
        4
    );
}

#[test]
fn restore_all_from_a_run_handles_a_mix_of_one_to_one_and_split_saves() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let split = e.open("split.jpg", &grid4());
    e.engine.accept_scan(split.id).unwrap();
    // A second image with no split: the stub only fires for scenes, so use the Never policy.
    e.engine.set_settings(Settings {
        split_policy: auto_crop_core::SplitPolicy::Never,
        ..Settings::default()
    });
    let single = e.open("single.jpg", &[grid4()[1]]);
    let outs = e
        .engine
        .save_items(&[split.id, single.id], SaveTarget::Replace, "Mixed", &nop);
    assert!(outs.iter().all(|o| o.ok), "{outs:?}");
    let run = e.engine.list_backups().runs[0].id.clone();
    let rs = e
        .engine
        .restore_run_derived(&run, DerivedAction::Remove, &nop);
    assert_eq!(rs.len(), 2);
    assert!(rs.iter().all(|r| r.ok), "{rs:?}");
    assert_eq!(e.files(), ["single.jpg", "split.jpg"]);
}

// ------------------------------------------------------------------ re-saving with another N

#[test]
fn a_re_save_from_the_pristine_backup_replaces_the_set_when_n_goes_down() {
    let e = env();
    let (v, outputs, scan) = saved_split(&e);
    assert_eq!(outputs.len(), 4);
    let before: Vec<Vec<u8>> = outputs
        .iter()
        .map(|n| fs::read(e.dir.join(n)).unwrap())
        .collect();
    // Drop the last item and straighten the first; the pixels come from the backup.
    let v = e.engine.remove_crop(v.id, v.crops[3].id).unwrap();
    let mut edit = v.crops[0].edit.clone().unwrap();
    edit.fine_deg = 1.5;
    let v = e
        .engine
        .set_crop_edit(v.id, v.crops[0].id, &edit, false, "Straighten", None)
        .unwrap();
    assert!(v.dirty_since_save);
    // The edit changed the state, so a held scan needs a new acceptance; an accepted one too.
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs, &outputs[..3]);
    assert_eq!(
        e.files(),
        &outputs[..3],
        "the fourth output was retired, the scan did not come back"
    );
    assert_ne!(
        fs::read(e.dir.join(&outputs[0])).unwrap(),
        before[0],
        "the straightened one changed"
    );
    // The retired and replaced files are kept in the store.
    let bid = e.engine.list_backups().runs[0].files[0]
        .id
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let superseded = e.engine.paths().backups_dir().join(&bid).join("superseded");
    assert_eq!(fs::read_dir(&superseded).unwrap().count(), 4);
    // Still one backup, still the pristine scan, and its derived list is the new set.
    let runs = e.engine.list_backups().runs;
    assert_eq!(runs.iter().map(|r| r.file_count).sum::<usize>(), 1);
    assert_eq!(runs[0].files[0].derived.len(), 3);
    let r = e.engine.restore_file_derived(
        &runs[0].files[0].id,
        RestoreMode::Auto,
        DerivedAction::Remove,
        &nop,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(
        sha(&e.dir.join("scan.jpg")),
        scan,
        "no generation loss through two saves"
    );
    assert_eq!(e.files(), ["scan.jpg"]);
}

#[test]
fn a_re_save_with_more_items_adds_files_and_n_back_to_one_uses_the_plain_name() {
    let e = env();
    // Start from two items.
    let two = &grid4()[..2];
    e.engine.set_item_detector(Stub::new(two));
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(v.crops.len(), 2);
    e.engine.accept_scan(v.id).unwrap();
    assert!(e.save(v.id, SaveTarget::Replace).ok);
    assert_eq!(e.files(), ["scan_01.jpg", "scan_02.jpg"]);
    // The user draws the other two by hand.
    e.engine.add_crop(v.id, Some(grid4()[2]), None).unwrap();
    let v = e.engine.add_crop(v.id, Some(grid4()[3]), None).unwrap();
    assert_eq!(v.crops.len(), 4);
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(
        e.files(),
        ["scan_01.jpg", "scan_02.jpg", "scan_03.jpg", "scan_04.jpg"]
    );
    // Down to one: the original name, the old set retired, no leftovers.
    let mut v = v;
    for c in v.crops.clone().iter().skip(1) {
        v = e.engine.remove_crop(v.id, c.id).unwrap();
    }
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(e.files(), ["scan.jpg"]);
    let f = e.engine.list_backups().runs[0].files[0].clone();
    assert_eq!(f.derived.len(), 1);
}

#[test]
fn a_re_save_never_replaces_a_derived_file_the_user_edited() {
    let e = env();
    let (v, outputs, _) = saved_split(&e);
    fs::write(e.dir.join(&outputs[1]), b"edited by the user").unwrap();
    let v = e.engine.remove_crop(v.id, v.crops[3].id).unwrap();
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert!(out.notices.contains(&"derived.user_edited".to_owned()));
    // Their file is exactly as they left it; the new set has its own base name.
    assert_eq!(
        fs::read(e.dir.join(&outputs[1])).unwrap(),
        b"edited by the user"
    );
    let names = out.saved.unwrap().outputs;
    assert_eq!(
        names,
        ["scan (2)_01.jpg", "scan (2)_02.jpg", "scan (2)_03.jpg"]
    );
    for n in &names {
        assert!(e.dir.join(n).exists());
    }
}

// ------------------------------------------------------------------ operations and history

#[test]
fn every_operation_is_one_undo_step_labelled_with_the_item_and_undo_redo_are_exact() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v0 = e.open("scan.jpg", &grid4());
    let ids: Vec<u32> = v0.crops.iter().map(|c| c.id).collect();

    let v = e.engine.remove_crop(v0.id, ids[1]).unwrap();
    assert_eq!(v.undo_label.as_deref(), Some("Remove (item 2)"));
    assert_eq!(v.crops.iter().filter(|c| c.include).count(), 3);
    assert_eq!(v.crops[1].order, 0, "excluded");
    let v = e.engine.restore_crop(v0.id, ids[1]).unwrap();
    assert_eq!(v.undo_label.as_deref(), Some("Restore (item 2)"));
    let v = e.engine.turn_crop(v0.id, ids[2], true).unwrap();
    assert_eq!(v.crops[2].edit.as_ref().unwrap().quarter_turns, 1);
    assert_eq!(v.undo_label.as_deref(), Some("Turn right (item 3)"));
    let v = e.engine.flip_crop(v0.id, ids[2]).unwrap();
    assert!(v.crops[2].mirror);
    let v = e
        .engine
        .cut_crop(v0.id, ids[0], Cut::halves(CutAxis::Vertical))
        .unwrap();
    assert_eq!(v.crops.len(), 5);
    let v = e
        .engine
        .merge_crops(v0.id, &[v.crops[0].id, v.crops[1].id])
        .unwrap();
    assert_eq!(v.crops.len(), 4);
    let last = e.engine.move_crop(v0.id, v.crops[3].id, 0).unwrap();
    assert_eq!(
        last.split.as_ref().unwrap().order_mode,
        auto_crop_core::OrderMode::Manual
    );

    // Undo everything, step by step: ends at the analysis result, and redo walks back.
    let mut steps = 0;
    let mut cur = last.clone();
    while cur.can_undo {
        cur = e.engine.undo(v0.id).unwrap();
        steps += 1;
    }
    assert_eq!(steps, 7, "one entry per operation");
    assert_eq!(cur.crops, v0.crops);
    while cur.can_redo {
        cur = e.engine.redo(v0.id).unwrap();
    }
    assert_eq!(cur.crops, last.crops);
}

#[test]
fn a_refused_operation_is_a_typed_error_and_changes_nothing() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let removed = e.engine.remove_crop(v.id, v.crops[0].id).unwrap();
    for r in [
        e.engine.merge_crops(v.id, &[v.crops[0].id, v.crops[1].id]), // an excluded item
        e.engine.merge_crops(v.id, &[v.crops[1].id]),                // one is not a merge
        e.engine.cut_crop(v.id, 999, Cut::halves(CutAxis::Vertical)),
        e.engine.remove_crop(v.id, 999),
        e.engine.set_crop_edit(
            v.id,
            999,
            &v.crops[1].edit.clone().unwrap(),
            false,
            "x",
            None,
        ),
    ] {
        assert_eq!(r.unwrap_err(), ErrKind::ItemOp);
    }
    let after = e.engine.item_view(v.id).unwrap();
    assert_eq!(after.crops, removed.crops);
    assert_eq!(after.history_position, removed.history_position);
}

#[test]
fn reverting_one_crop_touches_only_that_crop() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let mut edit = v.crops[0].edit.clone().unwrap();
    edit.fine_deg = 3.0;
    let mid = e
        .engine
        .set_crop_edit(v.id, v.crops[0].id, &edit, false, "Straighten", None)
        .unwrap();
    let position = mid.history_position;
    let mut edit2 = v.crops[2].edit.clone().unwrap();
    edit2.fine_deg = -4.0;
    let both = e
        .engine
        .set_crop_edit(v.id, v.crops[2].id, &edit2, false, "Straighten", None)
        .unwrap();
    // Revert crop 1 to the detector's proposal: crop 3's edit stays.
    let r = e
        .engine
        .revert_crop(v.id, v.crops[0].id, RevertTo::Auto)
        .unwrap();
    assert_eq!(r.crops[0], v.crops[0]);
    for k in 1..4 {
        assert_eq!(r.crops[k], both.crops[k], "crop {k} changed");
    }
    // Revert crop 3 to the state after the first edit (where it was still untouched).
    let r2 = e
        .engine
        .revert_crop(v.id, v.crops[2].id, RevertTo::Step { position })
        .unwrap();
    assert_eq!(r2.crops[2].edit, v.crops[2].edit);
    // A crop added by hand has no proposal to go back to.
    let added = e
        .engine
        .add_crop(v.id, None, Some(Pt::new(0.5, 0.5)))
        .unwrap();
    let new_id = added
        .crops
        .iter()
        .find(|c| c.origin == auto_crop_engine::CropOrigin::Manual)
        .unwrap()
        .id;
    assert_eq!(
        e.engine
            .revert_crop(v.id, new_id, RevertTo::Auto)
            .unwrap_err(),
        ErrKind::ItemOp
    );
}

#[test]
fn treat_as_one_item_is_one_undo_step_and_split_again_restores_it() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(v.crops.len(), 4);
    let one = e
        .engine
        .redetect(
            v.id,
            SplitPatch {
                policy: Some(auto_crop_core::SplitPolicy::Never),
                profile: None,
            },
        )
        .unwrap();
    assert!(
        one.crops.len() <= 1,
        "the single-item route found {} crops",
        one.crops.len()
    );
    assert_eq!(one.undo_label.as_deref(), Some("Treat as one item"));
    assert!(!one.split.as_ref().unwrap().is_split);
    let back = e.engine.undo(v.id).unwrap();
    assert_eq!(back.crops, v.crops);
    // Split into items again with the detector: the same crops, same ids.
    let again = e
        .engine
        .redetect(
            v.id,
            SplitPatch {
                policy: Some(auto_crop_core::SplitPolicy::Always),
                profile: Some(auto_crop_core::SplitProfile::Receipts),
            },
        )
        .unwrap();
    assert_eq!(again.crops.len(), 4);
    assert_eq!(
        again.split.as_ref().unwrap().profile,
        auto_crop_core::SplitProfile::Receipts
    );
}

#[test]
fn redetection_keeps_what_the_user_placed_edited_or_removed() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let mut edit = v.crops[0].edit.clone().unwrap();
    edit.fine_deg = 2.0;
    e.engine
        .set_crop_edit(v.id, v.crops[0].id, &edit, false, "Straighten", None)
        .unwrap();
    e.engine.remove_crop(v.id, v.crops[1].id).unwrap();
    let edited = e.engine.item_view(v.id).unwrap();
    let r = e.engine.redetect(v.id, SplitPatch::default()).unwrap();
    assert_eq!(r.crops.len(), 4, "nothing doubled");
    assert_eq!(
        r.crops[0], edited.crops[0],
        "the edited crop is as the user left it"
    );
    assert!(!r.crops[1].include, "a removed item stays removed");
    assert_eq!(
        r.crops[2].id, v.crops[2].id,
        "untouched ones keep their ids"
    );
}

#[test]
fn the_legacy_single_item_edit_still_edits_the_first_crop_and_keeps_the_rest() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let mut edit = v.edit.clone().unwrap();
    edit.fine_deg = 2.5;
    let r = e.engine.set_edit(v.id, &edit, false, "Rotate").unwrap();
    assert_eq!(r.crops.len(), 4);
    assert_eq!(r.crops[0].edit.as_ref().unwrap().fine_deg, 2.5);
    for k in 1..4 {
        assert_eq!(r.crops[k], v.crops[k]);
    }
    let reset = e.engine.reset_to_auto(v.id).unwrap();
    assert_eq!(reset.crops, v.crops);
}

// ------------------------------------------------------------------ faults inside the engine

#[test]
fn a_crash_at_any_step_of_an_engine_save_is_recovered_at_the_next_start() {
    // Record the steps of one save, then crash at each, restart the engine and check the folder.
    let steps = {
        let e = env();
        e.engine.set_item_detector(Stub::new(&grid4()));
        let v = e.open("scan.jpg", &grid4());
        e.engine.accept_scan(v.id).unwrap();
        let seen = RefCell::new(Vec::new());
        let hook = |s: &Step| {
            seen.borrow_mut().push(*s);
            Fault::Pass
        };
        let o = e
            .engine
            .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &hook);
        assert!(o[0].ok, "{o:?}");
        seen.into_inner()
    };
    assert!(steps.len() > 20);
    for step in steps {
        let e = env();
        e.engine.set_item_detector(Stub::new(&grid4()));
        let v = e.open("scan.jpg", &grid4());
        let scan = fs::read(e.dir.join("scan.jpg")).unwrap();
        e.engine.accept_scan(v.id).unwrap();
        let hook = move |s: &Step| {
            if *s == step {
                Fault::Crash
            } else {
                Fault::Pass
            }
        };
        let o = e
            .engine
            .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &hook);
        assert!(!o[0].ok);
        // The next start of the app.
        let e2 = Engine::new(e.paths());
        let files = e.files();
        let ctx = format!("crash at {step:?}: {files:?}");
        assert!(
            files == ["scan.jpg"]
                || files == ["scan_01.jpg", "scan_02.jpg", "scan_03.jpg", "scan_04.jpg"],
            "{ctx}"
        );
        let runs = e2.list_backups().runs;
        if files == ["scan.jpg"] {
            assert_eq!(fs::read(e.dir.join("scan.jpg")).unwrap(), scan, "{ctx}");
            assert!(runs.is_empty(), "{ctx}: an unused backup is left");
        } else {
            // The complete set, and the scan restorable byte for byte.
            let f = &runs[0].files[0];
            assert_eq!(f.derived.len(), 4, "{ctx}");
            let r = e2.restore_file_derived(&f.id, RestoreMode::Auto, DerivedAction::Remove, &nop);
            assert!(r.ok, "{ctx}: {r:?}");
            assert_eq!(fs::read(e.dir.join("scan.jpg")).unwrap(), scan, "{ctx}");
        }
        assert!(
            fs::read_dir(&e.dir)
                .unwrap()
                .flatten()
                .all(|f| !f.file_name().to_string_lossy().starts_with(".autocrop-")),
            "{ctx}: temps"
        );
    }
}

#[test]
fn a_failure_in_the_commit_writes_nothing_and_is_reported() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let scan = fs::read(e.dir.join("scan.jpg")).unwrap();
    e.engine.accept_scan(v.id).unwrap();
    let hook = |s: &Step| {
        if *s == Step::AfterRename(1) {
            Fault::Fail(ErrKind::DiskFull)
        } else {
            Fault::Pass
        }
    };
    let o = e
        .engine
        .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &hook);
    assert_eq!(o[0].error, Some(ErrKind::DiskFull));
    assert_eq!(e.files(), ["scan.jpg"]);
    assert_eq!(fs::read(e.dir.join("scan.jpg")).unwrap(), scan);
    assert!(e.engine.list_backups().runs.is_empty());
    // And the image can be saved after the cause is gone.
    assert!(e.save(v.id, SaveTarget::Replace).ok);
}

// ------------------------------------------------------------------ determinism, panics, session

#[test]
fn a_crop_renders_to_identical_bytes_on_one_thread_and_on_eight() {
    let render = |threads: usize| {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap();
        pool.install(|| {
            let e = env();
            e.engine.set_item_detector(Stub::new(&grid4()));
            let v = e.open("scan.jpg", &grid4());
            v.crops
                .iter()
                .map(|c| {
                    let (b, _) = e
                        .engine
                        .crop_image_bytes(v.id, c.id, CropImage::Result)
                        .unwrap();
                    b.as_ref().clone()
                })
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(render(1), render(8));
}

/// Panics on the scene whose top-left pixel is red 7, answers normally otherwise.
struct Panicky(Arc<Stub>);

impl ItemDetector for Panicky {
    fn detect(
        &self,
        r: &Raster,
        p: auto_crop_core::SplitPolicy,
        q: auto_crop_core::SplitProfile,
    ) -> Option<SplitDetection> {
        assert!(r.pixel(0, 0)[0] != 7, "a detector bug on this scan");
        self.0.detect(r, p, q)
    }
}

#[test]
fn a_panic_while_analysing_one_scan_fails_only_that_scan() {
    let e = env();
    e.engine
        .set_item_detector(Arc::new(Panicky(Stub::new(&grid4()))));
    // Two scenes; the first has a marker pixel that makes the detector panic.
    let bad = e.dir.join("bad.png");
    let mut r = scene(&grid4(), &[COLOURS[0], COLOURS[1], COLOURS[2], COLOURS[3]]);
    r.set_pixel(0, 0, [7, 7, 7]);
    fs::write(&bad, encode(&r, Format::Png, 90, None).unwrap()).unwrap();
    let good = e.open("good.jpg", &grid4());
    assert_eq!(good.crops.len(), 4);
    let s = e.engine.open_paths(std::slice::from_ref(&bad), false);
    let v = e.engine.analyse(s.ids[0]).unwrap();
    assert_eq!(v.status, auto_crop_engine::ItemStatus::Error);
    assert_eq!(v.error, Some(ErrKind::Internal));
    assert_eq!(
        fs::read(&bad).unwrap(),
        encode(&r, Format::Png, 90, None).unwrap()
    );
    // The engine and the other scan are fine.
    assert_eq!(e.engine.item_view(good.id).unwrap().crops.len(), 4);
}

#[test]
fn one_undo_reverts_a_preset_applied_to_twenty_scans() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let ids: Vec<u32> = (0..20)
        .map(|i| {
            e.open_sized(&format!("s{i:02}.jpg", i = i), &grid4(), 480, 360)
                .id
        })
        .collect();
    let before: Vec<_> = ids
        .iter()
        .map(|id| e.engine.item_view(*id).unwrap())
        .collect();
    assert!(before.iter().all(|v| v.crops.len() == 4));
    let results = e.engine.redetect_many(
        &ids,
        SplitPatch {
            policy: Some(auto_crop_core::SplitPolicy::Never),
            profile: None,
        },
    );
    assert!(results.iter().all(|(_, r)| r.is_ok()));
    assert!(
        ids.iter()
            .all(|id| e.engine.item_view(*id).unwrap().crops.len() <= 1)
    );
    // One undo, all twenty back, every crop identical to before.
    let (label, views) = e.engine.session_undo().expect("a session step");
    assert_eq!(label, "Treat as one item (20 images)");
    assert_eq!(views.len(), 20);
    for (id, b) in ids.iter().zip(&before) {
        assert_eq!(e.engine.item_view(*id).unwrap().crops, b.crops);
    }
    assert!(e.engine.session_undo().is_none());
    // And redo takes them all forward again.
    let (_, forward) = e.engine.session_redo().expect("redo");
    assert_eq!(forward.len(), 20);
    assert!(forward.iter().all(|v| v.crops.len() <= 1));
}

#[test]
fn a_panic_in_the_middle_of_a_save_fails_that_scan_and_the_next_start_repairs_the_folder() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    let other = e.open("other.jpg", &grid4());
    let scan = fs::read(e.dir.join("scan.jpg")).unwrap();
    e.engine.accept_scan(v.id).unwrap();
    e.engine.accept_scan(other.id).unwrap();
    // The commit panics after the first output is in place.
    let hook = |s: &Step| {
        if *s == Step::AfterRename(0) {
            panic!("a bug in the middle of a save");
        }
        Fault::Pass
    };
    let o = e
        .engine
        .save_items_with_faults(&[v.id], SaveTarget::Replace, "r", &hook);
    assert_eq!(o[0].error, Some(ErrKind::InternalPanic));
    // The engine is alive and the other scan saves normally.
    assert!(e.save(other.id, SaveTarget::Replace).ok);
    // The next start finishes the interrupted group (it had reached the committing phase), so the
    // folder holds the complete set or the scan, never a partial set.
    let e2 = Engine::new(e.paths());
    let files = e.files();
    let set: Vec<String> = (1..=4).map(|n| format!("scan_{n:02}.jpg")).collect();
    let partial = files
        .iter()
        .filter(|f| f.starts_with("scan_") || *f == "scan.jpg")
        .cloned()
        .collect::<Vec<_>>();
    assert!(partial == set || partial == ["scan.jpg"], "{files:?}");
    if partial == set {
        let r = e2.restore_file_derived(
            &e2.list_backups()
                .runs
                .iter()
                .flat_map(|r| r.files.iter())
                .find(|f| f.name == "scan.jpg")
                .unwrap()
                .id,
            RestoreMode::Auto,
            DerivedAction::Remove,
            &nop,
        );
        assert!(r.ok, "{r:?}");
        assert_eq!(fs::read(e.dir.join("scan.jpg")).unwrap(), scan);
    }
}

// ------------------------------------------------------------------ the real detector

/// IoU of two convex quads in pixels (the engine's own geometry is in `core`, but a test needs
/// only a rough check): the share of the bounding boxes' overlap is enough at these separations.
fn bbox_iou(a: &Quad, b: &Quad) -> f64 {
    let bb = |q: &Quad| {
        let xs = q.iter().map(|p| p.x);
        let ys = q.iter().map(|p| p.y);
        (
            xs.clone().fold(f64::MAX, f64::min),
            ys.clone().fold(f64::MAX, f64::min),
            xs.fold(f64::MIN, f64::max),
            ys.fold(f64::MIN, f64::max),
        )
    };
    let (a, b) = (bb(a), bb(b));
    let (iw, ih) = (
        (a.2.min(b.2) - a.0.max(b.0)).max(0.0),
        (a.3.min(b.3) - a.1.max(b.1)).max(0.0),
    );
    let inter = iw * ih;
    inter / ((a.2 - a.0) * (a.3 - a.1) + (b.2 - b.0) * (b.3 - b.1) - inter)
}

#[test]
fn the_classical_detector_splits_a_synthetic_scan_and_the_whole_flow_works() {
    // The engine's default detector is the classical one: no stub installed here.
    let e = env();
    let v = e.open("scan.jpg", &grid4());
    assert_eq!(v.status, auto_crop_engine::ItemStatus::Ready);
    assert_eq!(v.crops.len(), 4, "{:?}", v.confidence);
    // Every known item is found, once, close to where it is.
    let truth = {
        let mut t = grid4();
        t.sort_by(|a, b| {
            (a[0].y.round(), a[0].x)
                .partial_cmp(&(b[0].y.round(), b[0].x))
                .unwrap()
        });
        t
    };
    for (c, t) in v.crops.iter().zip(&truth) {
        let got = c.edit.as_ref().unwrap().quad;
        assert!(
            bbox_iou(&got, t) > 0.9,
            "crop {} off: {:?} vs {:?}",
            c.id,
            got,
            t
        );
    }
    // Held by default; accepted by the user; saved as four files; restored.
    // A clean scene may be Approved by triage, but the default still holds a split.
    assert_eq!(
        e.save(v.id, SaveTarget::Replace).error,
        Some(ErrKind::HeldForReview)
    );
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs.len(), 4);
    assert_eq!(e.files().len(), 4);
}

#[test]
fn a_single_document_on_a_desk_still_takes_the_single_item_route() {
    // The existing sample set: none of these is a multi-item scan, so the classical detector must
    // leave every one on the single-item route and the old results unchanged.
    let e = env();
    let files = auto_crop_engine::samples::write_samples(&e.dir).unwrap();
    let s = e.engine.open_paths(&files, false);
    for id in s.ids {
        let v = e.engine.analyse(id).unwrap();
        assert!(v.crops.len() <= 1, "{}: {} crops", v.name, v.crops.len());
        assert!(!v.split.as_ref().unwrap().is_split, "{}", v.name);
    }
}

#[test]
fn a_single_crop_after_a_copy_split_replaces_in_place_the_ordinary_way() {
    let e = env();
    e.engine.set_item_detector(Stub::new(&grid4()));
    let v = e.open("scan.jpg", &grid4());
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Copy);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().outputs.len(), 4);
    // Edit it down to one crop and save as a replacement: the plain name, in place, with a backup.
    let mut v = v;
    for c in v.crops.clone().iter().skip(1) {
        v = e.engine.remove_crop(v.id, c.id).unwrap();
    }
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    assert_eq!(out.saved.unwrap().output, "scan.jpg");
    assert_eq!(e.files(), ["scan.jpg"]);
    let f = e.engine.list_backups().runs[0].files[0].clone();
    assert_eq!(f.kind, auto_crop_engine::store::BackupKind::OneToOne);
    // The copies from before are still there; nothing of the user's was removed.
    assert_eq!(fs::read_dir(e.dir.join("AutoCrop")).unwrap().count(), 4);
}

#[test]
fn a_replaced_single_save_that_gains_items_retires_the_old_single_output() {
    let e = env();
    // Start as an ordinary single-item save.
    e.engine
        .set_item_detector(Arc::new(auto_crop_engine::NoSplit));
    let one = e.open("scan.jpg", &grid4());
    assert!(one.crops.len() <= 1);
    let mut v = one;
    if v.crops.is_empty() {
        v = e.engine.draw_crop(v.id).unwrap();
    }
    assert!(e.save(v.id, SaveTarget::Replace).ok);
    let single_output = fs::read(e.dir.join("scan.jpg")).unwrap();
    // Now the user draws two more items and saves again: the cropped file moves into the store,
    // two new files appear.
    let v = e.engine.add_crop(v.id, Some(grid4()[0]), None).unwrap();
    let v = e.engine.add_crop(v.id, Some(grid4()[1]), None).unwrap();
    assert!(v.crops.len() >= 3);
    e.engine.accept_scan(v.id).unwrap();
    let out = e.save(v.id, SaveTarget::Replace);
    assert!(out.ok, "{out:?}");
    let names = out.saved.unwrap().outputs;
    assert!(names.len() >= 3 && !e.files().contains(&"scan.jpg".to_owned()));
    let bid = e.engine.list_backups().runs[0].files[0]
        .id
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let superseded = e.engine.paths().backups_dir().join(&bid).join("superseded");
    let kept: Vec<Vec<u8>> = fs::read_dir(superseded)
        .unwrap()
        .flatten()
        .map(|f| fs::read(f.path()).unwrap())
        .collect();
    assert!(
        kept.contains(&single_output),
        "the old single output is in the store"
    );
    // And the scan restores byte for byte.
    let r = e.engine.restore_file_derived(
        &e.engine.list_backups().runs[0].files[0].id,
        RestoreMode::Auto,
        DerivedAction::Remove,
        &nop,
    );
    assert!(r.ok, "{r:?}");
    assert_eq!(e.files(), ["scan.jpg"]);
}
