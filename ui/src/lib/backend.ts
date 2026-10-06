// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The one place that knows whether the UI runs inside the Tauri shell or in a plain browser. Inside the
// shell every `Api` method is a Tauri `invoke` (snake_case command, camelCase argument keys, as in types.ts).
// In a browser (no `__TAURI_INTERNALS__`) the in-memory mock of mock.ts stands in, so the whole UI can be
// developed and tested without the Rust side. Ids only cross the boundary: no pixels and no paths.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { curveQuery } from './curve.ts';
import type {
  Api,
  BackupsView,
  CropImageKind,
  CurvePreviewKind,
  CurveSet,
  Cut,
  DerivedAction,
  Edit,
  Events,
  ImageKind,
  ItemView,
  LaunchInfo,
  OpenSummary,
  QuadPts,
  RedetectResult,
  RestoreMode,
  RestoreOutcome,
  RevertTo,
  SaveOutcome,
  SaveTarget,
  SessionStep,
  Settings,
  SplitPatch,
  Pt,
} from './types.ts';

export const isMock: boolean = typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window);

const tauriApi: Api = {
  launchInfo: () => invoke<LaunchInfo>('launch_info'),
  pickFiles: () => invoke<OpenSummary>('pick_files'),
  pickFolder: (includeSubfolders: boolean) => invoke<OpenSummary>('pick_folder', { includeSubfolders }),
  addSamples: () => invoke<OpenSummary>('add_samples'),
  listItems: () => invoke<ItemView[]>('list_items'),
  setEdit: (id: number, edit: Edit, phase: 'live' | 'end', label: string) =>
    invoke<ItemView>('set_edit', { id, edit, phase, label }),
  undo: (id: number) => invoke<ItemView>('undo', { id }),
  redo: (id: number) => invoke<ItemView>('redo', { id }),
  resetToAuto: (id: number) => invoke<ItemView>('reset_to_auto', { id }),
  drawCrop: (id: number) => invoke<ItemView>('draw_crop', { id }),
  removeItems: (ids: number[]) => invoke<void>('remove_items', { ids }),
  saveItems: (ids: number[], target: SaveTarget, runName: string) =>
    invoke<SaveOutcome[]>('save_items', { ids, target, runName }),
  getSettings: () => invoke<Settings>('get_settings'),
  // The argument key is the parameter name used in the `Api` interface (`s`).
  setSettings: (s: Settings) => invoke<Settings>('set_settings', { s }),
  listBackups: () => invoke<BackupsView>('list_backups'),
  restoreFile: (fileId: string, mode: RestoreMode) => invoke<RestoreOutcome>('restore_file', { fileId, mode }),
  restoreRun: (runId: string) => invoke<RestoreOutcome[]>('restore_run', { runId }),
  pinRun: (runId: string, pinned: boolean) => invoke<void>('pin_run', { runId, pinned }),
  purgeNow: () => invoke<number>('purge_now'),
  openBackupsFolder: () => invoke<void>('open_backups_folder'),

  setCropEdit: (id: number, crop: number, edit: Edit, phase: 'live' | 'end', label: string, gesture: number | null) =>
    invoke<ItemView>('set_crop_edit', { id, crop, edit, phase, label, gesture }),
  addCrop: (id: number, quad: QuadPts | null, at: Pt | null) => invoke<ItemView>('add_crop', { id, quad, at }),
  removeCrop: (id: number, crop: number) => invoke<ItemView>('remove_crop', { id, crop }),
  restoreCrop: (id: number, crop: number) => invoke<ItemView>('restore_crop', { id, crop }),
  mergeCrops: (id: number, crops: number[]) => invoke<ItemView>('merge_crops', { id, crops }),
  cutCrop: (id: number, crop: number, cut: Cut) => invoke<ItemView>('cut_crop', { id, crop, cut }),
  moveCrop: (id: number, crop: number, toIndex: number) => invoke<ItemView>('move_crop', { id, crop, toIndex }),
  useReadingOrder: (id: number) => invoke<ItemView>('use_reading_order', { id }),
  turnCrop: (id: number, crop: number, clockwise: boolean) => invoke<ItemView>('turn_crop', { id, crop, clockwise }),
  setCropAngle: (id: number, crop: number, deg: number, gesture: number | null) =>
    invoke<ItemView>('set_crop_angle', { id, crop, deg, gesture }),
  flipCrop: (id: number, crop: number) => invoke<ItemView>('flip_crop', { id, crop }),
  revertCrop: (id: number, crop: number, to: RevertTo) => invoke<ItemView>('revert_crop', { id, crop, to }),
  redetect: (id: number, patch: SplitPatch) => invoke<ItemView>('redetect', { id, patch }),
  redetectMany: (ids: number[], patch: SplitPatch) => invoke<RedetectResult[]>('redetect_many', { ids, patch }),
  sessionUndo: () => invoke<SessionStep | null>('session_undo'),
  sessionRedo: () => invoke<SessionStep | null>('session_redo'),
  acceptScan: (id: number) => invoke<ItemView>('accept_scan', { id }),
  unacceptScan: (id: number) => invoke<ItemView>('unaccept_scan', { id }),
  restoreFileDerived: (fileId: string, mode: RestoreMode, derived: DerivedAction) =>
    invoke<RestoreOutcome>('restore_file_derived', { fileId, mode, derived }),
  restoreRunDerived: (runId: string, derived: DerivedAction) =>
    invoke<RestoreOutcome[]>('restore_run_derived', { runId, derived }),

  curveFromQuad: (id: number, crop: number) => invoke<ItemView>('curve_from_quad', { id, crop }),
  setCurves: (id: number, crop: number, curves: CurveSet, phase: 'live' | 'end', label: string, gesture: number | null) =>
    invoke<ItemView>('set_curves', { id, crop, curves, phase, label, gesture }),
  clearCurves: (id: number, crop: number) => invoke<ItemView>('clear_curves', { id, crop }),
};

export type Unlisten = () => void;

interface Impl {
  api: Api;
  imageUrl(kind: ImageKind, id: number, gen: number): string;
  cropImageUrl(kind: CropImageKind, id: number, crop: number, renderKey: string): string;
  /** The engine's flattened preview of a candidate curve set, rendered without committing anything. */
  curvePreviewUrl(kind: CurvePreviewKind, id: number, crop: number, curves: CurveSet): string;
  on<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): Promise<Unlisten>;
  /** Mock only: a file dropped on the window in a plain browser. */
  dropFiles?: (files: File[]) => Promise<OpenSummary>;
}

let token = '';
let windows = false;

function schemeUrl(path: string): string {
  // Custom URI scheme `acimg`. WebView2 serves it over http://<scheme>.localhost.
  return windows ? `http://acimg.localhost/${path}` : `acimg://localhost/${path}`;
}

const tauriImpl: Impl = {
  api: tauriApi,
  imageUrl(kind, id, gen) {
    return schemeUrl(`${token}/${id}/${kind}?g=${gen}`);
  },
  cropImageUrl(kind, id, crop, renderKey) {
    return schemeUrl(`${token}/${id}/crop/${crop}/${kind}?k=${encodeURIComponent(renderKey)}`);
  },
  curvePreviewUrl(kind, id, crop, curves) {
    // The curves ride in the query (digits and commas only); the Rust side parses them strictly and renders from the
    // display proxy at preview size. Nothing is committed or cached; a newer request cancels an older one.
    return schemeUrl(`${token}/${id}/crop/${crop}/curve-${kind}?${curveQuery(curves)}`);
  },
  async on(event, cb) {
    const un = await listen<Events[typeof event]>(event, (e) => cb(e.payload));
    return un;
  },
};

let impl: Impl = tauriImpl;
let launch: LaunchInfo | null = null;

/** Picks the implementation, fetches launch info (token) and returns it. Call once, before the first render. */
export async function initBackend(): Promise<LaunchInfo> {
  if (isMock) {
    const m = await import('./mock.ts');
    impl = m.mockImpl;
  } else {
    impl = tauriImpl;
  }
  launch = await impl.api.launchInfo();
  token = launch.token;
  windows = launch.platform === 'windows';
  return launch;
}

export const api: Api = {
  launchInfo: () => impl.api.launchInfo(),
  pickFiles: () => impl.api.pickFiles(),
  pickFolder: (sub) => impl.api.pickFolder(sub),
  addSamples: () => impl.api.addSamples(),
  listItems: () => impl.api.listItems(),
  setEdit: (id, edit, phase, label) => impl.api.setEdit(id, edit, phase, label),
  undo: (id) => impl.api.undo(id),
  redo: (id) => impl.api.redo(id),
  resetToAuto: (id) => impl.api.resetToAuto(id),
  drawCrop: (id) => impl.api.drawCrop(id),
  removeItems: (ids) => impl.api.removeItems(ids),
  saveItems: (ids, target, runName) => impl.api.saveItems(ids, target, runName),
  getSettings: () => impl.api.getSettings(),
  setSettings: (s) => impl.api.setSettings(s),
  listBackups: () => impl.api.listBackups(),
  restoreFile: (fileId, mode) => impl.api.restoreFile(fileId, mode),
  restoreRun: (runId) => impl.api.restoreRun(runId),
  pinRun: (runId, pinned) => impl.api.pinRun(runId, pinned),
  purgeNow: () => impl.api.purgeNow(),
  openBackupsFolder: () => impl.api.openBackupsFolder(),

  setCropEdit: (id, crop, edit, phase, label, gesture) => impl.api.setCropEdit(id, crop, edit, phase, label, gesture),
  addCrop: (id, quad, at) => impl.api.addCrop(id, quad, at),
  removeCrop: (id, crop) => impl.api.removeCrop(id, crop),
  restoreCrop: (id, crop) => impl.api.restoreCrop(id, crop),
  mergeCrops: (id, crops) => impl.api.mergeCrops(id, crops),
  cutCrop: (id, crop, cut) => impl.api.cutCrop(id, crop, cut),
  moveCrop: (id, crop, toIndex) => impl.api.moveCrop(id, crop, toIndex),
  useReadingOrder: (id) => impl.api.useReadingOrder(id),
  turnCrop: (id, crop, clockwise) => impl.api.turnCrop(id, crop, clockwise),
  setCropAngle: (id, crop, deg, gesture) => impl.api.setCropAngle(id, crop, deg, gesture),
  flipCrop: (id, crop) => impl.api.flipCrop(id, crop),
  revertCrop: (id, crop, to) => impl.api.revertCrop(id, crop, to),
  redetect: (id, patch) => impl.api.redetect(id, patch),
  redetectMany: (ids, patch) => impl.api.redetectMany(ids, patch),
  sessionUndo: () => impl.api.sessionUndo(),
  sessionRedo: () => impl.api.sessionRedo(),
  acceptScan: (id) => impl.api.acceptScan(id),
  unacceptScan: (id) => impl.api.unacceptScan(id),
  restoreFileDerived: (fileId, mode, derived) => impl.api.restoreFileDerived(fileId, mode, derived),
  restoreRunDerived: (runId, derived) => impl.api.restoreRunDerived(runId, derived),

  curveFromQuad: (id, crop) => impl.api.curveFromQuad(id, crop),
  setCurves: (id, crop, curves, phase, label, gesture) => impl.api.setCurves(id, crop, curves, phase, label, gesture),
  clearCurves: (id, crop) => impl.api.clearCurves(id, crop),
};

/** URL of an image of an item. `gen` is part of the URL so a committed change always reloads. */
export function imageUrl(kind: ImageKind, id: number, gen: number): string {
  return impl.imageUrl(kind, id, gen);
}

/**
 * URL of one crop of an item (M10.28). `renderKey` is the cache buster: editing crop 2 changes only crop 2's
 * key, so the other crops' images are not fetched again.
 */
export function cropImageUrl(kind: CropImageKind, id: number, crop: number, renderKey: string): string {
  return impl.cropImageUrl(kind, id, crop, renderKey);
}

/**
 * URL of the flattened preview of `curves` for one crop (the engine's `preview_curves`): `thumb` while a handle is
 * dragged, `result` for the straight-crop comparison. The committed picture is `cropImageUrl`.
 */
export function curvePreviewUrl(kind: CurvePreviewKind, id: number, crop: number, curves: CurveSet): string {
  return impl.curvePreviewUrl(kind, id, crop, curves);
}

export function onItemsAdded(cb: (s: OpenSummary) => void): Promise<Unlisten> {
  return impl.on('items-added', cb);
}

export function onItemUpdated(cb: (v: ItemView) => void): Promise<Unlisten> {
  return impl.on('item-updated', cb);
}

/** In a plain browser, files dropped on the window go to the mock. Inside the shell the Rust side handles drops. */
export function dropFilesIntoMock(files: File[]): Promise<OpenSummary> | null {
  return isMock && impl.dropFiles ? impl.dropFiles(files) : null;
}

export function launchInfoSnapshot(): LaunchInfo | null {
  return launch;
}

export type { Impl as BackendImpl };
