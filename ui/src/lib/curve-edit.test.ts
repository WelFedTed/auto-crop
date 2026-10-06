// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  addPointNear,
  addPointOnSelectedEdge,
  canAdd,
  canRemove,
  keyCommand,
  moveTo,
  nudge,
  removeSelected,
  resetAll,
  select,
  start,
  stepFor,
  stepSelection,
  straightenSelected,
  tabStop,
  type CurveEditState,
} from './curve-edit.ts';
import { curvesFromQuad, edgeOf, handleKey, isStraightPage, validateCurves } from './curve.ts';
import type { Quad } from './quad.ts';

const quad = (): Quad => [
  { x: 0.1, y: 0.1 },
  { x: 0.9, y: 0.1 },
  { x: 0.9, y: 0.9 },
  { x: 0.1, y: 0.9 },
];
const fresh = (): CurveEditState => start(curvesFromQuad(quad()));
const keyOf = (s: CurveEditState) => (s.sel ? handleKey(s.sel) : null);

test('keys map to commands, and a key that is not ours is left to the page', () => {
  assert.deepEqual(keyCommand({ key: 'ArrowLeft' }), { kind: 'nudge', dx: -1, dy: 0 });
  assert.deepEqual(keyCommand({ key: 'ArrowDown' }), { kind: 'nudge', dx: 0, dy: 1 });
  assert.deepEqual(keyCommand({ key: ']' }), { kind: 'step', dir: 'next' });
  assert.deepEqual(keyCommand({ key: 'PageUp' }), { kind: 'step', dir: 'prev' });
  assert.deepEqual(keyCommand({ key: 'Home' }), { kind: 'step', dir: 'first' });
  assert.deepEqual(keyCommand({ key: 'Delete' }), { kind: 'remove' });
  assert.deepEqual(keyCommand({ key: 'Escape' }), { kind: 'deselect' });
  assert.deepEqual(keyCommand({ key: 'Enter' }), { kind: 'activate' });
  assert.equal(keyCommand({ key: 'x' }), null);
  assert.equal(keyCommand({ key: 'z', ctrlKey: true }), null, 'Ctrl+Z is undo, not ours');
  assert.equal(keyCommand({ key: 'ArrowLeft', ctrlKey: true }), null, 'Ctrl+arrows belong to the quad grip, not to curves');
});

test('arrow step sizes: 1 px, Shift 10 px, Alt a fine quarter pixel', () => {
  assert.equal(stepFor({}), 1);
  assert.equal(stepFor({ shiftKey: true }), 10);
  assert.equal(stepFor({ altKey: true }), 0.25);
  assert.equal(stepFor({ altKey: true, shiftKey: true }), 0.25);
});

test('the roving tab stop is the selected handle, else the first; selection walks the boundary order and wraps', () => {
  let s = fresh();
  assert.equal(handleKey(tabStop(s)), 'c0');
  s = stepSelection(s, 'next');
  assert.equal(keyOf(s), 'c0', 'with nothing selected, next starts at the first handle');
  s = stepSelection(s, 'next');
  assert.equal(keyOf(s), 'g0', 'a straight edge has one hollow handle after its corner');
  s = stepSelection(s, 'next');
  assert.equal(keyOf(s), 'c1');
  s = stepSelection(s, 'last');
  assert.equal(keyOf(s), 'g3');
  s = stepSelection(s, 'next');
  assert.equal(keyOf(s), 'c0', 'wraps');
  s = stepSelection(s, 'prev');
  assert.equal(keyOf(s), 'g3', 'wraps back');
  assert.equal(keyOf(select(s, null)), null, 'Esc deselects');
  assert.equal(handleKey(tabStop(select(s, null))), 'c0');
});

test('touching a hollow handle makes it a point; a nudge moves it; the shape stays valid', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next'); // g0
  let r = moveTo(s, s.sel!, { x: 0.5, y: 0.06 });
  assert.ok(r.changed && r.what === 'move');
  s = r.state;
  assert.equal(keyOf(s), 'p0.0', 'it is a real point now and stays selected');
  assert.equal(edgeOf(s.curves, 0).length, 3);
  r = nudge(s, 0, -0.01);
  assert.ok(r.changed && r.what === 'nudge');
  assert.ok(Math.abs(edgeOf(r.state.curves, 0)[1].y - 0.05) < 1e-12);
  assert.equal(validateCurves(r.state.curves), null);
});

test('a drag or nudge the engine would refuse changes nothing and says why', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next');
  s = moveTo(s, s.sel!, { x: 0.5, y: 0.06 }).state;
  // the top edge pushed through the bottom edge: the outline crosses itself
  const r = moveTo(s, s.sel!, { x: 0.5, y: 0.95 });
  assert.equal(r.changed, false);
  assert.equal(r.refused, 'crossing');
  assert.equal(r.state, s, 'the state is the same object: nothing changed');
  assert.equal(nudge(fresh(), 0.01, 0).refused, 'nothing', 'no handle is selected');
});

test('a point is added where the person double-clicked, only close to an edge', () => {
  const s = fresh();
  const scale = [1000, 800] as const;
  const near = addPointNear(s, { x: 0.5, y: 0.1 + 5 / 800 }, scale, 24);
  assert.ok(near.changed && near.what === 'add');
  assert.equal(keyOf(near.state), 'p0.0');
  const far = addPointNear(s, { x: 0.5, y: 0.5 }, scale, 24);
  assert.equal(far.changed, false);
  assert.equal(far.refused, 'nothing');
});

test('the Add point button splits the longest gap of the selected edge', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next'); // g0, the top edge
  const scale = [1, 1] as const;
  let r = addPointOnSelectedEdge(s, scale);
  assert.ok(r.changed);
  assert.equal(edgeOf(r.state.curves, 0).length, 3);
  assert.ok(Math.abs(edgeOf(r.state.curves, 0)[1].x - 0.5) < 1e-12, 'in the middle of the only gap');
  s = r.state;
  s = moveTo(s, s.sel!, { x: 0.3, y: 0.1 }).state; // now the gaps are 0.2 and 0.6
  r = addPointOnSelectedEdge(s, scale);
  const xs = edgeOf(r.state.curves, 0).map((p) => p.x);
  assert.ok(Math.abs(xs[2] - 0.6) < 1e-12, `the new point halves the long gap: ${xs}`);
  assert.equal(addPointOnSelectedEdge(fresh(), scale).refused, 'nothing');
});

test('an edge holds 2 to 32 points: the 33rd is refused, and Delete never takes an end point', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next');
  for (let i = 0; i < 30; i++) {
    const r = addPointOnSelectedEdge(s, [1, 1]);
    assert.ok(r.changed, `point ${i + 1}`);
    s = r.state;
  }
  assert.equal(edgeOf(s.curves, 0).length, 32);
  assert.equal(canAdd(s), false);
  const refused = addPointOnSelectedEdge(s, [1, 1]);
  assert.equal(refused.refused, 'max');
  while (edgeOf(s.curves, 0).length > 2) {
    assert.ok(canRemove(s));
    const r = removeSelected(s);
    assert.ok(r.changed && r.what === 'remove');
    s = r.state;
  }
  assert.equal(keyOf(s), 'g0', 'the edge shows its hollow handle again');
  assert.equal(removeSelected(s).refused, 'min');
  const corner = select(s, { kind: 'corner', e: 0, m: -1 });
  assert.equal(removeSelected(corner).refused, 'min', 'a corner is not a point');
  assert.equal(canRemove(corner), false);
});

test('removing a point leaves a neighbour selected', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next');
  s = addPointOnSelectedEdge(s, [1, 1]).state;
  s = addPointOnSelectedEdge(s, [1, 1]).state; // 2 interior points
  s = select(s, { kind: 'point', e: 0, m: 1 });
  const r = removeSelected(s);
  assert.equal(keyOf(r.state), 'p0.0');
  assert.equal(edgeOf(r.state.curves, 0).length, 3);
});

test('straighten an edge and reset all curves', () => {
  let s = stepSelection(stepSelection(fresh(), 'next'), 'next');
  s = moveTo(s, s.sel!, { x: 0.5, y: 0.05 }).state;
  s = select(s, { kind: 'corner', e: 2, m: -1 });
  s = moveTo(s, { kind: 'ghost', e: 2, m: -1 }, { x: 0.5, y: 0.95 }).state;
  assert.ok(!isStraightPage(s.curves));
  s = select(s, { kind: 'point', e: 0, m: 0 });
  const r = straightenSelected(s);
  assert.ok(r.changed && r.what === 'straighten');
  assert.equal(edgeOf(r.state.curves, 0).length, 2);
  assert.equal(edgeOf(r.state.curves, 2).length, 3, 'the other edge keeps its bend');
  assert.equal(straightenSelected(select(r.state, { kind: 'ghost', e: 0, m: -1 })).refused, 'nothing');
  const all = resetAll(r.state);
  assert.ok(all.changed && isStraightPage(all.state.curves));
  assert.equal(all.state.sel, null);
  assert.equal(resetAll(all.state).refused, 'nothing', 'already straight');
});
