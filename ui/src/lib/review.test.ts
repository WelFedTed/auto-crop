// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  canAccept,
  classify,
  countsOf,
  defaultQueue,
  matchesFilter,
  needsDrawCrop,
  nextToReview,
  reasonLine,
  saveCandidates,
  sortClassified,
  tierFromConfidence,
  tileLabel,
  tierOf,
  type Filter,
} from './review.ts';
import { item, withConfidence } from './testkit.ts';

test('cut-offs: 0.95 strict, 0.90 balanced, 0.80 aggressive (score >= cut-off is Good)', () => {
  const at = (score: number) => ({ score, forced: null, reasons: [] });
  assert.equal(tierFromConfidence(at(0.95), 'strict'), 'good');
  assert.equal(tierFromConfidence(at(0.949), 'strict'), 'check');
  assert.equal(tierFromConfidence(at(0.9), 'balanced'), 'good');
  assert.equal(tierFromConfidence(at(0.899), 'balanced'), 'check');
  assert.equal(tierFromConfidence(at(0.8), 'aggressive'), 'good');
  assert.equal(tierFromConfidence(at(0.799), 'aggressive'), 'check');
});

test('below 0.60 is Failed in every mode; 0.60 itself is Check', () => {
  for (const s of ['strict', 'balanced', 'aggressive'] as const) {
    assert.equal(tierFromConfidence({ score: 0.599, forced: null, reasons: [] }, s), 'failed');
    assert.equal(tierFromConfidence({ score: 0.6, forced: null, reasons: [] }, s), 'check');
  }
});

test('forced failed is always Failed, forced check is never Good', () => {
  for (const s of ['strict', 'balanced', 'aggressive'] as const) {
    assert.equal(tierFromConfidence({ score: 0.99, forced: 'failed', reasons: [] }, s), 'failed');
    assert.equal(tierFromConfidence({ score: 0.99, forced: 'check', reasons: [] }, s), 'check');
    // forced check does not rescue a very low score
    assert.equal(tierFromConfidence({ score: 0.2, forced: 'check', reasons: [] }, s), 'failed');
  }
});

test('analysing and error items', () => {
  assert.equal(tierOf(item(1, { status: 'analysing', confidence: null, edit: null }), 'strict'), 'analysing');
  assert.equal(tierOf(item(2, { status: 'error', error: 'CORRUPT', confidence: null, edit: null }), 'strict'), 'failed');
});

test('strictness re-buckets instantly', () => {
  const it = withConfidence(1, 0.92);
  assert.equal(tierOf(it, 'strict'), 'check');
  assert.equal(tierOf(it, 'balanced'), 'good');
  assert.equal(tierOf(it, 'aggressive'), 'good');
});

test('Draw crop banner: failed and not yet edited', () => {
  const f = withConfidence(1, 0.3, 'failed', [{ code: 'NO_QUAD' }]);
  assert.equal(needsDrawCrop(f, 'strict'), true);
  assert.equal(needsDrawCrop({ ...f, edited: true }, 'strict'), false);
  assert.equal(needsDrawCrop(withConfidence(2, 0.8), 'strict'), false);
});

test('canAccept: Failed items need a crop drawn first', () => {
  const f = withConfidence(1, 0.3, 'failed');
  assert.equal(canAccept(f, 'strict'), false);
  assert.equal(canAccept({ ...f, edited: true }, 'strict'), true);
  assert.equal(canAccept(withConfidence(2, 0.8), 'strict'), true);
  assert.equal(canAccept(item(3, { status: 'analysing', edit: null, confidence: null }), 'strict'), false);
});

test('counts, filters and needs-review honour decisions and edits', () => {
  const items = [
    withConfidence(1, 0.99), // good, skipped
    withConfidence(2, 0.8), // check
    withConfidence(3, 0.7), // check, accepted
    withConfidence(4, 0.7, null, [], { edited: true }), // check, edited
    withConfidence(5, 0.3, 'failed'), // failed
    item(6, { status: 'analysing', confidence: null, edit: null }),
    withConfidence(7, 0.99, null, [], { saved: { backupId: 'r/1', output: 'a.jpg', copy: false } }),
  ];
  const list = classify(items, 'strict', { 3: 'accepted', 1: 'skipped' });
  const c = countsOf(list);
  assert.equal(c.total, 7);
  assert.equal(c.analysing, 1);
  assert.equal(c.needs, 2); // ids 2 and 5
  assert.equal(c.good, 2); // ids 1 and 7
  assert.equal(c.edited, 1);
  assert.equal(c.skipped, 1);
  assert.equal(c.failed, 1);
  assert.equal(c.saved, 1);
  const ids = (f: Filter) => list.filter((x) => matchesFilter(x, f)).map((x) => x.item.id);
  assert.deepEqual(ids('needs'), [2, 5, 6]); // analysing shows as a skeleton
  assert.deepEqual(ids('skipped'), [1]);
  assert.deepEqual(ids('failed'), [5]);
  assert.deepEqual(ids('saved'), [7]);
  assert.equal(ids('all').length, 7);
});

test('sort: failed, then check lowest first, then good, analysing last; name sort is natural', () => {
  const items = [
    withConfidence(1, 0.99),
    withConfidence(2, 0.7),
    withConfidence(3, 0.3, 'failed'),
    item(4, { status: 'analysing', confidence: null, edit: null }),
    withConfidence(5, 0.8),
  ];
  const list = classify(items, 'strict', {});
  assert.deepEqual(sortClassified(list, 'confidence').map((x) => x.item.id), [3, 2, 5, 1, 4]);
  const named = classify([item(1, { name: 'b10.jpg' }), item(2, { name: 'b9.jpg' }), item(3, { name: 'a.jpg' })], 'strict', {});
  assert.deepEqual(sortClassified(named, 'name').map((x) => x.item.id), [3, 2, 1]);
});

test('Save all writes Good, accepted and edited; never skipped, flagged, or saved-and-clean', () => {
  const saved = { backupId: 'r/1', output: 'a.jpg', copy: false };
  const items = [
    withConfidence(1, 0.99), // good -> yes
    withConfidence(2, 0.8), // check unreviewed -> no
    withConfidence(3, 0.8), // check accepted -> yes
    withConfidence(4, 0.99), // good but skipped -> no
    withConfidence(5, 0.99, null, [], { saved }), // saved and clean -> no
    withConfidence(6, 0.99, null, [], { saved, dirtySinceSave: true }), // saved but dirty -> yes
    withConfidence(7, 0.3, 'failed', [], { edited: true }), // crop drawn -> yes
    withConfidence(8, 0.3, 'failed'), // failed untouched -> no
    item(9, { status: 'analysing', confidence: null, edit: null }), // -> no
    withConfidence(10, 0.99, 'check'), // forced check, not reviewed -> no
  ];
  const list = classify(items, 'strict', { 3: 'accepted', 4: 'skipped' });
  assert.deepEqual(saveCandidates(list).map((x) => x.item.id), [1, 3, 6, 7]);
});

test('reason line uses the first reason code, the side for WEAK_EDGE, or a generic line', () => {
  const w = withConfidence(1, 0.7, null, [{ code: 'WEAK_EDGE', side: 'right' }]);
  assert.equal(reasonLine(w, 'check'), 'Edge unclear on the right side');
  assert.equal(reasonLine(withConfidence(2, 0.7), 'check'), 'Not sure about this crop');
  assert.equal(reasonLine(withConfidence(3, 0.2, 'failed'), 'failed'), 'Low confidence. Check the crop.');
  assert.equal(reasonLine(withConfidence(4, 0.99), 'good'), null);
});

test('queue: flagged first, then the rest by name; next skips resolved ones and wraps', () => {
  const items = [
    withConfidence(1, 0.99, null, [], { name: 'a.jpg' }),
    withConfidence(2, 0.7),
    withConfidence(3, 0.8),
    withConfidence(4, 0.99, null, [], { name: 'b.jpg' }),
  ];
  const list = classify(items, 'strict', {});
  const q = defaultQueue(list);
  assert.deepEqual(q, [2, 3, 1, 4]);
  assert.equal(nextToReview(q, 2, new Set([3])), 3);
  assert.equal(nextToReview(q, 3, new Set([2])), 2); // wraps
  assert.equal(nextToReview(q, 3, new Set()), null);
});

test('tile label announces status, reason and saved state', () => {
  const w = withConfidence(7, 0.7, null, [{ code: 'WEAK_EDGE', side: 'right' }], { name: 'IMG_07' });
  const [x] = classify([w], 'strict', {});
  assert.equal(tileLabel(x), 'IMG_07, needs review, edge unclear on the right side, not saved');
  const [acc] = classify([w], 'strict', { 7: 'accepted' });
  assert.ok(tileLabel(acc).includes('accepted'));
  const [good] = classify([withConfidence(8, 0.99, null, [], { name: 'G', saved: { backupId: null, output: 'G', copy: true } })], 'strict', {});
  assert.equal(tileLabel(good), 'G, good, saved');
  const [busy] = classify([item(9, { name: 'B', status: 'analysing', confidence: null, edit: null })], 'strict', {});
  assert.equal(tileLabel(busy), 'B, still analysing');
});
