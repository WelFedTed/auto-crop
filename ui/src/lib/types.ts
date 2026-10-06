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
  | 'ITEM_OP' // a refused item operation; nothing changed
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

/** Image kinds served by the `acimg` scheme: `/<token>/<id>/<kind>?g=<gen>`. */
export type ImageKind = 'thumb' | 'src' | 'result';
/** Crop images: `/<token>/<id>/crop/<crop>/<kind>?k=<renderKey>`. */
export type CropImageKind = 'thumb' | 'result';

export type QuadPts = [Pt, Pt, Pt, Pt];

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
}

/** Events the shell emits (`@tauri-apps/api/event`). */
export interface Events {
  /** Items were added by a dialog, a drop or Open-with. */
  'items-added': OpenSummary;
  /** One item changed (analysis finished, save finished...). */
  'item-updated': ItemView;
}
