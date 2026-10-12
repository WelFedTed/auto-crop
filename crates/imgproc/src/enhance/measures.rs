// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! The suggestion (PLAN 5.3.3, ROADMAP M7.15): does this crop look like a receipt or a document
//! that enhancement would visibly improve? Cheap measures from the analysis, thresholds from
//! synthetic data, and a decision that is tuned for precision: it is better to stay silent on a
//! page than to offer enhancement on a photograph.
//!
//! The decision has three parts, each of which must hold:
//!
//! 1. **Page-like.** Paper is the dominant background (flattened luma within 10% of the local
//!    paper level for at least [`MIN_PAPER_FRACTION`] of the page), ink is a minority
//!    ([`MIN_INK_FRACTION`] to [`MAX_INK_FRACTION`]), and the ink is print: at least
//!    [`MIN_TEXT_COMPONENTS`] glyph-sized components, most of them close to one size
//!    ([`MIN_CONCENTRATION`]) and carrying most of the ink ([`MIN_GLYPH_INK_SHARE`]).
//! 2. **A visible effect.** Uneven lighting ([`MIN_SHADING`]), dull paper
//!    ([`DULL_PAPER_LUMA`]), a colour cast ([`MIN_CAST`]) or faded print. A flat, bright, black on
//!    white scan gets no suggestion: the original is already what the modes would produce.
//! 3. **Reliable.** The estimate did not reject most of the page.
//!
//! What is suggested is [`Mode::Clean`] (B13: colour kept, paper whitened): measured on the OCR
//! matrix it never made a slice worse and read best, where the binarising modes did make some
//! slices worse. The black-and-white mode that fits the page (Receipt for a long thin page or
//! faded print, Document otherwise) is named too, as an option for the person to take.
//! `suggest` never applies anything.

use super::{Mode, Report, Tuning, analyse};
use crate::Raster;
use crate::cancel::{Cancel, Cancelled};

/// Paper share of the page (PROVISIONAL, PLAN 5.3.3: 55%).
pub const MIN_PAPER_FRACTION: f32 = 0.55;
/// Ink share of the page, lower and upper bound (PLAN 5.3.3: 1% to 25%; the upper bound is wider
/// because ink here includes the anti-aliased halo of every glyph, and the synthetic pages are
/// denser than print).
pub const MIN_INK_FRACTION: f32 = 0.01;
pub const MAX_INK_FRACTION: f32 = 0.35;
/// Glyph-sized components of the dominant size.
pub const MIN_TEXT_COMPONENTS: u32 = 40;
/// Share of the plausible components near the dominant size.
pub const MIN_CONCENTRATION: f32 = 0.30;
/// Share of the ink that sits in those components.
pub const MIN_GLYPH_INK_SHARE: f32 = 0.35;
/// Illumination variation `(p95 - p5) / P` (PLAN 5.3.3: 8%).
pub const MIN_SHADING: f32 = 0.08;
/// Paper luma below this reads as dull grey.
pub const DULL_PAPER_LUMA: f32 = 228.0;
/// Spread of the paper channels that counts as a colour cast.
pub const MIN_CAST: f32 = 0.06;
/// Long side over short side from which a page is a receipt.
pub const RECEIPT_ASPECT: f32 = 2.2;
/// More rejected cells than this and the estimate is not trusted.
pub const MAX_REJECTED: f32 = 0.5;

/// Why enhancement would help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// The lighting varies across the page.
    UnevenLight,
    /// The paper is grey rather than white.
    DullPaper,
    /// The paper has a colour cast.
    ColourCast,
    /// The print is faint (thermal paper).
    FadedPrint,
}

impl Reason {
    /// A stable code (the UI looks its text up by it).
    pub fn code(self) -> &'static str {
        match self {
            Reason::UnevenLight => "uneven_light",
            Reason::DullPaper => "dull_paper",
            Reason::ColourCast => "colour_cast",
            Reason::FadedPrint => "faded_print",
        }
    }
}

/// The numbers behind a suggestion, all measured on the analysis proxy.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measures {
    /// Long side over short side.
    pub aspect: f32,
    pub paper_fraction: f32,
    pub ink_fraction: f32,
    /// Illumination variation before the flatten.
    pub shading: f32,
    /// Luma of the brightest paper (the 90th percentile of the cells).
    pub paper_luma: f32,
    pub cast: f32,
    pub colour_fraction: f32,
    /// Glyph-sized components of the dominant size, and how regular they are.
    pub text_components: u32,
    pub concentration: f32,
    pub glyph_ink_share: f32,
    /// Text height over the short side.
    pub text_h_rel: f32,
    pub ink_contrast: f32,
    pub faded: bool,
    pub rejected: f32,
}

impl Measures {
    pub fn from_report(r: &Report, width: u32, height: u32) -> Self {
        let (long, short) = (width.max(height) as f32, width.min(height).max(1) as f32);
        Self {
            aspect: long / short,
            paper_fraction: r.paper_fraction,
            ink_fraction: r.ink_fraction,
            shading: r.shading,
            paper_luma: 0.299 * r.paper[0] + 0.587 * r.paper[1] + 0.114 * r.paper[2],
            cast: r.cast,
            colour_fraction: r.colour_fraction,
            text_components: r.components,
            concentration: r.concentration,
            glyph_ink_share: r.glyph_ink_share,
            text_h_rel: r.text_h / short,
            ink_contrast: r.ink_contrast,
            faded: r.faded,
            rejected: r.rejected,
        }
    }

    /// Paper is the background and the ink is print (part 1 of the decision).
    pub fn is_page_like(&self) -> bool {
        self.paper_fraction >= MIN_PAPER_FRACTION
            && (MIN_INK_FRACTION..=MAX_INK_FRACTION).contains(&self.ink_fraction)
            && self.text_components >= MIN_TEXT_COMPONENTS
            && self.concentration >= MIN_CONCENTRATION
            && self.glyph_ink_share >= MIN_GLYPH_INK_SHARE
    }

    /// What a mode would visibly change (part 2).
    pub fn reasons(&self) -> Vec<Reason> {
        let mut out = Vec::new();
        if self.shading >= MIN_SHADING {
            out.push(Reason::UnevenLight);
        }
        if self.paper_luma < DULL_PAPER_LUMA {
            out.push(Reason::DullPaper);
        }
        if self.cast >= MIN_CAST {
            out.push(Reason::ColourCast);
        }
        if self.faded {
            out.push(Reason::FadedPrint);
        }
        out
    }
}

/// A recommendation. Nothing is applied by it.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// What is suggested: always [`Mode::Clean`] (B13: colour kept, paper whitened). On every
    /// slice of the OCR matrix (`docs/perf/enhance.md`, ROADMAP M7.50) Clean is the one mode that
    /// never made a result worse and read best; the binarising modes are a look the person
    /// chooses, not a recommendation.
    pub mode: Mode,
    /// The black-and-white mode that fits this page if the person wants it: Receipt for a long
    /// thin page or faded print, Document otherwise.
    pub black_and_white: Mode,
    pub reasons: Vec<Reason>,
}

/// The decision of the module docs; `None` when the page should be left alone.
pub fn suggest(m: &Measures) -> Option<Suggestion> {
    if !m.is_page_like() || m.rejected > MAX_REJECTED {
        return None;
    }
    let reasons = m.reasons();
    if reasons.is_empty() {
        return None;
    }
    let black_and_white = if m.faded || m.aspect >= RECEIPT_ASPECT {
        Mode::Receipt
    } else {
        Mode::Document
    };
    Some(Suggestion {
        mode: Mode::Clean,
        black_and_white,
        reasons,
    })
}

/// Measures a crop (the page as it would be saved, or a preview of it). Runs the analysis of the
/// Document look, so the cost is that of one analysis on a 1.5 MP proxy.
pub fn measure(img: &Raster, cancel: &dyn Cancel) -> Result<Measures, Cancelled> {
    let an = analyse(img, Mode::Document, &Tuning::default(), None, cancel)?;
    Ok(Measures::from_report(an.report(), img.width, img.height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> Measures {
        Measures {
            aspect: 1.4,
            paper_fraction: 0.8,
            ink_fraction: 0.08,
            shading: 0.2,
            paper_luma: 200.0,
            cast: 0.02,
            colour_fraction: 0.0,
            text_components: 400,
            concentration: 0.6,
            glyph_ink_share: 0.8,
            text_h_rel: 0.02,
            ink_contrast: 180.0,
            faded: false,
            rejected: 0.2,
        }
    }

    #[test]
    fn the_suggestion_is_clean_and_names_the_black_and_white_look_that_fits() {
        let s = suggest(&page()).expect("suggested");
        assert_eq!(s.mode, Mode::Clean, "the measured winner of the OCR matrix");
        assert_eq!(s.black_and_white, Mode::Document);
        assert!(s.reasons.contains(&Reason::UnevenLight) && s.reasons.contains(&Reason::DullPaper));
        // A long thin page, or faint print, fits Receipt.
        for m in [
            Measures {
                aspect: 4.0,
                ..page()
            },
            Measures {
                faded: true,
                ..page()
            },
        ] {
            let r = suggest(&m).unwrap();
            assert_eq!((r.mode, r.black_and_white), (Mode::Clean, Mode::Receipt));
        }
    }

    #[test]
    fn silence_is_the_default_every_condition_can_veto() {
        assert!(suggest(&page()).is_some());
        let vetoes = [
            Measures {
                paper_fraction: 0.3,
                ..page()
            },
            Measures {
                ink_fraction: 0.4,
                ..page()
            },
            Measures {
                ink_fraction: 0.0,
                ..page()
            },
            Measures {
                text_components: 5,
                ..page()
            },
            Measures {
                concentration: 0.05,
                ..page()
            },
            Measures {
                glyph_ink_share: 0.1,
                ..page()
            },
            Measures {
                rejected: 0.9,
                ..page()
            },
            // Nothing to improve: flat, bright, neutral, strong print.
            Measures {
                shading: 0.02,
                paper_luma: 250.0,
                cast: 0.0,
                ..page()
            },
        ];
        for (i, v) in vetoes.iter().enumerate() {
            assert!(suggest(v).is_none(), "veto {i}");
        }
        // Faded print alone is a reason.
        let faded = Measures {
            shading: 0.0,
            paper_luma: 250.0,
            faded: true,
            ..page()
        };
        assert_eq!(suggest(&faded).unwrap().reasons, [Reason::FadedPrint]);
    }
}
