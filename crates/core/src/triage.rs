// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Bands and scan-level triage (PLAN 4.9, ROADMAP M10.29). A scan with several items is written
//! only when every item is good: one doubtful item holds the whole scan and nothing is written,
//! because the output is all or nothing (a partial set is never an option, PLAN 2.7).

use crate::confidence::{Confidence, Forced};
use crate::edit::{EditState, Origin};
use serde::{Deserialize, Serialize};

/// The Failed floor in every mode (PLAN 4.9, PROVISIONAL).
pub const FAILED_FLOOR: f32 = 0.60;

/// The Strict cutoff on the uncalibrated v0 score (PLAN 4.9, PROVISIONAL until M2.18). The 0.x
/// preview rule for multi-item scans uses this cutoff whatever mode the batch runs in (M10.29).
pub const STRICT_CUTOFF: f32 = 0.95;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Band {
    Good,
    Check,
    Failed,
}

impl Confidence {
    /// The band at `cutoff`: Failed below the floor or when forced; Check below the cutoff, when
    /// forced, or when any hold reason fired (a hold reason caps an item at Check whatever its
    /// score, PLAN 4.9); else Good.
    pub fn band(&self, cutoff: f32) -> Band {
        if self.forced == Some(Forced::Failed) || self.score < FAILED_FLOOR || self.score.is_nan() {
            Band::Failed
        } else if self.forced == Some(Forced::Check)
            || self.score < cutoff
            || !self.reasons.is_empty()
        {
            Band::Check
        } else {
            Band::Good
        }
    }
}

/// What the engine may do with a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScanTriage {
    /// Every included item is good at the cutoff and no hold code fires: may be saved without a
    /// review (when the user has allowed auto-saving splits).
    Approved,
    /// At least one included item needs a look. Nothing is written until the user accepts.
    HeldForReview {
        #[serde(rename = "itemsNeedCheck")]
        items_need_check: usize,
    },
    /// No included item has a crop: nothing to write.
    NoItems,
}

/// The scan-level triage of `state` at `cutoff` (use [`STRICT_CUTOFF`] for the 0.x rule): the
/// minimum over the included items. An item the user placed or edited counts as reviewed (good);
/// an auto item with no confidence, or a band other than Good, needs a check.
pub fn scan_triage(state: &EditState, cutoff: f32) -> ScanTriage {
    let mut total = 0usize;
    let mut need = 0usize;
    for it in state.included().filter(|i| i.geometry.quad().is_some()) {
        total += 1;
        let reviewed = !matches!(it.origin, Origin::Auto { .. });
        let good = reviewed
            || it
                .confidence
                .as_ref()
                .is_some_and(|c| c.band(cutoff) == Band::Good);
        if !good {
            need += 1;
        }
    }
    match (total, need) {
        (0, _) => ScanTriage::NoItems,
        (_, 0) => ScanTriage::Approved,
        (_, n) => ScanTriage::HeldForReview {
            items_need_check: n,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{Reason, ReasonCode};
    use crate::edit::{Item, ItemId};
    use crate::geometry::QuadWarp;

    fn conf(score: f32, forced: Option<Forced>, reasons: Vec<ReasonCode>) -> Confidence {
        Confidence {
            score,
            forced,
            reasons: reasons
                .into_iter()
                .map(|code| Reason { code, side: None })
                .collect(),
        }
    }

    fn auto(id: u32, c: Option<Confidence>) -> Item {
        Item {
            confidence: c,
            ..Item::quad(
                ItemId(id),
                QuadWarp::inset_frame(0.1),
                Origin::Auto { pipeline_ver: 1 },
            )
        }
    }

    fn state(items: Vec<Item>) -> EditState {
        EditState {
            items,
            ..EditState::default()
        }
    }

    #[test]
    fn bands_follow_the_floor_the_cutoff_and_forced() {
        let b = |c: Confidence| c.band(STRICT_CUTOFF);
        assert_eq!(b(conf(0.97, None, vec![])), Band::Good);
        assert_eq!(b(conf(0.95, None, vec![])), Band::Good);
        assert_eq!(b(conf(0.94, None, vec![])), Band::Check);
        assert_eq!(b(conf(0.60, None, vec![])), Band::Check);
        assert_eq!(b(conf(0.59, None, vec![])), Band::Failed);
        assert_eq!(b(conf(0.99, Some(Forced::Check), vec![])), Band::Check);
        assert_eq!(b(conf(0.99, Some(Forced::Failed), vec![])), Band::Failed);
        assert_eq!(b(conf(0.99, None, vec![ReasonCode::WeakEdge])), Band::Check);
        assert_eq!(b(conf(f32::NAN, None, vec![])), Band::Failed);
        assert_eq!(conf(0.92, None, vec![]).band(0.90), Band::Good);
    }

    /// M10.29's predicate table: Approved only if every included item is Good at the Strict
    /// cutoff and no hold code fires.
    #[test]
    fn the_scan_predicate_table() {
        let good = || Some(conf(0.98, None, vec![]));
        let check = || Some(conf(0.80, None, vec![]));
        let held_by_code = || Some(conf(0.99, None, vec![ReasonCode::PartialFrame]));
        let failed = || Some(conf(0.30, Some(Forced::Failed), vec![ReasonCode::NoQuad]));
        let cases: Vec<(&str, Vec<Item>, ScanTriage)> = vec![
            (
                "all good",
                vec![auto(1, good()), auto(2, good())],
                ScanTriage::Approved,
            ),
            (
                "one check",
                vec![auto(1, good()), auto(2, check())],
                ScanTriage::HeldForReview {
                    items_need_check: 1,
                },
            ),
            (
                "a hold code on a high score",
                vec![auto(1, good()), auto(2, held_by_code())],
                ScanTriage::HeldForReview {
                    items_need_check: 1,
                },
            ),
            (
                "failed and check",
                vec![auto(1, failed()), auto(2, check()), auto(3, good())],
                ScanTriage::HeldForReview {
                    items_need_check: 2,
                },
            ),
            (
                "an auto item with no confidence",
                vec![auto(1, good()), auto(2, None)],
                ScanTriage::HeldForReview {
                    items_need_check: 1,
                },
            ),
            ("nothing", vec![], ScanTriage::NoItems),
        ];
        for (name, items, want) in cases {
            assert_eq!(scan_triage(&state(items), STRICT_CUTOFF), want, "{name}");
        }
    }

    #[test]
    fn excluded_items_do_not_hold_a_scan_and_user_items_count_as_reviewed() {
        let mut bad = auto(2, Some(conf(0.2, Some(Forced::Failed), vec![])));
        bad.include = false;
        let mut edited = auto(3, Some(conf(0.3, None, vec![])));
        edited.origin = Origin::AutoThenEdited;
        let manual = Item::quad(ItemId(4), QuadWarp::inset_frame(0.2), Origin::Manual);
        let s = state(vec![
            auto(1, Some(conf(0.99, None, vec![]))),
            bad,
            edited,
            manual,
        ]);
        assert_eq!(scan_triage(&s, STRICT_CUTOFF), ScanTriage::Approved);
        // An excluded-only scan has nothing to write.
        let mut only = auto(1, Some(conf(0.99, None, vec![])));
        only.include = false;
        assert_eq!(
            scan_triage(&state(vec![only]), STRICT_CUTOFF),
            ScanTriage::NoItems
        );
    }
}
