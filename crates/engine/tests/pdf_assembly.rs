// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Assembling a 200-page PDF from already-encoded pages (ROADMAP M11.37, M11.38, M11.84; the 12 MP
//! figure of M11.65 is a separate test):
//!
//! * the writer streams: into a counting sink the peak heap of 200 pages is not larger than that of
//!   20 pages by more than a little slack, and the negative control (what holding the document in
//!   memory would peak at) is far above that bar;
//! * the structure check `verify_structure` (header, trailer, every cross-reference offset, page
//!   count, the page tree, stream framing and filters) takes at most 50 ms for the 200 pages (the
//!   bar of M11.38 and M11.84), measured on the file as the commit protocol reads it;
//! * the parse-back the engine runs (`read_pages_streaming`, which also copies every picture's
//!   stream, so its time grows with the bytes) is timed and printed but has no bar here: with scan-size
//!   pages it does not meet 50 ms (docs/dev/output-formats.md has the numbers);
//! * what is read is right: 200 pages in order, with the sizes and boxes that were written.
//!
//! One test in this binary: the allocator's peak is global, so a second test running beside it
//! would pollute it. Timing is the median of several runs after a warm-up (the file is in the cache
//! in the commit protocol too: it was written a moment before).

use auto_crop_codecs::encoders::pdf::{
    A4_PT, PageImage, PageLayout, PageSize, PdfWriter, read_pages_streaming, verify_structure,
};
use auto_crop_codecs::fixtures::photo;
use auto_crop_codecs::{Format, encode};
use auto_crop_imgproc::Raster;
use peak_alloc::PeakAlloc;
use std::io::Write;
use std::time::{Duration, Instant};

// A counting allocator from a maintained crate (it carries the `unsafe`; this crate forbids it).
#[global_allocator]
static ALLOC: PeakAlloc = PeakAlloc;

const PAGES: usize = 200;
/// The bar of M11.38 and M11.84: 200 pages verify in at most this long.
const VERIFY_BAR: Duration = Duration::from_millis(50);
const MARGIN_PT: f64 = 36.0;

/// A sink that only counts.
struct Count(u64);

impl Write for Count {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0 += b.len() as u64;
        Ok(b.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A scan-sized JPEG (A4 at about 150 dpi) as the encoder makes it; the width depends on the seed so
/// that the pages can be told apart.
fn page_jpeg(seed: u64) -> (Vec<u8>, u32) {
    let (w, h) = (1240 + 8 * seed as u32, 1754u32);
    let jpeg = encode(
        &Raster::from_raw(w, h, photo(w, h, seed)).unwrap(),
        Format::Jpeg,
        85,
        None,
    )
    .unwrap();
    (jpeg, w)
}

/// Writes `pages` pages (cycling through `jpegs`) on A4 sheets with a margin to `sink`.
fn assemble<W: Write>(sink: W, jpegs: &[Vec<u8>], pages: usize) -> W {
    let mut doc = PdfWriter::new(sink).unwrap();
    doc.set_title("200 pages");
    let layout = PageLayout {
        size: PageSize::A4,
        margin_pt: MARGIN_PT,
        upscale: false,
    };
    for i in 0..pages {
        doc.add_page_with(
            PageImage::Jpeg {
                bytes: &jpegs[i % jpegs.len()],
                icc: None,
            },
            Some((150, 150)),
            &layout,
        )
        .unwrap();
    }
    doc.finish().unwrap()
}

fn median_of(runs: usize, mut f: impl FnMut() -> Duration) -> (Duration, Duration, Duration) {
    let mut v: Vec<Duration> = (0..runs).map(|_| f()).collect();
    v.sort();
    (v[v.len() / 2], v[0], v[v.len() - 1])
}

#[test]
fn two_hundred_encoded_pages_assemble_flat_and_verify_within_the_bar() {
    let made: Vec<(Vec<u8>, u32)> = (1..=4).map(page_jpeg).collect();
    let jpegs: Vec<Vec<u8>> = made.iter().map(|(j, _)| j.clone()).collect();
    let widths: Vec<u32> = made.iter().map(|(_, w)| *w).collect();

    // 1. Flat memory into a counting sink.
    let peak_of = |pages: usize| {
        let baseline = ALLOC.current_usage();
        ALLOC.reset_peak_usage();
        let t = Instant::now();
        let n = assemble(Count(0), &jpegs, pages).0;
        let took = t.elapsed();
        (ALLOC.peak_usage().saturating_sub(baseline), n, took)
    };
    let slack = 2 * 1024 * 1024;
    let (small, small_bytes, _) = peak_of(PAGES / 10);
    let (large, large_bytes, took) = peak_of(PAGES);
    eprintln!(
        "assemble: 20 pages peak {} KiB ({} KiB written); 200 pages peak {} KiB ({} KiB written) in {took:?}",
        small / 1024,
        small_bytes / 1024,
        large / 1024,
        large_bytes / 1024,
    );
    assert!(large_bytes > small_bytes * 8, "the output grew ten-fold");
    assert!(
        large <= small + slack,
        "the writer's peak grew with the page count: {small} -> {large}"
    );
    assert!(
        large_bytes as usize > small + 4 * slack,
        "the negative control (holding the document) must be far above the bar"
    );

    // 2. The document on disk, as the commit protocol has it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("two-hundred.pdf");
    {
        let f = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        assemble(f, &jpegs, PAGES)
            .into_inner()
            .unwrap()
            .sync_all()
            .unwrap();
    }
    let size = std::fs::metadata(&path).unwrap().len();

    // What is read is right: 200 pages in order, the sheet, the box inside the margins.
    let mut seen = 0usize;
    let count = read_pages_streaming(&mut std::fs::File::open(&path).unwrap(), &mut |i, p| {
        assert_eq!(i, seen);
        seen += 1;
        assert_eq!(p.width_px, widths[i % widths.len()], "page {i} is page {i}");
        assert_eq!(p.media_box, A4_PT);
        let (x, y, w, h) = p.image_box;
        let m = MARGIN_PT - 0.011;
        assert!(x >= m && y >= m && x + w <= A4_PT.0 - m && y + h <= A4_PT.1 - m);
        true
    });
    assert_eq!(count, Some(PAGES));
    assert_eq!(
        verify_structure(&mut std::fs::File::open(&path).unwrap()),
        Some(PAGES)
    );

    // 3. Timing: the structure check has the bar; the copying parse-back is measured.
    let structure = || {
        let mut f = std::fs::File::open(&path).unwrap();
        let t = Instant::now();
        let n = verify_structure(&mut f);
        let took = t.elapsed();
        assert_eq!(n, Some(PAGES));
        took
    };
    let parse_back = || {
        let mut f = std::fs::File::open(&path).unwrap();
        let t = Instant::now();
        let n = read_pages_streaming(&mut f, &mut |_, _| true);
        let took = t.elapsed();
        assert_eq!(n, Some(PAGES));
        took
    };
    structure();
    parse_back();
    let (s_med, s_min, s_max) = median_of(9, structure);
    let (p_med, p_min, p_max) = median_of(9, parse_back);
    eprintln!(
        "{PAGES} pages, {} KiB file ({} KiB per page): structure check median {s_med:?} (min {s_min:?}, max {s_max:?}, bar {VERIFY_BAR:?}); parse-back with stream copies median {p_med:?} (min {p_min:?}, max {p_max:?})",
        size / 1024,
        size as usize / PAGES / 1024,
    );
    assert!(
        s_med <= VERIFY_BAR,
        "the structure check of 200 pages took {s_med:?}, the bar is {VERIFY_BAR:?}"
    );
}
