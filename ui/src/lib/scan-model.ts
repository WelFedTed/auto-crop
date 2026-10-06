// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// A small stand-in for the engine's multi-item edit state (crates/core/src/items.rs), used by the browser
// mock only. It follows the same rules so the UI can be exercised without the Rust side: ids are stable and
// never reused, the order of `crops` IS the output order, an excluded crop is kept (never deleted), every
// operation is refused with `ITEM_OP` or `DEGENERATE` and changes nothing, merge takes the minimum-area
// rectangle of the union, cut follows `Cut`, re-detection keeps what the person placed, edited or removed.

import {
  MAX_ITEMS,
  MIN_PIECE_FRACTION,
  areaFraction,
  boxIou,
  bounds,
  cutPieces,
  mergedQuad,
} from './geometry.ts';
import { cloneCurves, cornersOf, curvesFromQuad, validateCurves } from './curve.ts';
import type { Quad } from './quad.ts';
import type {
  Band,
  Confidence,
  CropOrigin,
  CropView,
  CurveSet,
  Cut,
  Edit,
  ErrorCode,
  OrderMode,
  ScanTriage,
  SplitPolicy,
  SplitProfile,
  SplitView,
} from './types.ts';

export interface ModelCrop {
  id: number;
  include: boolean;
  quad: Quad;
  quarterTurns: number;
  fineDeg: number;
  mirror: boolean;
  /** The four boundary curves of a curved page, else null. Its corners ARE `quad`; its turns and mirror follow the crop's. */
  curves: CurveSet | null;
  origin: CropOrigin;
  confidence: Confidence | null;
}

export interface ScanState {
  crops: ModelCrop[];
  policy: SplitPolicy;
  profile: SplitProfile;
  orderMode: OrderMode;
  nextId: number;
}

/** A refused operation: the state is unchanged. */
export class ItemOpError extends Error {
  readonly code: ErrorCode;
  constructor(code: ErrorCode = 'ITEM_OP', message = 'refused') {
    super(message);
    this.code = code;
  }
}

export const STRICT = 0.95;
export const FAILED_FLOOR = 0.6;

export function emptyState(policy: SplitPolicy = 'auto', profile: SplitProfile = 'photos'): ScanState {
  return { crops: [], policy, profile, orderMode: 'reading', nextId: 1 };
}

export function cloneCrop(c: ModelCrop): ModelCrop {
  return {
    ...c,
    quad: c.quad.map((p) => ({ x: p.x, y: p.y })) as Quad,
    curves: c.curves ? cloneCurves(c.curves) : null,
    confidence: c.confidence ? { ...c.confidence, reasons: c.confidence.reasons.map((r) => ({ ...r })) } : null,
  };
}

export function cloneState(s: ScanState): ScanState {
  return { ...s, crops: s.crops.map(cloneCrop) };
}

/** The band at the Strict cutoff: Failed below the floor or when forced; Check below the cutoff, when forced or when a hold reason fired. */
export function bandOf(c: Confidence, cutoff = STRICT): Band {
  if (c.forced === 'failed' || c.score < FAILED_FLOOR || Number.isNaN(c.score)) return 'failed';
  if (c.forced === 'check' || c.score < cutoff || c.reasons.length > 0) return 'check';
  return 'good';
}

/** A curved page is held (Check) until the person accepts this exact state; any other crop the person placed or edited is reviewed. */
export function cropBand(c: ModelCrop, accepted = false): Band | null {
  if (c.curves) return accepted ? 'good' : 'check';
  if (c.origin !== 'auto') return 'good'; // placed or edited by the person: reviewed
  return c.confidence ? bandOf(c.confidence) : null;
}

/** True when any included crop is a curved page: the scan is held for review and accepted through the engine. */
export function hasCurved(s: ScanState): boolean {
  return included(s).some((c) => !!c.curves);
}

export function included(s: ScanState): ModelCrop[] {
  return s.crops.filter((c) => c.include);
}

export function outputRank(s: ScanState, id: number): number {
  const inc = included(s);
  const i = inc.findIndex((c) => c.id === id);
  return i < 0 ? 0 : i + 1;
}

export function triage(s: ScanState): ScanTriage {
  const inc = included(s);
  if (inc.length === 0) return { kind: 'noItems' };
  const need = inc.filter((c) => cropBand(c) !== 'good').length;
  return need > 0 ? { kind: 'heldForReview', itemsNeedCheck: need } : { kind: 'approved' };
}

/** `{name}_{nn}.{ext}` for a split (zero padded to at least two digits), `{name}.{ext}` for one file. */
export function outputName(stem: string, ext: string, rank: number, total: number): string {
  if (total <= 1) return `${stem}.${ext}`;
  const width = Math.max(2, String(total).length);
  return `${stem}_${String(rank).padStart(width, '0')}.${ext}`;
}

export function outputNames(stem: string, ext: string, total: number): string[] {
  return Array.from({ length: total }, (_, i) => outputName(stem, ext, i + 1, total));
}

/** Everything that decides the pixels: acceptance and the render key are tied to it. */
export function renderSignature(s: ScanState): string {
  return JSON.stringify(
    s.crops.map((c) => [c.id, c.include, c.quad.map((p) => [round(p.x), round(p.y)]), c.quarterTurns, c.fineDeg, c.mirror, curvesSig(c.curves)]),
  );
}

export function cropKey(c: ModelCrop): string {
  const text = JSON.stringify([c.id, c.quad.map((p) => [round(p.x), round(p.y)]), c.quarterTurns, c.fineDeg, c.mirror, curvesSig(c.curves)]);
  let h = 2166136261;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 16777619) >>> 0;
  return h.toString(16).padStart(8, '0') + c.id.toString(16).padStart(4, '0');
}

const round = (v: number) => Math.round(v * 1e6) / 1e6;

/** The control points rounded like the quad: part of the render signature and the per-crop render key. */
function curvesSig(c: CurveSet | null): unknown {
  if (!c) return 0;
  const k = (p: { x: number; y: number }[]) => p.map((q) => [round(q.x), round(q.y)]);
  return [k(c.top), k(c.right), k(c.bottom), k(c.left)];
}

const clamp01 = (v: number) => Math.min(1, Math.max(0, v));

function sanitise(q: Quad): Quad {
  return q.map((p) => ({ x: clamp01(p.x), y: clamp01(p.y) })) as Quad;
}

function degenerate(q: Quad, w: number, h: number): boolean {
  return areaFraction(q, w, h) < 1e-4;
}

function find(s: ScanState, id: number): ModelCrop {
  const c = s.crops.find((x) => x.id === id);
  if (!c) throw new ItemOpError('ITEM_OP', `no crop ${id}`);
  return c;
}

export type Dims = [number, number];

/** Reading order: rows by vertical overlap (at least 50% of the shorter height), then left to right. */
export function readingOrder(boxes: { id: number; b: [number, number, number, number] }[]): number[] {
  const sorted = boxes.slice().sort((a, b) => (a.b[1] + a.b[3]) / 2 - (b.b[1] + b.b[3]) / 2 || a.id - b.id);
  const rows: { top: number; bottom: number; members: { cx: number; id: number }[] }[] = [];
  for (const { id, b } of sorted) {
    const row = rows.find((r) => {
      const overlap = Math.min(b[3], r.bottom) - Math.max(b[1], r.top);
      const shorter = Math.max(1e-9, Math.min(b[3] - b[1], r.bottom - r.top));
      return overlap / shorter >= 0.5;
    });
    const cx = (b[0] + b[2]) / 2;
    if (row) row.members.push({ cx, id });
    else rows.push({ top: b[1], bottom: b[3], members: [{ cx, id }] });
  }
  return rows.flatMap((r) => r.members.sort((a, b) => a.cx - b.cx || a.id - b.id).map((m) => m.id));
}

function sortReading(s: ScanState): void {
  const order = readingOrder(
    s.crops.map((c) => {
      const b = bounds(c.quad);
      return { id: c.id, b: [b.x0, b.y0, b.x1, b.y1] as [number, number, number, number] };
    }),
  );
  s.crops = order.map((id) => s.crops.find((c) => c.id === id)!);
}

function resortIfReading(s: ScanState): void {
  if (s.orderMode === 'reading') sortReading(s);
}

function newCrop(s: ScanState, quad: Quad, origin: CropOrigin, confidence: Confidence | null = null): ModelCrop {
  return { id: s.nextId++, include: true, quad: sanitise(quad), quarterTurns: 0, fineDeg: 0, mirror: false, curves: null, origin, confidence };
}

// ------------------------------------------------------------------------------------------ operations
// Every operation returns a NEW state, or throws `ItemOpError` and leaves the old one alone.

export function addCrop(s: ScanState, quad: Quad, origin: CropOrigin, dims: Dims, confidence: Confidence | null = null): { state: ScanState; id: number } {
  const next = cloneState(s);
  if (next.crops.length >= MAX_ITEMS) throw new ItemOpError('ITEM_OP', 'too many items');
  if (degenerate(quad, dims[0], dims[1])) throw new ItemOpError('DEGENERATE');
  const c = newCrop(next, quad, origin, confidence);
  next.crops.push(c);
  resortIfReading(next);
  return { state: next, id: c.id };
}

export function setInclude(s: ScanState, id: number, include: boolean): ScanState {
  const next = cloneState(s);
  find(next, id).include = include;
  return next;
}

export function editCrop(s: ScanState, id: number, edit: Edit, dims: Dims): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  // A quad edit would silently drop the curves, so the engine refuses it: the person goes back to straight first.
  if (c.curves) throw new ItemOpError('ITEM_OP', 'a curved crop is edited through its curves');
  const quad = sanitise(edit.quad);
  if (degenerate(quad, dims[0], dims[1])) throw new ItemOpError('DEGENERATE');
  c.quad = quad;
  c.quarterTurns = ((edit.quarterTurns % 4) + 4) % 4;
  c.fineDeg = Math.min(45, Math.max(-45, Number.isFinite(edit.fineDeg) ? edit.fineDeg : 0));
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

export function turnCrop(s: ScanState, id: number, clockwise: boolean): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  c.quarterTurns = (c.quarterTurns + (clockwise ? 1 : 3)) % 4;
  if (c.curves) c.curves.quarterTurns = c.quarterTurns;
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

export function angleCrop(s: ScanState, id: number, deg: number): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  if (c.curves) throw new ItemOpError('ITEM_OP', 'a curved page has no fine angle');
  c.fineDeg = Math.min(45, Math.max(-45, Number.isFinite(deg) ? deg : 0));
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

export function flipCrop(s: ScanState, id: number): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  c.mirror = !c.mirror;
  if (c.curves) c.curves.mirror = c.mirror;
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

/** Merges two or more included crops into the minimum-area rectangle of their union; the merged crops are deleted (undo restores them). */
export function mergeCrops(s: ScanState, ids: number[], dims: Dims): { state: ScanState; id: number } {
  const uniq = [...new Set(ids)];
  if (uniq.length < 2) throw new ItemOpError('ITEM_OP', 'merging needs at least two items');
  const next = cloneState(s);
  const crops = uniq.map((id) => find(next, id));
  if (crops.some((c) => !c.include)) throw new ItemOpError('ITEM_OP', 'an excluded item cannot be merged');
  if (crops.some((c) => c.curves)) throw new ItemOpError('ITEM_OP', 'a curved page cannot be merged');
  const rect = mergedQuad(
    crops.map((c) => c.quad),
    dims[0],
    dims[1],
  );
  if (!rect || degenerate(rect, dims[0], dims[1])) throw new ItemOpError('DEGENERATE');
  const at = Math.min(...crops.map((c) => next.crops.indexOf(c)));
  const base = next.crops[at];
  const merged: ModelCrop = {
    id: next.nextId++,
    include: true,
    quad: rect,
    quarterTurns: base.quarterTurns,
    fineDeg: 0,
    mirror: base.mirror,
    curves: null,
    origin: 'manual',
    confidence: null,
  };
  next.crops[at] = merged;
  next.crops = next.crops.filter((c) => c.id === merged.id || !uniq.includes(c.id));
  return { state: next, id: merged.id };
}

/** Cuts an included crop in two; both pieces must cover at least 2% of the scan. */
export function cutCrop(s: ScanState, id: number, cut: Cut, dims: Dims): { state: ScanState; ids: [number, number] } {
  const next = cloneState(s);
  const at = next.crops.findIndex((c) => c.id === id);
  if (at < 0) throw new ItemOpError('ITEM_OP', 'no such item');
  const c = next.crops[at];
  if (!c.include) throw new ItemOpError('ITEM_OP', 'an excluded item cannot be cut');
  if (c.curves) throw new ItemOpError('ITEM_OP', 'a curved page cannot be cut');
  if (next.crops.length + 1 > MAX_ITEMS) throw new ItemOpError('ITEM_OP', 'too many items');
  const pieces = cutPieces(c.quad, cut);
  if (!pieces) throw new ItemOpError('ITEM_OP', 'bad cut');
  for (const p of pieces) if (areaFraction(p, dims[0], dims[1]) < MIN_PIECE_FRACTION) throw new ItemOpError('ITEM_OP', 'piece too small');
  const make = (q: Quad): ModelCrop => ({ ...cloneCrop(c), id: next.nextId++, quad: q, origin: 'manual', confidence: null });
  const a = make(pieces[0]);
  const b = make(pieces[1]);
  next.crops[at] = a;
  next.crops.splice(at + 1, 0, b);
  return { state: next, ids: [a.id, b.id] };
}

/** Moves a crop to `to` (an index into the full list) and switches to manual order. */
export function moveCrop(s: ScanState, id: number, to: number): ScanState {
  const next = cloneState(s);
  const from = next.crops.findIndex((c) => c.id === id);
  if (from < 0) throw new ItemOpError('ITEM_OP', 'no such item');
  const [c] = next.crops.splice(from, 1);
  next.crops.splice(Math.min(Math.max(0, to), next.crops.length), 0, c);
  next.orderMode = 'manual';
  return next;
}

export function useReadingOrder(s: ScanState): ScanState {
  const next = cloneState(s);
  next.orderMode = 'reading';
  sortReading(next);
  return next;
}

/** Reverts one crop to what it is in `baseline` (geometry, inclusion, provenance, confidence); nothing else changes. */
export function revertCrop(s: ScanState, id: number, baseline: ScanState): ScanState {
  const src = baseline.crops.find((c) => c.id === id);
  if (!src) throw new ItemOpError('ITEM_OP', 'not in the baseline');
  const next = cloneState(s);
  const at = next.crops.findIndex((c) => c.id === id);
  if (at < 0) throw new ItemOpError('ITEM_OP', 'no such item');
  next.crops[at] = cloneCrop(src);
  return next;
}

/**
 * Re-detection: `detected` are new auto crops (their ids are ignored). Crops the person placed, edited or
 * removed are kept exactly; an untouched auto crop is replaced by the detection that matches it (keeping its
 * id) or dropped; a detection that overlaps something the person kept is dropped; the rest are added.
 */
export function redetect(s: ScanState, detected: { quad: Quad; confidence: Confidence | null }[], dims: Dims): ScanState {
  const next = cloneState(s);
  const userKept = (c: ModelCrop) => c.origin !== 'auto' || !c.include;
  const pool = detected.map((d) => ({ ...d, used: false }));
  const out: ModelCrop[] = [];
  for (const c of next.crops) {
    if (userKept(c)) {
      out.push(c);
      continue;
    }
    let best = -1;
    let bestIou = 0.7;
    pool.forEach((d, k) => {
      if (d.used) return;
      const iou = boxIou(c.quad, d.quad);
      if (iou >= bestIou) {
        best = k;
        bestIou = iou;
      }
    });
    if (best >= 0) {
      pool[best].used = true;
      out.push({ ...c, quad: sanitise(pool[best].quad), confidence: pool[best].confidence, quarterTurns: 0, fineDeg: 0, mirror: false, curves: null, include: true, origin: 'auto' });
    }
  }
  next.crops = out;
  for (const d of pool) {
    if (d.used) continue;
    const overlaps = next.crops.some((k) => userKept(k) && boxIou(k.quad, d.quad) >= 0.3);
    if (overlaps || next.crops.length >= MAX_ITEMS || degenerate(d.quad, dims[0], dims[1])) continue;
    next.crops.push(newCrop(next, d.quad, 'auto', d.confidence));
  }
  resortIfReading(next);
  return next;
}

// ------------------------------------------------------------------------------------------ curved pages

/** The corners of `quad` with the fine angle baked in: the quad turned about its centre in pixel space. */
export function bakeAngle(quad: Quad, deg: number, dims: Dims): Quad {
  if (Math.abs(deg) < 1e-9) return quad.map((p) => ({ x: p.x, y: p.y })) as Quad;
  const th = (-deg * Math.PI) / 180; // the result is rotated clockwise by `deg`: the source quad by the opposite
  const cos = Math.cos(th);
  const sin = Math.sin(th);
  const cx = quad.reduce((a, p) => a + p.x, 0) / 4;
  const cy = quad.reduce((a, p) => a + p.y, 0) / 4;
  return quad.map((p) => {
    const dx = (p.x - cx) * dims[0];
    const dy = (p.y - cy) * dims[1];
    return { x: cx + (dx * cos - dy * sin) / dims[0], y: cy + (dx * sin + dy * cos) / dims[1] };
  }) as Quad;
}

/** A quad crop becomes a curved page with four straight edges (idempotent on a curved crop). */
export function curveCrop(s: ScanState, id: number, dims: Dims): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  if (c.curves) return next;
  const corners = sanitise(bakeAngle(c.quad, c.fineDeg, dims));
  c.quad = corners;
  c.fineDeg = 0;
  c.curves = curvesFromQuad(corners, c.quarterTurns, c.mirror);
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

/** Replaces the curves of a quad or curved crop; the corners move with the end points. A refused set changes nothing. */
export function setCurves(s: ScanState, id: number, curves: CurveSet): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  if (validateCurves(curves) !== null) throw new ItemOpError('DEGENERATE');
  c.curves = cloneCurves(curves);
  c.quad = cornersOf(curves);
  c.quarterTurns = ((curves.quarterTurns % 4) + 4) % 4;
  c.mirror = curves.mirror;
  c.fineDeg = 0;
  if (c.origin === 'auto') c.origin = 'autoThenEdited';
  return next;
}

/** A curved crop becomes the straight quad through its corners (turns and mirror kept). */
export function clearCurves(s: ScanState, id: number): ScanState {
  const next = cloneState(s);
  const c = find(next, id);
  if (!c.curves) return next;
  c.curves = null;
  return next;
}

// ----------------------------------------------------------------------------------------------- views
export function toEdit(c: ModelCrop): Edit {
  return { quad: c.quad.map((p) => ({ x: p.x, y: p.y })) as Quad, quarterTurns: c.quarterTurns, fineDeg: c.fineDeg };
}

export interface ViewContext {
  stem: string;
  ext: string;
  baseline: ScanState | null;
  /** The person accepted exactly this state: a curved page is no longer held. */
  accepted?: boolean;
}

export function cropViews(s: ScanState, ctx: ViewContext): CropView[] {
  const total = included(s).length;
  return s.crops.map((c) => {
    const order = outputRank(s, c.id);
    const base = ctx.baseline?.crops.find((b) => b.id === c.id) ?? null;
    return {
      id: c.id,
      order,
      include: c.include,
      edit: toEdit(c),
      autoEdit: base ? toEdit(base) : null,
      mirror: c.mirror,
      curves: c.curves ? { ...cloneCurves(c.curves), quarterTurns: c.quarterTurns, mirror: c.mirror } : null,
      origin: c.origin,
      confidence: c.confidence ? { ...c.confidence, reasons: c.confidence.reasons.map((r) => ({ ...r })) } : null,
      band: cropBand(c, ctx.accepted),
      edited:
        !base ||
        base.include !== c.include ||
        base.origin !== c.origin ||
        JSON.stringify([base.quad, base.quarterTurns, base.fineDeg, base.mirror, base.curves]) !== JSON.stringify([c.quad, c.quarterTurns, c.fineDeg, c.mirror, c.curves]),
      outputName: order > 0 ? outputName(ctx.stem, ctx.ext, order, total) : null,
      renderKey: cropKey(c),
    };
  });
}

/** The state differs from what the detector proposed (any crop added, removed, edited or reordered). */
export function isEdited(s: ScanState, baseline: ScanState | null): boolean {
  if (!baseline) return false;
  const sig = (x: ScanState) =>
    JSON.stringify([x.policy, x.profile, x.crops.map((c) => [c.id, c.include, c.origin, c.quad, c.quarterTurns, c.fineDeg, c.mirror, c.curves])]);
  return sig(s) !== sig(baseline);
}

export function splitView(s: ScanState, accepted: boolean, savedAsGroup: boolean): SplitView {
  const inc = included(s).length;
  return {
    policy: s.policy,
    profile: s.profile,
    orderMode: s.orderMode,
    triage: triage(s),
    accepted,
    isSplit: inc >= 2 || savedAsGroup,
    included: inc,
  };
}
