// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

//! Format conversion (ROADMAP M2.36, M6.47, M6.49, M6.50, M6.56; PLAN 2.7 "Conversion replaces the
//! source (B3, B12)", 3.5, 3.6, 3.7): any image this build decodes becomes a JPEG or a PNG, with no
//! crop and no detection. `IMG_0001.HEIC` becomes `IMG_0001.jpg`, `scan.tif` becomes `scan.png`.
//!
//! **What one file goes through** ([`Engine::convert_file`]): sniff and probe the header (nothing is
//! decoded for a file that is already in the target format, or that is refused), decode once with
//! the EXIF turn applied once and translucent pixels composited over white, convert the colours to
//! sRGB ([`crate::colour`], unless the caller asked to keep the profile), encode through the same
//! encoders and metadata policy as a save ([`crate::output`]), then commit.
//!
//! **Placement** ([`Placement`]):
//!
//! * [`Placement::Replace`] (the default, B3): the group commit of [`crate::group`] with the source
//!   unlinked last: encode and verify the temp, back up the source (a verified copy, link or
//!   clone), commit the output by **no-clobber** move, re-check the source and unlink it. Each step
//!   is journalled; a crash between the last two leaves both files and recovery finishes the
//!   unlink. A name already taken gets `IMG_0001 (2).jpg`, never an overwrite. When the new name is
//!   the source's own (a HEIC saved as `IMG.jpg`, or `--reencode` of a JPEG) the in-place commit
//!   ([`crate::group::commit_single`]) swaps it. Restore returns the source to its path and, with
//!   [`crate::DerivedAction::Remove`], moves the converted file into the backup entry.
//! * [`Placement::Keep`]: the converted file is written beside the source, which is not touched
//!   (no backup is needed: nothing that exists changes).
//! * [`Placement::Folder`]: the same, into a folder the caller names.
//!
//! **What is never replaced** (PLAN 2.7, D3): a source with several frames, pages or images, and a
//! CMYK source whose conversion is naive. In `Replace` mode those are skipped with a notice and
//! stay byte-identical; `Keep` and `Folder` convert the first frame and say so. A source already
//! in the target format is skipped ([`Skip::SameFormat`]) unless `reencode` is set.
//!
//! **Notices** (stable codes, never English): `alpha.flattened` (translucent pixels over white),
//! `depth.reduced_to_8`, `colour.converted_to_srgb`, `colour.profile_unusable`,
//! `colour.nclx_not_srgb`, `hdr.not_tonemapped`, `heic.multi_image`, `heic.sequence`,
//! `tiff.multi_page`, `anim.first_frame_only`, `cmyk.naive_conversion`, `livephoto.video_untouched`
//! (the paired `.MOV` of a Live Photo is never read or moved) and `sync.root`.

use crate::api::*;
use crate::colour::{self, ColourMode, IccKind};
use crate::engine::{Engine, SavedRec, Snapshot, lock, stat_of};
use crate::error::{ErrKind, codec_err};
use crate::fsplan::{GroupPlan, OnCollision, PlanInput, ReservedKeys, path_key};
use crate::fsstate;
use crate::group::{
    BackupPlan, FaultHook, GroupError, GroupRequest, NoFaults, OutSpec, Output, Produced,
    SingleRequest, SourceFingerprint, commit_group_verified, commit_single,
};
use crate::memory::{MemoryBudget, job_weight};
use crate::output::{EngineOptions, QualitySetting, SourceMeta, encode_raster};
use crate::space::{Need, preflight};
use crate::store::NewBackup;
use crate::util::blake3_hex;
use auto_crop_codecs::{CodecError, Format, decode_flattened_with, probe_with, sniff};
use auto_crop_core::CancelToken;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// Notice code of a source that is already in its target format (nothing to convert).
pub const NOTICE_SAME_FORMAT: &str = "convert.same_format";
/// Notice: this build has no HEIC and AVIF decoder (the HEIC package does).
pub const NOTICE_HEIF_UNAVAILABLE: &str = "heif.unavailable";
/// Notice: a Live Photo's paired video sits next to the still; it is never read, moved or changed.
pub const NOTICE_LIVE_PHOTO: &str = "livephoto.video_untouched";
/// Notice: translucent pixels were composited over white.
pub const NOTICE_ALPHA_FLATTENED: &str = "alpha.flattened";

/// The most a source may weigh (the engine's own cap).
const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

/// The format a source is converted to by the automatic choice, or `None` if it is not converted:
/// HEIC and HEIF become JPEG, formats with no writer that PNG can hold without loss (BMP; GIF, ICO,
/// TGA, PNM, QOI and HDR when they are decodable) become PNG. A JPEG or PNG is already in its
/// target; WebP, TIFF and AVIF are only converted when the caller names a target
/// ([`ConvertOptions::target`]). This is the target of [`Engine::convert_items`].
pub fn conversion_target(format: Format, frames: u32) -> Option<Format> {
    if frames > 1 {
        return None;
    }
    match format {
        Format::Heic => Some(Format::Jpeg),
        Format::Bmp | Format::Gif => Some(Format::Png),
        _ => None,
    }
}

/// What a source is converted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConvertTarget {
    #[default]
    Jpg,
    Png,
}

impl ConvertTarget {
    pub fn format(self) -> Format {
        match self {
            ConvertTarget::Jpg => Format::Jpeg,
            ConvertTarget::Png => Format::Png,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ConvertTarget::Jpg => "jpg",
            ConvertTarget::Png => "png",
        }
    }
}

/// Where the converted file goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Placement {
    /// Replace the source after a verified backup (B3).
    #[default]
    Replace,
    /// Write beside the source and leave it alone.
    Keep,
    /// Write into a folder the caller names and leave the source alone.
    Folder,
}

impl Placement {
    pub fn name(self) -> &'static str {
        match self {
            Placement::Replace => "replace",
            Placement::Keep => "keep",
            Placement::Folder => "folder",
        }
    }
}

/// What the caller chooses about a conversion. `Default` is the shipped behaviour: JPEG,
/// Balanced quality, colours converted to sRGB, GPS kept, the source replaced after a backup.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConvertOptions {
    pub target: ConvertTarget,
    pub colour: ColourMode,
    /// JPEG quality: a preset (from the source's own quality, if it is a JPEG) or a number.
    pub quality: QualitySetting,
    /// Drop EXIF GPS, XMP and IPTC.
    pub strip_location: bool,
    pub placement: Placement,
    /// Convert a source that is already in the target format too (a JPEG to a new JPEG).
    pub reencode: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            target: ConvertTarget::Jpg,
            colour: ColourMode::Srgb,
            quality: QualitySetting::default(),
            strip_location: false,
            placement: Placement::Replace,
            reencode: false,
        }
    }
}

impl ConvertOptions {
    /// The engine options of this conversion: the engine's own, with the quality and the metadata
    /// choice of this one.
    pub fn engine_options(&self, base: EngineOptions) -> EngineOptions {
        EngineOptions {
            quality: self.quality,
            strip_location: self.strip_location,
            ..base
        }
    }
}

/// Why a file was left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    /// Already in the target format (`--reencode` converts it anyway).
    SameFormat,
    /// Replacing it would lose content (several frames, naive CMYK): the notice says which. The
    /// file stays byte-identical; `Keep` and `Folder` convert it.
    NotReplaceable(&'static str),
    /// A HEIC, HEIF or AVIF file in a build without the HEIC decoder.
    HeifUnavailable,
    /// Not an image this build reads.
    Unsupported,
}

impl Skip {
    /// The stable code (the CLI's registry names).
    pub fn code(&self) -> &'static str {
        match self {
            Skip::SameFormat => "SAME_FORMAT",
            Skip::NotReplaceable(_) => "NOT_REPLACEABLE",
            Skip::HeifUnavailable => "HEIF_UNAVAILABLE",
            Skip::Unsupported => "UNSUPPORTED_FORMAT",
        }
    }

    /// The notice code that goes with the code, if any.
    pub fn notice(&self) -> Option<&'static str> {
        match self {
            Skip::SameFormat => Some(NOTICE_SAME_FORMAT),
            Skip::NotReplaceable(n) => Some(n),
            Skip::HeifUnavailable => Some(NOTICE_HEIF_UNAVAILABLE),
            Skip::Unsupported => None,
        }
    }
}

/// A file that could not be converted: left alone on purpose, or failed (the source is untouched
/// either way, or restored by the protocol's rollback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    Skipped(Skip, Vec<String>),
    Failed(ErrKind, Vec<String>),
}

impl ConvertError {
    fn fail(k: ErrKind) -> Self {
        ConvertError::Failed(k, Vec::new())
    }
}

/// One finished conversion (or, in a dry run, the plan of one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub output: PathBuf,
    /// blake3 of the written file (empty in a dry run).
    pub output_blake3: String,
    /// The backup that holds the source ([`Placement::Replace`] only).
    pub backup_id: Option<String>,
    pub from: Format,
    pub to: Format,
    /// The output's pixel size (the source's, with the EXIF turn applied).
    pub dims: (u32, u32),
    pub bytes_in: u64,
    /// Size of the written file (0 in a dry run).
    pub bytes_out: u64,
    pub notices: Vec<String>,
    /// Things after the commit point that did not go through: `SavedSourceInUse` (the source could
    /// not be removed), `SourceChanged` (it was changed meanwhile and left alone).
    pub notes: Vec<ErrKind>,
    /// The source is gone from its path (it is in the backup store).
    pub source_removed: bool,
    /// The profile the output embeds (`None` in a dry run, where nothing is decoded).
    pub icc: IccKind,
    pub dry_run: bool,
}

/// One conversion request. `path` is read and, in [`Placement::Replace`], replaced.
pub struct ConvertRequest<'a> {
    pub path: &'a Path,
    pub options: ConvertOptions,
    /// The destination folder of [`Placement::Folder`] (the caller mirrors a tree, or not).
    pub folder: Option<&'a Path>,
    /// The backup run this file belongs to (`restore --run`).
    pub run_id: &'a str,
    pub run_name: &'a str,
    /// Classify and plan the name; read nothing but the header and write nothing.
    pub dry_run: bool,
    /// Memory admission of the decode (a batch shares one budget).
    pub budget: Option<&'a MemoryBudget>,
    pub cancel: Option<&'a CancelToken>,
    /// An item of the session that is this file: its own name is not "taken".
    pub(crate) item: Option<u32>,
    /// The state the file was in when it was opened: a different hash is `SourceChanged`.
    pub(crate) expected: Option<&'a Snapshot>,
}

impl<'a> ConvertRequest<'a> {
    pub fn new(
        path: &'a Path,
        options: ConvertOptions,
        run_id: &'a str,
        run_name: &'a str,
    ) -> Self {
        Self {
            path,
            options,
            folder: None,
            run_id,
            run_name,
            dry_run: false,
            budget: None,
            cancel: None,
            item: None,
            expected: None,
        }
    }
}

/// Notices of a decode that make a replacement lose content, in the order they are reported.
const REPLACE_BLOCKERS: [&str; 5] = [
    "tiff.multi_page",
    "anim.first_frame_only",
    "heic.multi_image",
    "heic.sequence",
    "cmyk.naive_conversion",
];

fn livephoto_video_beside(path: &Path) -> bool {
    ["mov", "MOV", "Mov"]
        .iter()
        .any(|e| path.with_extension(e).is_file())
}

fn ms_to_time(ms: i64) -> std::time::SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

impl Engine {
    /// Converts one file. See the module docs for the order of the steps and what each placement
    /// guarantees. Never panics; a decoder panic is a `Failed(InternalPanic)`.
    pub fn convert_file(&self, req: &ConvertRequest<'_>) -> Result<Converted, ConvertError> {
        self.convert_file_with_faults(req, &NoFaults)
    }

    #[doc(hidden)]
    pub fn convert_file_with_faults(
        &self,
        req: &ConvertRequest<'_>,
        hook: &dyn FaultHook,
    ) -> Result<Converted, ConvertError> {
        crate::run_isolated(std::panic::AssertUnwindSafe(|| {
            self.convert_file_inner(req, hook)
        }))
        .unwrap_or(Err(ConvertError::fail(ErrKind::InternalPanic)))
    }

    fn convert_file_inner(
        &self,
        req: &ConvertRequest<'_>,
        hook: &dyn FaultHook,
    ) -> Result<Converted, ConvertError> {
        let co = req.options;
        let base = self.options();
        let opts = co.engine_options(base);
        let replace = co.placement == Placement::Replace;
        let target = co.target.format();
        let path = req.path;
        let never = CancelToken::never();
        let cancel = req.cancel.unwrap_or(&never);
        let cancelled = || cancel.is_cancelled();
        let fail = |k: ErrKind| ConvertError::Failed(k, Vec::new());
        if cancelled() {
            return Err(fail(ErrKind::Cancelled));
        }

        // The file's state, before anything is read for the conversion or written.
        if replace && !req.dry_run {
            fsstate::check_replaceable(path, opts.hydrate_cloud_files).map_err(fail)?;
        } else {
            fsstate::check_readable(path, opts.hydrate_cloud_files).map_err(fail)?;
        }
        let meta = fs::metadata(path).map_err(|e| fail(ErrKind::from_io(&e)))?;
        if !meta.is_file() {
            return Err(fail(ErrKind::Unreadable));
        }
        if meta.len() > MAX_SOURCE_BYTES {
            return Err(fail(ErrKind::TooLarge));
        }
        let bytes = fs::read(path).map_err(|e| fail(ErrKind::from_io(&e)))?;
        let snap = Snapshot {
            size: bytes.len() as u64,
            mtime_ms: meta.modified().map(crate::util::unix_ms).unwrap_or(0),
            blake3: blake3_hex(&bytes),
        };
        if let Some(e) = req.expected
            && e.blake3 != snap.blake3
        {
            return Err(fail(ErrKind::SourceChanged));
        }

        // What it is: the magic bytes, never the name.
        let Some(src_format) = sniff(&bytes) else {
            return Err(ConvertError::Skipped(Skip::Unsupported, Vec::new()));
        };
        if !src_format.is_decodable() {
            // HEIC and AVIF are probed in every build but decoded only with the `heif` feature.
            return Err(ConvertError::Skipped(
                if matches!(src_format, Format::Heic | Format::Avif) {
                    Skip::HeifUnavailable
                } else {
                    Skip::Unsupported
                },
                Vec::new(),
            ));
        }
        let limits = opts.limits();
        let probe = match probe_with(&bytes, &limits) {
            Ok(p) => p,
            Err(CodecError::Unsupported | CodecError::NotDecodable(_)) => {
                return Err(ConvertError::Skipped(Skip::Unsupported, Vec::new()));
            }
            Err(e) => return Err(fail(codec_err(e))),
        };
        let pixels = u64::from(probe.width) * u64::from(probe.height);
        if pixels > opts.max_pixels {
            return Err(fail(ErrKind::TooLarge));
        }
        if src_format == target && !co.reencode {
            return Err(ConvertError::Skipped(
                Skip::SameFormat,
                vec![NOTICE_SAME_FORMAT.to_owned()],
            ));
        }
        let mut notices: Vec<String> = Vec::new();
        if probe.frames > 1 {
            // One rule for what may be replaced (D3): a source with several frames never is.
            let n = crate::fsplan::replace_refusal(src_format, probe.frames, &[])
                .unwrap_or("anim.first_frame_only");
            if replace {
                return Err(ConvertError::Skipped(
                    Skip::NotReplaceable(n),
                    vec![n.to_owned()],
                ));
            }
        }
        if matches!(src_format, Format::Heic | Format::Avif) && livephoto_video_beside(path) {
            notices.push(NOTICE_LIVE_PHOTO.to_owned());
        }

        // The folder the output goes to, and its name.
        let src_dir = path.parent().ok_or(fail(ErrKind::Internal))?.to_path_buf();
        let dest_dir = match co.placement {
            Placement::Replace | Placement::Keep => src_dir.clone(),
            Placement::Folder => req
                .folder
                .map(Path::to_path_buf)
                .ok_or(fail(ErrKind::Internal))?,
        };
        let ext = target.extension();
        // A name that is the source's own (case-folded): a HEIC saved as `IMG.jpg`, or `--reencode`
        // of a JPEG. In `Replace` mode that file is swapped in place.
        let own_name = replace
            && dest_dir == src_dir
            && path
                .file_stem()
                .map(|s| {
                    path_key(&dest_dir.join(format!("{}.{ext}", s.to_string_lossy())))
                        == path_key(path)
                })
                .unwrap_or(false);

        if req.dry_run {
            let (plan, _r) = self.plan_dest(path, &dest_dir, ext, own_name, req.item)?;
            return Ok(Converted {
                output: if own_name {
                    path.to_path_buf()
                } else {
                    plan.paths[0].clone()
                },
                output_blake3: String::new(),
                backup_id: None,
                from: src_format,
                to: target,
                dims: (probe.width, probe.height),
                bytes_in: snap.size,
                bytes_out: 0,
                notices,
                notes: Vec::new(),
                source_removed: false,
                icc: IccKind::None,
                dry_run: true,
            });
        }

        // Admission: the decoded image is held in memory until the output is encoded.
        let _permit = match req.budget {
            Some(b) => Some(b.acquire(job_weight(pixels), cancel).map_err(fail)?),
            None => None,
        };
        preflight(&Need::new(
            &dest_dir,
            snap.size,
            replace.then(|| (self.inner.store.dir(), snap.size)),
        ))
        .map_err(fail)?;

        // The pixels: decoded once, the EXIF turn applied once, translucent pixels over white.
        let decoded = decode_flattened_with(&bytes, &limits).map_err(|e| fail(codec_err(e)))?;
        if cancelled() {
            return Err(fail(ErrKind::Cancelled));
        }
        for n in &decoded.notices {
            if !notices.iter().any(|x| x == n) {
                notices.push((*n).to_owned());
            }
        }
        if replace
            && let Some(n) = REPLACE_BLOCKERS
                .iter()
                .find(|n| decoded.notices.contains(n))
                .copied()
        {
            return Err(ConvertError::Skipped(
                Skip::NotReplaceable(n),
                vec![n.to_owned()],
            ));
        }
        let icc_src = decoded.icc.clone();
        let mut raster = decoded.raster;
        let sm = SourceMeta::read_for_convert(
            src_format,
            &bytes,
            icc_src.clone().map(std::sync::Arc::new),
            &limits,
        );
        drop(bytes);

        // The colours.
        let (icc_out, to_srgb) = match co.colour {
            ColourMode::Preserve => (icc_src, false),
            ColourMode::Srgb => {
                let r = colour::to_srgb(&mut raster, icc_src.as_deref());
                if let Some(n) = r.notice
                    && !notices.iter().any(|x| x == n)
                {
                    notices.push(n.to_owned());
                }
                (r.icc, r.converted)
            }
        };
        let icc_kind = colour::classify(icc_out.as_deref());
        let sm = sm.with_colour(icc_out, to_srgb);
        let enc = encode_raster(&raster, target, &sm, &opts).map_err(fail)?;
        let dims = (raster.width, raster.height);
        drop(raster);
        if cancelled() {
            return Err(fail(ErrKind::Cancelled));
        }

        // The name, and the commit.
        let (plan, _reservation) = self.plan_dest(path, &dest_dir, ext, own_name, req.item)?;
        let mtime = ms_to_time(snap.mtime_ms);
        let fingerprint = SourceFingerprint {
            path: path.to_path_buf(),
            size: snap.size,
            mtime_ms: snap.mtime_ms,
            blake3: snap.blake3.clone(),
        };
        let produced = Produced {
            bytes: enc.bytes,
            dims,
            format: target,
        };
        let mut once = Some(Output {
            produced,
            expect: Some(enc.expect),
        });
        let backup = || {
            BackupPlan::New(NewBackup {
                source: path,
                source_blake3: &snap.blake3,
                source_size: snap.size,
                source_mtime_ms: snap.mtime_ms,
                format_ext: src_format.extension(),
                run_id: req.run_id,
                run_name: req.run_name,
                retention_days: lock(&self.inner.settings).retention_days,
                edit: None,
            })
        };
        if replace && fsstate::in_sync_root(path) {
            notices.push(fsstate::NOTICE_SYNC_ROOT.to_owned());
        }
        let commit_err = |e: GroupError| match e {
            GroupError::Failed(k) => fail(k),
            GroupError::Crashed => fail(ErrKind::Internal),
        };

        let (output, output_blake3, backup_id, notes, removed) = if own_name {
            let sreq = SingleRequest {
                store: &self.inner.store,
                target: fingerprint,
                backup: backup(),
                mtime,
                edit: None,
                verify: opts.verify,
            };
            let mut produce = || once.take().ok_or(ErrKind::Internal);
            let done = commit_single(&sreq, &mut produce, hook).map_err(commit_err)?;
            (
                path.to_path_buf(),
                done.output.blake3,
                Some(done.backup_id),
                Vec::new(),
                false,
            )
        } else {
            let dest = plan.paths[0].clone();
            if co.placement == Placement::Folder {
                fs::create_dir_all(&dest_dir).map_err(|e| fail(ErrKind::from_io(&e)))?;
            }
            let greq = GroupRequest {
                store: &self.inner.store,
                source: fingerprint,
                unlink_source: replace,
                backup: if replace { backup() } else { BackupPlan::None },
                outputs: vec![OutSpec {
                    item_id: 0,
                    index: 1,
                    final_path: dest.clone(),
                    replaces_blake3: None,
                }],
                retire: Vec::new(),
                mtime,
                edit: None,
            };
            let mut produce = |_: usize| once.take().ok_or(ErrKind::Internal);
            let done = commit_group_verified(&greq, &mut produce, opts.verify, hook)
                .map_err(commit_err)?;
            let rec = done.outputs.first().ok_or(fail(ErrKind::Internal))?;
            (
                dest,
                rec.blake3.clone(),
                done.backup_id,
                done.notes,
                done.source_removed,
            )
        };
        Ok(Converted {
            bytes_out: stat_of(&output).map_or(0, |s| s.0),
            output,
            output_blake3,
            backup_id,
            from: src_format,
            to: target,
            dims,
            bytes_in: snap.size,
            notices,
            notes,
            source_removed: removed || own_name,
            icc: icc_kind,
            dry_run: false,
        })
    }

    /// Chooses the output name (phase 1 of the plan) and reserves it (phase 2), so two
    /// conversions in flight never plan the same file. `own_name`: the source's own path is free
    /// (it is swapped in place).
    fn plan_dest<'a>(
        &'a self,
        source: &Path,
        dir: &Path,
        ext: &str,
        own_name: bool,
        item: Option<u32>,
    ) -> Result<(GroupPlan, crate::scan::Reservation<'a>), ConvertError> {
        let own: Vec<PathBuf> = if own_name {
            vec![source.to_path_buf()]
        } else {
            Vec::new()
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let reserved = self.reserved_names(item.unwrap_or(0));
            let plan = crate::fsplan::plan_group(&PlanInput {
                source,
                dir,
                count: 1,
                template: None,
                ext,
                on_collision: OnCollision::Rename,
                reserved: &reserved,
                own: &own,
            })
            .map_err(|e| ConvertError::fail(e.kind()))?;
            match crate::scan::Reservation::take(self, &plan.paths, &own) {
                Ok(r) => {
                    plan.recheck(&ReservedKeys::default(), &own)
                        .map_err(ConvertError::fail)?;
                    return Ok((plan, r));
                }
                Err(_) if attempt < 4 => continue,
                Err(e) => return Err(ConvertError::fail(e)),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The item-based entry point (a session image that was opened and analysed)
// ---------------------------------------------------------------------------------------------

impl Engine {
    /// Converts each open image to its automatic target ([`conversion_target`]: HEIC to JPEG, BMP
    /// to PNG), replacing the source after a verified backup. `notify` hears the new view of every
    /// image that was handled. For a chosen target, folder and options use [`Engine::convert_file`]
    /// or the batch in [`crate::convert_batch`].
    pub fn convert_items(
        &self,
        ids: &[u32],
        run_name: &str,
        notify: &dyn Fn(ItemView),
    ) -> Vec<SaveOutcome> {
        self.convert_items_with_faults(ids, run_name, &NoFaults, notify)
    }

    #[doc(hidden)]
    pub fn convert_items_with_faults(
        &self,
        ids: &[u32],
        run_name: &str,
        hook: &dyn FaultHook,
        notify: &dyn Fn(ItemView),
    ) -> Vec<SaveOutcome> {
        let run_id = crate::util::new_id();
        let run_name: String = run_name
            .chars()
            .filter(|c| !c.is_control())
            .take(80)
            .collect();
        ids.iter()
            .map(|id| {
                let outcome = crate::run_isolated(std::panic::AssertUnwindSafe(|| {
                    self.convert_one(*id, &run_id, &run_name, hook)
                }))
                .unwrap_or(Err((ErrKind::InternalPanic, Vec::new())));
                let outcome = match outcome {
                    Ok(o) => o,
                    Err((e, notices)) => SaveOutcome {
                        notices,
                        ..SaveOutcome::failed(*id, e)
                    },
                };
                if let Some(v) = self.item_view(*id) {
                    notify(v);
                }
                outcome
            })
            .collect()
    }

    fn convert_one(
        &self,
        id: u32,
        run_id: &str,
        run_name: &str,
        hook: &dyn FaultHook,
    ) -> std::result::Result<SaveOutcome, (ErrKind, Vec<String>)> {
        let plain = |e: ErrKind| (e, Vec::new());
        let item = self.item(id).ok_or(plain(ErrKind::Internal))?;
        let (path, state, src_format, snap, saved, frames, notices) = {
            let it = lock(&item);
            if it.status != ItemStatus::Ready {
                return Err(plain(ErrKind::Internal));
            }
            let state = it
                .history
                .as_ref()
                .ok_or(plain(ErrKind::Internal))?
                .current()
                .clone();
            (
                it.path.clone(),
                state,
                it.format,
                it.snapshot.clone(),
                it.saved.clone(),
                it.frames,
                it.notices.clone(),
            )
        };
        // One rule for what may be replaced (D3): a multi-frame source is never converted either.
        let Some(target) = conversion_target(src_format, frames) else {
            let notice = if frames > 1 {
                crate::fsplan::replace_refusal(src_format, frames, &notices)
            } else if src_format.is_encodable() {
                Some(NOTICE_SAME_FORMAT)
            } else {
                crate::fsplan::replace_refusal(src_format, frames, &notices)
            };
            let kind = if src_format.is_encodable() && frames <= 1 {
                ErrKind::UnsupportedOutput
            } else {
                ErrKind::NotReplaceable
            };
            return Err((kind, notice.map(|n| vec![n.to_owned()]).unwrap_or_default()));
        };
        if let Some(n) = crate::fsplan::LOSSY_DECODE_NOTICES
            .iter()
            .find(|n| notices.contains(n))
        {
            // The target cannot hold the whole picture (alpha, depth): the source stays as it is.
            return Err((ErrKind::NotReplaceable, vec![(*n).to_owned()]));
        }
        if saved.is_some() {
            // Already converted in this session: its source is in the store, not at `path`.
            return Err(plain(ErrKind::NotReplaceable));
        }
        let options = ConvertOptions {
            target: if target == Format::Png {
                ConvertTarget::Png
            } else {
                ConvertTarget::Jpg
            },
            quality: self.options().quality,
            strip_location: self.options().strip_location,
            ..ConvertOptions::default()
        };
        let mut req = ConvertRequest::new(&path, options, run_id, run_name);
        req.item = Some(id);
        req.expected = Some(&snap);
        let done = match self.convert_file_with_faults(&req, hook) {
            Ok(d) => d,
            Err(ConvertError::Failed(k, n)) => return Err((k, n)),
            Err(ConvertError::Skipped(s, n)) => {
                return Err((
                    match s {
                        Skip::SameFormat => ErrKind::UnsupportedOutput,
                        Skip::Unsupported | Skip::HeifUnavailable => ErrKind::UnsupportedFormat,
                        Skip::NotReplaceable(_) => ErrKind::NotReplaceable,
                    },
                    n,
                ));
            }
        };

        let out_snap = stat_of(&done.output)
            .map(|(size, mtime_ms)| Snapshot {
                size,
                mtime_ms,
                blake3: done.output_blake3.clone(),
            })
            .ok_or(plain(ErrKind::Internal))?;
        let backup_original = done
            .backup_id
            .as_ref()
            .and_then(|b| self.inner.store.read(b))
            .and_then(|m| self.inner.store.original_path(&m));
        {
            let mut it = lock(&item);
            it.saved = Some(SavedRec {
                backup_id: done.backup_id.clone(),
                output_path: done.output.clone(),
                copy: false,
                state,
                out: out_snap,
                group: Vec::new(),
            });
            if let Some(p) = backup_original {
                it.original_path = p;
            }
        }
        let name = done
            .output
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Ok(SaveOutcome {
            id,
            ok: true,
            error: None,
            saved: Some(SavedInfo {
                backup_id: done.backup_id,
                output: name.clone(),
                copy: false,
                outputs: vec![name],
            }),
            notes: done.notes,
            notices: Vec::new(),
        })
    }
}
