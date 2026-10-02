// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Interleaved sample buffers for the generic kernels: 8-bit and 16-bit, 1 to 4 channels.
//! [`crate::Raster`] stays the 8-bit RGB type of the early engine slice.

/// One channel sample (`u8` or `u16`).
pub trait Sample: Copy + Default + Send + Sync + 'static {
    /// Largest representable value as f32.
    const MAX_F32: f32;
    /// Whether warps must interpolate the weight table (16-bit samples) rather than take the
    /// nearest entry (8-bit).
    const PRECISE: bool;
    fn to_f32(self) -> f32;
    /// Rounds to nearest and clamps into range (NaN maps to 0).
    fn from_f32_round(v: f32) -> Self;
}

impl Sample for u8 {
    const MAX_F32: f32 = 255.0;
    const PRECISE: bool = false;
    #[inline(always)]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    #[inline(always)]
    fn from_f32_round(v: f32) -> Self {
        (v + 0.5).clamp(0.0, 255.0) as u8
    }
}

impl Sample for u16 {
    const MAX_F32: f32 = 65535.0;
    const PRECISE: bool = true;
    #[inline(always)]
    fn to_f32(self) -> f32 {
        f32::from(self)
    }
    #[inline(always)]
    fn from_f32_round(v: f32) -> Self {
        (v + 0.5).clamp(0.0, 65535.0) as u16
    }
}

/// A borrowed interleaved image: `data.len() == width * height * channels`.
#[derive(Debug, Clone, Copy)]
pub struct ImageRef<'a, T: Sample> {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: &'a [T],
}

impl<'a, T: Sample> ImageRef<'a, T> {
    /// `None` if the channel count is not 1 to 4 or the length does not match.
    pub fn new(width: u32, height: u32, channels: u8, data: &'a [T]) -> Option<Self> {
        let ok = (1..=4).contains(&channels)
            && data.len() == width as usize * height as usize * usize::from(channels);
        ok.then_some(Self {
            width,
            height,
            channels,
            data,
        })
    }
}

/// An owned interleaved image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image<T: Sample> {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub data: Vec<T>,
}

impl<T: Sample> Image<T> {
    /// A zeroed image. Panics if `channels` is not 1 to 4 or the size overflows `usize`.
    pub fn new(width: u32, height: u32, channels: u8) -> Self {
        assert!((1..=4).contains(&channels), "channels must be 1..=4");
        Self {
            width,
            height,
            channels,
            data: vec![T::default(); width as usize * height as usize * usize::from(channels)],
        }
    }

    /// Wraps existing samples; `None` if the channel count or length is wrong.
    pub fn from_raw(width: u32, height: u32, channels: u8, data: Vec<T>) -> Option<Self> {
        let ok = (1..=4).contains(&channels)
            && data.len() == width as usize * height as usize * usize::from(channels);
        ok.then_some(Self {
            width,
            height,
            channels,
            data,
        })
    }

    pub fn as_ref(&self) -> ImageRef<'_, T> {
        ImageRef {
            width: self.width,
            height: self.height,
            channels: self.channels,
            data: &self.data,
        }
    }

    /// The samples of pixel `(x, y)`.
    pub fn pixel(&self, x: u32, y: u32) -> &[T] {
        let c = usize::from(self.channels);
        let i = (y as usize * self.width as usize + x as usize) * c;
        &self.data[i..i + c]
    }

    pub fn pixel_mut(&mut self, x: u32, y: u32) -> &mut [T] {
        let c = usize::from(self.channels);
        let i = (y as usize * self.width as usize + x as usize) * c;
        &mut self.data[i..i + c]
    }
}

/// An 8-bit grey image (one channel).
pub type Gray8 = Image<u8>;
