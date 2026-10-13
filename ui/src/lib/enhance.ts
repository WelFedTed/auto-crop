// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Enhancement in the editor: the pure rules (docs/dev/enhance-engine.md, PLAN 5, B13). Enhancement is SUGGESTED, never
// forced: nothing here applies a look by itself. The engine answers `suggestEnhance`; the person decides (Preview,
// Apply, Not now). No Svelte, no DOM, so it runs under `node --test`.
//
// What the panel does with this file:
//   * the looks and what they are called (`LOOKS`), the strength of a look, whether a crop is enhanced;
//   * the preview query the `acimg` route wants (`enhanceQuery`, strict like `parse_enhance_query` in the shell);
//   * the suggestion's life (`SuggestState`): asked for a page, shown as a banner, previewed without a record,
//     dismissed for the session, dropped when the page changes;
//   * the counts for "Enhancement is on for 2 of 3 items" and the Enhanced badge on a tile;
//   * what a look writes (`writesGrey`, `wantsPngHint`) for the hints near Save.

import { S } from './strings.ts';
import type { CropView, Enhance, EnhanceMode, EnhanceSuggestion, ItemView } from './types.ts';

/** The looks in the order the panel shows them. */
export const LOOKS: readonly EnhanceMode[] = ['original', 'clean', 'document', 'receipt'];

/** The label of the undo step "Apply" on a suggestion makes. */
export const APPLY_LABEL = 'Enhance: Clean (suggested)';

export const FULL = 100;

export const ORIGINAL: Enhance = { mode: 'original' };

export function modeOf(e: Enhance | null | undefined): EnhanceMode {
  return e?.mode ?? 'original';
}

/** Strength in whole percent: 0 for Original, else 0 to 100 (an absent strength is 100, more is read as 100). */
export function strengthOf(e: Enhance | null | undefined): number {
  if (!e || e.mode === 'original') return 0;
  const s = e.strength ?? FULL;
  return Number.isFinite(s) ? Math.min(FULL, Math.max(0, Math.round(s))) : FULL;
}

/** The look changes the pixels: not Original, and not a look at strength 0 (the engine treats that as Original too). */
export function isEnhanced(e: Enhance | null | undefined): boolean {
  return modeOf(e) !== 'original' && strengthOf(e) > 0;
}

export function sameLook(a: Enhance | null | undefined, b: Enhance | null | undefined): boolean {
  return isEnhanced(a) === isEnhanced(b) && (!isEnhanced(a) || (modeOf(a) === modeOf(b) && strengthOf(a) === strengthOf(b)));
}

/** A black-and-white look writes ONE grey channel and no colour profile. */
export function writesGrey(mode: EnhanceMode): boolean {
  return mode === 'document' || mode === 'receipt';
}

/** The query of an enhancement preview, exactly the shape `parse_enhance_query` accepts: `mode=<m>&s=<0..100>`. */
export function enhanceQuery(mode: EnhanceMode, strength: number): string {
  const s = mode === 'original' ? 0 : Math.min(FULL, Math.max(0, Math.round(Number.isFinite(strength) ? strength : 0)));
  return `mode=${mode}&s=${s}`;
}

/** Where a slider reading goes when the mode changes: a look keeps the strength it had; from Original it starts at full. */
export function strengthFor(from: Enhance | null | undefined, to: EnhanceMode): number {
  if (to === 'original') return 0;
  return isEnhanced(from) ? strengthOf(from) : FULL;
}

// ------------------------------------------------------------------------------------------ counts

export function includedCropsOf(item: ItemView): CropView[] {
  return item.crops.filter((c) => c.include && c.edit);
}

/** "On for `on` of `total` items": the included crops whose effective look changes the pixels. */
export function enhanceCounts(item: ItemView): { on: number; total: number } {
  const crops = includedCropsOf(item);
  return { on: crops.filter((c) => isEnhanced(c.enhance)).length, total: crops.length };
}

export function hasEnhancement(item: ItemView): boolean {
  return enhanceCounts(item).on > 0;
}

/** Crops that follow the image's default no longer: the "customised" dot. */
export function customisedCount(item: ItemView): number {
  return includedCropsOf(item).filter((c) => c.enhanceOverride !== null).length;
}

/**
 * The words next to a tile's badge: "Enhanced" for one item, "Enhanced 2 of 3" when only some items are.
 * Empty when nothing is enhanced.
 */
export function tileBadge(item: ItemView): string {
  const { on, total } = enhanceCounts(item);
  if (on === 0) return '';
  return total > 1 && on < total ? S.enhance.tileBadgeSome(on, total) : S.enhance.tileBadge;
}

// ------------------------------------------------------------------------------------------ the output

/** A source whose saved result is a JPEG unless the format changes (the hint near Save is about those). */
export function isJpegName(name: string): boolean {
  return /\.jpe?g$/i.test(name);
}

/** Some included crop will be saved as a black-and-white look from a JPEG source: PNG would keep faint marks exactly. */
export function wantsPngHint(item: ItemView): boolean {
  return isJpegName(item.name) && includedCropsOf(item).some((c) => isEnhanced(c.enhance) && writesGrey(modeOf(c.enhance)));
}

/** Some included crop will be saved as grey: the colour profile of the original is not kept. */
export function savesGrey(item: ItemView): boolean {
  return includedCropsOf(item).some((c) => isEnhanced(c.enhance) && writesGrey(modeOf(c.enhance)));
}

// ------------------------------------------------------------------------------------------ the suggestion

/** The geometry of a crop as one string: it changes when the PAGE changes (a corner, an angle, a turn, a curve), not with a look. */
export function pageKey(c: CropView): string {
  const r = (v: number) => Math.round(v * 1e5) / 1e5;
  const pts = (ps: { x: number; y: number }[]) => ps.map((p) => [r(p.x), r(p.y)]);
  const e = c.edit;
  const cv = c.curves;
  return JSON.stringify([
    e ? [pts(e.quad), e.quarterTurns, r(e.fineDeg)] : 0,
    c.mirror,
    cv ? [pts(cv.top), pts(cv.right), pts(cv.bottom), pts(cv.left), cv.quarterTurns, cv.mirror] : 0,
  ]);
}

/** What a suggestion was asked about: the image, the crop and its page. */
export interface SuggestCtx {
  item: number;
  crop: number;
  page: string;
}

export const sameCtx = (a: SuggestCtx | null, b: SuggestCtx | null): boolean =>
  !!a && !!b && a.item === b.item && a.crop === b.crop && a.page === b.page;

export const ctxOf = (item: number, c: CropView): SuggestCtx => ({ item, crop: c.id, page: pageKey(c) });

export const dismissKey = (item: number, crop: number): string => `${item}:${crop}`;

export type SuggestPhase =
  | { kind: 'idle' }
  | { kind: 'loading'; ctx: SuggestCtx }
  /** The engine has something to recommend for this page. `previewing` shows it without recording an edit. */
  | { kind: 'ready'; ctx: SuggestCtx; suggestion: EnhanceSuggestion; previewing: boolean }
  /** The engine says leave this crop alone: nothing is shown. */
  | { kind: 'none'; ctx: SuggestCtx }
  | { kind: 'error'; ctx: SuggestCtx };

export interface SuggestState {
  /** Counts requests: an answer is only taken for the latest one. */
  seq: number;
  phase: SuggestPhase;
}

export const initialSuggest = (): SuggestState => ({ seq: 0, phase: { kind: 'idle' } });

/** The suggestion in a state, if the banner has one to offer. */
export function readySuggestion(st: SuggestState): EnhanceSuggestion | null {
  return st.phase.kind === 'ready' ? st.phase.suggestion : null;
}

export function isPreviewing(st: SuggestState): boolean {
  return st.phase.kind === 'ready' && st.phase.previewing;
}

/** The ask: only when the person wants suggestions, the crop is on screen and has no look yet. */
export function wantsSuggestion(opts: { setting: boolean; ready: boolean; crop: CropView | null }): boolean {
  return opts.setting && opts.ready && !!opts.crop && opts.crop.include && !!opts.crop.edit && !isEnhanced(opts.crop.enhance);
}

/** A request is started for `ctx` (a repeat of the one in flight, or of an answer already held, changes nothing). */
export function suggestBegin(st: SuggestState, ctx: SuggestCtx): { state: SuggestState; ask: boolean } {
  const p = st.phase;
  if ((p.kind === 'loading' || p.kind === 'ready' || p.kind === 'none') && sameCtx(p.ctx, ctx)) return { state: st, ask: false };
  const seq = st.seq + 1;
  return { state: { seq, phase: { kind: 'loading', ctx } }, ask: true };
}

/** The engine answered request `seq`; `now` is the context on screen at this moment. Stale answers are ignored. */
export function suggestArrived(st: SuggestState, seq: number, now: SuggestCtx | null, answer: EnhanceSuggestion | null): SuggestState {
  const p = st.phase;
  if (seq !== st.seq || p.kind !== 'loading' || !sameCtx(p.ctx, now)) return st;
  return { ...st, phase: answer ? { kind: 'ready', ctx: p.ctx, suggestion: answer, previewing: false } : { kind: 'none', ctx: p.ctx } };
}

export function suggestFailed(st: SuggestState, seq: number, now: SuggestCtx | null): SuggestState {
  const p = st.phase;
  if (seq !== st.seq || p.kind !== 'loading' || !sameCtx(p.ctx, now)) return st;
  return { ...st, phase: { kind: 'error', ctx: p.ctx } };
}

/** The crop or its page changed (another crop, a corner moved, an undo): what was held is dropped, nothing is recorded. */
export function suggestContextChanged(st: SuggestState, now: SuggestCtx | null): SuggestState {
  const p = st.phase;
  if (p.kind === 'idle') return st;
  if (sameCtx(p.ctx, now)) return st;
  return { seq: st.seq + 1, phase: { kind: 'idle' } };
}

/** Preview on or off. It never records an edit: the picture comes from the preview route. */
export function suggestPreview(st: SuggestState, on: boolean): SuggestState {
  const p = st.phase;
  if (p.kind !== 'ready' || p.previewing === on) return st;
  return { ...st, phase: { ...p, previewing: on } };
}

/** After Apply or Not now: the banner is gone for this page. */
export function suggestReset(st: SuggestState): SuggestState {
  return st.phase.kind === 'idle' ? st : { seq: st.seq + 1, phase: { kind: 'idle' } };
}

/** The banner shows only when the person has not said "Not now" for this crop in this session. */
export function bannerVisible(st: SuggestState, dismissed: ReadonlySet<string>, now: SuggestCtx | null): boolean {
  const p = st.phase;
  return p.kind === 'ready' && sameCtx(p.ctx, now) && !dismissed.has(dismissKey(p.ctx.item, p.ctx.crop));
}

/** The look a suggestion would give, as a mode and a strength in percent. */
export function suggestedLook(s: EnhanceSuggestion): { mode: EnhanceMode; strength: number } {
  return { mode: s.mode, strength: Math.min(FULL, Math.max(0, Math.round(s.strength))) };
}

/** The suggestion is for a long thin page: the engine names Receipt as the black-and-white look that fits. */
export function looksLikeReceipt(s: EnhanceSuggestion): boolean {
  return s.blackAndWhite === 'receipt';
}

// ------------------------------------------------------------------------------------------ the batch

/** The summary after "Enhance all suggested" (a toast and a spoken line). */
export function batchLine(b: { images: number; crops: number; already: number; none: number; failed: number }): string {
  if (b.crops === 0) return S.enhance.batchNone(b.none, b.already);
  return S.enhance.batchDone(b.crops, b.images, b.none);
}
