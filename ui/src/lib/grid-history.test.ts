// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { afterRedo, afterUndo, emptyGridHistory, nextRedo, nextUndo, pushAction, reconcile } from './grid-history.ts';

test('undo takes the most recent action of either kind', () => {
  let h = emptyGridHistory();
  h = pushAction(h, { kind: 'decision' });
  h = pushAction(h, { kind: 'session', label: 'Treat as one item (3 images)' });
  assert.deepEqual(nextUndo(h), { kind: 'session', label: 'Treat as one item (3 images)' });
  h = afterUndo(h);
  assert.deepEqual(nextUndo(h), { kind: 'decision' });
  assert.equal(nextRedo(h)?.label, 'Treat as one item (3 images)');
});

test('a session command can be redone; a decision cannot; a new action clears redo', () => {
  let h = pushAction(emptyGridHistory(), { kind: 'session', label: 'Split into items (2 images)' });
  h = afterUndo(h);
  assert.equal(nextUndo(h), null);
  h = afterRedo(h);
  assert.equal(nextUndo(h)?.kind, 'session');
  assert.equal(nextRedo(h), null);
  h = afterUndo(h);
  assert.ok(nextRedo(h));
  h = pushAction(h, { kind: 'decision' });
  assert.equal(nextRedo(h), null, 'something new was done, so there is nothing to redo');
  const d = afterUndo(pushAction(emptyGridHistory(), { kind: 'decision' }));
  assert.equal(nextRedo(d), null, 'an undone decision is not redoable');
});

test('undo and redo on an empty history are no-ops', () => {
  const h = emptyGridHistory();
  assert.equal(afterUndo(h), h);
  assert.equal(afterRedo(h), h);
});

test('decision entries beyond what the reducer remembers are dropped, session entries stay', () => {
  let h = emptyGridHistory();
  h = pushAction(h, { kind: 'decision' });
  h = pushAction(h, { kind: 'session', label: 'x' });
  h = pushAction(h, { kind: 'decision' });
  const r = reconcile(h, 1);
  assert.deepEqual(r.undo.map((a) => a.kind), ['session', 'decision']);
  assert.equal(reconcile(h, 5), h);
});
