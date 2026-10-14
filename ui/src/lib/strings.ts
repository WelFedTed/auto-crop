// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Every user-visible string lives here (English). The engine returns codes, never prose (B20, PLAN 6.7);
// this file is the single place that turns a code into words, so a Fluent catalogue can replace it later.

import type { CurveProblem } from './curve.ts';
import type { ErrorCode, Reason, Side } from './types.ts';

export interface HoldCopy {
  /** Tile and chip line. `{side}` is replaced for WEAK_EDGE. */
  title: string;
  /** Shown under "Why?". */
  cause: string;
  /** The action the person can take. */
  action: string;
}

/**
 * PLAN 6.7 table, in full. The webview contract (types.ts `ReasonCode`) only emits a subset today, the rest
 * are here so a new engine code never shows as a blank line.
 */
export const HOLD: Record<string, HoldCopy> = {
  NO_QUAD: {
    title: "Couldn't find the edges",
    cause: 'No page outline was found with enough confidence, so nothing was changed.',
    action: 'Draw crop, or skip it.',
  },
  IMPLAUSIBLE_QUAD: {
    title: 'The outline looks wrong',
    cause: 'The outline found has an odd shape or size, so nothing was changed.',
    action: 'Draw crop, or skip it.',
  },
  DETECTORS_DISAGREE: {
    title: 'Two possible outlines',
    cause: 'Another outline fits the picture too, for example the whole picture, or a print inside a border of paper.',
    action: 'Check the outline, or adjust it.',
  },
  WEAK_EDGE: {
    title: 'Edge unclear on the {side} side',
    cause: 'That edge blends into the background or is partly hidden.',
    action: 'Adjust the outline.',
  },
  PARTIAL_FRAME: {
    title: 'Content may be cut off',
    cause: 'The page runs past the edge of the picture.',
    action: 'Adjust the outline.',
  },
  ORIENT_UNSURE: {
    title: 'Check orientation',
    cause: 'It is unclear which way up the page is.',
    action: 'Rotate 90 L or 90 R.',
  },
  RESIDUAL_SKEW: {
    title: 'Text still looks tilted',
    cause: 'After straightening, the lines of text still slope slightly.',
    action: 'Use the rotation ruler.',
  },
  NO_DOCUMENT: {
    title: 'No document found',
    cause: 'It looks like an ordinary photo or a blank scan.',
    action: 'Skip it, or draw a crop.',
  },
  ML_UNAVAILABLE: {
    title: 'Detection model not available',
    cause: 'Only the basic method ran, so every result is held for a look.',
    action: 'Adjust the outline.',
  },
  ANALYSIS_LIMIT: {
    title: 'Analysis took too long',
    cause: 'The time limit was reached before a reliable result was ready.',
    action: 'Adjust the outline.',
  },
  BATCH_OUTLIER: {
    title: 'Differs from the rest of the batch',
    cause: 'Its crop size, shape or tilt is unlike most images in this batch.',
    action: 'Adjust, or accept it.',
  },
  TOUCHING_ITEMS: {
    title: 'Touching. Check the split.',
    cause: 'Two items touch, so the cut between them may be in the wrong place.',
    action: 'Check the split.',
  },
  ITEMS_TOO_CLOSE: {
    title: 'Items very close',
    cause: "The gap between items is under 1.5% of the scan's shorter side.",
    action: 'Check the split.',
  },
  OVERLAPPING_ITEMS: {
    title: 'Items overlap. Check the split.',
    cause: 'Items lie partly on top of each other and cannot be separated reliably.',
    action: 'Check the split.',
  },
  SPLIT_UNSTABLE: {
    title: 'Split changes with small settings changes',
    cause: 'Slightly different settings give a different number of items.',
    action: 'Check the split.',
  },
  LOW_CONTRAST_EDGE: {
    title: 'Edge hard to see',
    cause: 'The item is close in colour to the scanner bed or desk.',
    action: 'Adjust the outline.',
  },
  ODD_ASPECT: {
    title: 'Unusual shape for a photo',
    cause: 'The item is much longer than a photo; it may be a receipt or a false find.',
    action: 'Adjust, or accept it.',
  },
  TOO_MANY_ITEMS: {
    title: 'Too many items, showing the largest 32',
    cause: 'More than 32 items were found; the smaller ones are ignored.',
    action: 'Add an item.',
  },
  BED_UNCERTAIN: {
    title: 'Scan background unclear. Check the items.',
    cause: 'The scanner bed colour could not be measured reliably.',
    action: 'Treat as one item.',
  },
  FADED_PRINT: {
    title: 'Faint print detected',
    cause: 'The print is very light against the paper. Check amounts.',
    action: 'Show the original.',
  },
  INK_COVERAGE: {
    title: 'Text may look too light or too heavy',
    cause: 'The amount of ink after enhancement differs a lot from a plain conversion.',
    action: 'Show the original.',
  },
  LOST_MARKS: {
    title: 'Small marks may be missing',
    cause: 'Small dark marks, such as decimal points, are absent from the result. Check amounts.',
    action: 'Show the original.',
  },
  ESTIMATE_UNRELIABLE: {
    title: 'Paper could not be measured reliably',
    cause: 'Most of the page was too dark or too busy to estimate the paper tone.',
    action: 'Show the original.',
  },
  CURVED_PAGE_SUSPECTED: {
    title: 'This page looks curved',
    cause: 'The page edge or the text lines bend.',
    action: 'Not available yet.',
  },
  DEWARP_UNCERTAIN: {
    title: 'Flattening is uncertain',
    cause: 'The flattened result could not be fully verified.',
    action: 'Compare with the original.',
  },
};

/** Used when a tier is Check or Failed but the engine sent no reason, or an unknown one. */
export const FALLBACK_HOLD: Record<'check' | 'failed', HoldCopy> = {
  check: {
    title: 'Not sure about this crop',
    cause: 'The detector is less confident than the current strictness allows.',
    action: 'Review the outline, then accept it.',
  },
  failed: {
    title: 'Low confidence. Check the crop.',
    cause: 'The detector is not confident enough, so nothing was changed.',
    action: 'Draw crop, or skip it.',
  },
};

const SIDE_WORD: Record<Side, string> = { top: 'top', right: 'right', bottom: 'bottom', left: 'left' };

export function holdTitle(reason: Reason): string {
  const entry = HOLD[reason.code];
  if (!entry) return 'Held for review';
  return entry.title.replace('{side}', reason.side ? SIDE_WORD[reason.side] : 'one');
}

export function holdCause(reason: Reason): string {
  return HOLD[reason.code]?.cause ?? 'Auto Crop wants a person to look at this one.';
}

export function holdAction(reason: Reason): string {
  return HOLD[reason.code]?.action ?? 'Review the outline.';
}

/** Typed error messages (PLAN 6.7 and 2.10). Complete by construction: the Record is keyed by `ErrorCode`. */
export const ERRORS: Record<ErrorCode, string> = {
  CORRUPT: "This file looks damaged, so it couldn't be read.",
  UNSUPPORTED_FORMAT: "This file type isn't supported by this version of Auto Crop.",
  UNSUPPORTED_FEATURE: "This picture uses a feature Auto Crop can't read yet, so it was left as it is.",
  TOO_LARGE: 'This picture is larger than the 100 megapixel limit.',
  UNREADABLE: "Auto Crop couldn't read this file. Check that it still exists and that you can open it.",
  READ_ONLY: 'This file or its folder is read-only. Try Save as copy.',
  CLOUD_NOT_LOCAL: "This file is stored in the cloud and isn't on this computer yet. Make it available offline, then try again.",
  HEVC_DECODER_MISSING: "This HEIC file needs the HEVC decoder, which isn't installed, so it can't be opened.",
  DECODER_CRASHED: 'Reading this file crashed the safe reader, so it was skipped. The file is unchanged.',
  DECODE_TIMEOUT: 'Reading this file took too long, so it was skipped.',
  MEMORY_LIMIT: 'This picture needs more memory than Auto Crop is allowed to use, so it was skipped.',
  MODEL_LOAD_FAILED: "A detection model couldn't be loaded, so only the basic method ran.",
  OUT_OF_MEMORY: 'The computer ran out of memory. Close other programs and try again.',
  INTERNAL_PANIC: 'Something went wrong inside Auto Crop while working on this picture. The original is unchanged.',
  DEGENERATE: "That outline can't be turned into a picture. Move the corners apart and try again.",
  NO_CROP: 'This image has no crop yet, so it was left as it is. Draw a crop first.',
  ENCODE_FAILED: "Couldn't write the result. The original is unchanged.",
  UNSUPPORTED_OUTPUT: "This result can't be written in this format. Try Save as copy.",
  SOURCE_CHANGED:
    'This file changed while Auto Crop was working, so it was left as it is. Its backup is kept.',
  FILE_IN_USE: 'Another program is using this file, so it was left as it is. Close it and try again.',
  DISK_FULL: "There isn't enough free space to save safely. Nothing was replaced.",
  VERIFY_FAILED: "Couldn't verify the new file. The original is unchanged.",
  BACKUP_FAILED: "Can't back up this original, so it was not replaced.",
  ORIGINAL_EXPIRED: 'The backup of this original has expired and is no longer available.',
  // Multi-item scans (M10). Plain language: what happened to the files, then what to do.
  PLAN_STALE:
    'A file name this save planned was taken in the meantime, so nothing was written. Press Save again and Auto Crop will pick free names.',
  GROUP_COMMIT_FAILED:
    "The files of this split couldn't be written as a complete set, so nothing was changed. The scan is untouched.",
  SAVED_SOURCE_IN_USE:
    "The new files are saved, but the scan couldn't be removed because another program is using it. The set is complete; close the program and delete the scan yourself, or leave it.",
  HELD_FOR_REVIEW:
    'This scan is held for review, so nothing was written. Check the items and press Accept split, or use Save as copy.',
  ITEM_OP: "That change to the items isn't possible here. Nothing was changed.",
  NOT_REPLACEABLE: 'This file is never replaced in place. Use Save as copy.',
  SCHEMA_TOO_NEW: "This was saved by a newer version of Auto Crop, and this version can't read it.",
  CANCELLED: 'This was cancelled, so nothing was changed.',
  DEADLINE_EXCEEDED: 'This took too long, so it was stopped. Nothing was changed.',
  INTERNAL: 'Something went wrong inside Auto Crop. The original is unchanged.',
};

export function errorMessage(code: ErrorCode | null | undefined): string {
  return code ? (ERRORS[code] ?? ERRORS.INTERNAL) : ERRORS.INTERNAL;
}

/**
 * One-line notices that come with a result (`SaveOutcome.notices`, `ItemView.openOnly`). The code says what
 * happened, the line says what it means for the files and what to do.
 */
export const NOTICES: Record<string, string> = {
  'tiff.multi_page':
    'This TIFF has more than one page, so it is never replaced. Save as copy writes the first page as a PNG and leaves the file as it is.',
  'format.write_unavailable':
    "Auto Crop can open this kind of file but can't write it back, so it is never replaced. Save as copy writes a PNG (a JPG for HEIC) and leaves the file as it is.",
  'split.held':
    'This scan is held for review, so nothing was written. Accept the split to replace the original, or use Save as copy.',
  'derived.user_edited':
    'You edited a file from an earlier save, so it was left alone and this set was saved under a new name.',
  'curved.held':
    'This curved page is held for review, so nothing was written. Accept the page to replace the original, or use Save as copy.',
  'jpeg.lossless':
    'Saved without re-compressing the picture: the crop was cut on the JPEG blocks, so no quality was lost.',
  'sync.root':
    'This folder is kept in sync by a cloud service, which can upload a half-written file or undo the swap. Save as copy is safer here.',
  'convert.same_format': 'This file is already in the target format, so there was nothing to convert.',
  'enhance.icc_dropped':
    'This look is saved as a plain grey picture, so the colour profile of the original was not kept. It looks the same on ordinary screens.',
  'enhance.png_recommended':
    'A black-and-white look keeps faint marks best as a PNG. This JPG was saved at high quality; a PNG would be exact.',
  // Convert (crates/engine/src/convert.rs, crates/codecs): what happened to the picture itself. The five codes that
  // stop a replacement (tiff.multi_page ... cmyk.naive_conversion) read differently when the file was NOT converted:
  // CONVERT_SKIPPED_NOTICES below. `tiff.multi_page` above is the Save text; Convert uses `convertNoticeText`.
  'alpha.flattened': 'Transparency was filled with white.',
  'alpha.dropped': 'Transparency was removed.',
  'depth.reduced_to_8': 'Extra colour depth was reduced to the usual 8 bits per colour.',
  'colour.converted_to_srgb': 'Colours were converted to sRGB.',
  'colour.profile_unusable': 'The colour profile in this file could not be used, so its colours were kept as they are.',
  'colour.nclx_not_srgb': 'This picture uses colours outside sRGB, so it may look a little different in some programs.',
  'hdr.not_tonemapped': 'This is an HDR picture. It was converted as it is, so it may look flatter than on your phone.',
  'heic.multi_image': 'Only the main image of this file was converted.',
  'heic.sequence': 'This file holds a sequence of images. Only the first one was converted.',
  'anim.first_frame_only': 'This picture is animated. Only its first frame was converted.',
  'cmyk.naive_conversion': 'This is a CMYK picture. Its colours were converted without its colour profile, so they may look different.',
  'livephoto.video_untouched': 'The Live Photo video next to it was not touched.',
  'heif.unavailable': 'This build of Auto Crop has no HEIC decoder. The HEIC package of Auto Crop has one.',
  'icc.invalid': 'The colour profile in this file was damaged, so it was ignored.',
};

/** The Convert wording for the sources a replacement would damage, when the file was left as it is. */
export const CONVERT_SKIPPED_NOTICES: Record<string, string> = {
  'tiff.multi_page': 'This TIFF has several pages. Replacing it would lose all but the first.',
  'anim.first_frame_only': 'This picture is animated. Replacing it would keep only the first frame.',
  'heic.multi_image': 'This file holds several images. Replacing it would keep only the main one.',
  'heic.sequence': 'This file holds a sequence of images. Replacing it would keep only the first.',
  'cmyk.naive_conversion': 'This is a CMYK picture. Its colours cannot be converted accurately, so it was not replaced.',
};

/** Convert wording that differs from the Save wording of the same code. */
const CONVERT_NOTICES: Record<string, string> = {
  'tiff.multi_page': 'This TIFF has several pages. Only the first page was converted.',
  'sync.root': 'This folder syncs to the cloud. A sync program can upload a half-written file or undo the swap, so look at the result.',
};

const GENERIC_NOTICE = 'Auto Crop added a note to this result.';

export function noticeText(code: string): string {
  return NOTICES[code] ?? GENERIC_NOTICE;
}

/** A notice on a Convert result. `skipped` picks the wording for a file that was left as it is. Error codes (a note such as SOURCE_CHANGED) read as their error message. */
export function convertNoticeText(code: string, status: 'converted' | 'skipped' | 'failed' = 'converted'): string {
  if (status === 'skipped' && code in CONVERT_SKIPPED_NOTICES) return CONVERT_SKIPPED_NOTICES[code];
  if (code in CONVERT_NOTICES) return CONVERT_NOTICES[code];
  if (code in ERRORS) return ERRORS[code as ErrorCode];
  return noticeText(code);
}

/** Convert-specific sentences for the skip codes (and the error codes whose general text talks about Save as copy). */
const CONVERT_CODES: Record<string, string> = {
  SAME_FORMAT: 'Already in the chosen format, so it was left alone.',
  NOT_REPLACEABLE: 'Left as it is. Choose “Keep originals, save beside them” to convert it anyway.',
  HEIF_UNAVAILABLE: 'This build of Auto Crop has no HEIC decoder, so this file was not converted. The HEIC package of Auto Crop has one.',
  UNSUPPORTED_FORMAT: "Auto Crop can't convert this kind of file.",
  CANCELLED: 'Stopped before this file was converted. It is unchanged.',
  READ_ONLY: 'This file or its folder is read-only. Choose “Save to a folder” to convert it anyway.',
  UNSUPPORTED_OUTPUT: "This picture can't be written in the chosen format. It is unchanged.",
  HELD_FOR_REVIEW: 'This file was held back, so nothing was written. It is unchanged.',
};

/** Why a file was skipped or failed in a Convert run or preview, from the code the engine sent. */
export function convertCodeText(code: string | null | undefined): string {
  if (!code) return ERRORS.INTERNAL;
  return CONVERT_CODES[code] ?? (code in ERRORS ? ERRORS[code as ErrorCode] : ERRORS.INTERNAL);
}

/** The same, for the preview's list of files that cannot be converted: shorter, because the HEIC note above it says the rest. */
export function convertCannotText(code: string): string {
  return code === 'HEIF_UNAVAILABLE' ? 'Needs the HEIC decoder, which this build does not have.' : convertCodeText(code);
}

const FORMAT_NAMES: Record<string, string> = {
  jpeg: 'JPG',
  png: 'PNG',
  tiff: 'TIFF',
  webp: 'WebP',
  heic: 'HEIC',
  avif: 'AVIF',
  gif: 'GIF',
  bmp: 'BMP',
  jxl: 'JPEG XL',
  unknown: 'Unknown',
};

/** "heic" -> "HEIC", "jpeg" -> "JPG": a format name as the person knows it. */
export function formatName(format: string): string {
  return FORMAT_NAMES[format] ?? format.toUpperCase();
}

/** The short form shown on a badge: why a source is open-only. */
export function openOnlyShort(code: string | null): string {
  return code === 'tiff.multi_page' ? 'Multi-page: copy only' : code ? 'Open only: copy only' : '';
}

/** "JPG, PNG, WebP, TIFF and HEIC": the formats this build opens, from the extension list the shell reports. */
export function formatList(exts: readonly string[] | undefined): string {
  const order: [string, string][] = [
    ['jpg', 'JPG'],
    ['png', 'PNG'],
    ['webp', 'WebP'],
    ['tif', 'TIFF'],
    ['heic', 'HEIC'],
    ['avif', 'AVIF'],
  ];
  const have = new Set(exts ?? ['jpg', 'jpeg', 'png']);
  const names = order.filter(([e]) => have.has(e) || (e === 'tif' && have.has('tiff')) || (e === 'heic' && have.has('heif'))).map(([, n]) => n);
  if (names.length <= 1) return names.join('');
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/** Retention choices (days, or null for never), shared by Settings and Backups. */
export const RETENTION_OPTIONS: { value: number | null; label: string }[] = [
  { value: 7, label: '7 days' },
  { value: 30, label: '30 days' },
  { value: 90, label: '90 days' },
  { value: 365, label: '365 days' },
  { value: null, label: 'Never delete' },
];

export function retentionText(days: number | null): string {
  return days === null ? 'until you delete them' : `${days} days`;
}

/** "top", "top and right", "top, right and bottom": the lower-case names of edges, for a sentence. */
export function sideList(sides: readonly Side[]): string {
  if (sides.length <= 1) return sides[0] ?? '';
  return `${sides.slice(0, -1).join(', ')} and ${sides[sides.length - 1]}`;
}

export const S = {
  appName: 'Auto Crop',
  nav: {
    home: 'Home',
    backups: 'Backups',
    settings: 'Settings',
    backToHome: 'Back to Home',
    backToGrid: 'Grid',
    close: 'Close',
  },
  tier: { good: 'Good', check: 'Check', failed: 'Failed', analysing: 'Analysing' },
  home: {
    title: 'Start a batch',
    dropTitle: 'Drop photos, scans or a folder here',
    dropSub: 'or choose them below',
    formats: (exts: readonly string[] | undefined) => formatList(exts).replace(/, /g, ' · ').replace(' and ', ' · '),
    formatsNote: (exts: readonly string[] | undefined) => {
      const have = new Set(exts ?? []);
      const openOnly = ['webp', 'tif', 'heic', 'avif'].some((e) => have.has(e));
      return openOnly
        ? 'WebP, TIFF and HEIC files can be opened but not written back: they are saved as copies and never replaced.'
        : 'Other formats, such as WebP, TIFF and HEIC, need a newer build.';
    },
    openFiles: 'Open files',
    openFolder: 'Open folder',
    subfolders: 'Include subfolders',
    trySamples: 'Try sample images',
    samplesNote: 'New here? Samples are synthetic and include one hard image.',
    resumeTitle: 'Current batch',
    resumeNote: (n: number, need: number) =>
      `${n} ${n === 1 ? 'image' : 'images'} · ${need} still need review`,
    resume: 'Resume',
    saving: 'Saving',
    howSavingWorks: 'How saving works',
    saveAsCopy: 'Save as copy',
    copyOnLine: 'Save copies to a new AutoCrop folder, originals untouched',
    replaceLine: (days: number | null) =>
      days === null ? 'Replace originals, backups kept until you delete them' : `Replace originals, backup kept ${days} days`,
    copyOnCaption: 'On: results go to <source folder>/AutoCrop/.',
    copyOffCaption: 'Off: originals are replaced after a verified backup.',
    howBody: (days: number | null) =>
      `Auto Crop replaces your originals with the corrected version. Each original is copied to Backups first and kept ${
        days === null ? 'until you delete it' : `for ${days} days`
      }; you can restore it even after closing the app.`,
    gotIt: 'Got it',
    saveCopiesInstead: 'Save copies instead',
    whereBackups: 'Where are backups?',
    offline: 'Everything stays on this computer. Auto Crop never connects to the internet unless you ask it to.',
  },
  grid: {
    needReview: (n: number) => `${n} need review`,
    confident: (n: number) => `${n} confident`,
    savedNote: ', saved when you press Save all',
    analysing: (done: number, total: number) => `Analysing ${done} of ${total}`,
    strictness: 'Strictness',
    strictnessCaption: 'More results are saved without review.',
    strictnessSavedCaption: 'Saved items stay as they are. A change affects only unwritten items.',
    strict: 'Strict',
    balanced: 'Balanced',
    aggressive: 'Aggressive',
    experimental: 'experimental',
    filters: {
      needs: 'Needs review',
      all: 'All',
      edited: 'Edited',
      skipped: 'Skipped',
      failed: 'Failed',
      saved: 'Saved',
    },
    filterLabel: 'Filter',
    sort: 'Sort',
    sortConfidence: 'Confidence, lowest first',
    sortName: 'Name',
    size: 'Size',
    selectShown: 'Select shown',
    selected: (n: number) => `${n} selected`,
    accept: 'Accept',
    skip: 'Skip',
    unskip: 'Put back',
    removeFromList: 'Remove from list',
    clear: 'Clear',
    confirmAccept: (n: number) => `Accept ${n} ${n === 1 ? 'item' : 'items'} you have not reviewed?`,
    yesAccept: 'Yes, accept',
    cancel: 'Cancel',
    drawCrop: 'Draw crop',
    undoDecision: 'Undo last decision',
    nothingToUndo: 'Nothing to undo',
    nothingToRedo: 'Nothing to redo',
    saveAll: 'Save all',
    willSave: (n: number) => `Will save ${n}.`,
    flaggedStay: (n: number) => `${n} flagged stay untouched.`,
    reviewFlaggedFirst: 'Review flagged first',
    saveN: (n: number) => `Save ${n}`,
    nothingToSave: 'Nothing to save yet. Good results and accepted items are saved.',
    savedSummary: (saved: number, skipped: number, failed: number) =>
      `Saved ${saved} · Skipped ${skipped} · Failed ${failed}`,
    savedCopiesSummary: (saved: number, skipped: number, failed: number) =>
      `Saved ${saved} copies · Skipped ${skipped} · Failed ${failed}`,
    restoreAllOriginals: 'Restore all originals',
    openBackups: 'Open Backups',
    retryFailed: 'Retry failed',
    dismiss: 'Dismiss',
    backedUp: 'Originals backed up · Restore anytime',
    empty: {
      needs: 'Nothing needs review.',
      all: 'No images yet. Go Home to add some.',
      edited: 'Nothing edited.',
      skipped: 'Nothing skipped.',
      failed: 'No failed items.',
      saved: 'Nothing saved yet.',
    },
    heldFooter: (n: number) => `${n} held for review.`,
    heldNote: 'Their originals are untouched until you accept or edit them.',
    nothingHeld: (n: number) => `Nothing held. All ${n} resolved.`,
    nothingHeldNote: 'Press Save all to finish.',
    reviewFlagged: 'Review flagged',
    noItemsTitle: 'No images in this batch',
    noItemsNote: 'Open some files or try the sample images from Home.',
    meta: { skipped: 'Skipped', accepted: 'Accepted', edited: 'Edited', saved: 'Saved', analysing: 'Analysing…' },
    goodAlready: 'Good results are accepted already. Only flagged items need your OK.',
    failedNeedCrop: 'Failed items need a crop drawn first, so they were not accepted.',
    accepted: (n: number) => `Accepted ${n}.`,
    skipped: (n: number) => `Skipped ${n}.`,
    removed: (n: number) => `Removed ${n} from this batch. Files on disk are untouched.`,
    tileStatus: {
      needsReview: 'needs review',
      good: 'good',
      failed: 'failed',
      accepted: 'accepted',
      skipped: 'skipped',
      edited: 'edited',
      saved: 'saved',
      notSaved: 'not saved',
      analysing: 'still analysing',
      errored: 'could not be read',
    },
    selectItem: (name: string) => `Select ${name}`,
    openItem: (name: string) => `Open ${name}`,
    // Scans with several items and open-only sources (M10.42)
    splitSelected: 'Split into items',
    oneSelected: 'Treat as one item',
    sessionUndone: (label: string) => `Undid: ${label}.`,
    sessionRedone: (label: string) => `Redid: ${label}.`,
    sessionDone: (label: string) => label,
    undoSession: 'Undo',
    redoSession: 'Redo',
    splitFailed: (n: number) => `${n} ${n === 1 ? 'scan' : 'scans'} could not be changed.`,
    savedFiles: (files: number, scans: number) => `${files} ${files === 1 ? 'file' : 'files'} from ${scans} ${scans === 1 ? 'scan' : 'scans'}`,
    heldScans: (n: number) => `${n} ${n === 1 ? 'scan' : 'scans'} with several items stay held until you accept ${n === 1 ? 'it' : 'them'}.`,
    openOnlyBadge: 'Open only',
    openOnlyTitle: 'This format is open-only: Save as copy.',
    notReplaced: (n: number) => `${n} open-only ${n === 1 ? 'file was' : 'files were'} not replaced. Use Save as copy.`,
    openOnlyStay: (n: number) => `${n} open-only ${n === 1 ? 'file stays' : 'files stay'} as ${n === 1 ? 'it is' : 'they are'}: use Save as copy for ${n === 1 ? 'it' : 'them'}.`,
    willSaveCopies: (n: number) => `${n} open-only ${n === 1 ? 'file' : 'files'} will be saved as copies.`,
    noticesTitle: 'Notes',
  },
  firstWrite: {
    title: 'Before the first save',
    bodyReplace: (n: number) =>
      `Auto Crop will replace ${n} ${n === 1 ? 'original' : 'originals'} with the corrected versions. Each original is copied to Backups and checked first.`,
    bodyCopy: (n: number) =>
      `Auto Crop will save ${n} corrected ${n === 1 ? 'copy' : 'copies'} next to your originals. The originals are not touched.`,
    mode: 'Mode',
    modeReplace: 'Replace originals',
    modeCopy: 'Save copies',
    keptFor: 'Kept for',
    keptForValue: (days: number | null) =>
      days === null ? 'until you delete them, restorable after closing the app' : `${days} days, restorable after closing the app`,
    free: 'Backup space',
    freeValue: (free: string) => `${free} free`,
    replace: 'Replace originals',
    copies: 'Save copies instead',
    replaceInstead: 'Replace originals instead',
    saveCopies: 'Save copies',
    cancel: 'Cancel',
  },
  editor: {
    heading: (name: string, pos: number, total: number, status: string) =>
      `Editor: ${name}, ${pos} of ${total}, ${status}`,
    positionOf: (pos: number, total: number) => `${pos} of ${total}`,
    undo: 'Undo',
    redo: 'Redo',
    nothingToUndo: 'Nothing to undo',
    nothingToRedo: 'Nothing to redo',
    compare: 'Compare',
    compareHint: 'Hold or click to show the original',
    accept: 'Accept',
    accepted: 'Accepted',
    acceptAndNext: 'Accept & next',
    skip: 'Skip',
    unskip: 'Put back',
    resetToAuto: 'Reset to auto',
    drawCrop: 'Draw crop',
    zoomIn: 'Zoom in',
    zoomOut: 'Zoom out',
    fit: 'Fit',
    result: 'Result',
    original: 'Original',
    source: 'Source',
    resultAlt: 'Result preview',
    sourceAlt: (name: string) => `${name} with crop outline`,
    cropGroup: 'Crop outline',
    handleRole: 'crop handle',
    moveRole: 'move handle',
    corners: ['Top-left corner', 'Top-right corner', 'Bottom-right corner', 'Bottom-left corner'],
    cornersShort: ['TL', 'TR', 'BR', 'BL'],
    edges: ['Top edge', 'Right edge', 'Bottom edge', 'Left edge'],
    grip: 'Move whole outline',
    handleLabel: (name: string, x: string, y: string) => `${name}, x ${x}%, y ${y}%`,
    cropHeading: 'CROP',
    cornerPositions: 'Corner positions (% of image)',
    cornerInput: (corner: string, axis: 'x' | 'y') => `${corner} ${axis}`,
    rotateHeading: 'ROTATE',
    rotateLeft90: 'Rotate 90 degrees left',
    rotateRight90: 'Rotate 90 degrees right',
    left90: '90 L',
    right90: '90 R',
    nudgeLeft: 'Rotate 0.1 degrees left',
    nudgeRight: 'Rotate 0.1 degrees right',
    rulerLabel: 'Fine rotation',
    rulerHint: 'Home resets to the detected angle',
    auto: 'Auto',
    autoAngle: 'Use the detected angle',
    autoMarker: 'Detected angle',
    angleSpoken: (deg: number) => `${deg < 0 ? 'minus ' : ''}${Math.abs(deg).toFixed(1)} degrees`,
    angleLabel: (deg: number) => `${deg < 0 ? '−' : ''}${Math.abs(deg).toFixed(1)}°`,
    previous: 'Previous item',
    next: 'Next item',
    firstItem: 'This is the first item',
    lastItem: 'This is the last item',
    editedChip: 'Edited',
    savedChip: 'Saved',
    skippedChip: 'Skipped',
    acceptedChip: 'Accepted',
    outlineAuto: 'Auto result',
    outlineEdited: 'Edited by you',
    whyHeading: 'WHY IT WAS HELD',
    whyNone: 'Nothing to flag. This crop is confident.',
    action: 'What to do',
    editingFromOriginal: 'Editing from the original. Nothing is written until you press Save all in the grid.',
    help: 'Drag a handle or use arrow keys · Shift moves 10 px',
    savedState: (name: string, copy: boolean) => (copy ? `Saved as copy: ${name}` : `Saved as ${name}`),
    dirtySinceSave: 'Changed since saved. The file on disk is unchanged until you save again.',
    restoreOriginal: 'Restore original',
    failedBannerTitle: "Couldn't find the edges. The original is untouched.",
    failedBannerBody:
      'No page outline was found with enough confidence. Nothing is written to this file unless you draw a crop.',
    failedDrawNote: 'Draw crop places an editable outline 5% in from the edges.',
    analysing: 'Still analysing this image…',
    loadError: 'This image could not be shown.',
    undone: (label: string) => `Undid ${label}`,
    redone: (label: string) => `Redid ${label}`,
    undoSavedToast: 'Edit undone. The file on disk is unchanged until you save.',
    restoreOriginalFile: 'Restore original file',
    noMoreFlagged: 'No more flagged items. Back to the grid.',
    skipped: 'Skipped. You can put it back from the Skipped filter.',
    acceptedToast: 'Accepted.',
    cannotAcceptFailed: 'Draw a crop first, then accept.',
    editFailed: "That change couldn't be applied. The previous outline was kept.",
    labels: {
      moveCorner: 'Move corner',
      moveEdge: 'Move edge',
      moveOutline: 'Move outline',
      editCorner: 'Edit corner',
      rotate: 'Rotate',
      rotate90: 'Rotate 90',
      autoAngle: 'Auto angle',
    },
    notFound: 'This image is no longer in the batch.',
    shortcuts: 'Shortcuts',
  },
  // Items on a scan (M10.35 to M10.45): overlay, chip bar, tools, banner.
  items: {
    heading: 'ITEMS',
    barLabel: 'Items on this scan',
    itemN: (n: number) => `Item ${n}`,
    removedWord: 'removed',
    removedHeading: (n: number) => `Removed (${n})`,
    selectedOf: (n: number, total: number) => `Item ${n} of ${total}`,
    selectedNone: 'No item selected',
    overlayLabel: 'Items on the scan',
    ghostLabel: (n: number) => `Removed candidate ${n}`,
    addAsItem: 'Add as item',
    addAsItemFor: (n: number) => `Add removed candidate ${n} as an item`,
    chipHelp: 'Arrow keys move between items. Enter selects. Shift+F10 opens the menu. Alt+Left and Alt+Right reorder.',
    grip: 'Drag to reorder',
    gripFor: (n: number) => `Reorder item ${n}: drag, or use Move earlier and Move later`,
    menu: 'Item menu',
    menuFor: (n: number) => `Menu for item ${n}`,
    menuButton: 'Item actions',
    remove: 'Remove',
    removeHelp: 'Leaves it out of the saved files. You can restore it.',
    restore: 'Restore',
    resetToAuto: 'Reset to auto',
    resetAll: 'Reset all items to auto',
    resetHelp: 'Back to what Auto Crop found for this item.',
    revertSession: 'Undo my changes to this item',
    earlier: 'Earlier',
    later: 'Later',
    moveEarlier: 'Move earlier',
    moveLater: 'Move later',
    mergeWithNext: 'Merge with next',
    cut: 'Cut…',
    turnLeft: 'Turn left',
    turnRight: 'Turn right',
    flip: 'Mirror',
    readingOrder: 'Number in reading order',
    orderManual: 'Numbered by hand',
    orderReading: 'Numbered left to right, top to bottom',
    // tools
    toolsLabel: 'Item tools',
    addItem: 'Add item',
    addTap: 'Tap a missed item',
    addDraw: 'Draw a box',
    addInset: 'Add a box to adjust',
    addTapHint: 'Tap the item on the picture. Auto Crop finds its edges. Esc cancels.',
    addDrawHint: 'Drag a box around the item. Esc cancels.',
    addTapNothing: 'Nothing found there, so a box was placed around the point. Adjust its corners.',
    merge: 'Merge…',
    mergeHint: (n: number) => (n < 2 ? 'Tap two or more items to merge. Esc cancels.' : `Merge ${n} items into one. Check the outline, then Confirm.`),
    mergeConfirm: (n: number) => `Merge ${n} items`,
    mergeRefused: 'Those items cannot be merged. Removed items cannot be merged.',
    cutTitle: (n: number) => `Cut item ${n}`,
    cutHint: 'Drag a line across the item, or use the controls. Esc cancels.',
    cutAxisLabel: 'Cut direction',
    cutVertical: 'Up and down',
    cutHorizontal: 'Across',
    cutPosition: 'Position',
    cutTilt: 'Tilt',
    cutHalves: 'Split in halves',
    cutConfirm: 'Cut',
    cutTooSmall: 'A piece would be smaller than 2% of the scan. Move the cut.',
    cutBad: 'That cut does not cross the item.',
    cancel: 'Cancel',
    // announcements
    added: (n: number) => `Added item ${n}.`,
    removed: (n: number) => `Removed item ${n}.`,
    restored: (n: number) => `Restored as item ${n}.`,
    merged: (n: number) => `Merged into item ${n}.`,
    cutDone: 'Cut into two items.',
    moved: (n: number, of: number) => `Item moved to place ${n} of ${of}.`,
    selected: (n: number, band: string) => `Item ${n} selected, ${band}.`,
    opFailed: "That change couldn't be applied. Nothing was changed.",
    // inspector
    itemSettings: 'ITEMS',
    policyLabel: 'Items',
    policyAuto: 'Auto',
    policyNever: 'Treat as one item',
    policyAlways: 'Split into items',
    profileLabel: 'The items are',
    profilePhotos: 'Photos',
    profileReceipts: 'Receipts',
    policyDone: (label: string) => `${label}.`,
    why: (n: number) => `WHY ITEM ${n} WAS HELD`,
    noItemSelected: 'Select an item to see its outline and why it was held.',
    names: 'Files',
    namesNote: (names: string) => `Saved as ${names}`,
    namesOne: (name: string) => `Saved as ${name}`,
    reviewed: 'You placed or adjusted this item, so it counts as reviewed.',
    // chip bar caption
    count:(n: number) => (n === 1 ? '1 item' : `${n} items`),
  },
  // The scan-level banner (M10.41): shown in the editor and in the review grid.
  split: {
    held: (n: number) => `${n} items found: held for review`,
    heldNeed: (need: number) => (need === 1 ? '1 item needs a look.' : `${need} items need a look.`),
    heldNote: 'Nothing is written until you accept the split. You can always Save as copy.',
    ready: (n: number) => `${n} items found`,
    readyNote: 'They all look good, but a split replaces one file with several, so it waits for your OK.',
    accepted: (n: number) => `${n} items accepted`,
    acceptedNote: 'This split can be saved. Any change to the items asks for your OK again.',
    auto: (n: number) => `${n} items found: saved without asking`,
    autoNote: 'Auto-save splits (Experimental) is on and every item looks good.',
    noItems: 'No items found',
    noItemsNote: 'Nothing was found on this scan. Draw the items yourself, or treat the scan as one item.',
    accept: 'Accept split',
    acceptShort: 'Accept',
    acceptAndNext: 'Accept split & next',
    withdraw: 'Withdraw OK',
    review: 'Review items',
    drawItems: 'Draw items',
    treatAsOne: 'Treat as one item',
    skip: 'Skip',
    acceptedAnnounce: 'Split accepted.',
    withdrawnAnnounce: 'Acceptance withdrawn.',
    countBadge: (n: number) => `x${n}`,
    countBadgeLabel: (n: number) => `${n} items`,
    tileHeld: (n: number) => `${n} items: held for review`,
    tileAccepted: (n: number) => `${n} items: accepted`,
    tileReady: (n: number) => `${n} items found`,
    expand: 'Show items',
    collapse: 'Hide items',
    subtiles: 'Items on this scan',
    strictNote: 'Split scans always use the Strict setting.',
  },
  // Curved pages: four editable edges, flattened by the engine (docs/dev/curved-pages.md).
  curved: {
    modeLabel: 'Page shape',
    straight: 'Straight',
    curved: 'Curved',
    straightHint: 'Four straight edges: a normal crop.',
    curvedHint: 'Bend the edges of a page that is curved, like a crumpled receipt.',
    toolbarLabel: 'Curved edges',
    // the picture
    viewLabel: 'Result view',
    flattened: 'Flattened',
    straightCrop: 'Straight crop',
    viewFlattenedHint: 'The page flattened from its curved edges',
    viewStraightHint: 'The same corners with straight edges, for comparison',
    drawing: 'Drawing…',
    previewWord: 'Preview',
    renderedWord: 'Rendered',
    flattenedAlt: 'Flattened page preview',
    straightAlt: 'Straight crop preview',
    // handles
    handleRole: 'curve handle',
    pointLabel: (edge: string, n: number, of: number, x: string, y: string) => `${edge}, point ${n} of ${of}, x ${x}%, y ${y}%`,
    ghostLabel: (edge: string, x: string, y: string) => `${edge}, middle, not yet a point: drag to bend this edge, x ${x}%, y ${y}%`,
    group: 'Curved edge handles',
    // actions
    addPoint: 'Add point',
    addPointHint: 'Adds a point on the selected edge. Double-click or press and hold on an edge does the same.',
    removePoint: 'Remove point',
    removeShort: 'Remove',
    straightenShort: 'Straighten',
    resetShort: 'Reset',
    removePointHint: 'Removes the selected point (Delete). An edge keeps at least its two corners.',
    straightenEdge: 'Straighten edge',
    straightenEdgeHint: 'Takes the selected edge back to a straight line.',
    resetCurves: 'Reset curves',
    resetCurvesHint: 'All four edges straight again, corners kept.',
    backToStraight: 'Back to straight',
    backToStraightHint: 'Leaves curved mode. The page becomes a plain crop again; Undo brings the curves back.',
    selectedPoint: 'Selected point (% of image)',
    noSelection: 'Select a handle (Tab, then [ and ] to walk the handles) or double-click an edge to add a point.',
    keysHelp: 'Arrows nudge 1 px, Shift 10 px, Alt a fraction of a pixel. [ and ] walk the handles, Delete removes a point, Esc deselects.',
    help: 'Drag the middle of an edge to bend it. Double-click an edge to add a point.',
    // locked while curved
    locked: 'Locked while the edges are curved',
    angleLocked: 'A curved page has no fine angle: its curves already say how it lies. Use Back to straight to rotate by degrees.',
    mergeCutLocked: 'A curved page cannot be merged or cut. Use Back to straight first.',
    itemOp: 'That change is not possible on a curved page, so nothing was changed. Back to straight turns it into a plain crop again.',
    cornersNote: 'Moving a corner carries the two edges that meet there with it.',
    // honest limits
    limits: 'Fixes bowed edges and perspective. Wrinkles inside the page stay.',
    limitsMore: 'Only what the four edges show can be corrected: a page bent about a line whose edges stay straight is not flattened.',
    // banner and review
    held: 'Curved page: review, then accept',
    heldNote: 'The page is flattened from the edges you set. Check the flattened result, then accept. The original is not replaced until you do; Save as copy is always possible.',
    accepted: 'Curved page accepted',
    acceptedNote: 'This page can be saved. Any change to its edges asks for your OK again.',
    accept: 'Accept',
    withdraw: 'Withdraw OK',
    tileHeld: 'Curved page: held for review',
    tileAccepted: 'Curved page: accepted',
    reasonLine: 'Curved page: review, then accept',
    whyTitle: 'Curved page',
    whyCause: 'A page flattened from edges placed by hand is always held for review, so a result you have not looked at is never written over the original.',
    whyAction: 'Look at the flattened result, then press Accept, or use Back to straight.',
    curvedBadge: 'Curved',
    acceptedAnnounce: 'Curved page accepted.',
    withdrawnAnnounce: 'Acceptance withdrawn.',
    // history labels: what Undo and Redo name
    labels: {
      enter: 'Curve edges',
      leave: 'Straighten edges',
      moveCorner: 'Move corner',
      bendEdge: 'Bend edge',
      addPoint: 'Add point',
      removePoint: 'Remove point',
      straightenEdge: 'Straighten edge',
      resetCurves: 'Reset curves',
      editCorner: 'Edit corner',
    },
    // announcements
    entered: 'Curved edges on. Four straight edges to start: drag the handle in the middle of an edge to bend it.',
    left: 'Back to straight edges. Undo brings the curves back.',
    pointAdded: (edge: string, n: number) => `Point ${n} added on the ${edge.toLowerCase()}.`,
    pointRemoved: (edge: string) => `Point removed from the ${edge.toLowerCase()}.`,
    edgeStraightened: (edge: string) => `${edge} straight again.`,
    allStraight: 'All four edges straight again.',
    selected: (label: string) => `${label} selected.`,
    deselected: 'Handle deselected.',
    maxPoints: 'That edge already has the most points a curve can have (32).',
    minPoints: 'An edge keeps at least its two corners.',
    flattenedUpdated: 'Flattened preview updated.',
    previewFailed: 'The flattened preview could not be drawn for this shape.',
    // why a shape is refused (what the engine would answer with DEGENERATE)
    problems: {
      points: 'A curve can have 2 to 32 points.',
      nonFinite: 'A point has no position.',
      range: 'A point is too far outside the picture.',
      coincident: 'Two neighbouring points are in the same place. Move one of them.',
      corners: 'The corners of the edges do not meet.',
      noArea: 'The outline has no area.',
      crossing: 'The edges would cross each other, so that change was not applied.',
    } satisfies Record<CurveProblem, string>,
  },
  // Suggest curves: the engine proposes the four edges from the picture; the person checks, applies or dismisses
  // (docs/dev/curved-pages.md "Suggestion API"). The words are honest about what the fit does not know.
  suggest: {
    button: 'Suggest curves',
    buttonHint: 'Auto Crop proposes the curved edges from the picture. Nothing changes until you press Apply.',
    buttonHintStraight: 'Switches this page to Curved, then proposes the curved edges. Nothing else changes until you press Apply.',
    panelLabel: 'Suggested curves',
    loading: 'Looking for the edges…',
    ready: 'Suggested curves ready. Check the orange outline and the flattened page, then Apply or Dismiss.',
    nothing: 'Nothing to suggest for this page. Apply would leave the edges straight.',
    guess: 'This is a guess from the picture. Look at the edges before you apply it.',
    bendOf: (edges: Side[], pct: number) => {
      const names = sideList(edges);
      return `Bent: the ${names} ${edges.length === 1 ? 'edge' : 'edges'}, by up to ${pct.toFixed(1)}% of ${edges.length === 1 ? 'its' : 'their'} length.`;
    },
    // honesty fields of the answer
    noCurve: 'No clear curved edge found: the page may be flat',
    lowConfidence: (edges: Side[]) =>
      edges.length >= 4 ? 'Not sure about any of the edges: check the whole outline' : `Not sure about the ${sideList(edges)} ${edges.length === 1 ? 'edge: check it' : 'edges: check them'}`,
    nearlyStraight: (bentEdges: number) =>
      bentEdges > 1 ? 'These edges are nearly straight: the suggestion may not help' : 'This edge is nearly straight: the suggestion may not help',
    // actions
    apply: 'Apply',
    applyHint: 'Applies all four suggested edges as one step (Undo takes it back). The page stays held for review until you accept it.',
    dismiss: 'Dismiss',
    dismissHint: 'Throws the suggestion away. Nothing was changed. (Esc)',
    retry: 'Try again',
    label: 'Suggest curves',
    insetTitle: 'Suggested',
    insetAlt: 'Flattened page with the suggested curves',
    insetSub: 'Preview',
    // after
    applied: 'Suggested curves applied. Undo takes them back. The page is held for review: check the flattened result, then accept.',
    dismissed: 'Suggestion dismissed. Nothing was changed.',
    stale: 'The suggestion was dropped because the page changed. Ask again if you still want one.',
    asking: 'Looking for the edges.',
    notReady: 'The picture is not ready yet. Try again in a moment.',
    failed: 'Could not suggest curves for this page. Nothing was changed.',
    appliedFailed: 'The suggested curves could not be applied, so nothing was changed.',
    legendSuggested: 'Suggested outline',
    legendCurrent: 'Current outline',
  },
  // Trim to content: a picture inside a border of the page's own colour (a print on a wide white mat) has two outlines.
  // The detector keeps the paper edge (B14) and holds it; the engine proposes the picture, the person previews and applies
  // it as one undo step (docs/perf/detector-baseline.md, "The matted-print question").
  trim: {
    button: 'Trim to content',
    buttonHint: 'Crops to the picture inside the border. You see it first; nothing changes until you press Apply.',
    /** The undo step an applied trim makes (the engine's `TRIM_LABEL`, pinned by a test). */
    label: 'Trim to content',
    none: 'No picture inside to trim to',
    /** In the hold message of DETECTORS_DISAGREE, when a picture sits inside the border. */
    holdQuestion: 'A picture sits inside a border. Keep the whole page, or Trim to content?',
    keepPage: 'Keep the whole page',
    keepPageHint: 'Accepts the outline of the whole page as it is.',
    kept: 'Keeping the whole page.',
    panelLabel: 'Trim to content',
    ready: 'The dashed outline is the picture inside the border. Apply it to crop to the picture, or cancel.',
    why: {
      mat: 'A wide border surrounds the picture.',
      thinBorder: 'A thin border (the white edge of a print) surrounds the picture.',
    },
    apply: 'Apply',
    applyHint: 'Crops to the dashed outline as one step. Undo takes it back.',
    cancel: 'Cancel',
    cancelHint: 'Keeps the outline as it is. (Esc)',
    legendProposed: 'Picture inside the border',
    legendCurrent: 'Current outline',
    applied: 'Trimmed to the picture. Undo takes it back.',
    cancelled: 'Trim cancelled. The outline was not changed.',
    stale: 'The trim was dropped because the page changed.',
    failed: 'Could not trim this page. Nothing was changed.',
    curved: 'Trim to content works on straight pages. Go back to straight edges first.',
    available: 'A picture sits inside the border: Trim to content is available.',
    previewAlt: 'The page trimmed to the picture',
  },
  // Enhancement (PLAN 5, B13): suggested, never forced. The words say what each look does and where it falls short.
  enhance: {
    heading: 'Enhance',
    lead: 'Make a page easier to read. Nothing changes until you choose a look.',
    looksLabel: 'Look',
    looks: {
      original: { name: 'Original', line: 'Leave the picture as it is.' },
      clean: { name: 'Clean', line: 'Evens out the light and whitens the paper. Colour stays.' },
      document: { name: 'Document', line: 'Black ink on white paper. No colour, no shading.' },
      receipt: { name: 'Receipt', line: 'Black and white, tuned for faint printed text.' },
    },
    suggestedTag: 'Suggested',
    fitsTag: 'Fits this page',
    strength: 'Strength',
    strengthValue: (pct: number) => `${pct}%`,
    strengthHelp: 'How much of the look is used. 0% is the original.',
    strengthOff: 'Choose a look to set its strength.',
    // several items on one scan
    scopeLabel: 'Apply to',
    scopeThis: 'This item',
    scopeAll: 'All items in this scan',
    scopeAllNote: (n: number) => `Gives all ${n} items this look and replaces any look set on a single item. Undo takes it back as one step.`,
    onCount: (on: number, total: number) => (total === 1 ? (on === 1 ? 'Enhancement is on' : 'Enhancement is off') : `Enhancement is on for ${on} of ${total} items`),
    customised: 'This item has its own look.',
    followScan: 'Use the scan look again',
    followed: 'This item follows the scan look again.',
    // looking
    previewHeading: 'Preview',
    compareHold: 'Show original',
    compareHint: 'Press and hold to see the original. A quick tap keeps it showing.',
    splitView: 'Split view',
    splitSlider: 'Split position',
    splitHint: 'Original on the left, enhanced on the right.',
    capOriginal: 'Original',
    capEnhanced: 'Enhanced',
    capPreview: 'Preview, not applied',
    capLoading: 'Loading the preview',
    previewAlt: (look: string) => `Preview of this crop with the ${look} look`,
    originalAlt: 'This crop without enhancement',
    statusApplied: (look: string, pct: number) => `Showing ${look} at ${pct}%.`,
    statusOriginal: 'Showing the original.',
    statusPreview: (look: string, pct: number) => `Previewing ${look} at ${pct}%. Nothing has been changed yet.`,
    statusHeld: 'Showing the original while you hold the button.',
    // the suggestion
    bannerDoc: 'This looks like a document: Clean makes it easier to read',
    bannerReceipt: 'This looks like a receipt: Clean makes it easier to read',
    bannerLabel: 'Enhancement suggestion',
    reasons: {
      uneven_light: 'The light on the page is uneven.',
      dull_paper: 'The paper looks grey instead of white.',
      colour_cast: 'The page has a colour tint.',
      faded_print: 'The print looks faded.',
    } as Record<string, string>,
    bannerNote: 'A simple check of the picture suggested this, so it can be wrong. Preview it, then decide.',
    preview: 'Preview',
    stopPreview: 'Stop preview',
    apply: 'Apply',
    applyHint: 'Applies Clean to this page as one step. Undo takes it back.',
    notNow: 'Not now',
    notNowHint: 'Hides this suggestion for this picture until you close the app. Nothing is changed.',
    announcePreviewOn: 'Previewing Clean. Nothing has been changed yet.',
    announcePreviewOff: 'Preview stopped. Nothing was changed.',
    announceApplied: (look: string) => `Applied ${look}. Undo takes it back.`,
    announceDismissed: 'Suggestion hidden for this picture. You can still choose a look below.',
    announceFollow: 'This item follows the scan look again.',
    failed: 'Could not change the enhancement. Nothing was changed.',
    // honest limits
    helpTitle: 'About these looks',
    helpClean:
      'Clean is the one to try first. In tests on made-up pages, text-reading software (OCR) read Clean pages as well as the original or better, never worse. It has not been measured on real photos yet.',
    helpBw:
      'Document and Receipt turn the page into black and white. They can lose faint marks such as pencil, light stamps and photos. If you are not sure, keep Original or Clean.',
    helpOriginal: 'Original is never changed: the saved picture is exactly the crop.',
    helpPreview: 'The preview is made from a reduced copy of the picture. The saved file is made from the full-size original, so thin strokes can differ a little.',
    // what a look writes
    greyNote: 'Document and Receipt save a black-and-white picture (one grey channel). The colour profile of the original is not kept.',
    pngHint:
      'Black-and-white looks keep faint marks best as a PNG. Auto Crop saves a JPG like your original, at high quality. To get a PNG, convert the saved copy on the Convert screen.',
    convertLink: 'Open Convert',
    heldNote: 'This scan waits for your OK. Changing its look asks you to accept it again.',
    // the grid
    tileBadge: 'Enhanced',
    tileBadgeSome: (on: number, total: number) => `Enhanced ${on} of ${total}`,
    tileBadgeTitle: 'This picture will be saved with an enhancement.',
    chipLook: (name: string, pct: number) => `${name} ${pct}%`,
    chipOwn: 'This item has a look of its own.',
    gridAll: 'Enhance all suggested',
    gridSelected: 'Enhance suggested',
    gridHint: 'Applies Clean to every page where Auto Crop suggests it and nothing is set yet. One Undo takes it all back.',
    gridWorking: 'Looking for pages to enhance…',
    batchDone: (crops: number, images: number, none: number) =>
      `Enhanced ${crops} ${crops === 1 ? 'page' : 'pages'} in ${images} ${images === 1 ? 'image' : 'images'}.${none > 0 ? ` ${none} had nothing to suggest.` : ''}`,
    batchNone: (none: number, already: number) =>
      already > 0
        ? `Nothing new to enhance. ${already} ${already === 1 ? 'page already has' : 'pages already have'} a look${none > 0 ? ', and the rest have nothing to suggest' : ''}.`
        : 'Nothing to enhance: no page looks like a document or receipt that needs it.',
    batchUndo: 'Undo',
    batchFailed: 'Could not enhance the suggested pages. Nothing was changed.',
  },
  // The one-time sheet the first time Curved edges is entered.
  curvedIntro: {
    title: 'Before you bend the edges',
    lead: 'Curved edges let you describe a page whose edges bow, like a crumpled receipt or a book page. Auto Crop then flattens it.',
    fixesTitle: 'What it fixes',
    fixes: 'Edges that bow in or out, and a page photographed at an angle.',
    doesNotTitle: 'What it does not fix',
    doesNot: 'Wrinkles and folds inside the page. If the four edges look straight, the page comes out as it would from a plain crop.',
    safeTitle: 'Nothing is written yet',
    safe: 'A curved page is held for review. The original is not replaced until you accept the page, and Save as copy always works. Undo takes back any step.',
    go: 'Got it: bend the edges',
    goSuggest: 'Got it: suggest curves',
    notNow: 'Not now',
  },
  // Saving one scan from the editor (M10.43).
  save: {
    heading: 'SAVE THIS SCAN',
    saveAsCopy: 'Save as copy',
    replace: 'Replace original',
    saving: 'Saving…',
    acceptFirst: 'Accept the split first, then you can replace the original.',
    acceptFirstCurved: 'Replace original is off until you accept this curved page. Save as copy needs no OK.',
    openOnly: 'This format is open-only: Save as copy.',
    copyNote: 'A copy goes to the AutoCrop folder next to the scan. The scan is not touched.',
    replaceNote: 'The original is backed up first and can be restored.',
    splitPreview: (n: number) => `${n} files`,
    willSave: (names: string) => `Will save ${names}`,
    savedAs: (names: string) => `Saved as ${names}`,
    savedCopies: (names: string) => `Saved copies: ${names}`,
    movedToBackups: 'The scan was moved to Backups and can be restored.',
    collision: (wanted: string, got: string) => `A file named ${wanted} already existed, so the set was saved as ${got}. Nothing was overwritten.`,
    sourceInUse: 'The scan stayed where it was because another program is using it.',
    sourceChanged: 'The scan changed while saving, so it was left alone. The new files are saved.',
    noCrop: 'Draw a crop first.',
    failed: (reason: string) => `Not saved. ${reason}`,
    held: 'Held for review: nothing was written.',
    firstSplitTitle: 'Before the first split',
    firstSplitBody: (n: number, first: string) =>
      `1 scan becomes ${n} files (${first} and so on). The scan is moved to Backups and can be restored, or you can keep it and save the items as new files.`,
    firstSplitReplace: 'Replace scan (recommended)',
    firstSplitKeep: 'Keep the scan and save items as new files',
    viewBackups: 'Open Backups',
  },
  // Restore dialog for a split scan (M10.44).
  restoreDialog: {
    title: (name: string) => `Restore ${name}`,
    body: (n: number) => (n === 1 ? '1 file was made from it.' : `${n} files were made from it.`),
    legend: 'What happens to the files made from this scan',
    keep: 'Keep them',
    keepNote: 'The files stay where they are. Only the scan comes back.',
    remove: 'Remove them',
    removeNote: 'Moved to Backups, not deleted. You can bring them back from there.',
    changedHeading: 'Edited since they were saved (kept either way):',
    changedFileNote: 'edited',
    missingFileNote: 'no longer there',
    removedFileNote: 'moved to Backups',
    confirmRemoveChanged: 'I understand that the edited files are kept',
    restore: 'Restore',
    cancel: 'Cancel',
    result: (restored: string, kept: number, removed: number) =>
      `Restored ${restored}. ${kept} ${kept === 1 ? 'file' : 'files'} kept${removed > 0 ? `, ${removed} moved to Backups` : ''}.`,
    occupied: (name: string) => `Another file is at the original place, so the scan came back as ${name}.`,
    // The same sheet for a converted file (Convert results and Backups)
    legendConvert: 'What happens to the converted file',
    bodyConvert: 'This was converted. What should happen to the converted file?',
    keepConvert: 'Keep the converted file',
    keepConvertNote: 'The converted file stays where it is. Only the original comes back.',
    removeConvert: 'Remove the converted file',
    removeConvertNote: 'Moved to Backups, not deleted. You can bring it back from there.',
  },
  backups: {
    title: 'Backups',
    intro: 'Originals are kept here before Auto Crop replaces them. They survive closing the app.',
    location: 'Location',
    open: 'Open folder',
    keepFor: 'Keep for',
    using: (used: string) => `Using ${used}`,
    freeOf: (free: string) => `${free} free`,
    purge: 'Purge expired now',
    purged: (n: number) => (n === 0 ? 'Nothing had expired.' : `Purged ${n} expired ${n === 1 ? 'run' : 'runs'}.`),
    empty: 'No backups yet.',
    emptyNote: 'When Auto Crop replaces an original, a verified copy of it appears here.',
    files: (n: number) => `${n} ${n === 1 ? 'file' : 'files'}`,
    expires: (date: string) => `expires ${date}`,
    neverExpires: 'kept until you delete it',
    keep: 'Keep',
    kept: 'Kept (pinned)',
    unkeep: 'Unpin',
    restoreRun: 'Restore all from this run',
    restore: 'Restore',
    restored: 'Restored',
    restoredCopy: 'Restored as copy',
    restoreAsCopy: 'Restore as copy',
    replaceAnyway: 'Replace anyway',
    changedSince: 'The file on disk was edited after Auto Crop saved it.',
    changedTag: 'Changed since saved',
    unchangedTag: 'unchanged since saved',
    sizes: (from: string, to: string) => `${from} to ${to}`,
    expand: 'Show files',
    collapse: 'Hide files',
    note: 'Restoring never deletes the file it replaces; it is kept as “Result replaced by restore” until retention ends.',
    restoredOne: (name: string) => `Restored ${name}.`,
    restoredAsCopy: (name: string) => `Restored as a copy: ${name}. The edited file is untouched.`,
    restoredRun: (ok: number, choice: number, failed: number) => {
      const parts = [`Restored ${ok} ${ok === 1 ? 'original' : 'originals'}`];
      if (choice > 0) parts.push(`${choice} edited ${choice === 1 ? 'file needs' : 'files need'} your choice`);
      if (failed > 0) parts.push(`${failed} failed`);
      return parts.join('. ') + '.';
    },
    pinned: 'This run will be kept.',
    unpinned: 'This run follows the retention setting again.',
    loadFailed: "Couldn't read the backup list.",
    retentionSaved: 'Retention updated.',
    openFailed: "Couldn't open the folder.",
    // Split scans (M10.44)
    splitRunLine: (name: string, n: number) => `${name} to ${n} files`,
    derivedHeading: (n: number) => `${n} files made from it`,
    derivedState: {
      unchanged: 'as saved',
      changed: 'edited since saved',
      missing: 'no longer there',
      removed: 'moved to Backups',
    } as Record<string, string>,
    restoreSplit: 'Restore…',
    splitBadge: 'Split',
    // Conversions (Convert screen): one original, one converted file
    convertedBadge: 'Converted',
    convertedLine: (name: string, to: string) => `${name} to ${to}`,
    convertedHeading: 'The converted file',
  },
  // The Convert screen: turn pictures from one format into another (HEIC to JPG above all).
  convert: {
    title: 'Convert formats',
    intro: 'Change a whole batch of pictures from one format to another, for example HEIC photos from a phone into JPG.',
    homeTitle: 'Convert formats',
    homeBody: 'Turn HEIC photos from a phone into JPG, or PNG into JPG, a whole folder at once.',
    homeButton: 'Convert files',
    // files
    filesHeading: 'Files',
    listLabel: 'Files to convert',
    dropTitle: 'Drop pictures or a folder here',
    dropSub: 'or choose them below',
    dropFormats: 'HEIC · JPG · PNG · WebP · TIFF · AVIF · GIF · BMP',
    listKeys: 'The list is one stop: Up and Down arrows move between files, Enter removes the file.',
    dropMore: 'You can also drop more files or folders anywhere in this window.',
    chooseFiles: 'Choose files',
    chooseFolder: 'Choose a folder',
    subfolders: 'Include subfolders',
    fileCount: (n: number) => `${n} ${n === 1 ? 'file' : 'files'}`,
    clearAll: 'Clear all',
    remove: (name: string) => `Remove ${name} from the list`,
    added: (added: number, duplicates: number, truncated: boolean) => {
      const parts = [added === 0 ? 'No new files were added.' : `Added ${added} ${added === 1 ? 'file' : 'files'}.`];
      if (duplicates > 0) parts.push(`${duplicates} ${duplicates === 1 ? 'was' : 'were'} already in the list.`);
      if (truncated) parts.push('The list is full, so some files were not added.');
      return parts.join(' ');
    },
    removed: (n: number) => `Removed ${n} ${n === 1 ? 'file' : 'files'} from the list. Nothing on disk was changed.`,
    cleared: 'The list is empty. Nothing on disk was changed.',
    // options
    optionsHeading: 'Options',
    targetLabel: 'Convert to',
    targetJpg: 'JPG',
    targetPng: 'PNG',
    qualityLabel: 'JPG quality',
    qualitySmall: 'Small',
    qualityBalanced: 'Balanced',
    qualityBest: 'Best',
    qualityCustom: 'Custom',
    qualitySmallNote: 'Smaller files, a little less detail.',
    qualityBalancedNote: 'A good choice for most pictures.',
    qualityBestNote: 'Larger files, the most detail.',
    qualityCustomNote: 'Pick a number from 1 to 100. Higher keeps more detail.',
    qualityValue: 'Quality, 1 to 100',
    qualityNumber: 'Quality number',
    pngNoQuality: 'PNG is lossless, so there is no quality to choose.',
    colourLabel: 'Colours',
    colourSrgb: 'sRGB (recommended)',
    colourSrgbNote: 'Looks right in every program and on every screen.',
    colourPreserve: 'Keep original colours (wide gamut)',
    colourPreserveNote: 'Keeps each picture’s colour profile. Some programs show these colours dulled.',
    whereLabel: 'What happens to the originals',
    whereReplace: (days: number | null) =>
      days === null ? 'Replace originals (backed up until you delete them, restorable)' : `Replace originals (backed up for ${days} days, restorable)`,
    whereKeep: 'Keep originals, save beside them',
    whereFolder: 'Save to a folder',
    chooseOutput: 'Choose folder',
    changeOutput: 'Change folder',
    outputFolder: (name: string) => `Folder: ${name}`,
    outputFolderNone: 'No folder chosen yet.',
    stripLocation: 'Strip location (GPS)',
    stripLocationNote: 'Removes where each picture was taken from the converted file.',
    reencode: 'Convert files already in this format again',
    reencodeNote: (target: string) => `Off: a file that is already a ${target} is left alone.`,
    // preview
    previewHeading: 'What will happen',
    previewChecking: 'Checking the files…',
    previewFailed: "Couldn't check the files. Try changing an option, or remove and add the files again.",
    willConvert: (n: number) => `${n} will be converted`,
    sameFormat: (n: number, target: string) => `${n} already ${target}, left alone`,
    cannot: (n: number) => `${n} cannot be converted`,
    sizeToConvert: (size: string) => `${size} to convert`,
    byFormat: 'In the list',
    byFormatItem: (format: string, n: number) => `${n} ${format}`,
    cannotHeading: 'Cannot be converted',
    cannotMore: (n: number) => `and ${n} more`,
    heifMissing: (n: number) =>
      `This build of Auto Crop has no HEIC decoder, so ${n} HEIC or AVIF ${n === 1 ? 'file' : 'files'} cannot be converted. The HEIC package of Auto Crop has the decoder.`,
    originalsReplace: (days: number | null) =>
      days === null
        ? 'Each original is moved to Backups first and kept until you delete it. You can restore it any time.'
        : `Each original is moved to Backups first and kept ${days} days. You can restore it any time.`,
    originalsKeep: 'Originals are not touched. Each converted file is saved beside its original.',
    originalsFolder: (name: string | null) =>
      name ? `Originals are not touched. Converted files are saved in “${name}”.` : 'Originals are not touched. Choose a folder for the converted files.',
    // the button and why it is off
    convertN: (n: number) => `Convert ${n} ${n === 1 ? 'file' : 'files'}`,
    convertNone: 'Convert files',
    whyNoFiles: 'Add files to convert them.',
    whyNothing: 'Nothing to convert: every file is already in the chosen format or cannot be converted.',
    whyNeedFolder: 'Choose a folder for the converted files first.',
    whyChecking: 'Checking the files…',
    // run
    running: 'Converting',
    runningOf: (done: number, total: number) => `${done} of ${total} files`,
    progressLabel: 'Conversion progress',
    progressAnnounce: (done: number, total: number) => `${done} of ${total} files done.`,
    cancel: 'Cancel',
    cancelling: 'Stopping after the files in progress…',
    // results
    resultsLabel: 'Results',
    doneTitle: 'Done',
    stoppedTitle: 'Stopped',
    counts: (converted: number, skipped: number, failed: number) => `${converted} converted, ${skipped} skipped, ${failed} failed`,
    sizeSummary: (from: string, to: string) => `${from} to ${to}`,
    stoppedNote: 'The files not yet converted were not touched.',
    statusConverted: 'Converted',
    statusSkipped: 'Skipped',
    statusFailed: 'Failed',
    sizeChange: (from: string, to: string) => `${from} to ${to}`,
    dimensions: (w: number, h: number) => `${w} × ${h}`,
    outputName: (name: string) => `Saved as ${name}`,
    openFolder: 'Open folder',
    openFolderFor: (name: string) => `Open the folder of ${name}`,
    openFailed: "Couldn't open the folder.",
    convertMore: 'Convert more',
    openBackups: 'Open Backups',
    originalsInBackups: 'Replaced originals are in Backups. Restore one there, or here.',
    restoreOriginal: 'Restore original',
    restoreOriginalOf: (name: string) => `Restore the original of ${name}`,
    restoredBadge: 'Original restored',
    restoredOne: (name: string) => `Restored ${name}.`,
    restoredAsCopy: (name: string) => `Restored as a copy: ${name}. The edited file is untouched.`,
    changedSince: 'The file on disk was edited after it was converted.',
    restoreAsCopy: 'Restore as copy',
    replaceAnyway: 'Replace anyway',
    restoreFailed: (name: string, reason: string) => `${name}: ${reason}`,
    runFailed: 'The conversion stopped unexpectedly. Files that were already converted are listed; the others are unchanged.',
    done: (converted: number, skipped: number, failed: number) => `Finished. ${converted} converted, ${skipped} skipped, ${failed} failed.`,
    chooseFolderFailed: "Couldn't choose that folder.",
    busyNote: 'A conversion is running. Options come back when it ends.',
  },
  settings: {
    title: 'Settings',
    saving: 'SAVING',
    saveAsCopy: 'Save as copy',
    saveAsCopyHelp: 'Off means originals are replaced, after a verified backup.',
    backups: 'Backups keep',
    backupsHelp: 'How long a backed-up original is kept.',
    openBackups: 'Open Backups',
    appearance: 'APPEARANCE',
    theme: 'Theme',
    themeSystem: 'System',
    themeLight: 'Light',
    themeDark: 'Dark',
    privacy: 'PRIVACY',
    privacyNote: 'No telemetry. No network calls unless you ask.',
    about: 'ABOUT',
    version: 'Version',
    platform: 'Platform',
    notesTitle: 'Good to know',
    notes: [
      'Confidence scores are uncalibrated heuristics for now. Balanced is experimental, and Strict is the safe default.',
      'Scans with several items are found and held for review. Splitting is the newest part of Auto Crop, so it never replaces a scan without your OK unless you switch on the experimental option above.',
      'Enhancement is always your choice. Clean has only been measured on made-up test pages so far, and the black-and-white looks can lose faint marks, so keep Original if you are not sure.',
    ],
    enhancing: 'ENHANCEMENT',
    suggestEnhance: 'Suggest enhancement for documents and receipts',
    suggestEnhanceHelp: 'Shows a quiet suggestion when a page looks like a document or receipt that would read better. It never changes anything by itself.',
    splitting: 'SPLITTING SCANS',
    splitPolicy: 'Look for several items on a scan',
    splitPolicyHelp: 'Photos laid out on a scanner bed, or several receipts in one picture.',
    splitPolicyOptions: {
      auto: 'Auto: find them and hold for review',
      always: 'Always look for several items',
      never: 'Never: one item per file',
    },
    splitProfile: 'The items are',
    splitProfileHelp: 'Photos keep the way they were placed. Receipts are turned upright.',
    splitProfileOptions: { photos: 'Photos', receipts: 'Receipts and documents' },
    autoSaveSplits: 'Auto-save splits',
    autoSaveSplitsTag: 'Experimental',
    autoSaveSplitsHelp: 'Save a split scan without asking when every item looks Good.',
    autoSaveSplitsWarning:
      'Experimental. A split replaces one file with several, and Auto Crop is still learning where photos end. With this on, a scan whose items all look Good is split and saved without your OK, and the scan is moved to Backups. Restore original brings it back. Leave it off to look at every split first.',
    appliesToNew: 'Changes apply to scans you open from now on. Open scans keep their items; use “Items” in the editor to change one.',
    saved: 'Saved.',
    saveFailed: "Couldn't save the setting.",
    loading: 'Loading settings…',
  },
  toasts: {
    added: (n: number) => `Added ${n} ${n === 1 ? 'image' : 'images'}.`,
    addedSkipped: (n: number, s: number) =>
      `Added ${n} ${n === 1 ? 'image' : 'images'}. ${s} ${s === 1 ? 'file was' : 'files were'} skipped.`,
    noneAdded: 'No images were added.',
    skippedBecause: (reason: string) => `Skipped: ${reason}`,
    backendError: 'Something went wrong talking to Auto Crop. Try again.',
    copyModeOn: 'Save as copy is on. Originals will not be touched.',
    copyModeOff: 'Originals will be replaced after a verified backup.',
  },
  a11y: {
    skipToContent: 'Skip to content',
    toasts: 'Notifications',
  },
};

export type Strings = typeof S;
