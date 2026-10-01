// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Review decisions (accepted, skipped) are UI state, not engine state. This is the pure reducer with one
// undo step per bulk action ("Undo last decision" in the grid header).

import type { Decision, DecisionMap } from './review.ts';

export interface DecisionState {
  map: DecisionMap;
  /** Snapshots before each change, newest last. Capped so a long session cannot grow without bound. */
  history: DecisionMap[];
}

export const HISTORY_CAP = 100;

export function emptyDecisions(): DecisionState {
  return { map: {}, history: [] };
}

/** Sets `decision` on every id, or clears it when `decision` is null. A no-op change adds no history. */
export function applyDecision(state: DecisionState, ids: number[], decision: Decision | null): DecisionState {
  const map: DecisionMap = { ...state.map };
  let changed = false;
  for (const id of ids) {
    if (decision === null) {
      if (id in map) {
        delete map[id];
        changed = true;
      }
    } else if (map[id] !== decision) {
      map[id] = decision;
      changed = true;
    }
  }
  if (!changed) return state;
  const history = [...state.history, state.map].slice(-HISTORY_CAP);
  return { map, history };
}

export function undoDecision(state: DecisionState): DecisionState {
  if (state.history.length === 0) return state;
  return { map: state.history[state.history.length - 1], history: state.history.slice(0, -1) };
}

/** Drops decisions about items that are no longer in the batch. */
export function pruneDecisions(state: DecisionState, validIds: Set<number>): DecisionState {
  const keep = (m: DecisionMap): DecisionMap => {
    const out: DecisionMap = {};
    for (const [k, v] of Object.entries(m)) if (validIds.has(Number(k))) out[Number(k)] = v;
    return out;
  };
  const map = keep(state.map);
  if (Object.keys(map).length === Object.keys(state.map).length) return state;
  return { map, history: state.history.map(keep) };
}

export function serialiseDecisions(map: DecisionMap): string {
  return JSON.stringify(map);
}

export function parseDecisions(text: string | null): DecisionMap {
  if (!text) return {};
  try {
    const raw: unknown = JSON.parse(text);
    if (!raw || typeof raw !== 'object') return {};
    const out: DecisionMap = {};
    for (const [k, v] of Object.entries(raw as Record<string, unknown>)) {
      const id = Number(k);
      if (Number.isInteger(id) && (v === 'accepted' || v === 'skipped')) out[id] = v;
    }
    return out;
  } catch {
    return {};
  }
}
