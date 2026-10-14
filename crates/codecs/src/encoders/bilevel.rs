// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! A packed 1-bit image and the CCITT Group 4 (ITU-T T.6) codec the 1-bit TIFF and PDF writers
//! share (ROADMAP M7.28, M7.29, M11.30, M11.35; PLAN 3.3 and 5.9).
//!
//! * **Thresholding** ([`BilevelImage::from_view`]): the Rec. 601 luma of the rendered picture
//!   against a cut (128 by default), no dithering: a pixel strictly below the cut is black.
//! * **Polarity**: in memory a set bit is black. The Group 4 row coder works on colour runs, so the
//!   bytes it produces mean "white run, black run, ..." whatever the container says; the TIFF
//!   writer labels them `WhiteIsZero` (the convention of `fax::tiff::wrap` and of the repo's own
//!   decode fixture) and the PDF writer passes `/BlackIs1 false`, which says the same thing.
//! * **Strips** ([`BilevelImage::g4`]): each strip is a self-contained Group 4 stream (its
//!   reference line starts white), so any number of strips decode independently.
//!
//! `fax` writes the encoder side only for widths up to 65,535 pixels (its run coder is `u16`); a
//! wider page is refused (`UnsupportedOutput`), never truncated.

use super::check;
use auto_crop_core::ports::{RasterView, SampleSlice};
use auto_crop_core::{CancelToken, ErrKind};
use fax::{Color, VecWriter, encoder::Encoder};

/// The widest image the Group 4 coder takes.
pub const G4_MAX_WIDTH: u32 = u16::MAX as u32;
/// The default threshold: luma below it is black (PLAN 3.3).
pub const DEFAULT_CUT: u8 = 128;

/// A 1-bit image, rows padded to whole bytes, most significant bit first, **1 = black**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BilevelImage {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bits: Vec<u8>,
}

impl BilevelImage {
    /// An all-white image.
    pub fn white(width: u32, height: u32) -> Self {
        let stride = (width as usize).div_ceil(8);
        Self {
            width,
            height,
            stride,
            bits: vec![0; stride * height as usize],
        }
    }

    /// Thresholds `src` (grey or RGB, 8 or 16 bit) at `cut`: luma below `cut` is black.
    pub fn from_view(src: &RasterView<'_>, cut: u8, cancel: &CancelToken) -> Result<Self, ErrKind> {
        let mut out = Self::white(src.width(), src.height());
        let ch = src.format().channels();
        for y in 0..src.height() {
            if y % 64 == 0 {
                check(cancel)?;
            }
            let row = &mut out.bits[y as usize * out.stride..(y as usize + 1) * out.stride];
            let luma = |px: &[u16]| -> u32 {
                if ch == 1 {
                    u32::from(px[0])
                } else {
                    (299 * u32::from(px[0]) + 587 * u32::from(px[1]) + 114 * u32::from(px[2]))
                        / 1000
                }
            };
            match src.samples() {
                SampleSlice::U8(_) => {
                    let r = src.row_u8(y).ok_or(ErrKind::Internal)?;
                    for (x, px) in r.chunks_exact(ch).enumerate() {
                        let l = if ch == 1 {
                            u32::from(px[0])
                        } else {
                            (299 * u32::from(px[0])
                                + 587 * u32::from(px[1])
                                + 114 * u32::from(px[2]))
                                / 1000
                        };
                        if l < u32::from(cut) {
                            row[x / 8] |= 0x80 >> (x % 8);
                        }
                    }
                }
                SampleSlice::U16(_) => {
                    let r = src.row_u16(y).ok_or(ErrKind::Internal)?;
                    for (x, px) in r.chunks_exact(ch).enumerate() {
                        let hi: Vec<u16> = px.iter().map(|v| v >> 8).collect();
                        if luma(&hi) < u32::from(cut) {
                            row[x / 8] |= 0x80 >> (x % 8);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    pub fn is_black(&self, x: u32, y: u32) -> bool {
        self.bits[y as usize * self.stride + x as usize / 8] & (0x80 >> (x % 8)) != 0
    }

    /// Group 4 bytes for the rows `y0..y1` (a whole strip: the reference line is white).
    pub fn g4(&self, y0: u32, y1: u32, cancel: &CancelToken) -> Result<Vec<u8>, ErrKind> {
        if self.width == 0 || self.width > G4_MAX_WIDTH || y0 >= y1 || y1 > self.height {
            return Err(ErrKind::UnsupportedOutput);
        }
        let mut enc = Encoder::new(VecWriter::new());
        for y in y0..y1 {
            if y % 64 == 0 {
                check(cancel)?;
            }
            let row = &self.bits[y as usize * self.stride..(y as usize + 1) * self.stride];
            let pels = (0..self.width as usize).map(|x| {
                if row[x / 8] & (0x80 >> (x % 8)) != 0 {
                    Color::Black
                } else {
                    Color::White
                }
            });
            enc.encode_line(pels, self.width as u16)
                .map_err(|_| ErrKind::EncodeFailed)?;
        }
        let w = enc.finish().map_err(|_| ErrKind::EncodeFailed)?;
        Ok(w.finish())
    }

    /// Decodes a Group 4 stream of `width` x `height` pixels (a test oracle and the PDF read-back).
    /// `None` when the stream is damaged or short.
    pub fn from_g4(data: &[u8], width: u32, height: u32) -> Option<Self> {
        if width == 0 || width > G4_MAX_WIDTH || height == 0 || height > G4_MAX_WIDTH {
            return None;
        }
        let mut out = Self::white(width, height);
        let mut y = 0usize;
        fax::decoder::decode_g4(
            data.iter().copied(),
            width as u16,
            Some(height as u16),
            |line| {
                if y < height as usize {
                    let row = &mut out.bits[y * out.stride..(y + 1) * out.stride];
                    for (x, c) in fax::decoder::pels(line, width as u16).enumerate() {
                        if c == Color::Black {
                            row[x / 8] |= 0x80 >> (x % 8);
                        }
                    }
                }
                y += 1;
            },
        )?;
        (y >= height as usize).then_some(out)
    }
}
