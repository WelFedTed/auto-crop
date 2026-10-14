// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// "Trim to content": the state machine of asking the engine whether a picture sits inside a border of the page's own
// colour and of previewing and applying the trim (docs/perf/detector-baseline.md, "The matted-print question"). Pure,
// so it is tested without a webview.
//
//   idle -> checking -> unavailable
//                   \-> available -> preview -> applied
//                                         \-> (cancel) available
//
// The engine only PROPOSES. Asking (`checking`) records nothing and runs by itself whenever the crop on screen changes,
// because it is cheap and the button is shown only when a trim exists. Pressing the button opens a `preview` (a dashed
// outline on the picture and the trimmed page in the inset); only `apply` yields the one effect that changes the crop:
// `applyTrim`, which the engine recomputes itself (no number from here is trusted), as ONE undo step.
//
// Stale answers. An answer is only ever used for the crop and the picture it was asked about. The context is
// (image, crop, the crop's render key): switching crop or image, any committed edit, an undo or a redo changes it, and a
// checking or previewed trim is dropped (`staleDropped` says so once when a preview was dropped). An answer that arrives
// for a request that has since been replaced or dropped is ignored.

import type { ErrorCode, TrimProposal, TrimWhy } from './types.ts';

/** The label of the undo step an applied trim makes (the engine's `TRIM_LABEL`). */
export const TRIM_LABEL = 'Trim to content';

/** The reasons the engine can give (the engine's `TrimWhy`), in the order of its enum. */
export const TRIM_REASONS: readonly TrimWhy[] = ['mat', 'thinBorder'];

/** What a trim is about: the image, the crop and the crop's render key (changes with every committed edit). */
export interface TrimCtx {
  item: number;
  crop: number;
  renderKey: string;
}

export const sameTrimCtx = (a: TrimCtx | null, b: TrimCtx | null): boolean =>
  !!a && !!b && a.item === b.item && a.crop === b.crop && a.renderKey === b.renderKey;

export type TrimError = ErrorCode | 'UNKNOWN';

export type TrimPhase =
  | { kind: 'idle' }
  | { kind: 'checking'; ctx: TrimCtx }
  /** The engine looked: there is no picture inside to trim to. */
  | { kind: 'unavailable'; ctx: TrimCtx }
  /** A trim exists; nothing is drawn yet. */
  | { kind: 'available'; ctx: TrimCtx; proposal: TrimProposal }
  /** The person pressed Trim to content: the outline is drawn and the inset shows the result. */
  | { kind: 'preview'; ctx: TrimCtx; proposal: TrimProposal }
  /** `landed` is the render key after the apply has landed (null until it has). */
  | { kind: 'applied'; ctx: TrimCtx; landed: string | null }
  | { kind: 'error'; ctx: TrimCtx; code: TrimError };

export interface TrimState {
  /** Counts requests: an answer is only taken for the latest one. */
  seq: number;
  phase: TrimPhase;
  /** The last transition dropped a preview because the crop or its picture changed. */
  staleDropped: boolean;
}

export const initialTrim = (): TrimState => ({ seq: 0, phase: { kind: 'idle' }, staleDropped: false });

const idle = (st: TrimState, staleDropped = false): TrimState => ({ seq: st.seq, phase: { kind: 'idle' }, staleDropped });

// ------------------------------------------------------------------------------------------ transitions

/**
 * The crop on screen is `ctx`: ask the engine unless it has been asked about exactly this picture already. A second
 * ask for the same picture while one is loading changes nothing.
 */
export function trimCheck(st: TrimState, ctx: TrimCtx): { state: TrimState; ask: boolean } {
  const p = st.phase;
  if (p.kind !== 'idle' && p.kind !== 'error' && sameTrimCtx(p.ctx, ctx)) return { state: st, ask: false };
  // An applied trim keeps saying so, and is not asked about again, until the next change of the picture.
  if (p.kind === 'applied' && ctx.item === p.ctx.item && ctx.crop === p.ctx.crop && (p.landed === null || ctx.renderKey === p.landed)) {
    return { state: st, ask: false };
  }
  const seq = st.seq + 1;
  return { state: { seq, phase: { kind: 'checking', ctx }, staleDropped: false }, ask: true };
}

/** The engine answered request `seq` for `ctx`; `now` is the context on screen at this moment. */
export function trimArrived(st: TrimState, seq: number, now: TrimCtx | null, answer: TrimProposal | null): TrimState {
  const p = st.phase;
  if (p.kind !== 'checking' || seq !== st.seq) return st;
  if (!sameTrimCtx(p.ctx, now)) return idle(st);
  return { ...st, phase: answer ? { kind: 'available', ctx: p.ctx, proposal: answer } : { kind: 'unavailable', ctx: p.ctx }, staleDropped: false };
}

/** The ask failed (a curved crop, an unknown crop): nothing to offer, and no error shown for a check. */
export function trimFailed(st: TrimState, seq: number, now: TrimCtx | null): TrimState {
  const p = st.phase;
  if (p.kind !== 'checking' || seq !== st.seq) return st;
  return sameTrimCtx(p.ctx, now) ? { ...st, phase: { kind: 'unavailable', ctx: p.ctx }, staleDropped: false } : idle(st);
}

/**
 * The crop on screen changed (`now` is null when there is none or it cannot be trimmed). Whatever was asked, offered or
 * previewed for another picture is dropped (a dropped preview is said once). An applied trim stays while its own commit
 * lands, then is cleared by the next change; a different crop or image clears it at once.
 */
export function trimContextChanged(st: TrimState, now: TrimCtx | null): TrimState {
  const p = st.phase;
  switch (p.kind) {
    case 'idle':
      return st;
    case 'checking':
    case 'unavailable':
    case 'available':
      return sameTrimCtx(p.ctx, now) ? st : idle(st);
    case 'preview':
      return sameTrimCtx(p.ctx, now) ? st : idle(st, true);
    case 'applied':
      if (!now || now.item !== p.ctx.item || now.crop !== p.ctx.crop) return idle(st);
      if (p.landed === null) return now.renderKey === p.ctx.renderKey ? st : { ...st, phase: { ...p, landed: now.renderKey } };
      return now.renderKey === p.landed ? st : idle(st);
    case 'error':
      return now && now.item === p.ctx.item && now.crop === p.ctx.crop ? st : idle(st);
  }
}

/** The person pressed Trim to content: only an available trim for the picture on screen opens a preview. */
export function trimShow(st: TrimState, now: TrimCtx | null): TrimState {
  const p = st.phase;
  if (p.kind !== 'available') return st;
  if (!sameTrimCtx(p.ctx, now)) return idle(st, true);
  return { ...st, phase: { kind: 'preview', ctx: p.ctx, proposal: p.proposal }, staleDropped: false };
}

/** What applying does to the crop: `applyTrim`, one undo step. */
export interface TrimEffect {
  item: number;
  crop: number;
  label: string;
}

/** The person presses Apply. Only a preview of the picture still on screen can be applied. */
export function trimApply(st: TrimState, now: TrimCtx | null): { state: TrimState; effect: TrimEffect | null } {
  const p = st.phase;
  if (p.kind !== 'preview') return { state: st, effect: null };
  if (!sameTrimCtx(p.ctx, now)) return { state: idle(st, true), effect: null };
  return {
    state: { ...st, phase: { kind: 'applied', ctx: p.ctx, landed: null }, staleDropped: false },
    effect: { item: p.ctx.item, crop: p.ctx.crop, label: TRIM_LABEL },
  };
}

/** The engine refused the trim: nothing changed. */
export function trimApplyFailed(st: TrimState, code: TrimError): TrimState {
  const p = st.phase;
  if (p.kind !== 'applied') return st;
  return { ...st, phase: { kind: 'error', ctx: p.ctx, code }, staleDropped: false };
}

/** Cancel (the button, Esc): a preview goes back to "available". Nothing was changed. */
export function trimCancel(st: TrimState): TrimState {
  const p = st.phase;
  if (p.kind !== 'preview') return st;
  return { ...st, phase: { kind: 'available', ctx: p.ctx, proposal: p.proposal }, staleDropped: false };
}

/** The person started changing the page (a handle drag): a preview no longer fits it. */
export function trimStale(st: TrimState): TrimState {
  return st.phase.kind === 'preview' ? idle(st, true) : st;
}

// ------------------------------------------------------------------------------------------ reading the state

/** The proposal on offer (available or previewed), if any. */
export const offerOf = (st: TrimState): TrimProposal | null => (st.phase.kind === 'available' || st.phase.kind === 'preview' ? st.phase.proposal : null);

/** The proposal being previewed, if any: what the stage outlines and the inset shows. */
export const previewOfTrim = (st: TrimState): TrimProposal | null => (st.phase.kind === 'preview' ? st.phase.proposal : null);
