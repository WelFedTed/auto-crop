// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// One undo stack for the review grid. Two kinds of action live in it: a review decision (accepted, skipped:
// UI state, undone by decisions.ts) and a session command that changed many images at once ("Treat as one item
// (12 images)": engine state, undone by the engine's `sessionUndo`). Undo takes the most recent of either, so
// Ctrl+Z always does what the person just did. Only session commands can be redone: the engine keeps them.

export type GridAction = { kind: 'decision' } | { kind: 'session'; label: string };

export interface GridHistory {
  undo: GridAction[];
  redo: Extract<GridAction, { kind: 'session' }>[];
}

export const emptyGridHistory = (): GridHistory => ({ undo: [], redo: [] });

/** A new action: it is undoable and anything that could have been redone is gone. */
export function pushAction(h: GridHistory, a: GridAction): GridHistory {
  return { undo: [...h.undo, a].slice(-200), redo: [] };
}

export function nextUndo(h: GridHistory): GridAction | null {
  return h.undo.length ? h.undo[h.undo.length - 1] : null;
}

export function nextRedo(h: GridHistory): Extract<GridAction, { kind: 'session' }> | null {
  return h.redo.length ? h.redo[h.redo.length - 1] : null;
}

/** After the most recent action was undone (the caller undid it): a session command becomes redoable. */
export function afterUndo(h: GridHistory, label?: string): GridHistory {
  const a = nextUndo(h);
  if (!a) return h;
  const undo = h.undo.slice(0, -1);
  if (a.kind === 'session') return { undo, redo: [...h.redo, { kind: 'session', label: label ?? a.label }] };
  return { undo, redo: h.redo };
}

/** After the most recent redoable command was redone. */
export function afterRedo(h: GridHistory, label?: string): GridHistory {
  const a = nextRedo(h);
  if (!a) return h;
  return { undo: [...h.undo, { kind: 'session', label: label ?? a.label }], redo: h.redo.slice(0, -1) };
}

/** Drops decision entries beyond what the decision reducer still remembers (its history is capped). */
export function reconcile(h: GridHistory, decisionDepth: number): GridHistory {
  let seen = 0;
  const keep: GridAction[] = [];
  for (let i = h.undo.length - 1; i >= 0; i--) {
    const a = h.undo[i];
    if (a.kind === 'decision') {
      if (seen >= decisionDepth) continue;
      seen++;
    }
    keep.unshift(a);
  }
  return keep.length === h.undo.length ? h : { undo: keep, redo: h.redo };
}
