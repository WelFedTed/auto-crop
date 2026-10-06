// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Test helpers shared by the unit tests (not imported by the app).

import type { Band, Confidence, CropView, Edit, ItemView, Reason, ScanTriage, Settings, SplitView } from './types.ts';

export const sampleEdit = (): Edit => ({
  quad: [
    { x: 0.1, y: 0.1 },
    { x: 0.9, y: 0.1 },
    { x: 0.9, y: 0.9 },
    { x: 0.1, y: 0.9 },
  ],
  quarterTurns: 0,
  fineDeg: 0,
});

export const defaultSettings = (patch: Partial<Settings> = {}): Settings => ({
  saveAsCopy: false,
  retentionDays: 30,
  firstWriteAck: true,
  splitPolicy: 'auto',
  splitProfile: 'photos',
  autoSaveSplits: false,
  ...patch,
});

export function item(id: number, patch: Partial<ItemView> = {}): ItemView {
  return {
    id,
    name: `IMG_${String(id).padStart(4, '0')}.jpg`,
    width: 3000,
    height: 4000,
    status: 'ready',
    error: null,
    edit: sampleEdit(),
    autoEdit: sampleEdit(),
    confidence: { score: 0.97, forced: null, reasons: [] },
    gen: 1,
    edited: false,
    saved: null,
    dirtySinceSave: false,
    canUndo: false,
    canRedo: false,
    undoLabel: null,
    redoLabel: null,
    crops: [],
    split: null,
    historyPosition: 0,
    openOnly: null,
    ...patch,
  };
}

export function withConfidence(
  id: number,
  score: number,
  forced: Confidence['forced'] = null,
  reasons: Reason[] = [],
  patch: Partial<ItemView> = {},
): ItemView {
  return item(id, { confidence: { score, forced, reasons }, ...patch });
}

export function crop(id: number, order: number, band: Band | null = 'good', patch: Partial<CropView> = {}): CropView {
  const include = patch.include ?? order > 0;
  return {
    id,
    order: include ? order : 0,
    include,
    edit: sampleEdit(),
    autoEdit: sampleEdit(),
    mirror: false,
    origin: 'auto',
    confidence: { score: band === 'good' ? 0.98 : 0.8, forced: null, reasons: [] },
    band,
    edited: false,
    outputName: include ? `scan_${String(order).padStart(2, '0')}.jpg` : null,
    renderKey: `k${id}`,
    ...patch,
  };
}

/** A scan with several crops: `bands` gives the band of each included crop; `removed` adds excluded ones. */
export function scan(
  id: number,
  bands: Band[],
  opts: { removed?: number; accepted?: boolean; policy?: SplitView['policy']; patch?: Partial<ItemView> } = {},
): ItemView {
  const crops: CropView[] = bands.map((b, i) => crop(i + 1, i + 1, b));
  for (let r = 0; r < (opts.removed ?? 0); r++) crops.push(crop(100 + r, 0, 'check', { include: false }));
  const need = bands.filter((b) => b !== 'good').length;
  const triage: ScanTriage = bands.length === 0 ? { kind: 'noItems' } : need > 0 ? { kind: 'heldForReview', itemsNeedCheck: need } : { kind: 'approved' };
  return item(id, {
    name: `scan_${id}.jpg`,
    crops,
    edit: crops[0]?.edit ?? null,
    split: {
      policy: opts.policy ?? 'auto',
      profile: 'photos',
      orderMode: 'reading',
      triage,
      accepted: opts.accepted ?? false,
      isSplit: bands.length >= 2,
      included: bands.length,
    },
    ...opts.patch,
  });
}
