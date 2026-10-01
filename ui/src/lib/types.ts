// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The contract between the webview and the Rust shell (crates/shell). The Rust structs in
// crates/shell/src/dto.rs mirror these types one to one (serde, camelCase). The webview never sees a
// file path it could act on: items are opaque numeric ids plus a sanitised display name.

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
  | 'IMPLAUSIBLE_QUAD'; // forces Failed

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
  | 'TOO_LARGE'
  | 'UNREADABLE'
  | 'SOURCE_CHANGED'
  | 'FILE_IN_USE'
  | 'DISK_FULL'
  | 'READ_ONLY'
  | 'VERIFY_FAILED'
  | 'BACKUP_FAILED'
  | 'ORIGINAL_EXPIRED'
  | 'NO_CROP' // saving an item that has no crop (a Failed item the user has not drawn a crop for)
  | 'INTERNAL';

export interface SavedInfo {
  backupId: string | null; // null after Save as copy
  /** Display name of the file that was written. */
  output: string;
  copy: boolean;
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
  /** Null until analysed. */
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
}

export interface OpenSummary {
  added: number;
  skipped: number;
  /** Ids of the newly added items, in order. */
  ids: number[];
  /** Why files were skipped: unsupported, too large, unreadable, symlink loop... */
  skippedReasons: ErrorCode[];
}

export interface Settings {
  saveAsCopy: boolean;
  /** Days to keep backups; null = never delete. */
  retentionDays: number | null;
  /** The first-write sheet has been acknowledged. */
  firstWriteAck: boolean;
}

export type SaveTarget = 'replace' | 'copy';

export interface SaveOutcome {
  id: number;
  ok: boolean;
  error: ErrorCode | null;
  saved: SavedInfo | null;
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
}

export interface LaunchInfo {
  token: string;
  version: string;
  platform: 'windows' | 'macos' | 'linux';
  /** Which paths exist for the backup store, display only. */
  backupsLocation: string;
}

/** Image kinds served by the `acimg` scheme: `/<token>/<id>/<kind>?g=<gen>`. */
export type ImageKind = 'thumb' | 'src' | 'result';

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
}

/** Events the shell emits (`@tauri-apps/api/event`). */
export interface Events {
  /** Items were added by a dialog, a drop or Open-with. */
  'items-added': OpenSummary;
  /** One item changed (analysis finished, save finished...). */
  'item-updated': ItemView;
}
