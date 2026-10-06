// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Curved pages in the scan model (the browser mock's stand-in for crates/core/src/items.rs) and in the screens'
// rules (items.ts, review.ts): the engine's behaviour as the UI relies on it.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { curvedDetectedQuad, curvedEdges, curvedPageCoords, curvedPagePoint, CURVED_H, CURVED_W } from './curved-model.ts';
import { insertPointAt, movePoint, validateCurves } from './curve.ts';
import {
  bannerState,
  chipName,
  isCurvedScan,
  isHeldScan,
  mayAutoSave,
  saveGate,
} from './items.ts';
import { canAccept, classify, needsReview, reasonLine, saveCandidates, tierOf, tileLabel } from './review.ts';
import {
  ItemOpError,
  addCrop,
  angleCrop,
  clearCurves,
  cropViews,
  curveCrop,
  cutCrop,
  editCrop,
  emptyState,
  flipCrop,
  hasCurved,
  mergeCrops,
  renderSignature,
  revertCrop,
  setCurves,
  triage,
  turnCrop,
  type Dims,
  type ScanState,
} from './scan-model.ts';
import { crop, defaultSettings, item, sampleEdit, scan } from './testkit.ts';
import type { CurveSet, ItemView, SplitView } from './types.ts';
import type { Quad } from './quad.ts';

const DIMS: Dims = [2000, 1500];
const rect = (): Quad => [
  { x: 0.2, y: 0.2 },
  { x: 0.8, y: 0.2 },
  { x: 0.8, y: 0.8 },
  { x: 0.2, y: 0.8 },
];
const good = { score: 0.98, forced: null, reasons: [] };

function page(): { s: ScanState; id: number } {
  const r = addCrop(emptyState(), rect(), 'auto', DIMS, good);
  return { s: r.state, id: r.id };
}

function bent(s: ScanState, id: number): ScanState {
  const c = curveCrop(s, id, DIMS);
  const withPoint = insertPointAt(c.crops[0].curves!, 0, 0, { x: 0.5, y: 0.2 })!;
  return setCurves(c, id, movePoint(withPoint, 0, 0, { x: 0.5, y: 0.14 }));
}

const refused = (f: () => unknown, code = 'ITEM_OP') =>
  assert.throws(f, (e: unknown) => e instanceof ItemOpError && e.code === code);

// ------------------------------------------------------------------------------------------ the model

test('Curved: a quad becomes four straight edges, the corners stay, and a second call changes nothing', () => {
  const { s, id } = page();
  const c = curveCrop(s, id, DIMS);
  const curves = c.crops[0].curves!;
  assert.equal(curves.top.length + curves.right.length + curves.bottom.length + curves.left.length, 8);
  assert.deepEqual(c.crops[0].quad, rect());
  assert.equal(c.crops[0].origin, 'autoThenEdited');
  assert.deepEqual(curveCrop(c, id, DIMS), c, 'idempotent');
  assert.equal(s.crops[0].curves, null, 'the old state is untouched');
});

test('a fine angle is baked into the corners when a crop becomes curved', () => {
  const { s, id } = page();
  const tilted = angleCrop(s, id, 5);
  const c = curveCrop(tilted, id, DIMS);
  assert.equal(c.crops[0].fineDeg, 0);
  assert.notDeepEqual(c.crops[0].quad, rect());
  // a rotation about the centre keeps the centre and the side lengths (in pixels)
  const q = c.crops[0].quad;
  const len = (a: number, b: number) => Math.hypot((q[a].x - q[b].x) * DIMS[0], (q[a].y - q[b].y) * DIMS[1]);
  assert.ok(Math.abs(len(0, 1) - 0.6 * DIMS[0]) < 1e-6 && Math.abs(len(1, 2) - 0.6 * DIMS[1]) < 1e-6);
});

test('curves replace the outline: the corners move with the end points, and a refused set changes nothing', () => {
  const { s, id } = page();
  const c = bent(s, id);
  assert.equal(c.crops[0].curves!.top.length, 3);
  const moved: CurveSet = JSON.parse(JSON.stringify(c.crops[0].curves));
  moved.top[0] = { x: 0.15, y: 0.18 };
  moved.left[moved.left.length - 1] = { x: 0.15, y: 0.18 };
  const after = setCurves(c, id, moved);
  assert.deepEqual(after.crops[0].quad[0], { x: 0.15, y: 0.18 });
  const crossing: CurveSet = JSON.parse(JSON.stringify(c.crops[0].curves));
  crossing.top[1] = { x: 0.5, y: 0.9 };
  assert.notEqual(validateCurves(crossing), null);
  refused(() => setCurves(c, id, crossing), 'DEGENERATE');
  assert.equal(c.crops[0].curves!.top[1].y, 0.14, 'the state is unchanged');
});

test('the engine refuses the quad operations on a curved crop, and only those', () => {
  const { s, id } = page();
  const c = bent(s, id);
  refused(() => editCrop(c, id, sampleEdit(), DIMS));
  refused(() => angleCrop(c, id, 3));
  refused(() => cutCrop(c, id, { axis: 'vertical', t0: 0.5, t1: 0.5 }, DIMS));
  const other = addCrop(c, [
    { x: 0.05, y: 0.05 },
    { x: 0.15, y: 0.05 },
    { x: 0.15, y: 0.15 },
    { x: 0.05, y: 0.15 },
  ], 'manual', DIMS);
  refused(() => mergeCrops(other.state, [id, other.id], DIMS));
  // turn and flip are handled: the curves stay, the turns and mirror follow
  const t = turnCrop(c, id, true);
  assert.equal(t.crops[0].curves!.quarterTurns, 1);
  assert.equal(t.crops[0].curves!.top.length, 3, 'the curves stay');
  const f = flipCrop(t, id);
  assert.equal(f.crops[0].curves!.mirror, true);
  const view = cropViews(f, { stem: 's', ext: 'jpg', baseline: null }).find((v) => v.id === id)!;
  assert.equal(view.curves!.quarterTurns, 1);
  assert.equal(view.edit!.quarterTurns, 1, 'edit is the outline with the turns');
});

test('Back to straight keeps the corners, the turns and the mirror; undo is the history, not this function', () => {
  const { s, id } = page();
  const c = flipCrop(turnCrop(bent(s, id), id, true), id);
  const back = clearCurves(c, id);
  assert.equal(back.crops[0].curves, null);
  assert.deepEqual(back.crops[0].quad, rect());
  assert.equal(back.crops[0].quarterTurns, 1);
  assert.equal(back.crops[0].mirror, true);
  assert.deepEqual(clearCurves(back, id), back, 'already straight: nothing happens');
});

test('a curved crop is Check until the exact state is accepted, and the signature sees every change', () => {
  const { s, id } = page();
  const c = bent(s, id);
  const held = cropViews(c, { stem: 's', ext: 'jpg', baseline: s });
  assert.equal(held[0].band, 'check');
  assert.deepEqual(triage(c), { kind: 'heldForReview', itemsNeedCheck: 1 });
  const ok = cropViews(c, { stem: 's', ext: 'jpg', baseline: s, accepted: true });
  assert.equal(ok[0].band, 'good');
  assert.ok(hasCurved(c) && !hasCurved(s));
  const nudged = setCurves(c, id, movePoint(c.crops[0].curves!, 0, 0, { x: 0.5, y: 0.13 }));
  assert.notEqual(renderSignature(nudged), renderSignature(c), 'a curve edit is a new state: the acceptance is gone');
  assert.notEqual(held[0].renderKey, cropViews(nudged, { stem: 's', ext: 'jpg', baseline: s })[0].renderKey, 'and a new picture');
  assert.equal(held[0].edited, true);
});

test('reverting a crop to the baseline takes the curves away with it', () => {
  const { s, id } = page();
  const c = bent(s, id);
  const r = revertCrop(c, id, s);
  assert.equal(r.crops[0].curves, null);
});

// ------------------------------------------------------------------------------------------ the sample page

test('the curved sample: the model inverts, the edges meet at the corners and the page is a valid curve set', () => {
  for (const [s, t] of [[0.2, 0.3], [0.9, 0.1], [0.5, 0.5], [0.05, 0.95]]) {
    const p = curvedPagePoint(s, t);
    const back = curvedPageCoords(p.x, p.y);
    assert.ok(Math.abs(back.s - s) < 1e-9 && Math.abs(back.t - t) < 1e-9, `${s},${t}`);
  }
  const [top, right, bottom, left] = curvedEdges(9);
  assert.deepEqual(top[0], left[left.length - 1], 'top starts at the corner where the left edge ends');
  assert.deepEqual(top[top.length - 1], right[0]);
  assert.deepEqual(bottom[0], right[right.length - 1]);
  assert.deepEqual(bottom[bottom.length - 1], left[0]);
  const set: CurveSet = { top, right, bottom, left, quarterTurns: 0, mirror: false };
  assert.equal(validateCurves(set), null);
  assert.ok(top[4].y > top[0].y + 10 / CURVED_H, 'the top edge dips in the middle');
  assert.ok(Math.abs(curvedPagePoint(0.5, 0).y - curvedPagePoint(0, 0).y - 22) < 1e-9);
  const q = curvedDetectedQuad();
  assert.ok(q.every((p) => p.x > 0 && p.x < 1 && p.y > 0 && p.y < 1));
  assert.ok(q[0].x * CURVED_W > curvedPagePoint(0, 0).x, 'the detector quad sits a little inside the corner');
});

// ------------------------------------------------------------------------------------------ the screens' rules

function curvedItem(id: number, accepted = false): ItemView {
  const c = crop(1, 1, accepted ? 'good' : 'check', {
    curves: {
      top: [{ x: 0.1, y: 0.1 }, { x: 0.5, y: 0.06 }, { x: 0.9, y: 0.1 }],
      right: [{ x: 0.9, y: 0.1 }, { x: 0.9, y: 0.9 }],
      bottom: [{ x: 0.9, y: 0.9 }, { x: 0.1, y: 0.9 }],
      left: [{ x: 0.1, y: 0.9 }, { x: 0.1, y: 0.1 }],
      quarterTurns: 0,
      mirror: false,
    },
    origin: 'autoThenEdited',
    outputName: 'receipt.jpg',
  });
  const split: SplitView = {
    policy: 'auto',
    profile: 'photos',
    orderMode: 'reading',
    triage: { kind: 'heldForReview', itemsNeedCheck: 1 },
    accepted,
    isSplit: false,
    included: 1,
  };
  return item(id, { crops: [c], split, edit: c.edit, confidence: { score: 0.99, forced: null, reasons: [] }, edited: true });
}

test('a curved page is a held scan even though it is one file', () => {
  const v = curvedItem(1);
  assert.ok(isCurvedScan(v) && isHeldScan(v));
  assert.ok(!isCurvedScan(scan(2, ['good'])));
  assert.equal(tierOf(v, 'strict'), 'check', 'Check until accepted, however confident the detector was');
  assert.equal(tierOf(v, 'aggressive'), 'check');
  assert.equal(tierOf(curvedItem(1, true), 'strict'), 'good');
  assert.ok(needsReview(v, 'strict', {}));
  assert.ok(!needsReview(curvedItem(1, true), 'strict', {}));
  assert.ok(!needsReview(v, 'strict', { 1: 'skipped' }));
  assert.ok(canAccept(v, 'strict'));
  assert.equal(reasonLine(v, 'check'), 'Curved page: review, then accept');
  assert.match(tileLabel(classify([v], 'strict', {})[0]), /curved/);
});

test('Replace original waits for the acceptance of a curved page; Save as copy never does', () => {
  assert.deepEqual(saveGate(curvedItem(1), defaultSettings()), { replace: 'accept-curved' });
  assert.deepEqual(saveGate(curvedItem(1, true), defaultSettings()), { replace: 'ok' });
  // never auto-saved, whatever the Experimental setting says
  assert.deepEqual(saveGate(curvedItem(1), defaultSettings({ autoSaveSplits: true })), { replace: 'accept-curved' });
  assert.equal(mayAutoSave(curvedItem(1), { autoSaveSplits: true, saveAsCopy: true }), false);
  assert.equal(mayAutoSave(curvedItem(1, true), { autoSaveSplits: false, saveAsCopy: false }), true);
  // an item that is open-only keeps its own reason
  assert.deepEqual(saveGate({ ...curvedItem(1), openOnly: 'tiff.multi_page' }, defaultSettings()), { replace: 'open-only', reason: 'tiff.multi_page' });
});

test('Save all writes a curved page only once it is accepted', () => {
  const held = classify([curvedItem(1)], 'strict', {}, { autoSaveSplits: true, saveAsCopy: true });
  assert.equal(saveCandidates(held, { autoSaveSplits: true, saveAsCopy: true }).length, 0);
  const ok = classify([curvedItem(1, true)], 'strict', {});
  assert.equal(saveCandidates(ok).length, 1);
});

test('the banner for a curved page: review, then accepted, and a split scan keeps its own banner', () => {
  assert.deepEqual(bannerState(curvedItem(1), defaultSettings()), { kind: 'curved' });
  assert.deepEqual(bannerState(curvedItem(1, true), defaultSettings()), { kind: 'curvedAccepted' });
  assert.deepEqual(bannerState(scan(3, ['good']), defaultSettings()), { kind: 'none' });
  assert.equal(bannerState(scan(4, ['good', 'check']), defaultSettings()).kind, 'held');
});

test('the chip of a curved item names it as a held page', () => {
  const v = curvedItem(1);
  assert.match(chipName(v.crops[0], 'Check', null, 'removed'), /Item 1, Check/);
});
