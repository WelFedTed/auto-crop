// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The predictor interface and the predictors that need no image: `FullFrame`, the `Oracle` and
//! the `Jittered` oracle used by the self-check, and the JSON-lines adapter for any external tool.
//! The detector adapter lives in [`crate::detector`].
//!
//! A predictor sees only the image path and size, never the ground truth. The oracle predictors
//! are built from a manifest on purpose and exist to validate the harness, not to be measured.

use crate::geom::Quad;
use crate::manifest::{Manifest, ManifestItem};
use crate::stats::SplitMix64;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// What a predictor is shown.
#[derive(Debug, Clone)]
pub struct PredictInput {
    pub id: String,
    pub image: PathBuf,
    pub width: u32,
    pub height: u32,
}

impl PredictInput {
    pub fn of(m: &Manifest, it: &ManifestItem) -> Self {
        Self {
            id: it.id.clone(),
            image: m.resolve(it),
            width: it.width,
            height: it.height,
        }
    }
}

/// The triage state a predictor assigns (PLAN 7.4: Good is auto-accepted, Check is held for
/// review, Failed leaves the original untouched).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Good,
    Check,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prediction {
    /// TL, TR, BR, BL in 0..1 EXIF-oriented coordinates; `None` = "no page found".
    pub quad: Option<Quad>,
    /// Confidence in 0..1 that the result is acceptable; `None` if the predictor has none.
    pub confidence: Option<f64>,
    /// `None` means "no triage": the result counts as auto-accepted.
    pub verdict: Option<Verdict>,
}

impl Prediction {
    pub fn quad(q: Quad) -> Self {
        Self {
            quad: Some(q),
            confidence: None,
            verdict: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredictError {
    /// The predictor has no output for this image (a missing line, a skipped file).
    Missing,
    /// The predictor ran and failed.
    Failed(String),
}

pub trait Predictor: Send + Sync {
    /// A stable label recorded in the results header.
    fn name(&self) -> String;
    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError>;
    /// Run-level facts worth recording (for example, malformed input lines skipped).
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The whole frame, always. The floor every real predictor must beat.
pub struct FullFrame;

pub const FULL_FRAME_QUAD: Quad = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

impl Predictor for FullFrame {
    fn name(&self) -> String {
        "full-frame".to_owned()
    }

    fn predict(&self, _: &PredictInput) -> Result<Prediction, PredictError> {
        Ok(Prediction::quad(FULL_FRAME_QUAD))
    }
}

/// Returns the ground truth. Scores IoU 1.0 with no failures, or the harness is wrong.
pub struct Oracle {
    gt: HashMap<String, Quad>,
}

impl Oracle {
    pub fn from_manifest(m: &Manifest) -> Self {
        Self {
            gt: m.items.iter().map(|i| (i.id.clone(), i.quad)).collect(),
        }
    }
}

impl Predictor for Oracle {
    fn name(&self) -> String {
        "oracle".to_owned()
    }

    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError> {
        self.gt
            .get(&input.id)
            .map(|q| Prediction::quad(*q))
            .ok_or(PredictError::Missing)
    }
}

/// FNV-1a over the id: a stable per-image seed.
pub fn hash_id(id: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in id.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The oracle displaced by `shift`: the whole quad moves by `shift` of its own first edge (TL to
/// TR) along one side and `shift` of its second edge (TL to BL) along the other, each in a
/// pseudo-random direction per image. For a parallelogram ground truth the canonical-warp IoU is
/// then exactly `i / (2 - i)` with `i = (1 - shift)^2`, the "analytic curve" the self-check follows.
pub struct Jittered {
    gt: HashMap<String, Quad>,
    pub shift: f64,
    pub seed: u64,
}

impl Jittered {
    pub fn from_manifest(m: &Manifest, shift: f64, seed: u64) -> Self {
        Self {
            gt: m.items.iter().map(|i| (i.id.clone(), i.quad)).collect(),
            shift,
            seed,
        }
    }

    /// The analytic IoU of a parallelogram displaced by `shift` along both of its edges.
    pub fn analytic_iou(shift: f64) -> f64 {
        let i = (1.0 - shift.abs()) * (1.0 - shift.abs());
        i / (2.0 - i)
    }
}

impl Predictor for Jittered {
    fn name(&self) -> String {
        format!("jittered-oracle(shift={})", self.shift)
    }

    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError> {
        let q = self.gt.get(&input.id).ok_or(PredictError::Missing)?;
        let mut rng = SplitMix64(self.seed ^ hash_id(&input.id));
        let sa = if rng.next_u64() & 1 == 0 { 1.0 } else { -1.0 };
        let sb = if rng.next_u64() & 1 == 0 { 1.0 } else { -1.0 };
        let e1 = [q[1][0] - q[0][0], q[1][1] - q[0][1]];
        let e2 = [q[3][0] - q[0][0], q[3][1] - q[0][1]];
        let d = [
            sa * self.shift * e1[0] + sb * self.shift * e2[0],
            sa * self.shift * e1[1] + sb * self.shift * e2[1],
        ];
        let moved: Quad = std::array::from_fn(|i| [q[i][0] + d[0], q[i][1] + d[1]]);
        Ok(Prediction::quad(moved))
    }
}

/// Panics on every image whose id hash falls in the lowest `1/modulus`, returns the oracle
/// otherwise. Used to prove that crashes are counted as failures and do not abort the run.
pub struct Crashing {
    inner: Oracle,
    pub modulus: u64,
}

impl Crashing {
    pub fn from_manifest(m: &Manifest, modulus: u64) -> Self {
        Self {
            inner: Oracle::from_manifest(m),
            modulus,
        }
    }

    pub fn crashes(&self, id: &str) -> bool {
        hash_id(id).is_multiple_of(self.modulus)
    }
}

impl Predictor for Crashing {
    fn name(&self) -> String {
        "crashing-oracle".to_owned()
    }

    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError> {
        if self.crashes(&input.id) {
            panic!("planted predictor crash");
        }
        self.inner.predict(input)
    }
}

/// One line of a predictions file.
#[derive(Debug, Clone, Deserialize)]
struct PredictionLine {
    id: String,
    #[serde(default)]
    quad: Option<Quad>,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    state: Option<Verdict>,
}

/// Reads predictions from a JSON-lines file: `{"id":..,"quad":[[x,y]x4]|null,"confidence":0.93,
/// "state":"good"|"check"|"failed"}` per image, in 0..1 EXIF-oriented coordinates. Any tool in
/// any language can write it. Missing ids are missing predictions (failures); lines that do not
/// parse are skipped and counted in the run notes; a repeated id keeps the last line.
pub struct JsonLines {
    path: PathBuf,
    by_id: BTreeMap<String, Prediction>,
    bad_lines: usize,
    duplicates: usize,
}

impl JsonLines {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut by_id = BTreeMap::new();
        let (mut bad_lines, mut duplicates) = (0, 0);
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            match serde_json::from_str::<PredictionLine>(line) {
                Ok(p) => {
                    let conf_ok = p.confidence.is_none_or(|c| (0.0..=1.0).contains(&c));
                    if !conf_ok {
                        bad_lines += 1;
                        continue;
                    }
                    let pred = Prediction {
                        quad: p.quad,
                        confidence: p.confidence,
                        verdict: p.state,
                    };
                    if by_id.insert(p.id, pred).is_some() {
                        duplicates += 1;
                    }
                }
                Err(_) => bad_lines += 1,
            }
        }
        Ok(Self {
            path: path.to_owned(),
            by_id,
            bad_lines,
            duplicates,
        })
    }
}

impl Predictor for JsonLines {
    fn name(&self) -> String {
        let file = self
            .path
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
        format!("jsonl({file})")
    }

    fn predict(&self, input: &PredictInput) -> Result<Prediction, PredictError> {
        self.by_id
            .get(&input.id)
            .cloned()
            .ok_or(PredictError::Missing)
    }

    fn notes(&self) -> Vec<String> {
        let mut n = Vec::new();
        if self.bad_lines > 0 {
            n.push(format!(
                "{} malformed prediction lines skipped",
                self.bad_lines
            ));
        }
        if self.duplicates > 0 {
            n.push(format!(
                "{} duplicate ids (last line kept)",
                self.duplicates
            ));
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::parse;

    fn manifest(n: usize) -> Manifest {
        let mut text = String::new();
        for i in 0..n {
            let it = ManifestItem {
                v: 1,
                id: format!("img{i}"),
                image: format!("images/img{i}.jpg"),
                scene_id: format!("s{i}"),
                split: None,
                width: 200,
                height: 100,
                quad: [[0.2, 0.2], [0.7, 0.25], [0.72, 0.8], [0.22, 0.75]],
                tags: BTreeMap::new(),
            };
            text.push_str(&serde_json::to_string(&it).expect("serialises"));
            text.push('\n');
        }
        parse(&text, Path::new(".")).expect("valid")
    }

    fn input(m: &Manifest, i: usize) -> PredictInput {
        PredictInput::of(m, &m.items[i])
    }

    #[test]
    fn oracle_returns_the_truth_and_unknown_ids_are_missing() {
        let m = manifest(2);
        let o = Oracle::from_manifest(&m);
        assert_eq!(
            o.predict(&input(&m, 0)).expect("known").quad,
            Some(m.items[0].quad)
        );
        let mut other = input(&m, 0);
        other.id = "nope".to_owned();
        assert_eq!(o.predict(&other), Err(PredictError::Missing));
    }

    #[test]
    fn jitter_is_deterministic_per_id_and_moves_by_the_requested_fraction() {
        let m = manifest(3);
        let j = Jittered::from_manifest(&m, 0.05, 1);
        let a = j.predict(&input(&m, 1)).expect("known");
        assert_eq!(a, j.predict(&input(&m, 1)).expect("known"));
        let truth = m.items[1].quad;
        let moved = a.quad.expect("quad");
        // All four corners move by the same displacement.
        let d0 = [moved[0][0] - truth[0][0], moved[0][1] - truth[0][1]];
        for i in 1..4 {
            assert!((moved[i][0] - truth[i][0] - d0[0]).abs() < 1e-12);
            assert!((moved[i][1] - truth[i][1] - d0[1]).abs() < 1e-12);
        }
        assert!(d0[0] != 0.0 || d0[1] != 0.0);
        // Different seeds give different directions for at least one image.
        let k = Jittered::from_manifest(&m, 0.05, 2);
        assert!((0..3).any(|i| k.predict(&input(&m, i)) != j.predict(&input(&m, i))));
    }

    #[test]
    fn analytic_curve_endpoints() {
        assert_eq!(Jittered::analytic_iou(0.0), 1.0);
        assert!((Jittered::analytic_iou(0.5) - 0.25 / 1.75).abs() < 1e-15);
        assert!(Jittered::analytic_iou(1.0).abs() < 1e-15);
    }

    #[test]
    fn jsonl_adapter_reads_good_lines_and_counts_bad_ones() {
        let dir = std::env::temp_dir().join(format!("ac-eval-jsonl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let f = dir.join("p.jsonl");
        std::fs::write(
            &f,
            concat!(
                "{\"id\":\"a\",\"quad\":[[0,0],[1,0],[1,1],[0,1]],\"confidence\":0.9,\"state\":\"good\"}\n",
                "{\"id\":\"b\",\"quad\":null,\"state\":\"failed\"}\n",
                "not json at all\n",
                "{\"id\":\"c\",\"quad\":[[0,0],[1,0],[1,1],[0,1]],\"confidence\":7}\n",
                "{\"id\":\"a\",\"quad\":[[0,0],[1,0],[1,1],[0,1]],\"confidence\":0.5}\n",
            ),
        )
        .expect("write");
        let p = JsonLines::load(&f).expect("loads");
        let mk = |id: &str| PredictInput {
            id: id.to_owned(),
            image: PathBuf::new(),
            width: 1,
            height: 1,
        };
        assert_eq!(p.predict(&mk("a")).expect("a").confidence, Some(0.5));
        assert_eq!(p.predict(&mk("b")).expect("b").quad, None);
        assert_eq!(
            p.predict(&mk("b")).expect("b").verdict,
            Some(Verdict::Failed)
        );
        // Bad JSON and an out-of-range confidence are skipped; their ids are missing.
        assert_eq!(p.predict(&mk("c")), Err(PredictError::Missing));
        assert_eq!(p.predict(&mk("zzz")), Err(PredictError::Missing));
        let notes = p.notes().join(" | ");
        assert!(
            notes.contains("2 malformed") && notes.contains("1 duplicate"),
            "{notes}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crashing_predictor_panics_only_on_its_planted_ids() {
        let m = manifest(40);
        let c = Crashing::from_manifest(&m, 5);
        let planted: Vec<usize> = (0..40).filter(|&i| c.crashes(&m.items[i].id)).collect();
        assert!(!planted.is_empty() && planted.len() < 40);
        let r = std::panic::catch_unwind(|| c.predict(&input(&m, planted[0])));
        assert!(r.is_err());
        let ok = (0..40)
            .find(|i| !planted.contains(i))
            .expect("a non-crashing id");
        assert!(c.predict(&input(&m, ok)).is_ok());
    }
}
