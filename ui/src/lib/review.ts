// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Pure review logic: tiers from strictness cut-offs (PLAN 6.2.4), filters, sort, counts and what Save all
// writes. No Svelte, no DOM, so it runs under `node --test`.

import type { Confidence, ItemView } from './types.ts';
import { FALLBACK_HOLD, S, errorMessage, holdTitle } from './strings.ts';

export type Strictness = 'strict' | 'balanced' | 'aggressive';
export type Tier = 'good' | 'check' | 'failed' | 'analysing';
export type Decision = 'accepted' | 'skipped';
export type DecisionMap = Record<number, Decision>;
export type Filter = 'needs' | 'all' | 'edited' | 'skipped' | 'failed' | 'saved';
export type SortKey = 'confidence' | 'name';

/** Interim cut-offs of PLAN 6.2.4 until calibration.json exists. */
export const CUTOFFS: Record<Strictness, number> = { strict: 0.95, balanced: 0.9, aggressive: 0.8 };
/** Below this score an item is always Failed, in every mode. */
export const FAILED_BELOW = 0.6;

export const STRICTNESS_ORDER: Strictness[] = ['strict', 'balanced', 'aggressive'];

export function tierFromConfidence(c: Confidence, strictness: Strictness): Exclude<Tier, 'analysing'> {
  if (c.forced === 'failed') return 'failed';
  if (c.score < FAILED_BELOW) return 'failed';
  if (c.forced === 'check') return 'check';
  return c.score >= CUTOFFS[strictness] ? 'good' : 'check';
}

export function tierOf(item: ItemView, strictness: Strictness): Tier {
  if (item.status === 'error') return 'failed';
  if (item.status === 'analysing' || !item.confidence) return 'analysing';
  return tierFromConfidence(item.confidence, strictness);
}

/** Failed tile that still shows the untouched original and the "Draw crop" banner. */
export function needsDrawCrop(item: ItemView, strictness: Strictness): boolean {
  return item.status === 'ready' && tierOf(item, strictness) === 'failed' && !item.edited;
}

/** Whether Accept makes sense: there is an outline, and a Failed item has had a crop drawn. */
export function canAccept(item: ItemView, strictness: Strictness): boolean {
  if (item.status !== 'ready' || !item.edit) return false;
  return tierOf(item, strictness) !== 'failed' || item.edited;
}

export function isSavedClean(item: ItemView): boolean {
  return item.saved !== null && !item.dirtySinceSave;
}

/** Flagged, and nobody has resolved it yet. */
export function needsReview(item: ItemView, strictness: Strictness, decisions: DecisionMap): boolean {
  const tier = tierOf(item, strictness);
  if (tier === 'analysing' || tier === 'good') return false;
  if (item.status === 'error') return false; // nothing to review; shown under Failed
  if (decisions[item.id]) return false;
  return !item.edited;
}

export interface Classified {
  item: ItemView;
  tier: Tier;
  decision: Decision | null;
  needs: boolean;
}

export function classify(items: ItemView[], strictness: Strictness, decisions: DecisionMap): Classified[] {
  return items.map((item) => ({
    item,
    tier: tierOf(item, strictness),
    decision: decisions[item.id] ?? null,
    needs: needsReview(item, strictness, decisions),
  }));
}

export interface Counts {
  total: number;
  analysing: number;
  needs: number;
  good: number;
  edited: number;
  skipped: number;
  failed: number;
  saved: number;
}

export function countsOf(list: Classified[]): Counts {
  const c: Counts = { total: list.length, analysing: 0, needs: 0, good: 0, edited: 0, skipped: 0, failed: 0, saved: 0 };
  for (const x of list) {
    if (x.tier === 'analysing') c.analysing++;
    if (x.needs) c.needs++;
    if (x.tier === 'good') c.good++;
    if (x.item.edited) c.edited++;
    if (x.decision === 'skipped') c.skipped++;
    if (x.tier === 'failed') c.failed++;
    if (x.item.saved) c.saved++;
  }
  return c;
}

export function matchesFilter(x: Classified, filter: Filter): boolean {
  switch (filter) {
    case 'needs':
      return x.needs || x.tier === 'analysing';
    case 'all':
      return true;
    case 'edited':
      return x.item.edited;
    case 'skipped':
      return x.decision === 'skipped';
    case 'failed':
      return x.tier === 'failed';
    case 'saved':
      return x.item.saved !== null;
  }
}

const TIER_RANK: Record<Tier, number> = { failed: 0, check: 1, good: 2, analysing: 3 };

function naturalCompare(a: string, b: string): number {
  return a.localeCompare(b, 'en', { numeric: true, sensitivity: 'base' });
}

export function sortClassified(list: Classified[], key: SortKey): Classified[] {
  const out = list.slice();
  if (key === 'name') {
    out.sort((a, b) => naturalCompare(a.item.name, b.item.name) || a.item.id - b.item.id);
  } else {
    out.sort((a, b) => {
      const r = TIER_RANK[a.tier] - TIER_RANK[b.tier];
      if (r !== 0) return r;
      const sa = a.item.confidence?.score ?? 1;
      const sb = b.item.confidence?.score ?? 1;
      return sa - sb || naturalCompare(a.item.name, b.item.name) || a.item.id - b.item.id;
    });
  }
  return out;
}

/** Items Save all writes: Good, accepted or edited; never skipped, never flagged-and-untouched, never saved-and-clean. */
export function saveCandidates(list: Classified[]): Classified[] {
  return list.filter((x) => {
    if (x.item.status !== 'ready' || !x.item.edit) return false;
    if (x.decision === 'skipped') return false;
    if (isSavedClean(x.item)) return false;
    if (x.tier === 'analysing') return false;
    if (x.decision === 'accepted') return true;
    if (x.tier === 'good') return true;
    return x.item.edited; // a drawn or adjusted crop counts as reviewed
  });
}

/** One line for a tile: the title of the first reason, or the generic line for the tier. */
export function reasonLine(item: ItemView, tier: Tier): string | null {
  if (tier === 'good' || tier === 'analysing') return null;
  const first = item.confidence?.reasons[0];
  if (first) return holdTitle(first);
  return FALLBACK_HOLD[tier === 'failed' ? 'failed' : 'check'].title;
}

export function isItemFlaggedFirstQueue(list: Classified[]): number[] {
  return sortClassified(
    list.filter((x) => x.needs),
    'confidence',
  ).map((x) => x.item.id);
}

/** Queue for the editor when it is opened without a grid context: flagged first, then the rest by name. */
export function defaultQueue(list: Classified[]): number[] {
  const flagged = isItemFlaggedFirstQueue(list);
  const flaggedSet = new Set(flagged);
  const rest = sortClassified(
    list.filter((x) => x.item.status === 'ready' && !flaggedSet.has(x.item.id)),
    'name',
  ).map((x) => x.item.id);
  return [...flagged, ...rest];
}

/** Next id to review after `current`: the next flagged one, wrapping once; null when none is left. */
export function nextToReview(queue: number[], current: number, needsIds: Set<number>): number | null {
  const at = queue.indexOf(current);
  const order = at < 0 ? queue : [...queue.slice(at + 1), ...queue.slice(0, at)];
  for (const id of order) if (id !== current && needsIds.has(id)) return id;
  return null;
}

/** Accessible name of a tile: status is spoken, never colour only (PLAN 6.10). */
export function tileLabel(x: Classified): string {
  const { item, tier, decision, needs } = x;
  const t = S.grid.tileStatus;
  const parts: string[] = [item.name];
  if (tier === 'analysing') {
    parts.push(t.analysing);
    return parts.join(', ');
  }
  if (item.status === 'error') {
    parts.push(t.errored, errorMessage(item.error));
    return parts.join(', ');
  }
  if (decision === 'skipped') parts.push(t.skipped);
  else if (decision === 'accepted') parts.push(t.accepted);
  else if (needs) parts.push(t.needsReview);
  else if (item.edited) parts.push(t.edited);
  else parts.push(tier === 'good' ? t.good : tier === 'failed' ? t.failed : t.needsReview);
  const reason = reasonLine(item, tier);
  if (reason && (needs || decision === 'accepted')) parts.push(reason.charAt(0).toLowerCase() + reason.slice(1));
  parts.push(item.saved ? (item.dirtySinceSave ? 'saved, changed since' : t.saved) : t.notSaved);
  return parts.join(', ');
}
