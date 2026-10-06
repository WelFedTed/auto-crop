// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The state machine of the curved-edge editor: which handle is selected, what a key does, what a drag, an add, a
// delete, a nudge or a reset turns the curve set into, and what is refused and why. Pure (no DOM, no Svelte), so it
// runs under `node --test`; CropStage.svelte only turns pointer and key events into these calls. Nothing here is
// recorded: the caller sends the resulting curve set to the engine (`setCurves`), one undo step per gesture.

import {
  MAX_POINTS,
  MIN_POINTS,
  curveHandles,
  edgeOf,
  handleKey,
  handlePos,
  insertAtNearest,
  insertPointAt,
  moveHandle,
  nearestOnPage,
  removePoint,
  resetCurves,
  sameHandle,
  straightenEdge,
  validateCurves,
  edgeIsStraight,
  isStraightPage,
  type CurveHandle,
  type CurveProblem,
  type EdgeIndex,
  type Scale,
} from './curve.ts';
import type { CurveSet, Pt } from './types.ts';

export interface CurveEditState {
  curves: CurveSet;
  /** The handle with the roving tab stop, or null when none is selected (Esc). */
  sel: CurveHandle | null;
}

export type Refusal = CurveProblem | 'max' | 'min' | 'nothing';

export interface StepResult {
  state: CurveEditState;
  /** The curve set changed: the caller records it. */
  changed: boolean;
  /** Why nothing happened, when something was asked and refused. */
  refused?: Refusal;
  /** What the change was, for the label and the live region. */
  what?: 'move' | 'add' | 'remove' | 'straighten' | 'reset' | 'nudge';
}

const unchanged = (state: CurveEditState, refused?: Refusal): StepResult => ({ state, changed: false, refused });

/** The step an arrow key makes, in screen pixels: 1, Shift 10, Alt a quarter (a fine adjustment). Alt wins over Shift. */
export function stepFor(e: { shiftKey?: boolean; altKey?: boolean }): number {
  if (e.altKey) return 0.25;
  return e.shiftKey ? 10 : 1;
}

export type Command =
  | { kind: 'nudge'; dx: -1 | 0 | 1; dy: -1 | 0 | 1 }
  | { kind: 'step'; dir: 'prev' | 'next' | 'first' | 'last' }
  | { kind: 'remove' }
  | { kind: 'deselect' }
  | { kind: 'activate' };

/** What a key does on a curve handle; null when the key is not ours (it goes on to the page). */
export function keyCommand(e: { key: string; ctrlKey?: boolean; metaKey?: boolean }): Command | null {
  if (e.ctrlKey || e.metaKey) return null;
  switch (e.key) {
    case 'ArrowLeft':
      return { kind: 'nudge', dx: -1, dy: 0 };
    case 'ArrowRight':
      return { kind: 'nudge', dx: 1, dy: 0 };
    case 'ArrowUp':
      return { kind: 'nudge', dx: 0, dy: -1 };
    case 'ArrowDown':
      return { kind: 'nudge', dx: 0, dy: 1 };
    case '[':
    case 'PageUp':
      return { kind: 'step', dir: 'prev' };
    case ']':
    case 'PageDown':
      return { kind: 'step', dir: 'next' };
    case 'Home':
      return { kind: 'step', dir: 'first' };
    case 'End':
      return { kind: 'step', dir: 'last' };
    case 'Delete':
    case 'Backspace':
      return { kind: 'remove' };
    case 'Escape':
      return { kind: 'deselect' };
    case 'Enter':
    case ' ':
      return { kind: 'activate' };
    default:
      return null;
  }
}

export const start = (curves: CurveSet): CurveEditState => ({ curves, sel: null });

/** The roving tab stop: the selected handle if it still exists, else the first one. */
export function tabStop(state: CurveEditState): CurveHandle {
  const list = curveHandles(state.curves);
  return list.find((h) => sameHandle(h, state.sel)) ?? list[0];
}

export function select(state: CurveEditState, h: CurveHandle | null): CurveEditState {
  return { curves: state.curves, sel: h };
}

/** Walks the handles in boundary order (a corner, then the points of the edge after it), wrapping at the ends. */
export function stepSelection(state: CurveEditState, dir: 'prev' | 'next' | 'first' | 'last'): CurveEditState {
  const list = curveHandles(state.curves);
  if (dir === 'first') return select(state, list[0]);
  if (dir === 'last') return select(state, list[list.length - 1]);
  const at = list.findIndex((h) => sameHandle(h, state.sel));
  const next = at < 0 ? (dir === 'next' ? 0 : list.length - 1) : (at + (dir === 'next' ? 1 : -1) + list.length) % list.length;
  return select(state, list[next]);
}

/** Replaces the curves with `next` if the engine would accept them. */
function commit(state: CurveEditState, next: CurveSet, sel: CurveHandle | null, what: StepResult['what']): StepResult {
  const problem = validateCurves(next);
  if (problem) return unchanged(state, problem);
  return { state: { curves: next, sel }, changed: true, what };
}

/** Moves a handle to `p` (a drag or a typed position). A hollow handle becomes a real point. */
export function moveTo(state: CurveEditState, h: CurveHandle, p: Pt): StepResult {
  const r = moveHandle(state.curves, h, p);
  if (!r) return unchanged(state, 'max');
  return commit(state, r.curves, r.handle, 'move');
}

/** Moves the selected handle by (dx, dy) normalised units (an arrow key). */
export function nudge(state: CurveEditState, dx: number, dy: number): StepResult {
  const h = state.sel;
  if (!h) return unchanged(state, 'nothing');
  const at = handlePos(state.curves, h);
  const r = moveHandle(state.curves, h, { x: at.x + dx, y: at.y + dy });
  if (!r) return unchanged(state, 'max');
  return commit(state, r.curves, r.handle, 'nudge');
}

/** Adds a point where `q` is nearest to an edge, if it is within `reach` (in the units of `scale`). */
export function addPointNear(state: CurveEditState, q: Pt, scale: Scale, reach: number): StepResult {
  const n = nearestOnPage(state.curves, q, scale);
  if (!n || n.d > reach) return unchanged(state, 'nothing');
  const r = insertAtNearest(state.curves, n);
  if (!r) return unchanged(state, 'max');
  return commit(state, r.curves, { kind: 'point', e: n.edge, m: r.m }, 'add');
}

/** The edge a handle belongs to, or the edge after a corner. */
export const edgeOfHandle = (h: CurveHandle): EdgeIndex => h.e;

/**
 * Adds a point to the selected edge, halfway along its longest gap (the button for what a double-click does).
 * With a corner selected it adds to the edge that starts there.
 */
export function addPointOnSelectedEdge(state: CurveEditState, scale: Scale): StepResult {
  const h = state.sel;
  if (!h) return unchanged(state, 'nothing');
  const e = h.e;
  const pts = edgeOf(state.curves, e);
  if (pts.length >= MAX_POINTS) return unchanged(state, 'max');
  // the longest segment between neighbouring points, in the scale given
  let best = 0;
  let bestLen = -1;
  for (let i = 0; i + 1 < pts.length; i++) {
    const len = Math.hypot((pts[i + 1].x - pts[i].x) * scale[0], (pts[i + 1].y - pts[i].y) * scale[1]);
    if (len > bestLen) {
      bestLen = len;
      best = i;
    }
  }
  const mid = { x: (pts[best].x + pts[best + 1].x) / 2, y: (pts[best].y + pts[best + 1].y) / 2 };
  const next = insertPointAt(state.curves, e, best, mid);
  if (!next) return unchanged(state, 'max');
  return commit(state, next, { kind: 'point', e, m: best }, 'add');
}

/** Removes the selected point (Delete, a double-click on it). An edge keeps at least its two corners. */
export function removeSelected(state: CurveEditState, h: CurveHandle | null = state.sel): StepResult {
  if (!h || h.kind !== 'point') return unchanged(state, h ? 'min' : 'nothing');
  const next = removePoint(state.curves, h.e, h.m);
  if (!next) return unchanged(state, 'min');
  // keep a tab stop: the point before it, or the hollow handle the edge shows once it has no points left
  const left = edgeOf(next, h.e).length - 2;
  const sel: CurveHandle = left <= 0 ? { kind: 'ghost', e: h.e, m: -1 } : { kind: 'point', e: h.e, m: Math.max(0, h.m - 1) };
  return commit(state, next, sel, 'remove');
}

/** The selected edge back to a straight line. */
export function straightenSelected(state: CurveEditState): StepResult {
  const h = state.sel;
  if (!h) return unchanged(state, 'nothing');
  if (edgeIsStraight(edgeOf(state.curves, h.e))) return unchanged(state, 'nothing');
  const next = straightenEdge(state.curves, h.e);
  return commit(state, next, { kind: 'ghost', e: h.e, m: -1 }, 'straighten');
}

/** All four edges straight, corners kept. */
export function resetAll(state: CurveEditState): StepResult {
  if (isStraightPage(state.curves)) return unchanged(state, 'nothing');
  return commit(state, resetCurves(state.curves), null, 'reset');
}

/** Can the selected handle be removed (a real point on an edge with more than its two corners)? */
export const canRemove = (state: CurveEditState): boolean => state.sel?.kind === 'point' && edgeOf(state.curves, state.sel.e).length > MIN_POINTS;

/** Can the selected edge take another point? */
export const canAdd = (state: CurveEditState): boolean => !!state.sel && edgeOf(state.curves, state.sel.e).length < MAX_POINTS;

export { handleKey };
