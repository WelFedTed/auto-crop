// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! What every command that looks at an image does first: read it, check it can be handled, wait
//! for memory, let the engine decode and analyse it, and decide whether the result is good
//! enough to write. `process` goes on to write, `analyze` reports.
//!
//! The decision (`decide`) is the one place that holds results back (B4): a single crop is
//! written only when its confidence is Good at the run's cut-off; a failed detection is never
//! written; a scan with several items follows the engine's split rule (held in place unless
//! accepted, written as a copy). No flag forces a held item through.

use crate::inputs::Candidate;
use crate::manifest::{ConfidenceRec, ItemRecord, Status, code};
use auto_crop_codecs::{CodecError, Format, probe};
use auto_crop_core::{Band, CancelToken, Confidence, ErrKind};
use auto_crop_engine::memory::{MemoryBudget, Permit, job_weight};
use auto_crop_engine::util::blake3_hex;
use auto_crop_engine::{Engine, ItemStatus, ItemView};
use std::collections::HashMap;
use std::fs;
use std::time::{Duration, Instant, SystemTime};

/// The most a file may weigh before it is even read (the engine's own cap).
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

pub struct Shared {
    pub engine: Engine,
    pub budget: MemoryBudget,
    pub cancel: CancelToken,
    pub cutoff: f32,
    /// Hash of every output of an earlier run, with the backup that made it.
    pub processed: HashMap<String, String>,
    pub reprocess: bool,
    /// The run would replace originals: a source that cannot be written back is skipped.
    pub replacing: bool,
    /// Test hook (`AUTO_CROP_TEST_DELAY_MS`): sleep before each item.
    pub delay: Duration,
}

/// An image the engine has analysed.
pub struct Ready {
    pub id: u32,
    pub view: ItemView,
    /// Held until the item is done, so the memory of the running jobs stays under the cap.
    pub _permit: Permit,
    pub format: Format,
    pub mtime: Option<SystemTime>,
    pub read_ms: f64,
    pub analyse_ms: f64,
}

pub enum Prepared {
    /// Nothing more to do: skipped or failed.
    Done(Box<ItemRecord>),
    Ready(Box<Ready>),
}

/// The stable name of an error code.
pub fn err_code(e: ErrKind) -> String {
    serde_json::to_value(e)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "INTERNAL".to_owned())
}

/// Codes that mean "this file is not something this build reads", which is a skip, not a failure.
fn is_unsupported(k: ErrKind) -> bool {
    matches!(
        k,
        ErrKind::UnsupportedFormat | ErrKind::UnsupportedFeature | ErrKind::HevcDecoderMissing
    )
}

/// A failed or skipped record for an error code.
pub fn error_record(idx: usize, cand: &Candidate, k: ErrKind) -> ItemRecord {
    let status = if is_unsupported(k) {
        Status::Skipped
    } else {
        Status::Failed
    };
    ItemRecord::new(idx, &cand.display, status).with_code(err_code(k))
}

fn skipped(idx: usize, cand: &Candidate, c: &str) -> Box<ItemRecord> {
    Box::new(ItemRecord::new(idx, &cand.display, Status::Skipped).with_code(c))
}

fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 1000.0 * 10.0).round() / 10.0
}

/// Reads, checks, admits and analyses one file.
pub fn prepare(sh: &Shared, idx: usize, cand: &Candidate) -> Prepared {
    let t_read = Instant::now();
    if !sh.delay.is_zero() {
        std::thread::sleep(sh.delay);
    }
    if sh.cancel.is_cancelled() {
        return Prepared::Done(skipped(idx, cand, code::CANCELLED));
    }
    let fail = |k: ErrKind| Prepared::Done(Box::new(error_record(idx, cand, k)));
    let meta = match fs::symlink_metadata(&cand.path) {
        Ok(m) => m,
        Err(e) => return fail(ErrKind::from_io(&e)),
    };
    if crate::inputs::is_link(&meta) {
        return Prepared::Done(skipped(idx, cand, code::LINK));
    }
    if !meta.is_file() {
        return fail(ErrKind::Unreadable);
    }
    if meta.len() > MAX_SOURCE_BYTES {
        return fail(ErrKind::TooLarge);
    }
    let bytes = match fs::read(&cand.path) {
        Ok(b) => b,
        Err(e) => return fail(ErrKind::from_io(&e)),
    };
    let pr = match auto_crop_engine::run_isolated(|| probe(&bytes)) {
        Ok(Ok(p)) => p,
        Ok(Err(CodecError::Unsupported)) => {
            return Prepared::Done(skipped(idx, cand, code::UNSUPPORTED_FORMAT));
        }
        Ok(Err(e)) => return fail(auto_crop_engine::error::codec_err(e)),
        Err(_) => return fail(ErrKind::InternalPanic),
    };
    if !sh.reprocess
        && !sh.processed.is_empty()
        && let Some(backup) = sh.processed.get(&blake3_hex(&bytes))
    {
        let mut rec = ItemRecord::new(idx, &cand.display, Status::Skipped)
            .with_code(code::ALREADY_PROCESSED)
            .with_detail(format!("an output of backup {backup}"));
        rec.backup_id = Some(backup.clone());
        return Prepared::Done(Box::new(rec));
    }
    if sh.replacing && !(pr.format.is_encodable() && pr.frames <= 1) {
        // Never replaced (PLAN 2.7): the source stays byte-identical. Say why with the notice
        // vocabulary the engine uses.
        let mut rec =
            ItemRecord::new(idx, &cand.display, Status::Skipped).with_code(code::NOT_REPLACEABLE);
        rec.reasons.push(
            if pr.frames > 1 {
                "tiff.multi_page"
            } else {
                "format.write_unavailable"
            }
            .to_owned(),
        );
        return Prepared::Done(Box::new(rec));
    }
    let pixels = u64::from(pr.width) * u64::from(pr.height);
    if pixels > sh.engine.options().max_pixels {
        return fail(ErrKind::TooLarge);
    }
    drop(bytes);
    let permit = match sh.budget.acquire(job_weight(pixels), &sh.cancel) {
        Ok(p) => p,
        Err(ErrKind::Cancelled | ErrKind::DeadlineExceeded) => {
            return Prepared::Done(skipped(idx, cand, code::CANCELLED));
        }
        Err(k) => return fail(k),
    };
    let read_ms = ms(t_read);

    let t_an = Instant::now();
    let summary = sh
        .engine
        .open_paths(std::slice::from_ref(&cand.path), false);
    let Some(&id) = summary.ids.first() else {
        return fail(ErrKind::Internal);
    };
    let view = sh.engine.analyse(id);
    let analyse_ms = ms(t_an);
    let Some(view) = view else {
        sh.engine.remove_items(&[id]);
        return fail(ErrKind::Internal);
    };
    if view.status == ItemStatus::Error {
        sh.engine.remove_items(&[id]);
        return fail(view.error.unwrap_or(ErrKind::Internal));
    }
    Prepared::Ready(Box::new(Ready {
        id,
        view,
        _permit: permit,
        format: pr.format,
        mtime: meta.modified().ok(),
        read_ms,
        analyse_ms,
    }))
}

/// The reason codes of a confidence record.
pub fn reason_codes(c: &Confidence) -> Vec<String> {
    c.reasons
        .iter()
        .filter_map(|r| serde_json::to_value(r.code).ok())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

pub fn band_word(b: Band) -> &'static str {
    match b {
        Band::Good => "good",
        Band::Check => "check",
        Band::Failed => "failed",
    }
}

pub fn confidence_rec(c: &Confidence, cutoff: f32) -> ConfidenceRec {
    ConfidenceRec {
        score: (c.score * 1000.0).round() / 1000.0,
        band: band_word(c.band(cutoff)),
        reasons: reason_codes(c),
    }
}

/// What to do with an analysed image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Write {
        /// Files written: 1, or N for a split.
        crops: usize,
        split: bool,
    },
    Hold {
        code: &'static str,
        reasons: Vec<String>,
    },
}

/// The crops that would be written.
pub fn writable_crops(view: &ItemView) -> usize {
    view.crops
        .iter()
        .filter(|c| c.include && c.edit.is_some())
        .count()
}

pub fn decide(view: &ItemView, cutoff: f32) -> Decision {
    let reasons = view
        .confidence
        .as_ref()
        .map(reason_codes)
        .unwrap_or_default();
    match writable_crops(view) {
        0 => Decision::Hold {
            code: code::DETECTION_FAILED,
            reasons,
        },
        1 => match view.confidence.as_ref().map(|c| c.band(cutoff)) {
            Some(Band::Good) => Decision::Write {
                crops: 1,
                split: false,
            },
            Some(Band::Failed) => Decision::Hold {
                code: code::DETECTION_FAILED,
                reasons,
            },
            // No confidence at all is as good as an unsure one.
            Some(Band::Check) | None => Decision::Hold {
                code: code::LOW_CONFIDENCE,
                reasons,
            },
        },
        n => Decision::Write {
            crops: n,
            split: true,
        },
    }
}

/// Hold reasons of a split scan: what each item that needs a look raised.
pub fn split_reasons(view: &ItemView, cutoff: f32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in view.crops.iter().filter(|c| c.include && c.edit.is_some()) {
        if let Some(conf) = &c.confidence
            && conf.band(cutoff) != Band::Good
        {
            for r in reason_codes(conf) {
                if !out.contains(&r) {
                    out.push(r);
                }
            }
        }
    }
    out
}

/// The base of a record for an analysed image: confidence, crops and split.
pub fn analysed_record(
    idx: usize,
    cand: &Candidate,
    r: &Ready,
    cutoff: f32,
    status: Status,
) -> ItemRecord {
    let mut rec = ItemRecord::new(idx, &cand.display, status);
    rec.confidence = r
        .view
        .confidence
        .as_ref()
        .map(|c| confidence_rec(c, cutoff));
    rec.crops = Some(writable_crops(&r.view));
    rec.split = r.view.split.as_ref().is_some_and(|s| s.is_split);
    rec
}

pub fn round_ms(t: Instant) -> f64 {
    ms(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use auto_crop_core::{Forced, Reason, ReasonCode};

    fn conf(score: f32, forced: Option<Forced>) -> Confidence {
        Confidence {
            score,
            forced,
            reasons: vec![],
        }
    }

    #[test]
    fn error_codes_use_the_registry_names() {
        assert_eq!(err_code(ErrKind::HeldForReview), "HELD_FOR_REVIEW");
        assert_eq!(err_code(ErrKind::Corrupt), "CORRUPT");
        assert!(is_unsupported(ErrKind::UnsupportedFormat) && !is_unsupported(ErrKind::Corrupt));
    }

    #[test]
    fn bands_follow_the_cutoff() {
        let c = conf(0.92, None);
        assert_eq!(band_word(c.band(0.95)), "check");
        assert_eq!(band_word(c.band(0.90)), "good");
        assert_eq!(band_word(conf(0.5, None).band(0.8)), "failed");
        assert_eq!(
            band_word(conf(0.99, Some(Forced::Failed)).band(0.8)),
            "failed"
        );
        let mut held = conf(0.99, None);
        held.reasons.push(Reason {
            code: ReasonCode::PartialFrame,
            side: None,
        });
        assert_eq!(
            band_word(held.band(0.8)),
            "check",
            "a hold reason caps the band"
        );
        assert_eq!(reason_codes(&held), ["PARTIAL_FRAME"]);
    }
}
