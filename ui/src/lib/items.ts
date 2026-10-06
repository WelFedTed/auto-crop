// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Pure UI rules for multi-item scans: which crops there are, which one is selected, where a move lands, what
// the save buttons may do, what the output names will be and whether a name collision happened. No Svelte, no
// DOM, so it runs under `node --test`. The engine decides everything that touches disk; these rules only
// decide what the screens offer and say.

import type { Band, CropView, ItemView, Settings } from './types.ts';

/** Included crops that have a quad, in output order. */
export function includedCrops(item: ItemView): CropView[] {
  return item.crops.filter((c) => c.include && c.edit);
}

/** Crops the person (or the detector's reject list) left out: ghosted, restorable. */
export function removedCrops(item: ItemView): CropView[] {
  return item.crops.filter((c) => !c.include);
}

/** The image will be saved as several files. */
export function isSplitScan(item: ItemView): boolean {
  return (item.split?.included ?? 0) >= 2;
}

/** An included crop is a curved page (it has boundary curves). */
export function isCurvedScan(item: ItemView): boolean {
  return item.crops.some((c) => c.include && !!c.curves);
}

/**
 * The scan waits for the person's OK before the engine replaces the original: several files from one scan, or a page
 * flattened from curves. Both are accepted through the engine (`acceptScan`) and the acceptance is bound to the exact
 * state, so any later edit asks again.
 */
export function isHeldScan(item: ItemView): boolean {
  return isSplitScan(item) || isCurvedScan(item);
}

/** The editor shows the item layer (chips, overlay) even for a single crop: it is where a missed item is added. */
export function hasItemLayer(item: ItemView): boolean {
  return item.status === 'ready' && item.crops.length > 0;
}

const RANK: Record<Band, number> = { good: 0, check: 1, failed: 2 };

/** The worst band over included crops (the scan's band: "the minimum over items"). Null when there is no included crop. */
export function worstBand(item: ItemView): Band | null {
  let worst: Band | null = null;
  for (const c of includedCrops(item)) {
    const b = c.band ?? 'check';
    if (worst === null || RANK[b] > RANK[worst]) worst = b;
  }
  return worst;
}

/** How many included crops are not Good. */
export function itemsNeedingCheck(item: ItemView): number {
  return includedCrops(item).filter((c) => (c.band ?? 'check') !== 'good').length;
}

/** Keeps a selection that still exists and is included; else the first included crop; else the first crop; else null. */
export function pickSelection(item: ItemView, current: number | null): number | null {
  const cur = item.crops.find((c) => c.id === current);
  if (cur) return cur.id;
  return includedCrops(item)[0]?.id ?? item.crops[0]?.id ?? null;
}

/** The index in the FULL crop list a crop moves to when it goes one included place earlier (-1) or later (+1); null at the ends. */
export function moveTarget(item: ItemView, cropId: number, dir: -1 | 1): number | null {
  const list = item.crops;
  const from = list.findIndex((c) => c.id === cropId);
  if (from < 0 || !list[from].include) return null;
  for (let i = from + dir; i >= 0 && i < list.length; i += dir) {
    if (list[i].include) return i;
  }
  return null;
}

/** The next included crop in output order after `cropId` (for "Merge with next"). */
export function nextIncluded(item: ItemView, cropId: number): CropView | null {
  const inc = includedCrops(item);
  const at = inc.findIndex((c) => c.id === cropId);
  return at >= 0 && at + 1 < inc.length ? inc[at + 1] : null;
}

/**
 * Where a dragged chip lands: `centres` are the horizontal centres of the chips of the included crops in output
 * order (without the dragged one), `x` the pointer. Returns the insertion index among the OTHER included crops.
 */
export function dropSlot(centres: number[], x: number): number {
  let slot = 0;
  for (const c of centres) if (x > c) slot++;
  return slot;
}

/** Converts a slot among the included crops into the `toIndex` of the full list `moveCrop` takes. */
export function slotToIndex(item: ItemView, cropId: number, slot: number): number {
  const others = item.crops.filter((c) => c.id !== cropId);
  const incOthers = others.filter((c) => c.include);
  if (incOthers.length === 0) return 0;
  if (slot >= incOthers.length) {
    // after the last included crop: right behind it in the full list
    const last = incOthers[incOthers.length - 1];
    return others.indexOf(last) + 1;
  }
  return others.indexOf(incOthers[slot]);
}

/** The output names the next split save plans, in output order. */
export function plannedNames(item: ItemView): string[] {
  return includedCrops(item)
    .map((c) => c.outputName)
    .filter((n): n is string => !!n);
}

/** `a_01.jpg ... a_04.jpg` for four names; the whole list when three or fewer. */
export function nameSummary(names: string[]): string {
  if (names.length <= 3) return names.join(', ');
  return `${names[0]} ... ${names[names.length - 1]}`;
}

/**
 * A name collision: the engine moves a whole set to another base name (`scan (2)_01.jpg`) when any planned name
 * is taken. Compares the plan with what was written; null when they agree or there is nothing to compare.
 */
export function collisionNotice(planned: string[], written: string[]): { wanted: string; got: string } | null {
  if (planned.length === 0 || written.length === 0) return null;
  if (planned.length === written.length && planned.every((n, i) => n === written[i])) return null;
  return { wanted: planned[0], got: written[0] };
}

export type SaveGate =
  | { replace: 'ok' }
  | { replace: 'accept-first' } // a split scan the person has not accepted
  | { replace: 'accept-curved' } // a curved page the person has not accepted
  | { replace: 'open-only'; reason: string } // never replaced: copy only
  | { replace: 'no-crop' };

/**
 * What "Replace original" may do for this image. "Save as copy" is always available once there is a crop (it
 * removes and overwrites nothing). Replacing a split scan needs the person's acceptance of the exact state,
 * or the experimental auto-save with every item Good; a source this build cannot write back is never replaced.
 */
export function saveGate(item: ItemView, settings: Pick<Settings, 'autoSaveSplits'>): SaveGate {
  if (item.status !== 'ready' || includedCrops(item).length === 0) return { replace: 'no-crop' };
  if (item.openOnly) return { replace: 'open-only', reason: item.openOnly };
  if (isSplitScan(item)) {
    const approved = item.split?.triage.kind === 'approved';
    if (item.split?.accepted || (settings.autoSaveSplits && approved)) return { replace: 'ok' };
    return { replace: 'accept-first' };
  }
  // A curved page is never auto-saved: only the person's acceptance of this exact state lets it replace the original.
  if (isCurvedScan(item) && !item.split?.accepted) return { replace: 'accept-curved' };
  return { replace: 'ok' };
}

/** The scan-level banner state. */
export type BannerState =
  | { kind: 'none' }
  | { kind: 'held'; items: number; need: number }
  | { kind: 'ready'; items: number } // every item Good, waiting for the person (default rule)
  | { kind: 'accepted'; items: number }
  | { kind: 'auto'; items: number } // auto-save (Experimental) may write it
  | { kind: 'curved' } // a curved page, held until accepted
  | { kind: 'curvedAccepted' }
  | { kind: 'noItems' };

export function bannerState(item: ItemView, settings: Pick<Settings, 'autoSaveSplits'>): BannerState {
  if (item.status !== 'ready' || !item.split) return { kind: 'none' };
  const sp = item.split;
  // Nothing left to write on a scan that is allowed to split: offer Draw items, Treat as one item, Skip.
  if (sp.triage.kind === 'noItems' && sp.policy !== 'never') return { kind: 'noItems' };
  if (!isSplitScan(item)) {
    if (isCurvedScan(item)) return sp.accepted ? { kind: 'curvedAccepted' } : { kind: 'curved' };
    return { kind: 'none' };
  }
  const items = sp.included;
  if (sp.accepted) return { kind: 'accepted', items };
  if (sp.triage.kind === 'heldForReview') return { kind: 'held', items, need: sp.triage.itemsNeedCheck };
  if (settings.autoSaveSplits && sp.triage.kind === 'approved') return { kind: 'auto', items };
  return { kind: 'ready', items };
}

/** Whether Save all may write this image without the person looking at it, in the given mode. */
export function mayAutoSave(item: ItemView, settings: Pick<Settings, 'autoSaveSplits' | 'saveAsCopy'>): boolean {
  if (!isHeldScan(item)) return true;
  const sp = item.split;
  if (!sp) return false;
  if (sp.accepted) return true;
  if (isCurvedScan(item)) return false; // never auto-saved, whatever the settings
  if (sp.triage.kind !== 'approved') return false;
  return settings.autoSaveSplits || settings.saveAsCopy;
}

/** Accessible name of a chip: number, band word, reason, file name. Never colour only. */
export function chipName(c: CropView, bandWord: string, reason: string | null, removedWord: string): string {
  const parts = [c.include ? `Item ${c.order}` : `Item (${removedWord})`];
  if (c.include) parts.push(bandWord);
  if (reason) parts.push(reason);
  if (c.outputName) parts.push(c.outputName);
  return parts.join(', ');
}

/** A stable gesture id for a burst of nudges on the same crop: reused while the pauses are under `gapMs`. */
export class GestureClock {
  private seq = 0;
  private lastAt = -Infinity;
  private lastKey = '';
  private readonly gapMs: number;

  constructor(gapMs = 500) {
    this.gapMs = gapMs;
  }

  next(key: string, now: number): number {
    if (key !== this.lastKey || now - this.lastAt > this.gapMs) this.seq++;
    this.lastKey = key;
    this.lastAt = now;
    return this.seq;
  }
}
