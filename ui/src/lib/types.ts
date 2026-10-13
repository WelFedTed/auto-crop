// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The contract between the webview and the Rust shell (crates/shell). The Rust structs in
// crates/engine/src/api.rs (and the engine's `Settings`) mirror these types one to one (serde, camelCase).
// The webview never sees a file path it could act on: items are opaque numeric ids plus a sanitised
// display name; crops are opaque numeric ids inside an item.

/** A point in EXIF-oriented source space, normalised: x and y are 0..1 of width and height. */
export interface Pt {
  x: number;
  y: number;
}

export type Side = 'top' | 'right' | 'bottom' | 'left';

/**
 * The four boundary curves of a curved page (docs/dev/curved-pages.md). Each edge is 2 to 32 points in order along
 * it; the first and last point of every edge ARE the page corners (top: TL to TR, right: TR to BR, bottom: BR to
 * BL, left: BL to TL). `quarterTurns` and `mirror` act on the flattened page.
 */
export interface CurveSet {
  top: Pt[];
  right: Pt[];
  bottom: Pt[];
  left: Pt[];
  quarterTurns: number;
  mirror: boolean;
}

/** One number per edge, in the order top, right, bottom, left. */
export type EdgeNumbers = [number, number, number, number];

/**
 * The engine's PROPOSAL of curves for one crop (`Engine::suggest_curves`, docs/dev/curved-pages.md "Suggestion API").
 * Nothing is applied by asking: the UI previews it and, on the person's click, passes `curves` WHOLE to `setCurves`.
 * `looksCurved` is deliberately not used by the UI: on real receipts it fired on 13% of pages labelled flat
 * (docs/perf/curve-fit-baseline.md), so no chip, badge or filter is built on it.
 */
export interface CurveSuggestion {
  /** TL, TR, BR, BL: the end points of `curves`, possibly a few pixels from the crop's own corners. */
  quad: [Pt, Pt, Pt, Pt];
  /** 2 to 9 points per edge, valid, carrying the crop's turns and mirror. Pass it as it is to `setCurves`. */
  curves: CurveSet;
  /** 0..1 per edge: how well the edge was seen (low under a finger, a hard shadow, a desk the colour of the paper). */
  confidence: EdgeNumbers;
  /** Largest distance of the fitted edge from its chord, percent of the chord (0 for an edge left straight). */
  curvature: EdgeNumbers;
  /** The same deviation in percent of the page's shorter side. */
  bulge: EdgeNumbers;
  supportGain: EdgeNumbers;
  /** The largest `curvature` among the edges that pass every `looksCurved` gate (0 if none does). */
  maxCurvature: number;
  looksCurved: boolean;
  /** The fit was refused: `curves` are the crop's straight edges. */
  fellBack: boolean;
}

/** The looks enhancement offers (docs/dev/enhance-engine.md). `original` touches nothing. */
export type EnhanceMode = 'original' | 'clean' | 'document' | 'receipt';

/**
 * An enhancement as the engine serialises it: `{ mode: 'original' }` or `{ mode, strength }` with the strength in
 * whole percent (0 to 100; an absent strength is 100). The pictures and the saved file are blended with the input by it.
 */
export type Enhance = { mode: 'original' } | { mode: Exclude<EnhanceMode, 'original'>; strength?: number };

/** What a look writes, so the UI can say the right thing near Save (`OutputHint` in the engine). */
export interface EnhanceHint {
  /** The look writes ONE grey channel: no colour, no colour profile. */
  gray: boolean;
  /** PNG is the recommended format (lossless, keeps faint marks). */
  recommendPng: boolean;
}

/** The numbers behind a suggestion. The UI shows none of them; they are in the contract so a test can pin the wire shape. */
export interface EnhanceMeasures {
  aspect: number;
  paperFraction: number;
  inkFraction: number;
  shading: number;
  paperLuma: number;
  cast: number;
  colourFraction: number;
  textComponents: number;
  concentration: number;
  glyphInkShare: number;
  inkContrast: number;
  faded: boolean;
}

/** Why the engine suggests enhancing a page: the copy is looked up by this code (`enhance.reason.<code>`). */
export type EnhanceReason = 'uneven_light' | 'dull_paper' | 'colour_cast' | 'faded_print';

/**
 * The engine's RECOMMENDATION for one crop (`suggest_enhance`). `mode` is `clean` and never a black-and-white look;
 * `blackAndWhite` names the B&W look that fits the page as an OPTION. Asking records and applies nothing (B13).
 */
export interface EnhanceSuggestion {
  mode: Exclude<EnhanceMode, 'original'>;
  strength: number;
  reason: EnhanceReason;
  reasons: EnhanceReason[];
  blackAndWhite: Exclude<EnhanceMode, 'original' | 'clean'>;
  blackAndWhiteHint: EnhanceHint;
  hint: EnhanceHint;
  measures: EnhanceMeasures;
}

/** The answer of `enhanceSuggested`: what the one-click batch did. */
export interface EnhanceBatch {
  /** The new view of every image that changed. */
  views: ItemView[];
  /** Images with at least one enhanced crop, and crops enhanced. */
  images: number;
  crops: number;
  /** Crops that already had an enhancement (left as they were). */
  already: number;
  /** Crops with no recommendation (photographs, clean scans). */
  none: number;
  failed: number;
}

/** Parameters of a crop. `quad` is TL, TR, BR, BL; `quarterTurns` is 0..3 (clockwise). */
export interface Edit {
  quad: [Pt, Pt, Pt, Pt];
  quarterTurns: number;
  /** Fine rotation in degrees, -45..45. */
  fineDeg: number;
}

/** Stable codes for the "why was this held" copy (PLAN 6.7). Text lives in the UI, never in Rust. */
export type ReasonCode =
  | 'NO_QUAD' // forces Failed
  | 'WEAK_EDGE' // has `side`
  | 'PARTIAL_FRAME'
  | 'ODD_ASPECT'
  | 'LOW_CONTRAST_EDGE'
  | 'IMPLAUSIBLE_QUAD' // forces Failed
  | 'DETECTORS_DISAGREE' // another outline is as plausible (the whole picture, a print inside a border)
  // Multi-item scans (M10.13, M10.15). Each one holds the whole scan for review.
  | 'TOUCHING_ITEMS'
  | 'OVERLAPPING_ITEMS'
  | 'ITEMS_TOO_CLOSE'
  | 'SPLIT_UNSTABLE'
  | 'TOO_MANY_ITEMS'
  | 'BED_UNCERTAIN'
  | 'ANALYSIS_LIMIT'
  | 'NO_DOCUMENT';

export interface Reason {
  code: ReasonCode;
  side?: Side;
}

/**
 * Detection confidence. `score` is an UNCALIBRATED heuristic in 0..1 (no calibration exists yet,
 * ROADMAP M4); the UI turns it into Good, Check or Failed with the strictness cut-offs of PLAN 6.2.4
 * (0.95, 0.90, 0.80 for Strict, Balanced, Aggressive; below 0.60 is always Failed). `forced`
 * overrides the score: a `failed` item is always Failed, a `check` item is never Good.
 */
export interface Confidence {
  score: number;
  forced: 'failed' | 'check' | null;
  reasons: Reason[];
}

export type ItemStatus = 'analysing' | 'ready' | 'error';

/** Typed error codes (PLAN 2.10 `ErrKind`), shown by the UI from its own copy. */
export type ErrorCode =
  | 'CORRUPT'
  | 'UNSUPPORTED_FORMAT'
  | 'UNSUPPORTED_FEATURE'
  | 'TOO_LARGE'
  | 'UNREADABLE'
  | 'READ_ONLY'
  | 'CLOUD_NOT_LOCAL'
  | 'HEVC_DECODER_MISSING'
  | 'DECODER_CRASHED'
  | 'DECODE_TIMEOUT'
  | 'MEMORY_LIMIT'
  | 'MODEL_LOAD_FAILED'
  | 'OUT_OF_MEMORY'
  | 'INTERNAL_PANIC'
  | 'DEGENERATE'
  | 'NO_CROP' // saving an item that has no crop (a Failed item the user has not drawn a crop for)
  | 'ENCODE_FAILED'
  | 'UNSUPPORTED_OUTPUT'
  | 'SOURCE_CHANGED'
  | 'FILE_IN_USE'
  | 'DISK_FULL'
  | 'VERIFY_FAILED'
  | 'BACKUP_FAILED'
  | 'ORIGINAL_EXPIRED'
  // Multi-item scans (M10)
  | 'PLAN_STALE'
  | 'GROUP_COMMIT_FAILED'
  | 'SAVED_SOURCE_IN_USE'
  | 'HELD_FOR_REVIEW' // not a failure: nothing was written and the scan waits for the person
  | 'ITEM_OP' // a refused item operation; nothing changed (a corner edit, angle, merge or cut on a curved crop)
  | 'NOT_REPLACEABLE' // the source is never replaced in place; the reason is the notice
  | 'SCHEMA_TOO_NEW'
  | 'CANCELLED'
  | 'DEADLINE_EXCEEDED'
  | 'INTERNAL';

export interface SavedInfo {
  backupId: string | null; // null after Save as copy
  /** Display name of the file that was written (the first of `outputs` for a split scan). */
  output: string;
  copy: boolean;
  /** Every output file name of a split scan, in output order (empty for one-to-one saves). */
  outputs: string[];
}

/** Where a crop came from. */
export type CropOrigin = 'auto' | 'manual' | 'autoThenEdited';
export type Band = 'good' | 'check' | 'failed';

/** One crop (one output file) of an image; `ItemView.crops` lists them in output order (M10.34). */
export interface CropView {
  /** Stable across edits and undo, never reused. */
  id: number;
  /** 1-based output rank (the `{n}` of the file name); 0 while excluded. */
  order: number;
  include: boolean;
  edit: Edit | null;
  /** The detector's proposal for this crop; null for a crop drawn by hand. */
  autoEdit: Edit | null;
  mirror: boolean;
  /**
   * The boundary curves of a curved page, else null. `edit` is then the straight outline through the corners, and
   * the crop is held for review until the scan is accepted (band `check`).
   */
  curves: CurveSet | null;
  /** The enhancement this crop is previewed and saved with: its own override, else the image's default. */
  enhance: Enhance;
  /** The crop's own enhancement; `null` when it follows the default (show a "customised" dot when it is not). */
  enhanceOverride: Enhance | null;
  origin: CropOrigin;
  confidence: Confidence | null;
  /** Band at the Strict cutoff; a crop the person placed or edited counts as reviewed (good). */
  band: Band | null;
  edited: boolean;
  /** The file name this crop is saved as when the image is saved as a split. */
  outputName: string | null;
  /** Cache buster for this crop's pixels: part of the crop image URL. */
  renderKey: string;
}

export type SplitPolicy = 'auto' | 'always' | 'never';
export type SplitProfile = 'photos' | 'receipts';
export type OrderMode = 'reading' | 'manual';

export type ScanTriage = { kind: 'approved' } | { kind: 'heldForReview'; itemsNeedCheck: number } | { kind: 'noItems' };

export interface SplitView {
  policy: SplitPolicy;
  profile: SplitProfile;
  orderMode: OrderMode;
  triage: ScanTriage;
  /** The person accepted the current state, so a Replace of a held scan is allowed. */
  accepted: boolean;
  /** The scan would be saved as several files (or was). */
  isSplit: boolean;
  /** Number of included crops with a quad. */
  included: number;
}

/** What `redetect` changes: either may be left out. */
export interface SplitPatch {
  policy?: SplitPolicy;
  profile?: SplitProfile;
}

/** Cut line across a crop: fractions along the two edges the cut ends on (0..1). */
export interface Cut {
  axis: 'vertical' | 'horizontal';
  t0: number;
  t1: number;
}

export type RevertTo = { kind: 'auto' } | { kind: 'step'; position: number };

export interface RedetectResult {
  id: number;
  view: ItemView | null;
  error: ErrorCode | null;
}

/** The answer of `sessionUndo` and `sessionRedo`: the command's label and the images it changed. */
export interface SessionStep {
  label: string;
  items: ItemView[];
}

export interface ItemView {
  id: number;
  /** Sanitised file name, for display only. */
  name: string;
  /** EXIF-oriented source dimensions (0 until analysed). */
  width: number;
  height: number;
  status: ItemStatus;
  error: ErrorCode | null;
  /** Null until analysed. For a split scan: the first included crop. */
  edit: Edit | null;
  /** What the detector proposed, for "Reset to auto". */
  autoEdit: Edit | null;
  confidence: Confidence | null;
  /** Bumped on every committed change; part of every image URL (`?g=`). */
  gen: number;
  edited: boolean;
  saved: SavedInfo | null;
  /** Result changed since the last save. */
  dirtySinceSave: boolean;
  canUndo: boolean;
  canRedo: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  /** Every crop in output order, included or not. */
  crops: CropView[];
  /** Split policy, triage and acceptance; null until analysed. */
  split: SplitView | null;
  /** Stable undo position, for `revertCrop` to a step. */
  historyPosition: number;
  /**
   * The notice code of the reason this source is never replaced in place (`tiff.multi_page` or
   * `format.write_unavailable`), else null. Such a file can only be saved as a copy.
   */
  openOnly: string | null;
  /** The default enhancement of this image (crops without their own override get it). Original until a person chose. */
  enhance: Enhance;
}

export interface OpenSummary {
  added: number;
  skipped: number;
  /** Ids of the newly added items, in order. */
  ids: number[];
  /** Why files were skipped: unsupported, too large, unreadable, symlink loop... */
  skippedReasons: ErrorCode[];
}

/**
 * The UI must send back the WHOLE object it received from `getSettings` (spread it), or fields it does
 * not know about (a newer engine adds some) reset to their defaults.
 */
export interface Settings {
  saveAsCopy: boolean;
  /** Days to keep backups; null = never delete. */
  retentionDays: number | null;
  /** The first-write sheet has been acknowledged. */
  firstWriteAck: boolean;
  /** Whether to look for several items on one scan (default `auto`). */
  splitPolicy: SplitPolicy;
  /** What the items are: `photos` keep the placed orientation, `receipts` are made upright. */
  splitProfile: SplitProfile;
  /** EXPERIMENTAL: save a split scan without a review when every item is Good. Off by default. */
  autoSaveSplits: boolean;
  /** The one-time explanation of curved edges has been seen. */
  curvedIntroAck: boolean;
  /** Show the enhancement suggestion in the editor and the grid (on by default). It never applies anything. */
  suggestEnhance: boolean;
}

export type SaveTarget = 'replace' | 'copy';

export interface SaveOutcome {
  id: number;
  ok: boolean;
  error: ErrorCode | null;
  saved: SavedInfo | null;
  /** Things that did not stop the save: `SAVED_SOURCE_IN_USE`, `SOURCE_CHANGED`. */
  notes: ErrorCode[];
  /** One-line notice codes: `tiff.multi_page`, `format.write_unavailable`, `derived.user_edited`, `split.held`. */
  notices: string[];
}

export type DerivedState = 'unchanged' | 'changed' | 'missing' | 'removed';
export type DerivedAction = 'keep' | 'remove';

/** A file made from a split scan. */
export interface DerivedFile {
  name: string;
  bytes: number;
  state: DerivedState;
}

export interface BackupFile {
  /** `<run id>/<file index>`; opaque. */
  id: string;
  name: string;
  /** Display-only original location. */
  displayPath: string;
  originalBytes: number;
  outputBytes: number | null;
  /** The file on disk differs from what was saved (edited since). */
  changedSinceSaved: boolean;
  restored: boolean;
  /** `OneToN` for a split scan: then `derived` lists the files made from it. */
  kind: 'OneToOne' | 'OneToN';
  derived: DerivedFile[];
}

export interface BackupRun {
  id: string;
  name: string;
  createdAt: string; // RFC 3339
  expiresAt: string | null;
  fileCount: number;
  totalBytes: number;
  pinned: boolean;
  files: BackupFile[];
}

export interface BackupsView {
  location: string; // display only
  usedBytes: number;
  freeBytes: number | null;
  runs: BackupRun[];
}

export type RestoreMode = 'auto' | 'as_copy' | 'replace_anyway';

export interface RestoreOutcome {
  ok: boolean;
  /** `changed` = the file changed since saving; the UI must ask Restore as copy or Replace anyway. */
  needsChoice: boolean;
  error: ErrorCode | null;
  /** Display name of the restored or copied file. */
  restored: string | null;
  /** For a split scan: what became of each derived file. */
  derived: DerivedFile[];
}

export interface LaunchInfo {
  token: string;
  version: string;
  platform: 'windows' | 'macos' | 'linux';
  /** Which paths exist for the backup store, display only. */
  backupsLocation: string;
  /** File extensions (lower case, no dot) this build can open. Optional for an older shell. */
  inputExtensions?: string[];
}

/**
 * A curve preview served by the `acimg` scheme (the engine's `preview_curves`, nothing is committed or cached):
 * `/<token>/<id>/crop/<crop>/curve-<kind>?<curves>`. `thumb` is the small, fast one for a drag in progress.
 */
export type CurvePreviewKind = 'thumb' | 'result';

/**
 * An enhancement preview served by the `acimg` scheme (the engine's `preview_enhance`, nothing is committed or cached):
 * `/<token>/<id>/crop/<crop>/enhance-<kind>?mode=<m>&s=<0..100>`. `thumb` is the small, fast one for a slider in motion.
 */
export type EnhancePreviewKind = 'thumb' | 'result';

/** Image kinds served by the `acimg` scheme: `/<token>/<id>/<kind>?g=<gen>`. */
export type ImageKind = 'thumb' | 'src' | 'result';
/** Crop images: `/<token>/<id>/crop/<crop>/<kind>?k=<renderKey>`. */
export type CropImageKind = 'thumb' | 'result';

export type QuadPts = [Pt, Pt, Pt, Pt];

// ---- Convert (crates/engine/src/convert.rs and convert_batch.rs; the front end of ROADMAP M6.49) ----

export type ConvertTarget = 'jpg' | 'png';
export type ConvertColour = 'srgb' | 'preserve';
export type ConvertPlacement = 'replace' | 'keep' | 'folder';
export type QualityPreset = 'balanced' | 'small' | 'best';
export type ConvertQuality = { kind: 'preset'; preset: QualityPreset } | { kind: 'fixed'; value: number };

/** What a run does. Every key is optional on the wire; the UI always sends all of them. */
export interface ConvertOptions {
  target: ConvertTarget;
  colour: ConvertColour;
  quality: ConvertQuality;
  stripLocation: boolean;
  placement: ConvertPlacement;
  /** Convert files that are already in the target format again. */
  reencode: boolean;
}

/** The format names the engine reports. */
export type ConvertFormat = 'jpeg' | 'png' | 'tiff' | 'webp' | 'heic' | 'avif' | 'gif' | 'bmp' | 'jxl' | 'unknown';

export interface ConvertAdded {
  added: number;
  duplicates: number;
  linksSkipped: number;
  truncated: boolean;
  ids: number[];
}

export interface ConvertFileView {
  id: number;
  /** Display name only, never a path. */
  name: string;
  format: string;
  bytes: number;
}

export interface ConvertFormatCount {
  format: string;
  count: number;
}

export interface ConvertCannot {
  id: number;
  name: string;
  format: string;
  /** `HEIF_UNAVAILABLE` or `UNSUPPORTED_FORMAT`. */
  code: string;
}

export interface ConvertPreview {
  total: number;
  willConvert: number;
  sameFormat: number;
  cannot: number;
  byFormat: ConvertFormatCount[];
  cannotFiles: ConvertCannot[];
  heifAvailable: boolean;
  heifUnavailable: number;
  bytesToConvert: number;
  placement: ConvertPlacement;
  /** The chosen output folder's name (never its path); null until one is chosen. */
  folder: string | null;
}

export type ConvertStatus = 'converted' | 'skipped' | 'failed';

export interface ConvertFileResult {
  id: number;
  name: string;
  status: ConvertStatus;
  /** For a skip or a failure: `SAME_FORMAT`, `NOT_REPLACEABLE`, `HEIF_UNAVAILABLE`, `UNSUPPORTED_FORMAT`, `CANCELLED` or an `ErrorCode`. */
  code: string | null;
  notices: string[];
  /** The file name the converted picture was written as. */
  output: string | null;
  from: string;
  to: string | null;
  bytesIn: number;
  bytesOut: number;
  width: number | null;
  height: number | null;
  /** The backup that holds the original (replace mode only). */
  backupId: string | null;
}

export interface ConvertProgress {
  done: number;
  total: number;
  result: ConvertFileResult;
}

export interface ConvertSummary {
  /** Also the id of the backup run in the Backups view. */
  runId: string;
  converted: number;
  skipped: number;
  failed: number;
  cancelled: boolean;
  bytesIn: number;
  bytesOut: number;
  results: ConvertFileResult[];
}

/** The commands the shell exposes (Tauri `invoke` names, snake_case). */
export interface Api {
  launchInfo(): Promise<LaunchInfo>;
  pickFiles(): Promise<OpenSummary>;
  pickFolder(includeSubfolders: boolean): Promise<OpenSummary>;
  addSamples(): Promise<OpenSummary>;
  listItems(): Promise<ItemView[]>;
  /** `phase: 'live'` updates the state for a drag in progress (no history, no render); `'end'` commits one history entry. */
  setEdit(id: number, edit: Edit, phase: 'live' | 'end', label: string): Promise<ItemView>;
  undo(id: number): Promise<ItemView>;
  redo(id: number): Promise<ItemView>;
  resetToAuto(id: number): Promise<ItemView>;
  /** Failed items: places an editable quad inset ~5%. */
  drawCrop(id: number): Promise<ItemView>;
  removeItems(ids: number[]): Promise<void>;
  saveItems(ids: number[], target: SaveTarget, runName: string): Promise<SaveOutcome[]>;
  getSettings(): Promise<Settings>;
  setSettings(s: Settings): Promise<Settings>;
  listBackups(): Promise<BackupsView>;
  restoreFile(fileId: string, mode: RestoreMode): Promise<RestoreOutcome>;
  restoreRun(runId: string): Promise<RestoreOutcome[]>;
  pinRun(runId: string, pinned: boolean): Promise<void>;
  purgeNow(): Promise<number>;
  openBackupsFolder(): Promise<void>;

  // ---- multi-item operations (M10.34). Each is ONE undo step and returns the new view of the image. ----
  /** `phase: 'live'` records nothing; `gesture` coalesces a nudge burst into one step per crop. */
  setCropEdit(id: number, crop: number, edit: Edit, phase: 'live' | 'end', label: string, gesture: number | null): Promise<ItemView>;
  /** `quad` if a box was drawn, else the item found at `at`, else a box around `at`, else the frame inset 20%. */
  addCrop(id: number, quad: QuadPts | null, at: Pt | null): Promise<ItemView>;
  removeCrop(id: number, crop: number): Promise<ItemView>;
  restoreCrop(id: number, crop: number): Promise<ItemView>;
  mergeCrops(id: number, crops: number[]): Promise<ItemView>;
  cutCrop(id: number, crop: number, cut: Cut): Promise<ItemView>;
  /** `toIndex` is the index in the full `crops` list (included or not). */
  moveCrop(id: number, crop: number, toIndex: number): Promise<ItemView>;
  useReadingOrder(id: number): Promise<ItemView>;
  turnCrop(id: number, crop: number, clockwise: boolean): Promise<ItemView>;
  setCropAngle(id: number, crop: number, deg: number, gesture: number | null): Promise<ItemView>;
  flipCrop(id: number, crop: number): Promise<ItemView>;
  revertCrop(id: number, crop: number, to: RevertTo): Promise<ItemView>;
  redetect(id: number, patch: SplitPatch): Promise<ItemView>;
  /** One undo step of the session for all of them (`sessionUndo`). */
  redetectMany(ids: number[], patch: SplitPatch): Promise<RedetectResult[]>;
  sessionUndo(): Promise<SessionStep | null>;
  sessionRedo(): Promise<SessionStep | null>;
  acceptScan(id: number): Promise<ItemView>;
  unacceptScan(id: number): Promise<ItemView>;
  restoreFileDerived(fileId: string, mode: RestoreMode, derived: DerivedAction): Promise<RestoreOutcome>;
  restoreRunDerived(runId: string, derived: DerivedAction): Promise<RestoreOutcome[]>;

  // ---- curved pages (docs/dev/curved-pages.md). Each edit is ONE undo step and returns the new view. ----
  /** A quad crop becomes a curved page with four straight edges (idempotent on a curved crop). */
  curveFromQuad(id: number, crop: number): Promise<ItemView>;
  /**
   * Replaces the curves of a quad or curved crop. `phase: 'live'` validates and returns the view without recording
   * anything; `'end'` records one step, and the same `gesture` id coalesces a burst of nudges into one step.
   */
  setCurves(id: number, crop: number, curves: CurveSet, phase: 'live' | 'end', label: string, gesture: number | null): Promise<ItemView>;
  /** A curved crop becomes the straight quad through its corners ("Back to straight"). */
  clearCurves(id: number, crop: number): Promise<ItemView>;
  /**
   * The engine's proposal of curves for a crop (5 to 25 ms on a worker). Records and applies nothing; `null` while the
   * image is not ready. Applying is `setCurves` with the suggestion's `curves`.
   */
  suggestCurves(id: number, crop: number): Promise<CurveSuggestion | null>;

  // ---- enhancement (docs/dev/enhance-engine.md). Suggested, never forced (B13). Each edit is ONE undo step. ----
  /**
   * Sets one crop's own enhancement (`strength` in percent, 0 to 100). `phase: 'live'` records nothing; `'end'` records
   * one step, and the same `gesture` id coalesces a slider drag or a burst of key presses into one.
   */
  setEnhance(id: number, crop: number, mode: EnhanceMode, strength: number, phase: 'live' | 'end', label: string, gesture: number | null): Promise<ItemView>;
  /** The crop follows the image's default again. */
  clearEnhance(id: number, crop: number): Promise<ItemView>;
  /** "All items in this scan": the default AND no crop keeps its own value, as one step. */
  setScanEnhance(id: number, mode: EnhanceMode, strength: number, phase: 'live' | 'end', label: string, gesture: number | null): Promise<ItemView>;
  /** The engine's recommendation for a crop (a few tens of ms on a worker). Records and applies nothing; `null` = leave it alone. */
  suggestEnhance(id: number, crop: number): Promise<EnhanceSuggestion | null>;
  /** "Enhance all suggested": the recommendation of every crop that has one and no look yet, as ONE session step (`sessionUndo`). */
  enhanceSuggested(ids: number[]): Promise<EnhanceBatch>;

  // ---- Convert: its own list of files (opaque ids and names), separate from the images opened for cropping. ----
  /** Tells the shell the Convert screen is showing, so a window drop goes to the Convert list (`convert-added`). */
  convertMode(on: boolean): Promise<void>;
  convertPickFiles(): Promise<ConvertAdded>;
  convertPickFolder(includeSubfolders: boolean): Promise<ConvertAdded>;
  /** Opens a folder dialog; resolves with the chosen folder's NAME (the earlier choice when cancelled, else null). */
  convertPickOutputFolder(): Promise<string | null>;
  convertList(): Promise<ConvertFileView[]>;
  convertRemove(ids: number[]): Promise<void>;
  convertClear(): Promise<void>;
  convertPreview(options: ConvertOptions): Promise<ConvertPreview>;
  /** Resolves with the summary at the end; `convert-progress` events arrive meanwhile. */
  convertRun(options: ConvertOptions, runName: string): Promise<ConvertSummary>;
  /** Stops after the files in progress. */
  convertCancel(): Promise<void>;
  /** Opens the folder a converted file went to; `null` opens the first one of the run. */
  convertOpenFolder(id: number | null): Promise<void>;
}

/** Events the shell emits (`@tauri-apps/api/event`). */
export interface Events {
  /** Items were added by a dialog, a drop or Open-with. */
  'items-added': OpenSummary;
  /** One item changed (analysis finished, save finished...). */
  'item-updated': ItemView;
  /** Files were added to the Convert list by a window drop. */
  'convert-added': ConvertAdded;
  /** One file of a Convert run finished. */
  'convert-progress': ConvertProgress;
}
