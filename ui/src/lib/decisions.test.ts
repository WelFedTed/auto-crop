// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  HISTORY_CAP,
  applyDecision,
  emptyDecisions,
  parseDecisions,
  pruneDecisions,
  serialiseDecisions,
  undoDecision,
} from './decisions.ts';

test('apply, undo and a no-op that adds no history', () => {
  let s = emptyDecisions();
  s = applyDecision(s, [1, 2], 'accepted');
  assert.deepEqual(s.map, { 1: 'accepted', 2: 'accepted' });
  const same = applyDecision(s, [1], 'accepted');
  assert.equal(same, s);
  s = applyDecision(s, [2], 'skipped');
  assert.equal(s.history.length, 2);
  s = undoDecision(s);
  assert.deepEqual(s.map, { 1: 'accepted', 2: 'accepted' });
  s = undoDecision(undoDecision(s));
  assert.deepEqual(s.map, {});
  assert.equal(undoDecision(s), s);
});

test('null clears a decision (put back)', () => {
  let s = applyDecision(emptyDecisions(), [5], 'skipped');
  s = applyDecision(s, [5], null);
  assert.deepEqual(s.map, {});
  assert.equal(applyDecision(s, [5], null), s);
});

test('history is capped', () => {
  let s = emptyDecisions();
  for (let i = 0; i < HISTORY_CAP + 20; i++) s = applyDecision(s, [i], 'skipped');
  assert.equal(s.history.length, HISTORY_CAP);
});

test('prune removes decisions for items that left the batch', () => {
  const s = applyDecision(applyDecision(emptyDecisions(), [1, 2, 3], 'accepted'), [3], 'skipped');
  const p = pruneDecisions(s, new Set([1, 3]));
  assert.deepEqual(p.map, { 1: 'accepted', 3: 'skipped' });
  assert.equal(pruneDecisions(p, new Set([1, 3, 9])), p);
});

test('sessionStorage round trip tolerates garbage', () => {
  const map = { 4: 'accepted', 9: 'skipped' } as const;
  assert.deepEqual(parseDecisions(serialiseDecisions(map)), map);
  assert.deepEqual(parseDecisions('not json'), {});
  assert.deepEqual(parseDecisions(null), {});
  assert.deepEqual(parseDecisions('{"1":"nope","2":"accepted","x":"skipped"}'), { 2: 'accepted' });
});
