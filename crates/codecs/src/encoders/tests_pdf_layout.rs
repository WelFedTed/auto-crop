// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! PDF page size presets (ROADMAP M11.36, M11.54): a fixed sheet (A4, Letter), margins, the sheet
//! turning with the picture, the picture centred and never enlarged unless asked; and the structure
//! check that now looks at where the picture is drawn.

use super::pdf::*;

fn streaming(bytes: &[u8]) -> Option<Vec<PdfPageRead>> {
    let mut pages = Vec::new();
    read_pages_streaming(&mut std::io::Cursor::new(bytes), &mut |_, p| {
        pages.push(p);
        true
    })
    .map(|_| pages)
}

/// A one-page document of a grey picture of `w` x `h` pixels at `dpi`, on `layout`.
fn laid_out(w: u32, h: u32, dpi: Option<(u32, u32)>, layout: PageLayout) -> (Vec<u8>, PageInfo) {
    let samples = vec![200u8; (w * h) as usize];
    let mut doc = PdfWriter::new(Vec::new()).unwrap();
    let info = doc
        .add_page_with(
            PageImage::Flate {
                width: w,
                height: h,
                channels: 1,
                samples: &samples,
                icc: None,
            },
            dpi,
            &layout,
        )
        .unwrap();
    (doc.finish().unwrap(), info)
}

fn sheet(size: PageSize, margin_mm: f64, upscale: bool) -> PageLayout {
    PageLayout {
        size,
        margin_pt: margin_mm * PT_PER_MM,
        upscale,
    }
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() <= 0.011
}

#[test]
fn a_fixed_sheet_keeps_its_size_turns_with_the_picture_and_centres_it() {
    let d300 = Some((300, 300));
    // Portrait picture (600 x 800 px = 144 x 192 pt) on A4: the picture keeps its own size, centred.
    let (pdf, i) = laid_out(600, 800, d300, sheet(PageSize::A4, 0.0, false));
    assert_eq!((i.width_pt, i.height_pt), A4_PT);
    let (x, y, w, h) = i.image_pt;
    assert!(near(w, 144.0) && near(h, 192.0), "{i:?}");
    assert!(
        near(x, (A4_PT.0 - 144.0) / 2.0) && (y - (A4_PT.1 - 192.0) / 2.0).abs() <= 0.011,
        "{i:?}"
    );
    assert!(!i.scaled && !i.dpi_assumed);
    // The reader sees the same media box and picture box.
    let p = &read_pages(&pdf).unwrap()[0];
    assert_eq!(p.media_box, A4_PT);
    assert!(near(p.image_box.0, x) && near(p.image_box.2, w) && near(p.image_box.3, h));

    // A landscape picture bigger than the sheet (4000 x 3000 px = 960 x 720 pt): the sheet turns on
    // its side and the picture shrinks to fit, keeping its aspect ratio.
    let (_, i) = laid_out(4000, 3000, d300, sheet(PageSize::A4, 0.0, false));
    assert_eq!((i.width_pt, i.height_pt), (A4_PT.1, A4_PT.0));
    let (x, y, w, h) = i.image_pt;
    assert!(
        near(h, A4_PT.0) && near(y, 0.0),
        "touches top and bottom: {i:?}"
    );
    assert!((w / h - 4.0 / 3.0).abs() < 1e-3 && x > 0.0, "{i:?}");
    assert!((x - (A4_PT.1 - w) / 2.0).abs() <= 0.011, "{i:?}");
    assert!(!i.scaled, "fitting a sheet is not the page-limit scaling");

    // Letter; a square picture takes the sheet portrait.
    let (_, i) = laid_out(3000, 3000, d300, sheet(PageSize::LETTER, 0.0, false));
    assert_eq!((i.width_pt, i.height_pt), LETTER_PT);
    assert!(
        near(i.image_pt.2, LETTER_PT.0) && near(i.image_pt.3, LETTER_PT.0),
        "{i:?}"
    );

    // The orientation follows the picture as it is drawn: a 2:1 density makes a square picture
    // tall or wide.
    let (_, i) = laid_out(300, 300, Some((300, 150)), sheet(PageSize::A4, 0.0, false));
    assert_eq!((i.width_pt, i.height_pt), A4_PT, "72 x 144 pt is tall");
    let (_, i) = laid_out(300, 300, Some((150, 300)), sheet(PageSize::A4, 0.0, false));
    assert_eq!(
        (i.width_pt, i.height_pt),
        (A4_PT.1, A4_PT.0),
        "144 x 72 pt is wide"
    );

    // An unknown density is still the nominal 300 and says so.
    let (_, i) = laid_out(600, 800, None, sheet(PageSize::A4, 0.0, false));
    assert!(i.dpi_assumed && near(i.image_pt.2, 144.0), "{i:?}");
}

#[test]
fn a_margin_shrinks_the_picture_box_and_a_small_picture_is_not_enlarged() {
    let d300 = Some((300, 300));
    // 10 mm all round on A4: a full-sheet picture ends up inside 28.35 pt of each edge, centred.
    let m = 10.0 * PT_PER_MM;
    let (_, i) = laid_out(2480, 3508, d300, sheet(PageSize::A4, 10.0, false));
    let (x, y, w, h) = i.image_pt;
    assert!(x >= m - 0.011 && y >= m - 0.011, "{i:?}");
    assert!(
        x + w <= A4_PT.0 - m + 0.011 && y + h <= A4_PT.1 - m + 0.011,
        "{i:?}"
    );
    assert!((w / h - 2480.0 / 3508.0).abs() < 1e-3, "{i:?}");
    assert!(
        (x - (A4_PT.0 - x - w)).abs() <= 0.02 && (y - (A4_PT.1 - y - h)).abs() <= 0.02,
        "{i:?}"
    );
    // A picture that already fits inside the margins keeps its natural size.
    let (_, i) = laid_out(300, 300, d300, sheet(PageSize::A4, 10.0, false));
    assert!(
        near(i.image_pt.2, 72.0) && near(i.image_pt.3, 72.0),
        "{i:?}"
    );
    // Unless the caller asks: then it fills the box (limited by the width here).
    let (_, i) = laid_out(300, 300, d300, sheet(PageSize::A4, 10.0, true));
    let side = A4_PT.0 - 2.0 * m;
    assert!(
        near(i.image_pt.2, side) && near(i.image_pt.3, side),
        "{i:?}"
    );
    // The margin and the upscaling mean nothing for fit-to-image.
    let (_, i) = laid_out(600, 300, d300, sheet(PageSize::MatchImage, 10.0, true));
    assert_eq!(i.image_pt, (0.0, 0.0, 144.0, 72.0));
    assert_eq!((i.width_pt, i.height_pt), (144.0, 72.0));
    // A margin that leaves no room (or is not a number) cannot break the page.
    for bad in [1.0e9, f64::NAN, f64::INFINITY, -5.0] {
        let l = PageLayout {
            size: PageSize::A4,
            margin_pt: bad,
            upscale: false,
        };
        let (pdf, i) = laid_out(300, 400, d300, l);
        assert!(i.image_pt.2 > 0.0 && i.image_pt.3 > 0.0, "{bad}: {i:?}");
        assert!(read_pages(&pdf).is_some(), "{bad}");
    }
}

#[test]
fn a_document_of_fixed_sheets_is_deterministic_and_the_picture_box_is_checked() {
    let build = || {
        let mut w = PdfWriter::new(Vec::new()).unwrap();
        w.set_title("Receipts");
        let layout = sheet(PageSize::LETTER, 12.7, false);
        for (pw, ph) in [(900u32, 1200u32), (1600, 900), (500, 500)] {
            let s = vec![90u8; (pw * ph) as usize];
            w.add_page_with(
                PageImage::Flate {
                    width: pw,
                    height: ph,
                    channels: 1,
                    samples: &s,
                    icc: None,
                },
                Some((200, 200)),
                &layout,
            )
            .unwrap();
        }
        w.finish().unwrap()
    };
    let pdf = build();
    assert_eq!(pdf, build(), "equal input gives equal bytes");
    let pages = read_pages(&pdf).unwrap();
    assert_eq!(pages.len(), 3);
    assert_eq!(pages[0].media_box, LETTER_PT);
    assert_eq!(
        pages[1].media_box,
        (LETTER_PT.1, LETTER_PT.0),
        "a wide picture turns the sheet"
    );
    assert_eq!(pages[2].media_box, LETTER_PT);
    let s = streaming(&pdf).unwrap();
    for (a, b) in pages.iter().zip(&s) {
        assert_eq!((a.media_box, a.image_box), (b.media_box, b.image_box));
    }
    // A picture drawn off the sheet is refused by both readers (a same-length edit of x).
    let at = pdf
        .windows(8)
        .position(|w| w == b" cm /Im0")
        .expect("a content stream");
    let start = pdf[..at].iter().rposition(|&c| c == b'q').unwrap();
    let line = String::from_utf8_lossy(&pdf[start..at]).into_owned();
    let mut parts: Vec<String> = line.split(' ').map(str::to_owned).collect();
    let n = parts.len();
    parts[n - 2] = format!("9{}", &parts[n - 2][1..]);
    let mut bad = pdf.clone();
    bad[start..at].copy_from_slice(parts.join(" ").as_bytes());
    assert_ne!(bad, pdf);
    assert!(read_pages(&bad).is_none() && streaming(&bad).is_none());
}

#[test]
fn a5_is_148_by_210_mm_and_a_300_dpi_a4_scan_fits_its_own_page_within_half_a_point() {
    // A5 (M11.36): 148 x 210 mm to 0.01 pt, turned for a wide picture.
    assert!(near(A5_PT.0, 148.0 * PT_PER_MM) && near(A5_PT.1, 210.0 * PT_PER_MM));
    let (_, tall) = laid_out(600, 800, Some((300, 300)), sheet(PageSize::A5, 0.0, false));
    assert_eq!((tall.width_pt, tall.height_pt), A5_PT);
    let (pdf, wide) = laid_out(800, 600, Some((300, 300)), sheet(PageSize::A5, 0.0, false));
    assert_eq!((wide.width_pt, wide.height_pt), (A5_PT.1, A5_PT.0));
    let pages = streaming(&pdf).expect("the structure check reads it");
    assert_eq!(pages[0].media_box, (A5_PT.1, A5_PT.0));
    // A scan of an A4 sheet at 300 dpi (2480 x 3508 px) matched to the picture is A4 within 0.5 pt.
    let (_, fit) = laid_out(2480, 3508, Some((300, 300)), PageLayout::default());
    assert!(
        (fit.width_pt - A4_PT.0).abs() <= 0.5 && (fit.height_pt - A4_PT.1).abs() <= 0.5,
        "{fit:?}"
    );
}
