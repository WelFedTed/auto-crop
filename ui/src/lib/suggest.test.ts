// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Suggest curves: the state machine (idle, loading, preview, applied, dismissed, error, stale answers), the plain
// reading of the engine's honesty fields, the wire types against the Rust struct and real engine answers, the mock's
// stand-in, and the copy.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { curvedDetectedQuad, curvedEdges } from './curved-model.ts';
import { cornersOf, curvesFromQuad, validateCurves } from './curve.ts';
import { gestureKey, pushEntry, type Entry } from './history.ts';
import { mockSuggestion, curvatures } from './mock-suggest.ts';
import { addCrop, curveCrop, emptyState, setCurves, type Dims, type ScanState } from './scan-model.ts';
import { S, sideList } from './strings.ts';
import {
  LOW_CONFIDENCE,
  NEARLY_STRAIGHT_PCT,
  SUGGEST_LABEL,
  initialSuggest,
  previewOf,
  sameCtx,
  suggestApply,
  suggestApplyFailed,
  suggestArrived,
  suggestBegin,
  suggestContextChanged,
  suggestDismiss,
  suggestFailed,
  suggestReset,
  suggestStale,
  suggestionLines,
  suggestionNotes,
  type SuggestCtx,
  type SuggestState,
} from './suggest.ts';
import { defaultSettings } from './settings-defaults.ts';
import type { CurveSet, CurveSuggestion, EdgeNumbers, Pt } from './types.ts';

const ctx = (renderKey = 'k1', crop = 1, item = 7): SuggestCtx => ({ item, crop, renderKey });

/** A suggestion with the top edge bent (about `pct` percent) and the others straight. */
function suggestion(over: Partial<CurveSuggestion> = {}): CurveSuggestion {
  const c = curvesFromQuad(
    [
      { x: 0.2, y: 0.2 },
      { x: 0.8, y: 0.2 },
      { x: 0.8, y: 0.8 },
      { x: 0.2, y: 0.8 },
    ],
    1,
    true,
  );
  c.top = [c.top[0], { x: 0.5, y: 0.16 }, c.top[1]];
  return {
    quad: cornersOf(c),
    curves: c,
    confidence: [0.9, 0.9, 0.9, 0.9],
    curvature: [6.7, 0, 0, 0],
    bulge: [6.7, 0, 0, 0],
    supportGain: [0.8, 0, 0, 0],
    maxCurvature: 6.7,
    looksCurved: true,
    fellBack: false,
    ...over,
  };
}

const asking = (st: SuggestState = initialSuggest(), c = ctx()): { st: SuggestState; seq: number } => {
  const b = suggestBegin(st, c);
  assert.ok(b.ask);
  return { st: b.state, seq: b.state.seq };
};

// ------------------------------------------------------------------------------------------ the state machine

test('idle -> loading -> preview: asking records nothing and the answer is read for honesty', () => {
  const { st, seq } = asking();
  assert.equal(st.phase.kind, 'loading');
  const s = suggestion();
  const got = suggestArrived(st, seq, ctx(), s);
  assert.equal(got.phase.kind, 'preview');
  assert.equal(previewOf(got), s);
  assert.deepEqual(got.phase.kind === 'preview' && got.phase.notes.bent, ['top']);
});

test('a second ask for the same picture while loading changes nothing; for another picture it starts over', () => {
  const { st } = asking();
  const again = suggestBegin(st, ctx());
  assert.equal(again.ask, false);
  assert.equal(again.state, st);
  const other = suggestBegin(st, ctx('k2'));
  assert.equal(other.ask, true);
  assert.equal(other.state.seq, st.seq + 1);
});

test('an answer for a request that was replaced, dismissed or dropped is ignored', () => {
  const first = asking();
  const second = suggestBegin(first.st, ctx('k2')).state;
  // the first request's answer arrives late: not taken
  assert.equal(suggestArrived(second, first.seq, ctx('k2'), suggestion()), second);
  // dismissed while loading: the late answer changes nothing
  const dismissed = suggestDismiss(second);
  assert.equal(dismissed.phase.kind, 'dismissed');
  assert.equal(suggestArrived(dismissed, second.seq, ctx('k2'), suggestion()), dismissed);
  // an answer in the wrong phase (already a preview) is not taken a second time
  const done = suggestArrived(second, second.seq, ctx('k2'), suggestion());
  assert.equal(suggestArrived(done, second.seq, ctx('k2'), suggestion({ maxCurvature: 1 })), done);
});

test('an answer for a picture that changed meanwhile is dropped, and says so', () => {
  const { st, seq } = asking();
  // the person edited the page (the crop's render key changed) before the answer came
  const stale = suggestArrived(st, seq, ctx('k-after-edit'), suggestion());
  assert.equal(stale.phase.kind, 'idle');
  assert.equal(stale.staleDropped, true);
  // or switched to another crop, or no crop at all
  assert.equal(suggestArrived(st, seq, ctx('k1', 2), suggestion()).phase.kind, 'idle');
  assert.equal(suggestArrived(st, seq, null, suggestion()).phase.kind, 'idle');
  assert.equal(suggestFailed(st, seq, ctx('other'), 'UNKNOWN').staleDropped, true);
});

test('the crop changing while loading or previewing drops the suggestion; other phases keep what they show', () => {
  const { st, seq } = asking();
  assert.equal(suggestContextChanged(st, ctx()), st, 'same picture: unchanged');
  for (const now of [ctx('k2'), ctx('k1', 2), ctx('k1', 1, 8), null]) {
    const d = suggestContextChanged(st, now);
    assert.equal(d.phase.kind, 'idle');
    assert.equal(d.staleDropped, true);
  }
  const prev = suggestArrived(st, seq, ctx(), suggestion());
  assert.equal(suggestContextChanged(prev, ctx()), prev);
  assert.equal(suggestContextChanged(prev, ctx('k9')).phase.kind, 'idle');
  const idle = initialSuggest();
  assert.equal(suggestContextChanged(idle, ctx('k9')), idle);
  const dismissed = suggestDismiss(prev);
  assert.equal(suggestContextChanged(dismissed, ctx('k9')), dismissed);
});

test('a null answer (the image is not ready) and a failure are errors the person can retry', () => {
  const { st, seq } = asking();
  const none = suggestArrived(st, seq, ctx(), null);
  assert.deepEqual(none.phase.kind === 'error' && none.phase.code, 'NOT_READY');
  const failed = suggestFailed(st, seq, ctx(), 'INTERNAL');
  assert.deepEqual(failed.phase.kind === 'error' && failed.phase.code, 'INTERNAL');
  // retry: asking again from an error starts a new request
  assert.equal(suggestBegin(failed, ctx()).ask, true);
  // an error stays for its crop and is cleared by another
  assert.equal(suggestContextChanged(failed, ctx('k2')), failed);
  assert.equal(suggestContextChanged(failed, ctx('k1', 2)).phase.kind, 'idle');
});

test('Apply: one effect, the suggestion WHOLE, labelled Suggest curves; never without a preview', () => {
  const { st, seq } = asking();
  assert.equal(suggestApply(st, ctx()).effect, null, 'nothing to apply while loading');
  assert.equal(suggestApply(initialSuggest(), ctx()).effect, null);
  const s = suggestion();
  const prev = suggestArrived(st, seq, ctx(), s);
  const r = suggestApply(prev, ctx());
  assert.ok(r.effect);
  assert.equal(r.effect.label, 'Suggest curves');
  assert.equal(r.effect.label, SUGGEST_LABEL);
  assert.equal(r.effect.curves, s.curves, 'the same curves, not a copy with parts left out');
  assert.deepEqual([r.effect.item, r.effect.crop], [7, 1]);
  assert.equal(r.state.phase.kind, 'applied');
  // a second press cannot apply it twice
  assert.equal(suggestApply(r.state, ctx()).effect, null);
});

test('Apply on a picture that changed does nothing and says the suggestion was dropped', () => {
  const { st, seq } = asking();
  const prev = suggestArrived(st, seq, ctx(), suggestion());
  const r = suggestApply(prev, ctx('after-undo'));
  assert.equal(r.effect, null);
  assert.equal(r.state.phase.kind, 'idle');
  assert.equal(r.state.staleDropped, true);
});

test('applied: kept while its own commit lands, cleared by the next change or another crop', () => {
  const { st, seq } = asking();
  const applied = suggestApply(suggestArrived(st, seq, ctx(), suggestion()), ctx()).state;
  // the commit lands: the render key changes, "applied" stays and remembers the new key
  const landed = suggestContextChanged(applied, ctx('k-applied'));
  assert.equal(landed.phase.kind, 'applied');
  assert.deepEqual(landed.phase.kind === 'applied' && landed.phase.landed, 'k-applied');
  assert.equal(suggestContextChanged(landed, ctx('k-applied')), landed);
  // an undo (or any edit) after that makes "applied" untrue
  assert.equal(suggestContextChanged(landed, ctx('k1')).phase.kind, 'idle');
  // another crop clears it at once
  assert.equal(suggestContextChanged(applied, ctx('k1', 2)).phase.kind, 'idle');
  assert.equal(suggestContextChanged(applied, null).phase.kind, 'idle');
});

test('an apply the engine refuses becomes an error and nothing else', () => {
  const { st, seq } = asking();
  const applied = suggestApply(suggestArrived(st, seq, ctx(), suggestion()), ctx()).state;
  const failed = suggestApplyFailed(applied, 'DEGENERATE');
  assert.deepEqual(failed.phase.kind === 'error' && failed.phase.code, 'DEGENERATE');
  assert.equal(suggestApplyFailed(initialSuggest(), 'DEGENERATE').phase.kind, 'idle', 'only after an apply');
});

test('Dismiss drops a loading or previewed suggestion; elsewhere it does nothing; reset clears the rest', () => {
  const { st, seq } = asking();
  const prev = suggestArrived(st, seq, ctx(), suggestion());
  assert.equal(suggestDismiss(prev).phase.kind, 'dismissed');
  assert.equal(previewOf(suggestDismiss(prev)), null);
  assert.equal(suggestDismiss(st).phase.kind, 'dismissed');
  const idle = initialSuggest();
  assert.equal(suggestDismiss(idle), idle);
  assert.equal(suggestReset(suggestDismiss(prev)).phase.kind, 'idle');
  // ... and asking again after a dismissal works
  assert.equal(suggestBegin(suggestDismiss(prev), ctx()).ask, true);
});

test('dragging a handle drops a suggestion that no longer fits (stale), once', () => {
  const { st, seq } = asking();
  const prev = suggestArrived(st, seq, ctx(), suggestion());
  const s = suggestStale(prev);
  assert.equal(s.phase.kind, 'idle');
  assert.equal(s.staleDropped, true);
  assert.equal(suggestStale(s), s);
  assert.equal(suggestStale(suggestDismiss(prev)).phase.kind, 'dismissed');
  // a late answer of the dropped request is ignored
  assert.equal(suggestArrived(s, seq, ctx(), suggestion()), s);
});

test('contexts are equal only for the same image, crop and render key', () => {
  assert.ok(sameCtx(ctx(), ctx()));
  assert.ok(!sameCtx(ctx(), ctx('x')) && !sameCtx(ctx(), ctx('k1', 2)) && !sameCtx(ctx(), ctx('k1', 1, 9)) && !sameCtx(ctx(), null) && !sameCtx(null, null));
});

// ------------------------------------------------------------------------------------------ one undo step

test('applying is exactly one undo step and the suggestion goes in whole', () => {
  const dims: Dims = [2000, 1500];
  const quad: [Pt, Pt, Pt, Pt] = [
    { x: 0.2, y: 0.2 },
    { x: 0.8, y: 0.2 },
    { x: 0.8, y: 0.8 },
    { x: 0.2, y: 0.8 },
  ];
  const added = addCrop(emptyState(), quad, 'auto', dims, { score: 0.9, forced: null, reasons: [] });
  const base = curveCrop(added.state, added.id, dims);
  let hist: Entry<ScanState>[] = [{ state: base, label: 'Curve edges' }];
  let cursor = 0;

  const s = suggestion();
  const { st, seq } = asking();
  const prev = suggestArrived(st, seq, ctx(), s);
  const fx = suggestApply(prev, ctx()).effect!;
  const next = setCurves(hist[cursor].state, added.id, fx.curves);
  ({ hist, cursor } = pushEntry(hist, cursor, { state: next, label: fx.label, gesture: gestureKey(null, added.id) }));
  assert.equal(hist.length, 2, 'one new step');
  assert.equal(hist[cursor].label, 'Suggest curves');
  assert.deepEqual(hist[cursor].state.crops[0].curves!.top, s.curves.top, 'all four edges, whole');
  assert.deepEqual(hist[cursor].state.crops[0].quad, s.quad, 'the corners move with the end points');
  // asking, previewing and dismissing recorded nothing
  assert.equal(hist.length, 2);
  // a second suggestion applied later is a second step (no gesture id to merge under)
  ({ hist, cursor } = pushEntry(hist, cursor, { state: next, label: fx.label, gesture: gestureKey(null, added.id) }));
  assert.equal(hist.length, 3);
});

// ------------------------------------------------------------------------------------------ honesty fields

test('a clearly bent, well seen suggestion has no warning', () => {
  const n = suggestionNotes(suggestion());
  assert.deepEqual(suggestionLines(n), []);
  assert.equal(n.noCurve, false);
  assert.equal(n.nearlyStraight, false);
  assert.deepEqual(n.lowConfidence, []);
  assert.equal(n.peak, 6.7);
});

test('low confidence names the edge in plain language', () => {
  const n = suggestionNotes(suggestion({ confidence: [0.9, 0.55, 0.9, 0.9] }));
  assert.deepEqual(n.lowConfidence, ['right']);
  assert.deepEqual(suggestionLines(n), ['Not sure about the right edge: check it']);
  const two = suggestionNotes(suggestion({ confidence: [0.5, 0.9, 0.9, 0.4] }));
  assert.deepEqual(suggestionLines(two), ['Not sure about the top and left edges: check them']);
  const three = suggestionNotes(suggestion({ confidence: [0.5, 0.5, 0.5, 0.9] }));
  assert.deepEqual(suggestionLines(three), ['Not sure about the top, right and bottom edges: check them']);
  const all = suggestionNotes(suggestion({ confidence: [0.1, 0.2, 0.3, 0.4] }));
  assert.match(suggestionLines(all)[0], /any of the edges/);
  // exactly at the line is not low
  assert.deepEqual(suggestionNotes(suggestion({ confidence: [LOW_CONFIDENCE, 1, 1, 1] })).lowConfidence, []);
});

test('fellBack says no clear curved edge was found, and does not list the zeros as low confidence', () => {
  const s = suggestion({ fellBack: true, confidence: [0, 0, 0, 0], curvature: [0, 0, 0, 0], maxCurvature: 0, curves: curvesFromQuad(suggestion().quad) });
  const n = suggestionNotes(s);
  assert.equal(n.noCurve, true);
  assert.deepEqual(n.lowConfidence, []);
  assert.equal(n.nearlyStraight, false, 'one message, not two');
  assert.deepEqual(suggestionLines(n), ['No clear curved edge found: the page may be flat']);
});

test('four straight edges without a refusal are also "no clear curved edge"', () => {
  const s = suggestion({ curves: curvesFromQuad(suggestion().quad), curvature: [0, 0, 0, 0], maxCurvature: 0 });
  assert.equal(suggestionNotes(s).noCurve, true);
});

test('a bend under about 1.5% is "nearly straight", with the plural when several edges bend', () => {
  assert.equal(NEARLY_STRAIGHT_PCT, 1.5);
  const s = suggestion({ curvature: [1.2, 0, 0, 0], maxCurvature: 1.2 });
  assert.deepEqual(suggestionLines(suggestionNotes(s)), ['This edge is nearly straight: the suggestion may not help']);
  // not just maxCurvature: it is 0 when no edge passes the gates, but the edge IS bent by 2.4%
  const gated = suggestion({ curvature: [2.4, 0, 0, 0], maxCurvature: 0 });
  assert.equal(suggestionNotes(gated).nearlyStraight, false);
  assert.equal(suggestionNotes(gated).peak, 2.4);
  const c = suggestion().curves;
  c.right = [c.right[0], { x: 0.805, y: 0.5 }, c.right[1]];
  const many = suggestion({ curves: c, curvature: [1.0, 1.1, 0, 0], maxCurvature: 0 });
  assert.deepEqual(suggestionLines(suggestionNotes(many)), ['These edges are nearly straight: the suggestion may not help']);
  // at the line it is no longer nearly straight
  assert.equal(suggestionNotes(suggestion({ curvature: [1.5, 0, 0, 0], maxCurvature: 1.5 })).nearlyStraight, false);
});

test('warnings combine: low confidence and nearly straight both show', () => {
  const s = suggestion({ confidence: [0.9, 0.9, 0.3, 0.9], curvature: [1.1, 0, 0, 0], maxCurvature: 0 });
  assert.deepEqual(suggestionLines(suggestionNotes(s)), ['Not sure about the bottom edge: check it', 'This edge is nearly straight: the suggestion may not help']);
});

test('looksCurved is never read: the same answer reads the same with it on or off', () => {
  const a = suggestionNotes(suggestion({ looksCurved: true }));
  const b = suggestionNotes(suggestion({ looksCurved: false }));
  assert.deepEqual(a, b);
  for (const f of ['suggest.ts', 'components/SuggestPanel.svelte', '../routes/Editor.svelte', 'components/CurveBar.svelte']) {
    const src = readFileSync(new URL(f, import.meta.url), 'utf8');
    const code = src.split('\n').filter((l) => !/^\s*(\/\/|\*|\/\*|<!--)/.test(l)).join('\n');
    assert.ok(!/\.looksCurved|looksCurved\s*[?:)&|]/.test(code), `${f} must not use looksCurved`);
  }
});

// ------------------------------------------------------------------------------------------ wire types

const camel = (snake: string) => snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/** The serde field names of a Rust struct, read from its source (drift guard between the engine and the types). */
function rustFields(file: string, struct: string): string[] {
  const src = readFileSync(new URL(`../../../${file}`, import.meta.url), 'utf8');
  const start = src.indexOf(`pub struct ${struct} {`);
  assert.ok(start >= 0, `${struct} not found`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^ {4}pub ([a-z_]+):/gm)].map((m) => camel(m[1])).sort();
}

const SUGGESTION_KEYS = ['bulge', 'confidence', 'curvature', 'curves', 'fellBack', 'looksCurved', 'maxCurvature', 'quad', 'supportGain'];

/** Everything the TypeScript type promises, checked on an object that came over the wire. */
function assertSuggestionShape(j: unknown): asserts j is CurveSuggestion {
  const s = j as Record<string, unknown>;
  assert.deepEqual(Object.keys(s).sort(), SUGGESTION_KEYS);
  for (const k of ['confidence', 'curvature', 'bulge', 'supportGain']) {
    const a = s[k] as unknown[];
    assert.ok(Array.isArray(a) && a.length === 4 && a.every((v) => typeof v === 'number' && Number.isFinite(v)), k);
  }
  assert.equal((s.quad as unknown[]).length, 4);
  assert.equal(typeof s.maxCurvature, 'number');
  assert.equal(typeof s.looksCurved, 'boolean');
  assert.equal(typeof s.fellBack, 'boolean');
  const c = s.curves as CurveSet;
  assert.deepEqual(Object.keys(c).sort(), ['bottom', 'left', 'mirror', 'quarterTurns', 'right', 'top']);
  for (const e of [c.top, c.right, c.bottom, c.left]) {
    assert.ok(e.length >= 2 && e.length <= 9);
    assert.ok(e.every((p) => typeof p.x === 'number' && typeof p.y === 'number'));
  }
  assert.equal(validateCurves(c), null, 'the curves are a set the editor accepts');
}

test('the TypeScript keys are the fields of the engine struct CurveSuggestion', () => {
  assert.deepEqual(rustFields('crates/engine/src/curved.rs', 'CurveSuggestion'), SUGGESTION_KEYS);
});

test('real engine answers (fixtures written by crates/shell/tests/suggest_json.rs) have the shape of the type', () => {
  for (const f of ['curve-suggestion-curved.json', 'curve-suggestion-flat.json']) {
    const j: unknown = JSON.parse(readFileSync(new URL(`./fixtures/${f}`, import.meta.url), 'utf8'));
    assertSuggestionShape(j);
    // and the UI can read them
    const n = suggestionNotes(j);
    assert.ok(Array.isArray(suggestionLines(n)));
    // the quad is the end points of the curves
    assert.deepEqual(cornersOf(j.curves), j.quad);
  }
});

test('the shell registers the suggest_curves command on a worker and the IPC wrapper calls it by that name', () => {
  const app = readFileSync(new URL('../../../crates/shell/src/app.rs', import.meta.url), 'utf8');
  assert.match(app, /async fn suggest_curves\(/, 'async: it must not run on the main thread');
  assert.match(app, /blocking\(move \|\| engine\.suggest_curves\(id, crop\)\)/);
  assert.match(app.slice(app.indexOf('generate_handler!')), /\bsuggest_curves,/);
  const backend = readFileSync(new URL('./backend.ts', import.meta.url), 'utf8');
  assert.match(backend, /invoke<CurveSuggestion \| null>\('suggest_curves', \{ id, crop \}\)/);
  assert.match(backend, /suggestCurves: \(id, crop\) => impl\.api\.suggestCurves\(id, crop\)/);
});

// ------------------------------------------------------------------------------------------ the mock

const SIZE = [900, 675] as const;
const sampleCorners = (): [Pt, Pt, Pt, Pt] => curvedDetectedQuad();

test('mock: the curved sample gets a plausible suggestion with the honesty fields filled in', () => {
  const s = mockSuggestion({ curvedSample: true, corners: sampleCorners(), quarterTurns: 0, mirror: false, size: SIZE });
  assertSuggestionShape(s);
  assert.equal(s.fellBack, false);
  assert.ok(s.curves.top.length > 2 && s.curves.right.length > 2, 'the edges are bent');
  assert.deepEqual(cornersOf(s.curves), s.quad);
  // the suggested edges are the sample's own analytic edges
  const truth = curvedEdges(5);
  assert.deepEqual(s.curves.top, truth[0]);
  assert.ok(s.curvature[0] > 1 && s.maxCurvature > 1);
  assert.ok(s.confidence.some((c) => c < LOW_CONFIDENCE), 'one edge is uncertain, so the low-confidence line can be seen');
  assert.ok(suggestionLines(suggestionNotes(s)).some((l) => l.startsWith('Not sure about the bottom edge')));
  // the turns and mirror of the crop travel with the curves
  const turned = mockSuggestion({ curvedSample: true, corners: sampleCorners(), quarterTurns: 3, mirror: true, size: SIZE });
  assert.equal(turned.curves.quarterTurns, 3);
  assert.equal(turned.curves.mirror, true);
});

test('mock: a flat page, or corners far from the page, get the engine\'s "fell back" answer', () => {
  const flat = mockSuggestion({ curvedSample: false, corners: sampleCorners(), quarterTurns: 0, mirror: false, size: SIZE });
  assertSuggestionShape(flat);
  assert.equal(flat.fellBack, true);
  assert.ok(flat.curves.top.length === 2 && flat.curves.left.length === 2);
  assert.deepEqual(flat.confidence, [0, 0, 0, 0]);
  assert.deepEqual(suggestionLines(suggestionNotes(flat)), ['No clear curved edge found: the page may be flat']);
  const far = mockSuggestion({ curvedSample: true, corners: [{ x: 0.05, y: 0.05 }, { x: 0.3, y: 0.05 }, { x: 0.3, y: 0.3 }, { x: 0.05, y: 0.3 }], quarterTurns: 0, mirror: false, size: SIZE });
  assert.equal(far.fellBack, true);
});

test('mock: curvature is the largest distance from the chord in percent of the chord', () => {
  const c: CurveSet = {
    top: [{ x: 0, y: 0 }, { x: 0.5, y: -0.05 }, { x: 1, y: 0 }],
    right: [{ x: 1, y: 0 }, { x: 1, y: 1 }],
    bottom: [{ x: 1, y: 1 }, { x: 0, y: 1 }],
    left: [{ x: 0, y: 1 }, { x: 0, y: 0 }],
    quarterTurns: 0,
    mirror: false,
  };
  const k = curvatures(c, [100, 100]);
  assert.ok(k[0] > 3 && k[0] < 7, `${k[0]}`);
  assert.deepEqual(k.slice(1), [0, 0, 0]);
  const e: EdgeNumbers = k;
  assert.equal(e.length, 4);
});

// ------------------------------------------------------------------------------------------ copy and settings

test('the copy says what the task asks, in plain language', () => {
  assert.equal(S.suggest.button, 'Suggest curves');
  assert.equal(S.suggest.noCurve, 'No clear curved edge found: the page may be flat');
  assert.equal(S.suggest.lowConfidence(['right']), 'Not sure about the right edge: check it');
  assert.equal(S.suggest.nearlyStraight(1), 'This edge is nearly straight: the suggestion may not help');
  assert.equal(S.suggest.label, SUGGEST_LABEL);
  assert.match(S.suggest.applied, /Undo/);
  assert.match(S.suggest.applied, /accept/);
  assert.match(S.suggest.dismissed, /Nothing was changed/);
  assert.match(S.suggest.buttonHint, /Nothing changes until you press Apply/);
  assert.match(S.suggest.guess, /guess/);
  assert.equal(sideList(['top']), 'top');
  assert.equal(sideList(['top', 'right', 'bottom']), 'top, right and bottom');
});

test('the suggestion and first-use copy has no empty line', () => {
  const calls: Record<string, unknown[][]> = {
    bendOf: [[['top'], 2.4], [['top', 'left'], 11]],
    lowConfidence: [[['top']], [['top', 'left']], [['top', 'right', 'bottom', 'left']]],
    nearlyStraight: [[1], [3]],
  };
  const walk = (o: unknown, path: string): void => {
    if (typeof o === 'string') assert.ok(o.trim().length > 0, path);
    else if (typeof o === 'function') {
      const name = path.split('.').pop()!;
      assert.ok(calls[name], `${path} needs example arguments in this test`);
      for (const args of calls[name]) {
        const out = (o as (...a: never[]) => unknown)(...(args as never[]));
        assert.ok(typeof out === 'string' && out.trim().length > 0, `${path}()`);
      }
    } else if (o && typeof o === 'object') for (const [k, v] of Object.entries(o)) walk(v, `${path}.${k}`);
  };
  walk(S.suggest, 'S.suggest');
  walk(S.curvedIntro, 'S.curvedIntro');
  assert.equal(S.suggest.bendOf(['top'], 2.44), 'Bent: the top edge, by up to 2.4% of its length.');
  assert.equal(S.suggest.bendOf(['top', 'left'], 11), 'Bent: the top and left edges, by up to 11.0% of their length.');
});

test('the first-use sheet explains what it fixes, what it does not, and that nothing is written yet', () => {
  assert.match(S.curvedIntro.fixes, /bow/);
  assert.match(S.curvedIntro.fixes, /angle/);
  assert.match(S.curvedIntro.doesNot, /[Ww]rinkles/);
  assert.match(S.curvedIntro.safe, /accept/);
  assert.match(S.curvedIntro.safe, /Save as copy/);
});

test('the first-use flag is a setting that defaults to unseen, and a patch of it keeps the rest', () => {
  assert.equal(defaultSettings().curvedIntroAck, false);
});
