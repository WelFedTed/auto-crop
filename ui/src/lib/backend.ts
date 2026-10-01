// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The one place that knows whether the UI runs inside the Tauri shell or in a plain browser. Inside the
// shell every `Api` method is a Tauri `invoke` (snake_case command, camelCase argument keys, as in types.ts).
// In a browser (no `__TAURI_INTERNALS__`) the in-memory mock of mock.ts stands in, so the whole UI can be
// developed and tested without the Rust side.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type {
  Api,
  BackupsView,
  Edit,
  Events,
  ImageKind,
  ItemView,
  LaunchInfo,
  OpenSummary,
  RestoreMode,
  RestoreOutcome,
  SaveOutcome,
  SaveTarget,
  Settings,
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
};

export type Unlisten = () => void;

interface Impl {
  api: Api;
  imageUrl(kind: ImageKind, id: number, gen: number): string;
  on<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): Promise<Unlisten>;
  /** Mock only: a file dropped on the window in a plain browser. */
  dropFiles?: (files: File[]) => Promise<OpenSummary>;
}

let token = '';
let windows = false;

const tauriImpl: Impl = {
  api: tauriApi,
  imageUrl(kind, id, gen) {
    // PLAN 2.x: custom URI scheme `acimg`. WebView2 serves it over http://<scheme>.localhost.
    const path = `${token}/${id}/${kind}?g=${gen}`;
    return windows ? `http://acimg.localhost/${path}` : `acimg://localhost/${path}`;
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
};

/** URL of an image of an item. `gen` is part of the URL so a committed change always reloads. */
export function imageUrl(kind: ImageKind, id: number, gen: number): string {
  return impl.imageUrl(kind, id, gen);
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
