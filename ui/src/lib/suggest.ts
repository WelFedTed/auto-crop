// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// "Suggest curves": the state machine of asking the engine for a curve suggestion and the honest reading of its
// answer (docs/dev/curved-pages.md "Suggestion API", docs/perf/curve-fit-baseline.md). Pure, so it is tested without
// a webview.
//
//   idle -> loading -> preview -> applied
//                  \         \-> dismissed
//                   \-> error
//
// The engine only PROPOSES. Asking records nothing; `apply` yields the one effect that changes the crop: a
// `setCurves` with the suggestion's curves WHOLE, labelled 'Suggest curves', as ONE undo step. A page that is curved
// stays held for review and accept exactly as before.
//
// Stale answers. An answer is only ever shown for the crop and the picture it was asked about. The context is
// (image, crop, the crop's render key): switching crop or image, any committed edit, an undo or a redo changes it, and
// the loading or previewed suggestion is dropped (`staleDropped` says so once, so the screen can tell the person).
// An answer that arrives for a request that has since been dismissed, replaced or dropped is ignored.
//
// `looksCurved` is deliberately not read anywhere: on the owner's real receipts it fired on 13% of the pages
// labelled flat (and on a flat sample page of the app itself), so no chip, badge or filter is built on it.

import { EDGE_KEYS } from './curve.ts';
import { S } from './strings.ts';
import type { CurveSet, CurveSuggestion, ErrorCode, Side } from './types.ts';

/** An edge seen with less confidence than this is called out. (The engine's own `looksCurved` gate is 0.7 too.) */
export const LOW_CONFIDENCE = 0.7;
/** Under this bend (percent of the edge's length) the suggestion is "nearly straight". */
export const NEARLY_STRAIGHT_PCT = 1.5;
/** The label of the undo step an applied suggestion makes. */
export const SUGGEST_LABEL = 'Suggest curves';

export const SIDES: readonly Side[] = EDGE_KEYS;

/** What a suggestion is about: the image, the crop and the crop's render key (changes with every committed edit). */
export interface SuggestCtx {
  item: number;
  crop: number;
  renderKey: string;
}

export const sameCtx = (a: SuggestCtx | null, b: SuggestCtx | null): boolean =>
  !!a && !!b && a.item === b.item && a.crop === b.crop && a.renderKey === b.renderKey;

export type SuggestError = ErrorCode | 'NOT_READY' | 'UNKNOWN';

export type SuggestPhase =
  | { kind: 'idle' }
  | { kind: 'loading'; ctx: SuggestCtx }
  | { kind: 'preview'; ctx: SuggestCtx; suggestion: CurveSuggestion; notes: SuggestNotes }
  /** `landed` is the render key after the apply has landed (null until it has). */
  | { kind: 'applied'; ctx: SuggestCtx; landed: string | null }
  | { kind: 'dismissed' }
  | { kind: 'error'; ctx: SuggestCtx; code: SuggestError };

export interface SuggestState {
  /** Counts requests: an answer is only taken for the latest one. */
  seq: number;
  phase: SuggestPhase;
  /** The last transition dropped a suggestion because the crop or its picture changed. */
  staleDropped: boolean;
}

export const initialSuggest = (): SuggestState => ({ seq: 0, phase: { kind: 'idle' }, staleDropped: false });

// ------------------------------------------------------------------------------------------ reading the answer

export interface SuggestNotes {
  /** No bent edge: the fit was refused, or all four edges came back straight. "The page may be flat." */
  noCurve: boolean;
  /** The edges seen with confidence under `LOW_CONFIDENCE` (never listed when the fit was refused). */
  lowConfidence: Side[];
  /** There is a bend, but the largest is under `NEARLY_STRAIGHT_PCT`: applying may not help. */
  nearlyStraight: boolean;
  /** The edges that came back bent (more than two points), in the order top, right, bottom, left. */
  bent: Side[];
  /** The largest bend among the edges, percent of the edge's length. */
  peak: number;
}

/**
 * The plain reading of a suggestion's honesty fields. `peak` is the largest of `curvature` AND `maxCurvature`:
 * `maxCurvature` alone is 0 when no edge passes every `looksCurved` gate, which would call a clearly bent edge
 * "nearly straight".
 */
export function suggestionNotes(s: CurveSuggestion): SuggestNotes {
  const bent = SIDES.filter((side) => s.curves[side].length > 2);
  const peak = Math.max(0, s.maxCurvature, ...s.curvature);
  const noCurve = s.fellBack || bent.length === 0;
  const lowConfidence = s.fellBack ? [] : SIDES.filter((_, i) => s.confidence[i] < LOW_CONFIDENCE);
  return { noCurve, lowConfidence, nearlyStraight: !noCurve && peak < NEARLY_STRAIGHT_PCT, bent: s.fellBack ? [] : bent, peak };
}

/** The lines the panel shows and speaks for a suggestion, honesty first. */
export function suggestionLines(n: SuggestNotes): string[] {
  const lines: string[] = [];
  if (n.noCurve) lines.push(S.suggest.noCurve);
  if (n.lowConfidence.length > 0) lines.push(S.suggest.lowConfidence(n.lowConfidence));
  if (n.nearlyStraight) lines.push(S.suggest.nearlyStraight(n.bent.length));
  return lines;
}

// ------------------------------------------------------------------------------------------ transitions

export interface Step {
  state: SuggestState;
}

/** The person asks. A second ask for the same picture while one is loading changes nothing. */
export function suggestBegin(st: SuggestState, ctx: SuggestCtx): Step & { ask: boolean } {
  if (st.phase.kind === 'loading' && sameCtx(st.phase.ctx, ctx)) return { state: st, ask: false };
  const seq = st.seq + 1;
  return { state: { seq, phase: { kind: 'loading', ctx }, staleDropped: false }, ask: true };
}

/**
 * The engine answered request `seq`. `now` is the context on screen at this moment. Taken only for the latest
 * request while it is still loading and the picture is the one asked about; `null` (the image was not ready) is an
 * error the person can retry.
 */
export function suggestArrived(st: SuggestState, seq: number, now: SuggestCtx | null, answer: CurveSuggestion | null): SuggestState {
  const p = st.phase;
  if (p.kind !== 'loading' || seq !== st.seq) return st;
  if (!sameCtx(p.ctx, now)) return { ...st, phase: { kind: 'idle' }, staleDropped: true };
  if (!answer) return { ...st, phase: { kind: 'error', ctx: p.ctx, code: 'NOT_READY' }, staleDropped: false };
  return { ...st, phase: { kind: 'preview', ctx: p.ctx, suggestion: answer, notes: suggestionNotes(answer) }, staleDropped: false };
}

/** The ask failed. Same staleness rules as an answer. */
export function suggestFailed(st: SuggestState, seq: number, now: SuggestCtx | null, code: SuggestError): SuggestState {
  const p = st.phase;
  if (p.kind !== 'loading' || seq !== st.seq) return st;
  if (!sameCtx(p.ctx, now)) return { ...st, phase: { kind: 'idle' }, staleDropped: true };
  return { ...st, phase: { kind: 'error', ctx: p.ctx, code }, staleDropped: false };
}

/**
 * The crop on screen changed (`now` is null when there is none). A loading or previewed suggestion for any other
 * context is dropped. An applied one stays while its own commit lands, then is cleared by the next change (an undo,
 * a redo or an edit would make "applied" untrue); a different crop or image clears it at once.
 */
export function suggestContextChanged(st: SuggestState, now: SuggestCtx | null): SuggestState {
  const p = st.phase;
  if (p.kind === 'loading' || p.kind === 'preview') {
    return sameCtx(p.ctx, now) ? st : { ...st, phase: { kind: 'idle' }, staleDropped: true };
  }
  if (p.kind === 'applied') {
    if (!now || now.item !== p.ctx.item || now.crop !== p.ctx.crop) return { ...st, phase: { kind: 'idle' }, staleDropped: false };
    if (p.landed === null) return now.renderKey === p.ctx.renderKey ? st : { ...st, phase: { ...p, landed: now.renderKey } };
    return now.renderKey === p.landed ? st : { ...st, phase: { kind: 'idle' }, staleDropped: false };
  }
  if (p.kind === 'error') {
    return now && now.item === p.ctx.item && now.crop === p.ctx.crop ? st : { ...st, phase: { kind: 'idle' }, staleDropped: false };
  }
  return st;
}

/** What applying does to the crop: a `setCurves` of the suggestion's curves whole, one undo step. */
export interface ApplyEffect {
  item: number;
  crop: number;
  curves: CurveSet;
  label: string;
}

/** The person presses Apply. Only a preview of the picture still on screen can be applied. */
export function suggestApply(st: SuggestState, now: SuggestCtx | null): Step & { effect: ApplyEffect | null } {
  const p = st.phase;
  if (p.kind !== 'preview') return { state: st, effect: null };
  if (!sameCtx(p.ctx, now)) return { state: { ...st, phase: { kind: 'idle' }, staleDropped: true }, effect: null };
  return {
    state: { ...st, phase: { kind: 'applied', ctx: p.ctx, landed: null }, staleDropped: false },
    effect: { item: p.ctx.item, crop: p.ctx.crop, curves: p.suggestion.curves, label: SUGGEST_LABEL },
  };
}

/** The engine refused the applied curves: nothing changed. */
export function suggestApplyFailed(st: SuggestState, code: SuggestError): SuggestState {
  const p = st.phase;
  if (p.kind !== 'applied') return st;
  return { ...st, phase: { kind: 'error', ctx: p.ctx, code }, staleDropped: false };
}

/** Dismiss: drops a loading (its answer will be ignored) or previewed suggestion. Nothing was changed. */
export function suggestDismiss(st: SuggestState): SuggestState {
  const p = st.phase;
  if (p.kind !== 'loading' && p.kind !== 'preview') return st;
  return { seq: st.seq + 1, phase: { kind: 'dismissed' }, staleDropped: false };
}

/** Clears an applied, dismissed or failed suggestion (the request counter keeps counting). */
export function suggestReset(st: SuggestState): SuggestState {
  return { seq: st.seq + 1, phase: { kind: 'idle' }, staleDropped: false };
}

/** The person started changing the page (a handle drag): a loading or previewed suggestion no longer fits it. */
export function suggestStale(st: SuggestState): SuggestState {
  const k = st.phase.kind;
  if (k !== 'loading' && k !== 'preview') return st;
  return { seq: st.seq + 1, phase: { kind: 'idle' }, staleDropped: true };
}

/** The suggestion being previewed, if any (what the stage outlines and the inset flattens). */
export const previewOf = (st: SuggestState): CurveSuggestion | null => (st.phase.kind === 'preview' ? st.phase.suggestion : null);
