// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  GestureClock,
  bannerState,
  chipName,
  collisionNotice,
  dropSlot,
  includedCrops,
  isSplitScan,
  itemsNeedingCheck,
  mayAutoSave,
  moveTarget,
  nameSummary,
  nextIncluded,
  pickSelection,
  plannedNames,
  removedCrops,
  saveGate,
  slotToIndex,
  worstBand,
} from './items.ts';
import { defaultSettings, item, scan } from './testkit.ts';

test('included and removed crops are told apart and the scan band is the worst included band', () => {
  const s = scan(1, ['good', 'check', 'good'], { removed: 2 });
  assert.equal(includedCrops(s).length, 3);
  assert.equal(removedCrops(s).length, 2);
  assert.equal(worstBand(s), 'check');
  assert.equal(itemsNeedingCheck(s), 1);
  assert.equal(worstBand(scan(2, ['good', 'good'])), 'good');
  assert.equal(worstBand(scan(3, ['good', 'failed', 'check'])), 'failed');
  assert.equal(worstBand(scan(4, [])), null);
});

test('selection survives while the crop exists and falls back to the first included crop', () => {
  const s = scan(1, ['good', 'good', 'good']);
  assert.equal(pickSelection(s, 2), 2);
  assert.equal(pickSelection(s, 99), 1);
  assert.equal(pickSelection(s, null), 1);
  assert.equal(pickSelection(item(5), null), null);
});

test('a move goes to the neighbouring INCLUDED crop, skipping removed ones, and stops at the ends', () => {
  const s = scan(1, ['good', 'good', 'good']);
  // list order: 1, 2, 3, 100(removed)
  assert.equal(moveTarget(s, 2, -1), 0);
  assert.equal(moveTarget(s, 2, 1), 2);
  assert.equal(moveTarget(s, 1, -1), null);
  assert.equal(moveTarget(s, 3, 1), null, 'the removed crop at the end does not count');
  assert.equal(moveTarget(s, 100, 1), null, 'a removed crop cannot be moved');
});

test('merge with next takes the next included crop', () => {
  const s = scan(1, ['good', 'good', 'good'], { removed: 1 });
  assert.equal(nextIncluded(s, 1)?.id, 2);
  assert.equal(nextIncluded(s, 3), null);
  assert.equal(nextIncluded(s, 100), null);
});

test('a drag lands in the slot among the other included chips and converts to a full-list index', () => {
  assert.equal(dropSlot([50, 150, 250], 10), 0);
  assert.equal(dropSlot([50, 150, 250], 160), 2);
  assert.equal(dropSlot([50, 150, 250], 900), 3);
  const s = scan(1, ['good', 'good', 'good', 'good'], { removed: 1 });
  // drag crop 1 to the end: others are 2,3,4,(removed 100) -> after crop 4 -> index 3 in the full list of "others"
  assert.equal(slotToIndex(s, 1, 3), 3);
  assert.equal(slotToIndex(s, 4, 0), 0);
  assert.equal(slotToIndex(s, 2, 1), 1, 'slot 1 is where crop 2 already is');
  assert.equal(slotToIndex(s, 2, 2), 2, 'one place later: between crops 3 and 4');
});

test('the planned names come from the included crops; a long list is summarised', () => {
  const s = scan(1, ['good', 'good', 'good', 'good'], { removed: 1 });
  assert.deepEqual(plannedNames(s), ['scan_01.jpg', 'scan_02.jpg', 'scan_03.jpg', 'scan_04.jpg']);
  assert.equal(nameSummary(plannedNames(s)), 'scan_01.jpg ... scan_04.jpg');
  assert.equal(nameSummary(['a_01.jpg', 'a_02.jpg']), 'a_01.jpg, a_02.jpg');
});

test('a collision is noticed when the set was saved under another base name', () => {
  const planned = ['scan_01.jpg', 'scan_02.jpg'];
  assert.equal(collisionNotice(planned, ['scan_01.jpg', 'scan_02.jpg']), null);
  assert.deepEqual(collisionNotice(planned, ['scan (2)_01.jpg', 'scan (2)_02.jpg']), { wanted: 'scan_01.jpg', got: 'scan (2)_01.jpg' });
  assert.equal(collisionNotice([], ['x.jpg']), null);
  assert.equal(collisionNotice(planned, []), null);
});

test('Replace original: a split scan needs acceptance (or Experimental auto-save with every item Good)', () => {
  const held = scan(1, ['good', 'check']);
  assert.deepEqual(saveGate(held, defaultSettings()), { replace: 'accept-first' });
  assert.deepEqual(saveGate(scan(2, ['good', 'check'], { accepted: true }), defaultSettings()), { replace: 'ok' });
  const approved = scan(3, ['good', 'good']);
  assert.deepEqual(saveGate(approved, defaultSettings()), { replace: 'accept-first' }, 'approved is not enough by default');
  assert.deepEqual(saveGate(approved, defaultSettings({ autoSaveSplits: true })), { replace: 'ok' });
  assert.deepEqual(saveGate(held, defaultSettings({ autoSaveSplits: true })), { replace: 'accept-first' }, 'auto-save needs every item Good');
});

test('an open-only source is never replaced and a scan with no crop cannot be saved', () => {
  const tiff = scan(1, ['good'], { patch: { openOnly: 'tiff.multi_page' } });
  assert.deepEqual(saveGate(tiff, defaultSettings()), { replace: 'open-only', reason: 'tiff.multi_page' });
  assert.deepEqual(saveGate(scan(2, []), defaultSettings()), { replace: 'no-crop' });
  assert.deepEqual(saveGate(item(3, { status: 'analysing' }), defaultSettings()), { replace: 'no-crop' });
  assert.deepEqual(saveGate(scan(4, ['good']), defaultSettings()), { replace: 'ok' });
});

test('banner states', () => {
  const s = defaultSettings();
  assert.deepEqual(bannerState(scan(1, ['good', 'check', 'good']), s), { kind: 'held', items: 3, need: 1 });
  assert.deepEqual(bannerState(scan(2, ['good', 'good']), s), { kind: 'ready', items: 2 });
  assert.deepEqual(bannerState(scan(3, ['good', 'good']), defaultSettings({ autoSaveSplits: true })), { kind: 'auto', items: 2 });
  assert.deepEqual(bannerState(scan(4, ['good', 'check'], { accepted: true }), s), { kind: 'accepted', items: 2 });
  assert.deepEqual(bannerState(scan(5, ['good']), s), { kind: 'none' });
  assert.deepEqual(bannerState(item(6, { status: 'analysing' }), s), { kind: 'none' });
});

test('Save all writes a split scan only when it is accepted or approved (and copy mode or auto-save allows it)', () => {
  const held = scan(1, ['good', 'check']);
  const approved = scan(2, ['good', 'good']);
  const accepted = scan(3, ['good', 'check'], { accepted: true });
  const one = scan(4, ['good']);
  assert.ok(isSplitScan(held) && !isSplitScan(one));
  assert.equal(mayAutoSave(held, defaultSettings({ saveAsCopy: true })), false);
  assert.equal(mayAutoSave(approved, defaultSettings()), false, 'replace mode: held by default');
  assert.equal(mayAutoSave(approved, defaultSettings({ saveAsCopy: true })), true, 'a copy removes nothing');
  assert.equal(mayAutoSave(approved, defaultSettings({ autoSaveSplits: true })), true);
  assert.equal(mayAutoSave(accepted, defaultSettings()), true);
  assert.equal(mayAutoSave(one, defaultSettings()), true);
});

test('a chip is named by number, band word, reason and file, never by colour', () => {
  const s = scan(1, ['good', 'check']);
  const name = chipName(s.crops[1], 'Check', 'Items touch', 'removed');
  assert.equal(name, 'Item 2, Check, Items touch, scan_02.jpg');
  assert.equal(chipName(s.crops[0], 'Good', null, 'removed'), 'Item 1, Good, scan_01.jpg');
  const gone = scan(2, ['good'], { removed: 1 }).crops[1];
  assert.equal(chipName(gone, 'Check', null, 'removed'), 'Item (removed)');
});

test('a burst of nudges shares one gesture id, a pause or another crop starts a new one', () => {
  const g = new GestureClock(500);
  const a = g.next('1:2', 0);
  assert.equal(g.next('1:2', 200), a);
  assert.equal(g.next('1:2', 600), a, 'the gap is measured from the last nudge');
  assert.notEqual(g.next('1:2', 1500), a);
  const b = g.next('1:3', 1600);
  assert.notEqual(b, a);
});
