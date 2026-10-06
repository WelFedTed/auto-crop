// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { gestureKey, pushEntry, type Entry } from './history.ts';

const base: Entry<number>[] = [{ state: 0, label: 'Auto' }];

test('edits without a gesture id each make a step', () => {
  let h = pushEntry(base, 0, { state: 1, label: 'Bend' });
  h = pushEntry(h.hist, h.cursor, { state: 2, label: 'Bend' });
  assert.equal(h.hist.length, 3);
  assert.equal(h.cursor, 2);
});

test('edits under one gesture id are ONE undo step: the state is the last, the label the first', () => {
  const g = gestureKey(7, 3);
  let h = pushEntry(base, 0, { state: 1, label: 'Move point', gesture: g });
  h = pushEntry(h.hist, h.cursor, { state: 2, label: 'Move point (again)', gesture: g });
  h = pushEntry(h.hist, h.cursor, { state: 3, label: 'x', gesture: g });
  assert.equal(h.hist.length, 2, 'the base and one step');
  assert.deepEqual(h.hist[1], { state: 3, label: 'Move point', gesture: g });
});

test('a different gesture, crop or plain edit starts a new step; the base state is never merged into', () => {
  let h = pushEntry(base, 0, { state: 1, label: 'a', gesture: gestureKey(1, 1) });
  h = pushEntry(h.hist, h.cursor, { state: 2, label: 'b', gesture: gestureKey(2, 1) });
  h = pushEntry(h.hist, h.cursor, { state: 3, label: 'c', gesture: gestureKey(2, 2) });
  h = pushEntry(h.hist, h.cursor, { state: 4, label: 'd' });
  assert.equal(h.hist.length, 5);
  // a base entry that happens to carry the same id is not replaced
  const withGesture: Entry<number>[] = [{ state: 0, label: 'Auto', gesture: gestureKey(1, 1) }];
  const g = pushEntry(withGesture, 0, { state: 1, label: 'x', gesture: gestureKey(1, 1) });
  assert.equal(g.hist.length, 2);
});

test('after an undo the redo branch is dropped and a gesture does not merge across it', () => {
  const g = gestureKey(4, 1);
  let h = pushEntry(base, 0, { state: 1, label: 'a', gesture: g });
  h = pushEntry(h.hist, h.cursor, { state: 2, label: 'b' });
  const undone = { hist: h.hist, cursor: 1 }; // back on the gesture entry
  h = pushEntry(undone.hist, undone.cursor, { state: 5, label: 'c', gesture: g });
  assert.equal(h.hist.length, 2, 'the entry at the cursor had the same gesture: merged, redo branch gone');
  assert.equal(h.hist[1].state, 5);
});

test('gesture keys', () => {
  assert.equal(gestureKey(null, 1), undefined);
  assert.equal(gestureKey(undefined, 1), undefined);
  assert.equal(gestureKey(0, 5), '0:5');
});
