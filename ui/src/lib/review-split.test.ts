// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { canAccept, classify, matchesFilter, needsDrawCrop, openOnlyLeftAlone, reasonLine, saveCandidates, sortClassified, tierOf, tileLabel } from './review.ts';
import { item, scan, defaultSettings } from './testkit.ts';

const opts = (p: Partial<ReturnType<typeof defaultSettings>> = {}) => {
  const s = defaultSettings(p);
  return { autoSaveSplits: s.autoSaveSplits, saveAsCopy: s.saveAsCopy };
};

test('a scan takes the worst band over its items: one Check item keeps the scan in review', () => {
  const mixed = scan(1, ['good', 'good', 'check', 'good']);
  assert.equal(tierOf(mixed, 'strict'), 'check');
  assert.equal(tierOf(mixed, 'aggressive'), 'check', 'split scans always use the Strict cutoff (the engine hold rule)');
  assert.equal(tierOf(scan(2, ['good', 'good']), 'strict'), 'good');
  assert.equal(tierOf(scan(3, ['good', 'failed']), 'strict'), 'failed');
});

test('a split scan waits for acceptance even when every item is Good (held by default)', () => {
  const approved = scan(1, ['good', 'good', 'good']);
  const [c] = classify([approved], 'strict', {}, opts());
  assert.equal(c.needs, true);
  assert.equal(c.decision, null);
  const [auto] = classify([approved], 'strict', {}, opts({ autoSaveSplits: true }));
  assert.equal(auto.needs, false, 'Experimental auto-save with every item Good does not need a look');
  const [held] = classify([scan(2, ['good', 'check'])], 'strict', {}, opts({ autoSaveSplits: true }));
  assert.equal(held.needs, true);
});

test('an edit is not an acceptance: only the engine flag is, and a skipped scan is not reviewed', () => {
  const edited = scan(1, ['good', 'check'], { patch: { edited: true } });
  assert.equal(classify([edited], 'strict', {}, opts())[0].needs, true);
  const accepted = scan(2, ['good', 'check'], { accepted: true });
  const [a] = classify([accepted], 'strict', { 2: 'accepted' }, opts());
  assert.equal(a.needs, false);
  assert.equal(a.decision, 'accepted');
  // the UI decision says accepted but the engine withdrew it after an edit: the scan needs a look again
  const stale = scan(3, ['good', 'check'], { accepted: false });
  const [s] = classify([stale], 'strict', { 3: 'accepted' }, opts());
  assert.equal(s.needs, true);
  assert.equal(s.decision, null);
  const [k] = classify([stale], 'strict', { 3: 'skipped' }, opts());
  assert.equal(k.needs, false);
  assert.equal(k.decision, 'skipped');
});

test('Save all never writes a held split; accepted ones and approved ones (copy or auto-save) are written', () => {
  const held = scan(1, ['good', 'check']);
  const approved = scan(2, ['good', 'good']);
  const accepted = scan(3, ['good', 'check'], { accepted: true });
  const plain = item(4);
  const list = (o: ReturnType<typeof opts>) => saveCandidates(classify([held, approved, accepted, plain], 'strict', {}, o), o).map((x) => x.item.id);
  assert.deepEqual(list(opts()), [3, 4], 'replace mode: only the accepted split');
  assert.deepEqual(list(opts({ saveAsCopy: true })), [2, 3, 4], 'copy mode also writes the approved split');
  assert.deepEqual(list(opts({ autoSaveSplits: true })), [2, 3, 4]);
});

test('open-only sources are left alone by Save all in replace mode and written in copy mode', () => {
  const tiff = item(1, { openOnly: 'tiff.multi_page', name: 'ledger.tif' });
  const jpg = item(2);
  const cls = classify([tiff, jpg], 'strict', {});
  assert.deepEqual(saveCandidates(cls, opts()).map((x) => x.item.id), [2]);
  assert.deepEqual(openOnlyLeftAlone(cls, opts()).map((x) => x.item.id), [1]);
  assert.deepEqual(saveCandidates(cls, opts({ saveAsCopy: true })).map((x) => x.item.id), [1, 2]);
  assert.deepEqual(openOnlyLeftAlone(cls, opts({ saveAsCopy: true })), []);
});

test('the tile line for a split names the first item that needs a look; no Draw crop banner on a split', () => {
  const s = scan(1, ['good', 'check']);
  s.crops[1].confidence = { score: 0.82, forced: null, reasons: [{ code: 'TOUCHING_ITEMS' }] };
  assert.equal(reasonLine(s, 'check'), 'Touching. Check the split.');
  assert.equal(needsDrawCrop(scan(2, ['failed', 'good']), 'strict'), false);
  assert.ok(canAccept(scan(3, ['failed', 'good']), 'strict'), 'Accept split is for the person who looked');
  assert.equal(canAccept(scan(4, []), 'strict'), false);
});

test('the tile label says how many items and that a source is open-only', () => {
  const s = scan(1, ['good', 'check', 'good']);
  const [c] = classify([s], 'strict', {}, opts());
  assert.match(tileLabel(c), /3 items/);
  const [o] = classify([item(2, { openOnly: 'format.write_unavailable' })], 'strict', {});
  assert.match(tileLabel(o), /open only/);
});

test('the Needs review filter lists split scans that wait, and the sort puts the worst first', () => {
  const list = classify([scan(1, ['good', 'good']), scan(2, ['good', 'check']), item(3), scan(4, ['failed', 'good'])], 'strict', {}, opts());
  assert.deepEqual(
    list.filter((x) => matchesFilter(x, 'needs')).map((x) => x.item.id),
    [1, 2, 4],
  );
  assert.deepEqual(sortClassified(list, 'confidence').map((x) => x.item.id).slice(0, 2), [4, 2]);
});
