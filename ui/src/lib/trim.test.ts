// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Trim to content: the state machine (check, offer, preview, apply, cancel, stale answers), the wire types against the Rust
// struct and a real engine answer, the shell command, the mock's stand-in and its history step, and the copy.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { pushEntry, type Entry } from './history.ts';
import { MATTED_PAPER, MOCK_TRIM_CAPTURE, mattedPicture, mockTrim } from './matted-model.ts';
import { addCrop, cloneState, editCrop, emptyState, isEdited, type ScanState } from './scan-model.ts';
import { S } from './strings.ts';
import {
  TRIM_LABEL,
  TRIM_REASONS,
  initialTrim,
  offerOf,
  previewOfTrim,
  sameTrimCtx,
  trimApply,
  trimApplyFailed,
  trimArrived,
  trimCancel,
  trimCheck,
  trimContextChanged,
  trimFailed,
  trimShow,
  trimStale,
  type TrimCtx,
  type TrimState,
} from './trim.ts';
import type { Pt, TrimProposal } from './types.ts';

const ctx = (renderKey = 'k1', crop = 1, item = 7): TrimCtx => ({ item, crop, renderKey });
const proposal = (): TrimProposal => ({ quad: mattedPicture() as TrimProposal['quad'], confidence: 0.9, reason: 'mat' });

/** Drives the machine to an available trim for `c`. */
function available(c: TrimCtx = ctx()): TrimState {
  const r = trimCheck(initialTrim(), c);
  assert.equal(r.ask, true);
  return trimArrived(r.state, r.state.seq, c, proposal());
}

// ------------------------------------------------------------------------------------------ the state machine

test('asking: a check, an answer, an offer; the same picture is not asked twice', () => {
  const c = ctx();
  const first = trimCheck(initialTrim(), c);
  assert.equal(first.ask, true);
  assert.equal(first.state.phase.kind, 'checking');
  assert.equal(trimCheck(first.state, c).ask, false, 'a second ask while loading changes nothing');
  const offered = trimArrived(first.state, first.state.seq, c, proposal());
  assert.equal(offered.phase.kind, 'available');
  assert.deepEqual(offerOf(offered), proposal());
  assert.equal(previewOfTrim(offered), null, 'nothing is drawn until the person presses the button');
  assert.equal(trimCheck(offered, c).ask, false, 'an answered picture is not asked again');
  // A new picture of the same crop (an edit changed the render key) is asked about.
  assert.equal(trimCheck(offered, ctx('k2')).ask, true);
});

test('no picture inside: the check ends as unavailable and offers nothing', () => {
  const c = ctx();
  const r = trimCheck(initialTrim(), c);
  const done = trimArrived(r.state, r.state.seq, c, null);
  assert.equal(done.phase.kind, 'unavailable');
  assert.equal(offerOf(done), null);
  assert.equal(trimShow(done, c), done, 'the button has nothing to open');
  assert.equal(trimCheck(done, c).ask, false);
});

test('a failed check (a curved crop, a crop that is gone) offers nothing and shows no error', () => {
  const c = ctx();
  const r = trimCheck(initialTrim(), c);
  const st = trimFailed(r.state, r.state.seq, c);
  assert.equal(st.phase.kind, 'unavailable');
  // An answer for another picture is dropped instead.
  const other = trimFailed(r.state, r.state.seq, ctx('k9'));
  assert.equal(other.phase.kind, 'idle');
});

test('stale answers are dropped: another picture, another request, no request at all', () => {
  const c = ctx();
  const r = trimCheck(initialTrim(), c);
  assert.equal(trimArrived(r.state, r.state.seq, ctx('k2'), proposal()).phase.kind, 'idle', 'the crop changed meanwhile');
  assert.equal(trimArrived(r.state, r.state.seq, null, proposal()).phase.kind, 'idle');
  assert.equal(trimArrived(r.state, r.state.seq + 5, c, proposal()), r.state, 'an old request is ignored');
  const done = trimArrived(r.state, r.state.seq, c, proposal());
  assert.equal(trimArrived(done, done.seq, c, null), done, 'a late duplicate is ignored');
});

test('the person previews, cancels and previews again; the preview is what the stage outlines', () => {
  const c = ctx();
  const shown = trimShow(available(c), c);
  assert.equal(shown.phase.kind, 'preview');
  assert.deepEqual(previewOfTrim(shown), proposal());
  const back = trimCancel(shown);
  assert.equal(back.phase.kind, 'available');
  assert.equal(previewOfTrim(back), null);
  assert.equal(trimCancel(back), back, 'cancelling nothing changes nothing');
  assert.equal(trimShow(back, c).phase.kind, 'preview');
  // A button press for a picture that has changed drops the offer and says so.
  const stale = trimShow(available(c), ctx('k2'));
  assert.equal(stale.phase.kind, 'idle');
  assert.equal(stale.staleDropped, true);
});

test('Apply is ONE effect: applyTrim of that crop, labelled like the engine labels the step; only a preview can be applied', () => {
  const c = ctx();
  assert.equal(trimApply(available(c), c).effect, null, 'an offer that was not previewed is not applied');
  const r = trimApply(trimShow(available(c), c), c);
  assert.deepEqual(r.effect, { item: 7, crop: 1, label: TRIM_LABEL });
  assert.equal(r.state.phase.kind, 'applied');
  assert.equal(trimApply(r.state, c).effect, null, 'no second application');
  // The picture changed between the preview and the click: nothing is applied.
  const late = trimApply(trimShow(available(c), c), ctx('k2'));
  assert.equal(late.effect, null);
  assert.equal(late.state.staleDropped, true);
  // The engine refused: nothing changed, and the machine says so.
  const failed = trimApplyFailed(r.state, 'ITEM_OP');
  assert.deepEqual(failed.phase, { kind: 'error', ctx: c, code: 'ITEM_OP' });
  assert.equal(trimApplyFailed(available(c), 'ITEM_OP').phase.kind, 'available', 'only an apply can fail');
});

test('a context change drops what was asked or previewed for another picture, and says so only for a preview', () => {
  const c = ctx();
  assert.equal(trimContextChanged(trimCheck(initialTrim(), c).state, ctx('k2')).phase.kind, 'idle');
  const dropped = trimContextChanged(available(c), ctx('k2'));
  assert.equal(dropped.phase.kind, 'idle');
  assert.equal(dropped.staleDropped, false, 'an offer that was never shown is dropped silently');
  const previewed = trimContextChanged(trimShow(available(c), c), ctx('k2'));
  assert.equal(previewed.phase.kind, 'idle');
  assert.equal(previewed.staleDropped, true);
  assert.equal(trimContextChanged(trimShow(available(c), c), c).phase.kind, 'preview', 'the same picture keeps its preview');
  assert.equal(trimContextChanged(available(c), null).phase.kind, 'idle', 'no crop on screen');
});

test('an applied trim stays until its commit has landed and the next change, a different crop clears it at once', () => {
  const c = ctx();
  let st = trimApply(trimShow(available(c), c), c).state;
  assert.equal(trimContextChanged(st, c), st, 'nothing changed yet');
  // The commit landed: the crop has a new render key.
  const landed = ctx('k2');
  st = trimContextChanged(st, landed);
  assert.deepEqual(st.phase, { kind: 'applied', ctx: c, landed: 'k2' });
  assert.equal(trimCheck(st, landed).ask, false, 'the applied message is not replaced by a new check');
  assert.equal(trimContextChanged(st, landed), st);
  // The next change (an undo, an edit) clears it.
  assert.equal(trimContextChanged(st, ctx('k3')).phase.kind, 'idle');
  // Another crop clears it at once.
  const early = trimApply(trimShow(available(c), c), c).state;
  assert.equal(trimContextChanged(early, ctx('k1', 2)).phase.kind, 'idle');
  assert.equal(trimContextChanged(early, null).phase.kind, 'idle');
});

test('dragging a handle drops a preview (said once) and leaves everything else alone', () => {
  const c = ctx();
  const st = trimStale(trimShow(available(c), c));
  assert.equal(st.phase.kind, 'idle');
  assert.equal(st.staleDropped, true);
  const a = available(c);
  assert.equal(trimStale(a), a);
});

test('contexts compare by image, crop and render key', () => {
  assert.equal(sameTrimCtx(ctx(), ctx()), true);
  assert.equal(sameTrimCtx(ctx(), ctx('k2')), false);
  assert.equal(sameTrimCtx(ctx(), ctx('k1', 2)), false);
  assert.equal(sameTrimCtx(ctx(), ctx('k1', 1, 8)), false);
  assert.equal(sameTrimCtx(null, ctx()), false);
});

// ------------------------------------------------------------------------------------------ the contract with the engine

const camel = (snake: string) => snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
const rust = (file: string) => readFileSync(new URL(`../../../${file}`, import.meta.url), 'utf8');

/** The serde field names of a Rust struct, read from its source (drift guard between the engine and the types). */
function rustFields(file: string, struct: string): string[] {
  const src = rust(file);
  const start = src.indexOf(`pub struct ${struct} {`);
  assert.ok(start >= 0, `${struct} not found`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^ {4}pub ([a-z_]+):/gm)].map((m) => camel(m[1])).sort();
}

test('the TypeScript keys are the fields of the engine struct TrimProposal', () => {
  assert.deepEqual(rustFields('crates/engine/src/trim.rs', 'TrimProposal'), ['confidence', 'quad', 'reason']);
});

test('the reasons are the variants of the engine enum TrimWhy, and each has its own words', () => {
  const src = rust('crates/engine/src/trim.rs');
  const start = src.indexOf('pub enum TrimWhy {');
  assert.ok(start >= 0);
  const body = src.slice(start, src.indexOf('\n}', start));
  const variants = [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*),/gm)].map((m) => m[1][0].toLowerCase() + m[1].slice(1));
  assert.deepEqual([...TRIM_REASONS], variants);
  for (const r of TRIM_REASONS) assert.ok(S.trim.why[r].length > 10, r);
  assert.deepEqual(Object.keys(S.trim.why).sort(), [...TRIM_REASONS].sort());
});

test('the undo label is the engine\'s, and the copy agrees with it', () => {
  assert.match(rust('crates/engine/src/trim.rs'), new RegExp(`pub const TRIM_LABEL: &str = "${TRIM_LABEL}";`));
  assert.equal(S.trim.label, TRIM_LABEL);
});

test('the mock\'s paper and picture are the sample the engine writes (crates/engine/src/samples.rs)', () => {
  const src = rust('crates/engine/src/samples.rs');
  const m = src.match(/pub const MATTED_PAPER: \[\(f64, f64\); 4\] = \[(.*)\];/);
  assert.ok(m, 'MATTED_PAPER is in the engine');
  const nums = [...m[1].matchAll(/\(([\d.]+), ([\d.]+)\)/g)].map((x) => ({ x: Number(x[1]), y: Number(x[2]) }));
  assert.deepEqual(nums, MATTED_PAPER);
  const share = src.match(/const MATTED_PICTURE: \(f64, f64, f64, f64\) = \(([\d.]+), ([\d.]+), ([\d.]+), ([\d.]+)\);/);
  assert.ok(share);
  assert.deepEqual(share.slice(1).map(Number), [0.17, 0.13, 0.83, 0.78]);
});

test('a real engine answer (fixture written by crates/shell/tests/trim_json.rs) has the shape of the type', () => {
  const j = JSON.parse(readFileSync(new URL('./fixtures/trim-proposal-matted.json', import.meta.url), 'utf8')) as TrimProposal;
  assert.deepEqual(Object.keys(j).sort(), ['confidence', 'quad', 'reason']);
  assert.equal(j.quad.length, 4);
  for (const p of j.quad) assert.ok(Number.isFinite(p.x) && Number.isFinite(p.y) && p.x > 0 && p.x < 1 && p.y > 0 && p.y < 1);
  assert.ok(j.confidence >= 0.5 && j.confidence <= 0.95);
  assert.ok((TRIM_REASONS as readonly string[]).includes(j.reason));
  // The engine found the picture the mock knows: the same corners, within a couple of tenths of a percent.
  const truth = mattedPicture();
  for (let i = 0; i < 4; i++) assert.ok(Math.hypot(j.quad[i].x - truth[i].x, j.quad[i].y - truth[i].y) < 0.006, `corner ${i}`);
});

test('the shell registers both commands on a worker, and the IPC wrappers call them by those names', () => {
  const app = rust('crates/shell/src/app.rs');
  assert.match(app, /async fn trim_to_content\(/, 'async: it must not run on the main thread');
  assert.match(app, /blocking\(move \|\| engine\.trim_to_content\(id, crop\)\)/);
  assert.match(app, /async fn apply_trim\(/);
  assert.match(app, /blocking\(move \|\| engine\.apply_trim\(id, crop\)\)/);
  const handler = app.slice(app.indexOf('generate_handler!'));
  assert.match(handler, /\btrim_to_content,/);
  assert.match(handler, /\bapply_trim,/);
  const backend = readFileSync(new URL('./backend.ts', import.meta.url), 'utf8');
  assert.match(backend, /invoke<TrimProposal \| null>\('trim_to_content', \{ id, crop \}\)/);
  assert.match(backend, /invoke<ItemView>\('apply_trim', \{ id, crop \}\)/);
  assert.match(backend, /trimToContent: \(id, crop\) => impl\.api\.trimToContent\(id, crop\)/);
  assert.match(backend, /applyTrim: \(id, crop\) => impl\.api\.applyTrim\(id, crop\)/);
});

// ------------------------------------------------------------------------------------------ the mock

test('mock: the matted sample offers the picture, any other page offers nothing', () => {
  const p = mockTrim(true, MATTED_PAPER);
  assert.ok(p);
  assert.equal(p.reason, 'mat');
  assert.deepEqual(p.quad, mattedPicture());
  // inside the paper, with a real mat around it
  for (const q of p.quad) assert.ok(q.x > 0.3 && q.x < 0.7 && q.y > 0.15 && q.y < 0.8);
  assert.equal(mockTrim(false, MATTED_PAPER), null, 'another page');
  // a crop that already is the picture has nothing to trim to
  assert.equal(mockTrim(true, mattedPicture()), null);
  // a loose crop a little outside the paper still finds the picture, a far one does not
  const loose = MATTED_PAPER.map((q) => ({ x: q.x + MOCK_TRIM_CAPTURE / 2, y: q.y - MOCK_TRIM_CAPTURE / 2 }));
  assert.ok(mockTrim(true, loose));
  const far: Pt[] = MATTED_PAPER.map((q) => ({ x: q.x + 0.2, y: q.y }));
  assert.equal(mockTrim(true, far), null);
  assert.equal(mockTrim(true, MATTED_PAPER.slice(0, 3)), null);
});

test('mock: applying is one history step that edits the crop; undo and redo walk it', () => {
  let s: ScanState = emptyState();
  s = addCrop(s, MATTED_PAPER.map((q) => ({ ...q })) as typeof MATTED_PAPER, 'auto', [1000, 1000], { score: 0.99, forced: 'check', reasons: [{ code: 'DETECTORS_DISAGREE' }] }).state;
  const base = cloneState(s);
  const crop = s.crops[0];
  const found = mockTrim(true, crop.quad);
  assert.ok(found);
  const next = editCrop(s, crop.id, { quad: found.quad, quarterTurns: crop.quarterTurns, fineDeg: 0 }, [1000, 1000]);
  let hist: Entry<ScanState>[] = [{ state: base, label: 'Auto' }];
  const pushed = pushEntry(hist, 0, { state: next, label: TRIM_LABEL });
  hist = pushed.hist;
  assert.equal(hist.length, 2, 'ONE undo step');
  assert.equal(hist[1].label, 'Trim to content');
  assert.equal(next.crops[0].origin, 'autoThenEdited', 'an edited crop');
  assert.equal(isEdited(next, base), true);
  assert.deepEqual(next.crops[0].quad, mattedPicture());
  // the detector's hold is untouched: the confidence of the crop is still the one it had
  assert.equal(next.crops[0].confidence?.reasons[0].code, 'DETECTORS_DISAGREE');
  // once trimmed there is nothing more to trim to
  assert.equal(mockTrim(true, next.crops[0].quad), null);
  // undo: the paper edge again
  assert.deepEqual(hist[0].state.crops[0].quad, MATTED_PAPER);
});

// ------------------------------------------------------------------------------------------ the copy

test('the copy says what the task asks, in plain language', () => {
  assert.equal(S.trim.button, 'Trim to content');
  assert.equal(S.trim.none, 'No picture inside to trim to');
  assert.equal(S.trim.holdQuestion, 'A picture sits inside a border. Keep the whole page, or Trim to content?');
  assert.equal(S.trim.keepPage, 'Keep the whole page');
  assert.match(S.trim.buttonHint, /nothing changes until you press Apply/i);
  assert.match(S.trim.applied, /Undo/);
  assert.match(S.trim.cancelled, /not changed/);
  assert.match(S.trim.applyHint, /one step/);
  assert.match(S.trim.cancelHint, /Esc/);
  const walk = (o: unknown, path: string): void => {
    if (typeof o === 'string') assert.ok(o.trim().length > 0, path);
    else if (o && typeof o === 'object') for (const [k, v] of Object.entries(o)) walk(v, `${path}.${k}`);
  };
  walk(S.trim, 'S.trim');
});
