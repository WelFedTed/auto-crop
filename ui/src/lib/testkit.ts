// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Test helpers shared by the unit tests (not imported by the app).

import type { Confidence, Edit, ItemView, Reason } from './types.ts';

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
