// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Colour accuracy of the conversion to sRGB, against an independent reference.
//!
//! The reference does not use moxcms: the ICC profiles are built here, byte by byte (a v2 matrix/TRC
//! profile with D50-adapted colorants that this file derives from the published D65 primaries with a
//! Bradford adaptation), and the expected sRGB values are computed in f64 straight from the
//! standard primaries (Display P3: Apple's, D65; Adobe RGB (1998): gamma 563/256; sRGB: IEC 61966-2-1)
//! without any profile at all. A patch the transform gets wrong by more than one 8-bit level fails.

use super::*;

type M3 = [[f64; 3]; 3];

const SRGB_XYZ_D65: M3 = [
    [0.4124564, 0.3575761, 0.1804375],
    [0.2126729, 0.7151522, 0.0721750],
    [0.0193339, 0.1191920, 0.9503041],
];
const P3_XYZ_D65: M3 = [
    [0.4865709, 0.2656677, 0.1982173],
    [0.2289746, 0.6917385, 0.0792869],
    [0.0000000, 0.0451134, 1.0439444],
];
const ADOBE_XYZ_D65: M3 = [
    [0.5767309, 0.1855540, 0.1881852],
    [0.2973769, 0.6273491, 0.0752741],
    [0.0270343, 0.0706872, 0.9911085],
];

fn mul(a: &M3, b: &M3) -> M3 {
    let mut r = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    r
}

fn mul_v(a: &M3, v: [f64; 3]) -> [f64; 3] {
    let mut r = [0.0; 3];
    for i in 0..3 {
        r[i] = (0..3).map(|k| a[i][k] * v[k]).sum();
    }
    r
}

fn inv(m: &M3) -> M3 {
    let d = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let c =
        |r1: usize, c1: usize, r2: usize, c2: usize| m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1];
    [
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ]
}

/// The Bradford adaptation from D65 to D50.
fn bradford_d65_to_d50() -> M3 {
    let m: M3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let d65 = [0.95047, 1.0, 1.08883];
    let d50 = [0.96422, 1.0, 0.82521];
    let (s, d) = (mul_v(&m, d65), mul_v(&m, d50));
    let scale: M3 = [
        [d[0] / s[0], 0.0, 0.0],
        [0.0, d[1] / s[1], 0.0],
        [0.0, 0.0, d[2] / s[2]],
    ];
    mul(&inv(&m), &mul(&scale, &m))
}

#[derive(Clone, Copy)]
enum Trc {
    /// The sRGB piecewise curve.
    Srgb,
    /// A pure power law.
    Gamma(f64),
}

impl Trc {
    fn decode(self, v: f64) -> f64 {
        match self {
            Trc::Srgb => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Trc::Gamma(g) => v.powf(g),
        }
    }

    fn encode(self, l: f64) -> f64 {
        let l = l.clamp(0.0, 1.0);
        match self {
            Trc::Srgb => {
                if l <= 0.0031308 {
                    l * 12.92
                } else {
                    1.055 * l.powf(1.0 / 2.4) - 0.055
                }
            }
            Trc::Gamma(g) => l.powf(1.0 / g),
        }
    }
}

fn s15(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(x: [f64; 3]) -> Vec<u8> {
    let mut t = b"XYZ \0\0\0\0".to_vec();
    for v in x {
        t.extend_from_slice(&s15(v));
    }
    t
}

fn trc_tag(t: Trc) -> Vec<u8> {
    match t {
        Trc::Gamma(g) => {
            let mut v = b"curv\0\0\0\0".to_vec();
            v.extend_from_slice(&1u32.to_be_bytes());
            v.extend_from_slice(&((g * 256.0).round() as u16).to_be_bytes());
            v.extend_from_slice(&[0, 0]);
            v
        }
        Trc::Srgb => {
            // 'para' function type 3: Y = (aX + b)^g for X >= d, cX otherwise.
            let mut v = b"para\0\0\0\0".to_vec();
            v.extend_from_slice(&3u16.to_be_bytes());
            v.extend_from_slice(&[0, 0]);
            for p in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045] {
                v.extend_from_slice(&s15(p));
            }
            v
        }
    }
}

fn text_tag(sig: &[u8; 4], s: &str) -> Vec<u8> {
    let mut v = sig.to_vec();
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

fn desc_tag(s: &str) -> Vec<u8> {
    // textDescriptionType (ICC v2): ASCII count, ASCII, then empty Unicode and ScriptCode parts.
    let mut v = b"desc\0\0\0\0".to_vec();
    v.extend_from_slice(&(s.len() as u32 + 1).to_be_bytes());
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v.extend_from_slice(&[0; 4 + 4 + 2 + 1 + 67]);
    v
}

/// A v2.1 RGB display profile with matrix/TRC and the colorants of `to_xyz_d65` adapted to D50.
fn matrix_profile(name: &str, to_xyz_d65: &M3, trc: Trc) -> Vec<u8> {
    let a = bradford_d65_to_d50();
    let d50 = mul(&a, to_xyz_d65);
    let col = |c: usize| [d50[0][c], d50[1][c], d50[2][c]];
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", desc_tag(name)),
        (b"cprt", text_tag(b"text", "public domain test profile")),
        (b"wtpt", xyz_tag([0.9642, 1.0, 0.8249])),
        (b"rXYZ", xyz_tag(col(0))),
        (b"gXYZ", xyz_tag(col(1))),
        (b"bXYZ", xyz_tag(col(2))),
        (b"rTRC", trc_tag(trc)),
        (b"gTRC", trc_tag(trc)),
        (b"bTRC", trc_tag(trc)),
    ];
    let table_len = 4 + tags.len() * 12;
    let mut data = Vec::new();
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    for (sig, body) in &tags {
        let off = 128 + table_len + data.len();
        table.extend_from_slice(*sig);
        table.extend_from_slice(&(off as u32).to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(body);
        while data.len() % 4 != 0 {
            data.push(0);
        }
    }
    let total = 128 + table.len() + data.len();
    let mut h = vec![0u8; 128];
    h[0..4].copy_from_slice(&(total as u32).to_be_bytes());
    h[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    h[12..16].copy_from_slice(b"mntr");
    h[16..20].copy_from_slice(b"RGB ");
    h[20..24].copy_from_slice(b"XYZ ");
    h[36..40].copy_from_slice(b"acsp");
    // The PCS illuminant, D50.
    h[68..72].copy_from_slice(&s15(0.9642));
    h[72..76].copy_from_slice(&s15(1.0));
    h[76..80].copy_from_slice(&s15(0.8249));
    let mut out = h;
    out.extend_from_slice(&table);
    out.extend_from_slice(&data);
    out
}

fn p3_profile() -> Vec<u8> {
    matrix_profile("Test Display P3", &P3_XYZ_D65, Trc::Srgb)
}

fn adobe_profile() -> Vec<u8> {
    matrix_profile("Test Adobe RGB", &ADOBE_XYZ_D65, Trc::Gamma(563.0 / 256.0))
}

fn srgb_test_profile() -> Vec<u8> {
    matrix_profile("Test sRGB", &SRGB_XYZ_D65, Trc::Srgb)
}

/// The reference: `rgb` in (primaries, trc) to 8-bit sRGB, in f64, with the standard D65 matrices.
fn reference(rgb: [u8; 3], to_xyz: &M3, trc: Trc) -> [u8; 3] {
    let lin = rgb.map(|v| trc.decode(f64::from(v) / 255.0));
    let xyz = mul_v(to_xyz, lin);
    let srgb_lin = mul_v(&inv(&SRGB_XYZ_D65), xyz);
    srgb_lin.map(|l| (Trc::Srgb.encode(l) * 255.0).round() as u8)
}

fn patches() -> Vec<[u8; 3]> {
    let mut v = vec![
        // Primaries, secondaries, white, black.
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [0, 255, 255],
        [255, 0, 255],
        [255, 255, 255],
        [0, 0, 0],
        // A gray ramp.
        [16, 16, 16],
        [64, 64, 64],
        [118, 118, 118],
        [128, 128, 128],
        [200, 200, 200],
        // Skin, foliage, sky, and some in-gamut mid-tones (ColorChecker-like, as 8-bit values).
        [194, 150, 130],
        [115, 82, 68],
        [98, 122, 157],
        [87, 108, 67],
        [133, 128, 177],
        [103, 189, 170],
        [214, 126, 44],
        [80, 91, 166],
        [193, 90, 99],
        [94, 60, 108],
        [157, 188, 64],
        [224, 163, 46],
        [56, 61, 150],
        [70, 148, 73],
        [175, 54, 60],
        [231, 199, 31],
        [187, 86, 149],
        [8, 133, 161],
        // Saturated wide-gamut colours that clip in sRGB.
        [255, 128, 0],
        [0, 200, 100],
        [40, 40, 255],
        [255, 40, 160],
    ];
    // A coarse cube.
    for r in [32u8, 96, 160, 224] {
        for g in [32u8, 96, 160, 224] {
            for b in [32u8, 96, 160, 224] {
                v.push([r, g, b]);
            }
        }
    }
    v
}

fn run(icc: &[u8], to_xyz: &M3, trc: Trc) -> (ColourResult, f64, u8, usize) {
    let ps = patches();
    let mut r = Raster::new(ps.len() as u32, 1);
    for (i, p) in ps.iter().enumerate() {
        r.set_pixel(i as u32, 0, *p);
    }
    let res = to_srgb(&mut r, Some(icc));
    let (mut worst, mut sum, mut over_one) = (0u8, 0.0f64, 0usize);
    for (i, p) in ps.iter().enumerate() {
        let want = reference(*p, to_xyz, trc);
        let got = r.pixel(i as u32, 0);
        for c in 0..3 {
            let d = want[c].abs_diff(got[c]);
            worst = worst.max(d);
            sum += f64::from(d);
            if d > 1 {
                over_one += 1;
                eprintln!("patch {p:?}: want {want:?}, got {got:?}");
            }
        }
    }
    (res, sum / (ps.len() * 3) as f64, worst, over_one)
}

#[test]
fn display_p3_converts_to_srgb_within_one_level_of_the_reference() {
    let (res, mean, worst, over) = run(&p3_profile(), &P3_XYZ_D65, Trc::Srgb);
    eprintln!("Display P3 -> sRGB: mean |diff| {mean:.3} levels, worst {worst}, over 1: {over}");
    assert!(res.converted);
    assert_eq!(res.notice, Some(NOTICE_TO_SRGB));
    assert!(worst <= 1, "worst {worst}");
    assert!(mean < 0.35, "mean {mean}");
    // The output declares sRGB.
    let out = res.icc.expect("the generated sRGB profile");
    assert_eq!(out.as_slice(), srgb_icc());
    assert_eq!(&out[36..40], b"acsp");
}

#[test]
fn adobe_rgb_converts_to_srgb_within_one_level_of_the_reference() {
    let (res, mean, worst, over) = run(&adobe_profile(), &ADOBE_XYZ_D65, Trc::Gamma(563.0 / 256.0));
    eprintln!("Adobe RGB -> sRGB: mean |diff| {mean:.3} levels, worst {worst}, over 1: {over}");
    assert!(res.converted);
    assert!(worst <= 1, "worst {worst}");
    assert!(mean < 0.35, "mean {mean}");
}

#[test]
fn the_wide_gamut_primaries_really_move() {
    // Guards the test itself: a transform that did nothing would pass the accuracy tests only if
    // the reference were also the identity.
    let mut r = Raster::new(1, 1);
    r.set_pixel(0, 0, [255, 0, 0]);
    let before = r.pixel(0, 0);
    let res = to_srgb(&mut r, Some(&p3_profile()));
    assert!(res.converted);
    // P3 red is outside sRGB: it clips to (255, 0, 0)-ish, while P3 (200, 100, 100) must change.
    let mut s = Raster::new(1, 1);
    s.set_pixel(0, 0, [200, 100, 100]);
    to_srgb(&mut s, Some(&p3_profile()));
    assert_ne!(s.pixel(0, 0), [200, 100, 100]);
    let _ = before;
}

#[test]
fn an_srgb_profile_is_recognised_and_kept_byte_exact() {
    for icc in [srgb_test_profile(), srgb_icc().to_vec()] {
        let mut r = Raster::new(3, 1);
        r.set_pixel(0, 0, [255, 0, 0]);
        r.set_pixel(1, 0, [12, 200, 90]);
        r.set_pixel(2, 0, [128, 128, 128]);
        let before = r.data.clone();
        let res = to_srgb(&mut r, Some(&icc));
        assert!(!res.converted);
        assert_eq!(res.notice, None);
        assert_eq!(res.icc.as_deref(), Some(icc.as_slice()), "byte-exact");
        assert_eq!(r.data, before, "pixels untouched");
    }
}

#[test]
fn no_profile_means_srgb_and_nothing_changes() {
    let mut r = Raster::new(2, 1);
    r.set_pixel(0, 0, [1, 2, 3]);
    let res = to_srgb(&mut r, None);
    assert_eq!(
        res,
        ColourResult {
            converted: false,
            icc: None,
            notice: None
        }
    );
    assert_eq!(to_srgb(&mut r, Some(&[])).icc, None);
}

#[test]
fn an_unusable_profile_keeps_pixels_and_profile_and_says_so() {
    for junk in [vec![0u8; 10], b"not an icc profile at all, really".to_vec()] {
        let mut r = Raster::new(2, 1);
        r.set_pixel(1, 0, [9, 8, 7]);
        let before = r.data.clone();
        let res = to_srgb(&mut r, Some(&junk));
        assert!(!res.converted);
        assert_eq!(res.notice, Some(NOTICE_PROFILE_UNUSABLE));
        assert_eq!(res.icc.as_deref(), Some(junk.as_slice()));
        assert_eq!(r.data, before);
    }
}

#[test]
fn the_icc_profile_of_the_committed_srgb_avif_is_recognised_as_srgb() {
    // `icc-srgb-32x24.avif` embeds this profile (made by Pillow, see docs/provenance.md): the HEIF
    // CI conversion test relies on it being kept byte-exact, which needs it to classify as sRGB.
    let icc = include_bytes!("../../../codecs/tests/fixtures/heif/icc-srgb.icc");
    assert_eq!(classify(Some(icc)), IccKind::Srgb);
    assert_eq!(classify(Some(&srgb_test_profile())), IccKind::Srgb);
    assert_eq!(classify(Some(&p3_profile())), IccKind::Other);
    assert_eq!(classify(Some(&adobe_profile())), IccKind::Other);
    assert_eq!(classify(None), IccKind::None);
    assert_eq!(classify(Some(b"junk")), IccKind::Other);
    assert_eq!(classify(Some(srgb_icc())), IccKind::Srgb);
}

#[test]
fn the_banded_transform_equals_one_pass_on_a_tall_image() {
    // More rows than one band, odd width: every row must be converted, none twice.
    let icc = p3_profile();
    let (w, h) = (7u32, (BAND_ROWS * 2 + 5) as u32);
    let mut tall = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            tall.set_pixel(x, y, [(x * 36) as u8, (y * 3) as u8, ((x + y) * 5) as u8]);
        }
    }
    let mut rows = Vec::new();
    for y in 0..h {
        let mut one = Raster::new(w, 1);
        for x in 0..w {
            one.set_pixel(x, 0, tall.pixel(x, y));
        }
        to_srgb(&mut one, Some(&icc));
        rows.extend_from_slice(&one.data);
    }
    to_srgb(&mut tall, Some(&icc));
    assert_eq!(tall.data, rows);
}
