// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mulberry32 } from './rng.ts';
import {
  ItemOpError,
  addCrop,
  angleCrop,
  bandOf,
  cropViews,
  cutCrop,
  editCrop,
  emptyState,
  flipCrop,
  included,
  mergeCrops,
  moveCrop,
  outputName,
  outputNames,
  readingOrder,
  redetect,
  renderSignature,
  revertCrop,
  setInclude,
  splitView,
  triage,
  turnCrop,
  useReadingOrder,
  type Dims,
  type ScanState,
} from './scan-model.ts';
import type { Quad } from './quad.ts';
import type { Confidence } from './types.ts';

const DIMS: Dims = [2000, 1500];
const rect = (x0: number, y0: number, x1: number, y1: number): Quad => [
  { x: x0, y: y0 },
  { x: x1, y: y0 },
  { x: x1, y: y1 },
  { x: x0, y: y1 },
];
const good: Confidence = { score: 0.98, forced: null, reasons: [] };

function four(): ScanState {
  let s = emptyState();
  for (const q of [rect(0.05, 0.05, 0.45, 0.45), rect(0.55, 0.05, 0.95, 0.45), rect(0.05, 0.55, 0.45, 0.95), rect(0.55, 0.55, 0.95, 0.95)]) {
    s = addCrop(s, q, 'auto', DIMS, { ...good, reasons: [] }).state;
  }
  return s;
}

test('crops are numbered in reading order and ids are never reused', () => {
  let s = emptyState();
  // added out of order on purpose
  const a = addCrop(s, rect(0.55, 0.55, 0.95, 0.95), 'auto', DIMS, good);
  s = a.state;
  const b = addCrop(s, rect(0.05, 0.05, 0.45, 0.45), 'auto', DIMS, good);
  s = b.state;
  assert.deepEqual(
    s.crops.map((c) => c.id),
    [b.id, a.id],
    'the top-left crop is first',
  );
  const removed = setInclude(s, b.id, false);
  assert.deepEqual(included(removed).map((c) => c.id), [a.id]);
  const c = addCrop(removed, rect(0.1, 0.6, 0.3, 0.9), 'manual', DIMS);
  assert.ok(c.id > Math.max(a.id, b.id), 'a new id is above every id ever used');
});

test('rows are built by vertical overlap, then left to right', () => {
  const order = readingOrder([
    { id: 1, b: [0.6, 0.1, 0.9, 0.4] },
    { id: 2, b: [0.1, 0.12, 0.4, 0.42] },
    { id: 3, b: [0.1, 0.6, 0.4, 0.9] },
  ]);
  assert.deepEqual(order, [2, 1, 3]);
});

test('output names pad to at least two digits and one file keeps its plain name', () => {
  assert.equal(outputName('scan', 'jpg', 1, 1), 'scan.jpg');
  assert.equal(outputName('scan', 'jpg', 1, 4), 'scan_01.jpg');
  assert.equal(outputName('scan', 'jpg', 12, 12), 'scan_12.jpg');
  assert.equal(outputName('scan', 'jpg', 5, 120), 'scan_005.jpg');
  assert.deepEqual(outputNames('scan', 'png', 3), ['scan_01.png', 'scan_02.png', 'scan_03.png']);
});

test('a removed crop has order 0, no file name, and the others close the gap', () => {
  let s = four();
  const [a, b] = s.crops;
  s = setInclude(s, a.id, false);
  const v = cropViews(s, { stem: 'scan', ext: 'jpg', baseline: null });
  assert.equal(v[0].order, 0);
  assert.equal(v[0].outputName, null);
  assert.equal(v[1].id, b.id);
  assert.equal(v[1].order, 1);
  assert.equal(v[1].outputName, 'scan_01.jpg');
  assert.equal(v[3].outputName, 'scan_03.jpg');
});

test('merge keeps the first place, gives a new id, refuses an excluded crop and needs two', () => {
  const s = four();
  const [a, b] = s.crops;
  const m = mergeCrops(s, [a.id, b.id], DIMS);
  assert.equal(m.state.crops.length, 3);
  assert.equal(m.state.crops[0].id, m.id);
  assert.equal(m.state.crops[0].origin, 'manual');
  assert.ok(!m.state.crops.some((c) => c.id === a.id || c.id === b.id));
  assert.throws(() => mergeCrops(s, [a.id], DIMS), ItemOpError);
  const gone = setInclude(s, b.id, false);
  assert.throws(() => mergeCrops(gone, [a.id, b.id], DIMS), ItemOpError);
  assert.equal(s.crops.length, 4, 'a refused operation changes nothing');
});

test('cut replaces a crop with two manual pieces in place, and refuses a tiny piece', () => {
  const s = four();
  const first = s.crops[0];
  const r = cutCrop(s, first.id, { axis: 'vertical', t0: 0.5, t1: 0.5 }, DIMS);
  assert.equal(r.state.crops.length, 5);
  assert.deepEqual(
    r.state.crops.slice(0, 2).map((c) => c.id),
    r.ids,
  );
  assert.ok(r.state.crops.slice(0, 2).every((c) => c.origin === 'manual'));
  assert.throws(() => cutCrop(s, first.id, { axis: 'vertical', t0: 0.01, t1: 0.01 }, DIMS), ItemOpError);
  assert.throws(() => cutCrop(s, 999, { axis: 'vertical', t0: 0.5, t1: 0.5 }, DIMS), ItemOpError);
});

test('moving a crop switches to manual order, and reading order restores the row order', () => {
  let s = four();
  const ids = s.crops.map((c) => c.id);
  s = moveCrop(s, ids[3], 0);
  assert.equal(s.orderMode, 'manual');
  assert.equal(s.crops[0].id, ids[3]);
  // a new crop in manual mode does not re-sort the others
  s = addCrop(s, rect(0.4, 0.4, 0.6, 0.6), 'manual', DIMS).state;
  assert.equal(s.crops[0].id, ids[3]);
  s = useReadingOrder(s);
  assert.equal(s.orderMode, 'reading');
  assert.equal(s.crops[0].id, ids[0]);
});

test('editing an auto crop makes it AutoThenEdited and a reviewed Good; revert brings the baseline back', () => {
  const base = four();
  const id = base.crops[1].id;
  const edited = editCrop(base, id, { quad: rect(0.5, 0.05, 0.9, 0.4), quarterTurns: 0, fineDeg: 1 }, DIMS);
  assert.equal(edited.crops[1].origin, 'autoThenEdited');
  assert.notEqual(renderSignature(edited), renderSignature(base));
  const back = revertCrop(edited, id, base);
  assert.equal(renderSignature(back), renderSignature(base));
  assert.equal(back.crops[1].origin, 'auto');
  assert.throws(() => revertCrop(edited, 9999, base), ItemOpError);
});

test('a flagged crop holds the scan until the person edits it; a user-placed one is reviewed', () => {
  let s = emptyState();
  s = addCrop(s, rect(0.05, 0.05, 0.45, 0.45), 'auto', DIMS, good).state;
  assert.deepEqual(triage(s), { kind: 'approved' });
  const flagged = addCrop(s, rect(0.55, 0.05, 0.95, 0.45), 'auto', DIMS, { score: 0.9, forced: null, reasons: [{ code: 'TOUCHING_ITEMS' }] });
  assert.deepEqual(triage(flagged.state), { kind: 'heldForReview', itemsNeedCheck: 1 });
  const fixed = editCrop(flagged.state, flagged.id, { quad: rect(0.56, 0.06, 0.94, 0.44), quarterTurns: 0, fineDeg: 0 }, DIMS);
  assert.deepEqual(triage(fixed), { kind: 'approved' });
  assert.deepEqual(triage(emptyState()), { kind: 'noItems' });
  const sv = splitView(fixed, false, false);
  assert.equal(sv.isSplit, true);
  assert.equal(sv.included, 2);
});

test('bands: hold reasons cap at Check, a forced failure is Failed, below 0.6 is Failed', () => {
  assert.equal(bandOf({ score: 0.99, forced: null, reasons: [] }), 'good');
  assert.equal(bandOf({ score: 0.99, forced: null, reasons: [{ code: 'WEAK_EDGE', side: 'top' }] }), 'check');
  assert.equal(bandOf({ score: 0.99, forced: 'failed', reasons: [] }), 'failed');
  assert.equal(bandOf({ score: 0.5, forced: null, reasons: [] }), 'failed');
  assert.equal(bandOf({ score: 0.9, forced: null, reasons: [] }), 'check');
});

test('re-detection keeps what the person placed, edited or removed and replaces untouched auto crops', () => {
  let s = four();
  const [a, b, c] = s.crops;
  s = editCrop(s, a.id, { quad: rect(0.06, 0.06, 0.44, 0.44), quarterTurns: 0, fineDeg: 0 }, DIMS);
  s = setInclude(s, b.id, false);
  const after = redetect(
    s,
    [
      { quad: rect(0.05, 0.05, 0.45, 0.45), confidence: good }, // overlaps the edited crop: suppressed
      { quad: rect(0.05, 0.55, 0.45, 0.95), confidence: good }, // matches crop c: replaces it, keeping its id
      { quad: rect(0.3, 0.46, 0.7, 0.54), confidence: good }, // new
    ],
    DIMS,
  );
  const ids = after.crops.map((x) => x.id);
  assert.ok(ids.includes(a.id) && ids.includes(b.id) && ids.includes(c.id));
  assert.equal(after.crops.find((x) => x.id === b.id)!.include, false);
  assert.equal(after.crops.find((x) => x.id === a.id)!.origin, 'autoThenEdited');
  assert.equal(after.crops.length, 5 - 1, 'the fourth auto crop had no match and was dropped; one new crop was added');
  assert.ok(after.nextId > s.nextId);
});

test('a 100-operation random script never breaks the invariants', () => {
  const rnd = mulberry32(7);
  let s = four();
  const everUsed = new Set(s.crops.map((c) => c.id));
  let refused = 0;
  for (let i = 0; i < 100; i++) {
    const pick = s.crops.length ? s.crops[Math.floor(rnd() * s.crops.length)] : null;
    try {
      switch (Math.floor(rnd() * 10)) {
        case 0:
          s = addCrop(s, rect(rnd() * 0.5, rnd() * 0.5, 0.5 + rnd() * 0.5, 0.5 + rnd() * 0.5), 'manual', DIMS).state;
          break;
        case 1:
          if (pick) s = setInclude(s, pick.id, !pick.include);
          break;
        case 2:
          if (pick) s = turnCrop(s, pick.id, rnd() < 0.5);
          break;
        case 3:
          if (pick) s = angleCrop(s, pick.id, (rnd() - 0.5) * 100);
          break;
        case 4:
          if (pick) s = flipCrop(s, pick.id);
          break;
        case 5: {
          const inc = included(s);
          if (inc.length >= 2) s = mergeCrops(s, [inc[0].id, inc[inc.length - 1].id], DIMS).state;
          break;
        }
        case 6:
          if (pick) s = cutCrop(s, pick.id, { axis: rnd() < 0.5 ? 'vertical' : 'horizontal', t0: rnd(), t1: rnd() }, DIMS).state;
          break;
        case 7:
          if (pick) s = moveCrop(s, pick.id, Math.floor(rnd() * 6));
          break;
        case 8:
          s = useReadingOrder(s);
          break;
        default:
          if (pick) s = editCrop(s, pick.id, { quad: rect(rnd() * 0.3, rnd() * 0.3, 0.5 + rnd() * 0.5, 0.5 + rnd() * 0.5), quarterTurns: 1, fineDeg: 3 }, DIMS);
      }
    } catch (e) {
      assert.ok(e instanceof ItemOpError);
      refused++;
    }
    for (const c of s.crops) everUsed.add(c.id);
    const ids = s.crops.map((c) => c.id);
    assert.equal(new Set(ids).size, ids.length, 'ids are unique');
    assert.ok(s.crops.length <= 32);
    assert.ok(ids.every((id) => id < s.nextId), 'nextId is above every live id');
    const inc = included(s);
    const ranks = cropViews(s, { stem: 'x', ext: 'jpg', baseline: null })
      .filter((v) => v.include)
      .map((v) => v.order);
    assert.deepEqual(
      ranks,
      inc.map((_, k) => k + 1),
      'ranks are 1..n in list order',
    );
  }
  assert.ok(everUsed.size >= 4);
  assert.ok(refused >= 0);
});
