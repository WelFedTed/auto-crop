// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Every user-visible string lives here (English). The engine returns codes, never prose (B20, PLAN 6.7);
// this file is the single place that turns a code into words, so a Fluent catalogue can replace it later.

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
    cause: 'The two detection methods found different edges.',
    action: 'Try outline 2 of 3.',
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
  UNSUPPORTED_FORMAT: "This file type isn't supported yet. Auto Crop reads JPG and PNG.",
  TOO_LARGE: 'This picture is larger than the 100 megapixel limit.',
  UNREADABLE: "Auto Crop couldn't read this file. Check that it still exists and that you can open it.",
  SOURCE_CHANGED:
    'This file changed while Auto Crop was working, so it was left as it is. Its backup is kept.',
  FILE_IN_USE: 'Another program is using this file, so it was left as it is. Close it and try again.',
  DISK_FULL: "There isn't enough free space to save safely. Nothing was replaced.",
  READ_ONLY: 'This file or its folder is read-only. Try Save as copy.',
  VERIFY_FAILED: "Couldn't verify the new file. The original is unchanged.",
  BACKUP_FAILED: "Can't back up this original, so it was not replaced.",
  ORIGINAL_EXPIRED: 'The backup of this original has expired and is no longer available.',
  NO_CROP: 'This image has no crop yet, so it was left as it is. Draw a crop first.',
  INTERNAL: 'Something went wrong inside Auto Crop. The original is unchanged.',
};

export function errorMessage(code: ErrorCode | null | undefined): string {
  return code ? (ERRORS[code] ?? ERRORS.INTERNAL) : ERRORS.INTERNAL;
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
    formats: 'JPG · PNG',
    formatsNote: 'More formats, including HEIC, are planned.',
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
      'HEIC, image enhancement and multi-item scans are not available yet. Auto Crop reads JPG and PNG.',
    ],
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
