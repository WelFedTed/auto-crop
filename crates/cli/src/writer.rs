// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Writing results that are NOT replacements of an original: `--output`, `--suffix`, `--copy` and
//! `render`. Replacing an original goes through the engine's save path (verified backup, journal,
//! atomic replace); none of that is needed here because nothing that exists is touched.
//!
//! Safety properties of this writer:
//!
//! * a file that exists is never overwritten: names are planned with the engine's collision plan
//!   (`fsplan`), and the final step is a no-clobber link or rename;
//! * every output is written to a temp file in its folder, made durable and decoded back (size,
//!   format and pixel dimensions checked) before any of the set is placed, so a failed encode or
//!   verify leaves nothing but the temp files, which are removed;
//! * the files of one scan are placed together: if one placement fails the ones already placed
//!   are removed again (they are ours: no-clobber guarantees they did not exist).
//!
//! The pixels come from the same functions as the engine's save: `decode`, `render_quad` with the
//! pixel limit, `encode`, so `render`, `process --output` and an in-place `process` agree
//! byte for byte for the same quality.

use crate::args::{FormatArg, IfExists, OutputMode};
use crate::inputs::Candidate;
use auto_crop_codecs::{Format, MAX_PIXELS, decode, encode};
use auto_crop_core::{ErrKind, QuadWarp};
use auto_crop_engine::fsplan::{OnCollision, PlanError, PlanInput, ReservedKeys, plan_group};
use auto_crop_engine::util::{blake3_hex, new_id};
use auto_crop_imgproc::Raster;
use auto_crop_imgproc::render::{Limits, render_quad};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The format of an output: the source's when this build can write it, else PNG (JPEG for HEIC,
/// the conversion target of PLAN 3.5), unless the user chose one.
pub fn out_format(source: Format, want: FormatArg) -> Format {
    match want {
        FormatArg::Jpg => Format::Jpeg,
        FormatArg::Png => Format::Png,
        FormatArg::Keep => {
            if source.is_encodable() {
                source
            } else if source == Format::Heic {
                Format::Jpeg
            } else {
                Format::Png
            }
        }
    }
}

pub fn format_name(f: Format) -> &'static str {
    match f {
        Format::Jpeg => "jpeg",
        Format::Png => "png",
        Format::Tiff => "tiff",
        Format::Webp => "webp",
        Format::Heic => "heic",
        Format::Avif => "avif",
        _ => "other",
    }
}

/// Where the files of one input go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyPlan {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanFailure {
    /// `--if-exists skip` and a name is taken.
    Exists,
    Error(ErrKind),
}

fn absolute(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Where `cand` is written in `mode`, and under which template.
fn destination(
    mode: &OutputMode,
    template: Option<&str>,
    cand: &Candidate,
) -> (PathBuf, Option<String>) {
    let parent = cand
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    match mode {
        OutputMode::Dir(out) => {
            let rel = parent
                .strip_prefix(&cand.root)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            (absolute(&out.join(rel)), template.map(str::to_owned))
        }
        OutputMode::Suffix(s) => (
            parent,
            Some(template.map_or_else(|| format!("{{name}}{s}"), str::to_owned)),
        ),
        OutputMode::Copy => (parent.join("AutoCrop"), template.map(str::to_owned)),
        OutputMode::InPlace { .. } => (parent, None),
    }
}

/// Chooses the names of `count` outputs of `cand` (phase 1 of the engine's plan) and reserves
/// them, so two inputs of one run never plan the same file. The reservation is kept for the run:
/// a dry run reserves too, so its plan equals the real run's.
pub fn plan_copies(
    mode: &OutputMode,
    template: Option<&str>,
    if_exists: IfExists,
    cand: &Candidate,
    count: usize,
    ext: &str,
    reserved: &std::sync::Mutex<ReservedKeys>,
) -> Result<CopyPlan, PlanFailure> {
    let (dir, template) = destination(mode, template, cand);
    let mut r = reserved.lock().unwrap_or_else(|e| e.into_inner());
    let plan = plan_group(&PlanInput {
        source: &cand.path,
        dir: &dir,
        count,
        template: template.as_deref(),
        ext,
        on_collision: match if_exists {
            IfExists::KeepBoth => OnCollision::Rename,
            IfExists::Skip => OnCollision::Skip,
        },
        reserved: &r,
        own: &[],
    })
    .map_err(|e| match e {
        PlanError::Taken => PlanFailure::Exists,
        other => PlanFailure::Error(other.kind()),
    })?;
    for p in &plan.paths {
        r.insert(p);
    }
    Ok(CopyPlan { paths: plan.paths })
}

/// One encoded output.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: Format,
}

/// Renders every quad of `raster` and encodes it. A panic in a codec fails the call, not the run.
pub fn render_all(
    raster: &Raster,
    quads: &[QuadWarp],
    format: Format,
    quality: u8,
    icc: Option<&[u8]>,
) -> Result<Vec<Rendered>, ErrKind> {
    quads
        .iter()
        .map(|q| {
            auto_crop_engine::run_isolated(std::panic::AssertUnwindSafe(|| {
                let out = render_quad(raster, q, Limits::pixels(MAX_PIXELS))
                    .map_err(|_| ErrKind::NoCrop)?;
                let bytes = encode(&out, format, quality, icc)
                    .map_err(auto_crop_engine::error::codec_err)?;
                Ok::<_, ErrKind>(Rendered {
                    bytes,
                    width: out.width,
                    height: out.height,
                    format,
                })
            }))
            .unwrap_or(Err(ErrKind::InternalPanic))
        })
        .collect()
}

/// Decodes `bytes` of a source file (the same limits as the engine) for rendering.
pub fn decode_source(bytes: &[u8]) -> Result<(Raster, Format, Option<Vec<u8>>), ErrKind> {
    let d = auto_crop_engine::run_isolated(|| decode(bytes))
        .map_err(|_| ErrKind::InternalPanic)?
        .map_err(auto_crop_engine::error::codec_err)?;
    Ok((d.raster, d.format, d.icc))
}

struct Temp {
    path: PathBuf,
}

impl Temp {
    fn discard(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn write_temp(dir: &Path, bytes: &[u8], mtime: Option<SystemTime>) -> Result<Temp, ErrKind> {
    let path = dir.join(format!(".autocrop-{}.tmp", new_id()));
    let go = || -> std::io::Result<()> {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(bytes)?;
        if let Some(t) = mtime {
            f.set_modified(t)?;
        }
        f.sync_all()
    };
    match go() {
        Ok(()) => Ok(Temp { path }),
        Err(e) => {
            let _ = fs::remove_file(&path);
            Err(ErrKind::from_io(&e))
        }
    }
}

/// The written file is what was encoded and decodes to the expected image.
fn verify(t: &Temp, r: &Rendered) -> Result<(), ErrKind> {
    let back = fs::read(&t.path).map_err(|_| ErrKind::VerifyFailed)?;
    if back.len() != r.bytes.len() || blake3_hex(&back) != blake3_hex(&r.bytes) {
        return Err(ErrKind::VerifyFailed);
    }
    let d = decode(&back).map_err(|_| ErrKind::VerifyFailed)?;
    if d.format != r.format || (d.raster.width, d.raster.height) != (r.width, r.height) {
        return Err(ErrKind::VerifyFailed);
    }
    Ok(())
}

/// Puts `tmp` at `dest` without ever replacing an existing file.
fn place_no_clobber(tmp: &Path, dest: &Path) -> Result<(), ErrKind> {
    // (`render --force` replaces on purpose: see `commit`.)
    match fs::hard_link(tmp, dest) {
        Ok(()) => {
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(ErrKind::PlanStale),
        Err(_) => {
            // A file system without hard links: check, then rename.
            if fs::symlink_metadata(dest).is_ok() {
                return Err(ErrKind::PlanStale);
            }
            fs::rename(tmp, dest).map_err(|e| ErrKind::from_io(&e))
        }
    }
}

/// Writes the set: all temps verified first, then all placed, or nothing is left behind.
pub fn commit(
    paths: &[PathBuf],
    files: &[Rendered],
    mtime: Option<SystemTime>,
) -> Result<(), ErrKind> {
    commit_with(paths, files, mtime, false)
}

/// [`commit`], optionally replacing files that exist (only `render --force` asks for that; the
/// replacement is an atomic rename of a verified file, so an old file is never half replaced).
pub fn commit_with(
    paths: &[PathBuf],
    files: &[Rendered],
    mtime: Option<SystemTime>,
    overwrite: bool,
) -> Result<(), ErrKind> {
    if paths.len() != files.len() || paths.is_empty() {
        return Err(ErrKind::Internal);
    }
    let mut temps: Vec<Temp> = Vec::new();
    let cleanup = |temps: &[Temp]| temps.iter().for_each(Temp::discard);
    for (dest, r) in paths.iter().zip(files) {
        let dir = dest.parent().ok_or(ErrKind::Internal)?;
        fs::create_dir_all(dir).map_err(|e| ErrKind::from_io(&e))?;
        let step = write_temp(dir, &r.bytes, mtime).and_then(|t| {
            let v = verify(&t, r);
            temps.push(t);
            v
        });
        if let Err(e) = step {
            cleanup(&temps);
            return Err(e);
        }
    }
    let mut placed: Vec<&PathBuf> = Vec::new();
    for (i, dest) in paths.iter().enumerate() {
        let step = if overwrite {
            fs::rename(&temps[i].path, dest).map_err(|e| ErrKind::from_io(&e))
        } else {
            place_no_clobber(&temps[i].path, dest)
        };
        match step {
            Ok(()) => placed.push(dest),
            Err(e) => {
                // Files placed by no-clobber are ours and go again. With `overwrite` they may have
                // replaced something that existed: those stay (each is a whole, verified file).
                if !overwrite {
                    for p in placed {
                        let _ = fs::remove_file(p);
                    }
                }
                cleanup(&temps);
                return Err(e);
            }
        }
    }
    cleanup(&temps);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn png(w: u32, h: u32, c: [u8; 3]) -> Rendered {
        let r = Raster::filled(w, h, c);
        Rendered {
            bytes: encode(&r, Format::Png, 90, None).unwrap(),
            width: w,
            height: h,
            format: Format::Png,
        }
    }

    fn cand(dir: &Path, name: &str) -> Candidate {
        Candidate {
            path: dir.join(name),
            display: name.to_owned(),
            root: dir.to_path_buf(),
        }
    }

    #[test]
    fn formats_follow_the_source_unless_chosen() {
        assert_eq!(out_format(Format::Jpeg, FormatArg::Keep), Format::Jpeg);
        assert_eq!(out_format(Format::Tiff, FormatArg::Keep), Format::Png);
        assert_eq!(out_format(Format::Heic, FormatArg::Keep), Format::Jpeg);
        assert_eq!(out_format(Format::Heic, FormatArg::Png), Format::Png);
        assert_eq!(out_format(Format::Png, FormatArg::Jpg), Format::Jpeg);
    }

    #[test]
    fn names_are_planned_per_mode_and_never_collide() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        let reserved = Mutex::new(ReservedKeys::default());
        let c = cand(r, "scan.jpg");
        let suffix = plan_copies(
            &OutputMode::Suffix("_c".into()),
            None,
            IfExists::KeepBoth,
            &c,
            1,
            "jpg",
            &reserved,
        )
        .unwrap();
        assert_eq!(suffix.paths, [r.join("scan_c.jpg")]);
        // The same name again in the same run is taken: numbered, not reused.
        let again = plan_copies(
            &OutputMode::Suffix("_c".into()),
            None,
            IfExists::KeepBoth,
            &c,
            1,
            "jpg",
            &reserved,
        )
        .unwrap();
        assert_eq!(again.paths, [r.join("scan (2)_c.jpg")]);
        let split = plan_copies(
            &OutputMode::Copy,
            None,
            IfExists::KeepBoth,
            &c,
            3,
            "png",
            &reserved,
        )
        .unwrap();
        assert_eq!(
            split.paths,
            [
                r.join("AutoCrop/scan_01.png"),
                r.join("AutoCrop/scan_02.png"),
                r.join("AutoCrop/scan_03.png")
            ]
        );
        let out = r.join("out");
        let sub = Candidate {
            path: r.join("a/b/x.jpg"),
            display: "x".into(),
            root: r.to_path_buf(),
        };
        let m = plan_copies(
            &OutputMode::Dir(out.clone()),
            None,
            IfExists::KeepBoth,
            &sub,
            1,
            "jpg",
            &reserved,
        )
        .unwrap();
        assert_eq!(m.paths, [out.join("a/b/x.jpg")]);
        // --if-exists skip with a taken name.
        fs::create_dir_all(&out).unwrap();
        fs::create_dir_all(out.join("a/b")).unwrap();
        fs::write(out.join("a/b/y.jpg"), b"x").unwrap();
        let sub_y = Candidate {
            path: r.join("a/b/y.jpg"),
            ..sub
        };
        let e = plan_copies(
            &OutputMode::Dir(out),
            None,
            IfExists::Skip,
            &sub_y,
            1,
            "jpg",
            &Mutex::new(ReservedKeys::default()),
        )
        .unwrap_err();
        assert_eq!(e, PlanFailure::Exists);
    }

    #[test]
    fn a_set_is_written_whole_and_never_over_an_existing_file() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("out");
        let paths = [dir.join("a_01.png"), dir.join("a_02.png")];
        let files = [png(8, 6, [10, 20, 30]), png(9, 7, [30, 20, 10])];
        commit(&paths, &files, None).unwrap();
        assert_eq!(fs::read(&paths[0]).unwrap(), files[0].bytes);
        assert_eq!(fs::read(&paths[1]).unwrap(), files[1].bytes);
        let temps = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".autocrop-"))
            .count();
        assert_eq!(temps, 0);

        // A taken name: the second file already exists, so nothing of the new set stays.
        let dir2 = d.path().join("out2");
        fs::create_dir_all(&dir2).unwrap();
        fs::write(dir2.join("b_02.png"), b"mine").unwrap();
        let paths2 = [dir2.join("b_01.png"), dir2.join("b_02.png")];
        assert_eq!(commit(&paths2, &files, None), Err(ErrKind::PlanStale));
        assert!(
            !dir2.join("b_01.png").exists(),
            "the first file was taken back"
        );
        assert_eq!(fs::read(dir2.join("b_02.png")).unwrap(), b"mine");
        let names: Vec<_> = fs::read_dir(&dir2)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["b_02.png"], "no temp files remain");
    }

    #[test]
    fn a_wrong_encode_is_caught_before_anything_is_placed() {
        let d = tempfile::tempdir().unwrap();
        let mut bad = png(8, 6, [1, 2, 3]);
        bad.width = 9; // the verify step decodes and compares dimensions
        let dest = d.path().join("x.png");
        assert_eq!(
            commit(std::slice::from_ref(&dest), &[bad], None),
            Err(ErrKind::VerifyFailed)
        );
        assert!(fs::read_dir(d.path()).unwrap().next().is_none());
    }
}
