// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import {
  APPLY_LABEL,
  bannerVisible,
  batchLine,
  ctxOf,
  customisedCount,
  dismissKey,
  enhanceCounts,
  enhanceQuery,
  initialSuggest,
  isEnhanced,
  isPreviewing,
  looksLikeReceipt,
  modeOf,
  pageKey,
  readySuggestion,
  sameLook,
  savesGrey,
  strengthFor,
  strengthOf,
  suggestArrived,
  suggestBegin,
  suggestContextChanged,
  suggestFailed,
  suggestPreview,
  suggestReset,
  suggestedLook,
  tileBadge,
  wantsPngHint,
  wantsSuggestion,
  writesGrey,
} from './enhance.ts';
import { effectiveEnhance, emptyState, addCrop, cropKey, cropViews, renderSignature, setCropEnhance, setScanEnhance } from './scan-model.ts';
import { defaultSettings } from './settings-defaults.ts';
import { NOTICES, S, noticeText } from './strings.ts';
import { crop, item, sampleEdit } from './testkit.ts';
import type { Enhance, EnhanceSuggestion, ItemView } from './types.ts';

const clean = (strength?: number): Enhance => (strength === undefined ? { mode: 'clean' } : { mode: 'clean', strength });
const doc = (strength = 100): Enhance => ({ mode: 'document', strength });

function fixture<T>(name: string): T {
  return JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8')) as T;
}

const suggestion = (): EnhanceSuggestion => fixture<EnhanceSuggestion>('enhance-suggestion.json');

function scanOf(looks: (Enhance | null)[], name = 'receipts.jpg'): ItemView {
  const crops = looks.map((o, i) => crop(i + 1, i + 1, 'good', { enhance: o ?? { mode: 'original' }, enhanceOverride: o }));
  return item(1, { name, crops, enhance: { mode: 'original' } });
}

// ------------------------------------------------------------------------------------------ the pure rules

test('strength: Original has none, an absent strength is 100, and the value is clamped to whole percent', () => {
  assert.equal(strengthOf({ mode: 'original' }), 0);
  assert.equal(strengthOf(clean()), 100);
  assert.equal(strengthOf(clean(63.4)), 63);
  assert.equal(strengthOf(clean(250)), 100);
  assert.equal(strengthOf(clean(-5)), 0);
  assert.equal(strengthOf(clean(Number.NaN)), 100);
  assert.equal(strengthOf(null), 0);
  assert.equal(modeOf(undefined), 'original');
});

test('a look at strength 0 is not an enhancement, like the engine (it keys as Original)', () => {
  assert.equal(isEnhanced({ mode: 'original' }), false);
  assert.equal(isEnhanced(clean(0)), false);
  assert.equal(isEnhanced(clean(1)), true);
  assert.equal(isEnhanced(clean()), true);
  assert.ok(sameLook(clean(0), { mode: 'original' }));
  assert.ok(!sameLook(clean(50), clean(60)));
  assert.ok(!sameLook(clean(), doc()));
  assert.ok(sameLook(clean(), clean(100)));
});

test('the strength a look starts with: it keeps the one it had, from Original it starts at full', () => {
  assert.equal(strengthFor({ mode: 'original' }, 'clean'), 100);
  assert.equal(strengthFor(clean(40), 'document'), 40);
  assert.equal(strengthFor(clean(40), 'original'), 0);
  assert.equal(strengthFor(clean(0), 'receipt'), 100, 'a look at 0% is no look: the next one starts at full');
});

test('the preview query is exactly what the shell parses: mode and s, digits only, 0 to 100', () => {
  assert.equal(enhanceQuery('clean', 80), 'mode=clean&s=80');
  assert.equal(enhanceQuery('original', 55), 'mode=original&s=0');
  assert.equal(enhanceQuery('receipt', 180), 'mode=receipt&s=100');
  assert.equal(enhanceQuery('document', -3), 'mode=document&s=0');
  assert.equal(enhanceQuery('clean', 33.6), 'mode=clean&s=34');
  assert.equal(enhanceQuery('clean', Number.NaN), 'mode=clean&s=0');
  for (const mode of ['original', 'clean', 'document', 'receipt'] as const) {
    for (const s of [0, 1, 50, 100, 250, -1, 12.5]) assert.match(enhanceQuery(mode, s), /^mode=(original|clean|document|receipt)&s=\d{1,3}$/);
  }
  // the Rust parser's rules, read from its source
  const lib = readFileSync(new URL('../../../crates/shell/src/lib.rs', import.meta.url), 'utf8');
  for (const m of ['original', 'clean', 'document', 'receipt']) assert.ok(lib.includes(`"${m}" => EnhanceMode::`), m);
  assert.ok(lib.includes('"mode" => &mut mode') && lib.includes('"s" => &mut strength'));
});

test('black-and-white looks write one grey channel; Original and Clean do not', () => {
  assert.equal(writesGrey('document'), true);
  assert.equal(writesGrey('receipt'), true);
  assert.equal(writesGrey('clean'), false);
  assert.equal(writesGrey('original'), false);
});

// ------------------------------------------------------------------------------------------ counts and badges

test('"Enhancement is on for 2 of 3 items" counts the included crops whose look changes the pixels', () => {
  const it = scanOf([clean(), null, doc(60)]);
  assert.deepEqual(enhanceCounts(it), { on: 2, total: 3 });
  assert.equal(S.enhance.onCount(2, 3), 'Enhancement is on for 2 of 3 items');
  assert.equal(S.enhance.onCount(3, 3), 'Enhancement is on for 3 of 3 items');
  assert.equal(S.enhance.onCount(0, 1), 'Enhancement is off');
  assert.equal(S.enhance.onCount(1, 1), 'Enhancement is on');
  // a removed crop and a look at 0% do not count
  const odd = scanOf([clean(0), clean()]);
  odd.crops.push(crop(9, 0, 'check', { include: false, enhance: clean() }));
  assert.deepEqual(enhanceCounts(odd), { on: 1, total: 2 });
});

test('the tile badge: Enhanced for a whole image, "Enhanced 2 of 3" for some of its items, nothing otherwise', () => {
  assert.equal(tileBadge(scanOf([null, null])), '');
  assert.equal(tileBadge(scanOf([clean()])), 'Enhanced');
  assert.equal(tileBadge(scanOf([clean(), clean()])), 'Enhanced');
  assert.equal(tileBadge(scanOf([clean(), null, doc()])), 'Enhanced 2 of 3');
  assert.equal(tileBadge(scanOf([clean(0)])), '');
});

test('"customised" counts the crops that have a look of their own', () => {
  assert.equal(customisedCount(scanOf([null, clean()])), 1);
  assert.equal(customisedCount(scanOf([null, null])), 0);
});

// ------------------------------------------------------------------------------------------ what a look writes

test('the PNG hint is for a black-and-white look on a JPEG source, nothing else', () => {
  assert.equal(wantsPngHint(scanOf([doc()], 'a.jpg')), true);
  assert.equal(wantsPngHint(scanOf([{ mode: 'receipt', strength: 40 }], 'A.JPEG')), true);
  assert.equal(wantsPngHint(scanOf([doc()], 'a.png')), false, 'a PNG source is saved as a PNG already');
  assert.equal(wantsPngHint(scanOf([clean()], 'a.jpg')), false, 'Clean keeps colour and the source format');
  assert.equal(wantsPngHint(scanOf([null], 'a.jpg')), false);
  assert.equal(wantsPngHint(scanOf([doc(0)], 'a.jpg')), false);
  assert.equal(savesGrey(scanOf([clean(), doc()], 'a.png')), true);
  assert.equal(savesGrey(scanOf([clean()], 'a.png')), false);
});

test('the notices the engine sends after a save have plain-language copy', () => {
  const out = readFileSync(new URL('../../../crates/engine/src/output.rs', import.meta.url), 'utf8');
  const codes = [...out.matchAll(/pub const NOTICE_[A-Z_]+: &str = "(enhance\.[a-z_]+)";/g)].map((m) => m[1]).sort();
  assert.deepEqual(codes, ['enhance.icc_dropped', 'enhance.png_recommended']);
  for (const c of codes) {
    assert.ok(NOTICES[c].length > 40, c);
    assert.equal(noticeText(c), NOTICES[c]);
  }
  assert.match(NOTICES['enhance.icc_dropped'], /colour profile/);
  assert.match(NOTICES['enhance.png_recommended'], /PNG/);
});

// ------------------------------------------------------------------------------------------ the suggestion

const page = crop(1, 1, 'good');
const ctx = ctxOf(1, page);

test('the page key follows the geometry, not the look', () => {
  const a = pageKey(crop(1, 1));
  assert.equal(pageKey(crop(1, 1, 'good', { enhance: clean(), enhanceOverride: clean(), renderKey: 'other' })), a);
  const moved = crop(1, 1);
  moved.edit = { ...sampleEdit(), fineDeg: 1.5 };
  assert.notEqual(pageKey(moved), a);
  assert.notEqual(pageKey(crop(1, 1, 'good', { mirror: true })), a);
});

test('a suggestion is asked for only when the setting is on, the image is ready and the crop has no look yet', () => {
  const on = { setting: true, ready: true, crop: page };
  assert.equal(wantsSuggestion(on), true);
  assert.equal(wantsSuggestion({ ...on, setting: false }), false, 'the setting only decides whether it is shown');
  assert.equal(wantsSuggestion({ ...on, ready: false }), false);
  assert.equal(wantsSuggestion({ ...on, crop: null }), false);
  assert.equal(wantsSuggestion({ ...on, crop: crop(1, 1, 'good', { enhance: clean() }) }), false, 'already enhanced');
  assert.equal(wantsSuggestion({ ...on, crop: crop(1, 0, 'check', { include: false }) }), false, 'a removed crop');
  assert.equal(wantsSuggestion({ ...on, crop: crop(1, 1, 'good', { enhance: clean(0) }) }), true, 'a look at 0% is no look');
});

test('asking, answering and the banner: asking records nothing and the answer is taken once', () => {
  let st = initialSuggest();
  const begun = suggestBegin(st, ctx);
  assert.equal(begun.ask, true);
  st = begun.state;
  assert.equal(st.phase.kind, 'loading');
  assert.equal(suggestBegin(st, ctx).ask, false, 'a repeat while loading changes nothing');
  st = suggestArrived(st, st.seq, ctx, suggestion());
  assert.equal(st.phase.kind, 'ready');
  assert.equal(readySuggestion(st)?.mode, 'clean');
  assert.equal(suggestBegin(st, ctx).ask, false, 'an answer already held is not asked for again');
  assert.equal(bannerVisible(st, new Set(), ctx), true);
  assert.equal(bannerVisible(st, new Set([dismissKey(1, 1)]), ctx), false, 'Not now hides it for this crop');
  assert.equal(bannerVisible(st, new Set([dismissKey(1, 2)]), ctx), true, 'and only for this crop');
  assert.equal(bannerVisible(st, new Set(), ctxOf(1, crop(2, 2))), false, 'never for another crop');
});

test('"null" from the engine means leave it alone: no banner, and no second ask for the same page', () => {
  let st = suggestBegin(initialSuggest(), ctx).state;
  st = suggestArrived(st, st.seq, ctx, null);
  assert.equal(st.phase.kind, 'none');
  assert.equal(readySuggestion(st), null);
  assert.equal(bannerVisible(st, new Set(), ctx), false);
  assert.equal(suggestBegin(st, ctx).ask, false);
});

test('an answer for a replaced request, or for a page that changed meanwhile, is ignored', () => {
  let st = suggestBegin(initialSuggest(), ctx).state;
  const first = st.seq;
  st = suggestContextChanged(st, null);
  assert.equal(st.phase.kind, 'idle');
  assert.equal(suggestArrived(st, first, ctx, suggestion()).phase.kind, 'idle', 'the request was dropped');
  st = suggestBegin(st, ctx).state;
  const other = ctxOf(1, crop(2, 2));
  assert.equal(suggestArrived(st, st.seq, other, suggestion()).phase.kind, 'loading', 'another crop is on screen now');
  assert.equal(suggestFailed(st, st.seq, ctx).phase.kind, 'error');
});

test('a ready suggestion is dropped when the crop or its page changes, and kept while they do not', () => {
  let st = suggestBegin(initialSuggest(), ctx).state;
  st = suggestArrived(st, st.seq, ctx, suggestion());
  assert.equal(suggestContextChanged(st, ctx), st, 'same page: nothing changes');
  const moved = crop(1, 1);
  moved.edit = { ...sampleEdit(), fineDeg: 2 };
  assert.equal(suggestContextChanged(st, ctxOf(1, moved)).phase.kind, 'idle');
  assert.equal(suggestContextChanged(st, null).phase.kind, 'idle', 'the crop got a look or was left');
});

test('Preview shows the result without applying anything; Apply and Not now are the only exits', () => {
  let st = suggestBegin(initialSuggest(), ctx).state;
  st = suggestArrived(st, st.seq, ctx, suggestion());
  assert.equal(isPreviewing(st), false);
  const on = suggestPreview(st, true);
  assert.equal(isPreviewing(on), true);
  assert.equal(on.phase.kind, 'ready', 'still only a suggestion');
  assert.equal(suggestPreview(on, true), on);
  assert.equal(isPreviewing(suggestPreview(on, false)), false);
  assert.equal(isPreviewing(suggestPreview(initialSuggest(), true)), false, 'nothing to preview');
  assert.equal(suggestReset(on).phase.kind, 'idle');
  // the look it would apply: Clean at the suggested strength, as a mode and a percent
  assert.deepEqual(suggestedLook(suggestion()), { mode: 'clean', strength: 100 });
  assert.equal(APPLY_LABEL, 'Enhance: Clean (suggested)');
});

test('the engine never suggests a black-and-white look as the main one; it names the one that fits as an option', () => {
  const s = suggestion();
  assert.equal(s.mode, 'clean');
  assert.equal(s.hint.gray, false);
  assert.equal(s.blackAndWhite, 'document');
  assert.equal(looksLikeReceipt(s), false);
  assert.equal(looksLikeReceipt({ ...s, blackAndWhite: 'receipt' }), true);
  assert.equal(s.blackAndWhiteHint.recommendPng, true);
});

// ------------------------------------------------------------------------------------------ the wire

test('the TypeScript types are the fields of the engine structs (read from crates/engine)', () => {
  const src = readFileSync(new URL('../../../crates/engine/src/enhance.rs', import.meta.url), 'utf8');
  const camel = (s: string) => s.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
  const fields = (name: string): string[] => {
    const start = src.indexOf(`pub struct ${name} {`);
    assert.ok(start >= 0, name);
    const body = src.slice(start, src.indexOf('\n}', start));
    return [...body.matchAll(/^ {4}pub ([a-z_]+):/gm)].map((m) => camel(m[1])).sort();
  };
  const s = suggestion();
  assert.deepEqual(Object.keys(s).sort(), fields('EnhanceSuggestion'));
  assert.deepEqual(Object.keys(s.measures).sort(), fields('EnhanceMeasures'));
  assert.deepEqual(Object.keys(s.hint).sort(), fields('OutputHint'));
  const batch = fixture<Record<string, unknown>>('enhance-batch.json');
  assert.deepEqual(Object.keys(batch).sort(), fields('EnhanceBatch'));
});

test('real engine answers: the suggestion and the views carry what the UI reads', () => {
  const s = suggestion();
  assert.equal(s.mode, 'clean');
  assert.ok(['uneven_light', 'dull_paper', 'colour_cast', 'faded_print'].includes(s.reason));
  for (const r of s.reasons) assert.ok(S.enhance.reasons[r], `copy for ${r}`);
  const v = fixture<Record<string, { itemEnhance: Enhance; cropEnhance: Enhance; cropEnhanceOverride: Enhance | null }>>('enhance-views.json');
  assert.equal(modeOf(v.original.itemEnhance), 'original');
  assert.equal(v.original.cropEnhanceOverride, null);
  assert.equal(strengthOf(v.set.cropEnhance), 70);
  assert.ok(sameLook(v.set.cropEnhance, v.set.cropEnhanceOverride));
  assert.equal(modeOf(v.scan.itemEnhance), 'receipt');
  assert.equal(v.scan.cropEnhanceOverride, null, 'the scan scope leaves no override');
  const b = fixture<{ views: { id: number; enhance: Enhance }[]; crops: number; images: number }>('enhance-batch.json');
  assert.equal(b.crops, 1);
  assert.equal(modeOf(b.views[0].enhance), 'clean');
});

test('every reason code the engine can send has copy (read from crates/imgproc)', () => {
  const src = readFileSync(new URL('../../../crates/imgproc/src/enhance/measures.rs', import.meta.url), 'utf8');
  const codes = [...src.slice(src.indexOf('pub fn code(self)')).matchAll(/=> "([a-z_]+)"/g)].map((m) => m[1]).slice(0, 4).sort();
  assert.deepEqual(codes, ['colour_cast', 'dull_paper', 'faded_print', 'uneven_light']);
  for (const c of codes) assert.ok(S.enhance.reasons[c].length > 10, c);
});

test('the shell registers the enhance commands, the suggestion on a worker, and the IPC wrappers call them by name', () => {
  const app = readFileSync(new URL('../../../crates/shell/src/app.rs', import.meta.url), 'utf8');
  const handler = app.slice(app.indexOf('generate_handler!'));
  for (const c of ['set_enhance', 'clear_enhance', 'set_default_enhance', 'set_scan_enhance', 'suggest_enhance', 'enhance_suggested']) {
    assert.match(handler, new RegExp(`\\b${c},`), c);
  }
  assert.match(app, /async fn suggest_enhance\(/, 'async: it must not run on the main thread');
  assert.match(app, /async fn enhance_suggested\(/);
  assert.match(app, /blocking\(move \|\| engine\.suggest_enhance\(id, crop\)\)/);
  assert.match(app, /blocking\(move \|\| engine\.enhance_suggested\(&ids\)\)/);
  const backend = readFileSync(new URL('./backend.ts', import.meta.url), 'utf8');
  // the shell takes a fraction, the UI speaks percent
  assert.match(backend, /invoke<ItemView>\('set_enhance', \{ id, crop, mode, strength: strength \/ 100, phase, label, gesture \}\)/);
  assert.match(backend, /invoke<ItemView>\('set_scan_enhance', \{ id, mode, strength: strength \/ 100, phase, label, gesture \}\)/);
  assert.match(backend, /invoke<ItemView>\('clear_enhance', \{ id, crop \}\)/);
  assert.match(backend, /invoke<EnhanceSuggestion \| null>\('suggest_enhance', \{ id, crop \}\)/);
  assert.match(backend, /invoke<EnhanceBatch>\('enhance_suggested', \{ ids \}\)/);
  assert.match(backend, /\/crop\/\$\{crop\}\/enhance-\$\{kind\}\?\$\{enhanceQuery\(mode, strength\)\}/);
  const lib = readFileSync(new URL('../../../crates/shell/src/lib.rs', import.meta.url), 'utf8');
  assert.match(lib, /\["crop", crop, "enhance-thumb"\]/);
  assert.match(lib, /\["crop", crop, "enhance-result"\]/);
});

// ------------------------------------------------------------------------------------------ never applied by itself (B13)

test('nothing records a look by itself: the writes sit behind a click, and a suggestion is only data', () => {
  const editor = readFileSync(new URL('../routes/Editor.svelte', import.meta.url), 'utf8');
  const writes = [...editor.matchAll(/api\.(setEnhance|setScanEnhance|clearEnhance|enhanceSuggested)\(/g)];
  // Editor: commitLook (three calls: the panel's choices), followScan, and Apply on the banner.
  assert.equal(writes.length, 5, 'a new write has to be added here on purpose');
  const fn = (name: string) => editor.slice(editor.indexOf(`function ${name}(`), editor.indexOf('\n  }\n', editor.indexOf(`function ${name}(`)));
  for (const m of writes) {
    const at = m.index ?? 0;
    const inside = ['commitLook', 'applyEnhanceSuggestion', 'followScan'].some((n) => {
      const start = editor.indexOf(`function ${n}(`);
      return at > start && at < start + fn(n).length;
    });
    assert.ok(inside, `api.${m[1]} at ${at} is outside the click handlers`);
  }
  // the answer of the engine is stored, never sent back
  const asked = fn('askEnhance');
  assert.ok(!/api\.set|enqueue|commitLook/.test(asked), 'asking only stores the answer');
  // the batch is a store method that only the grid's buttons call
  const grid = readFileSync(new URL('../routes/Grid.svelte', import.meta.url), 'utf8');
  assert.equal([...grid.matchAll(/store\.enhanceSuggested\(/g)].length, 2);
  assert.match(grid, /onclick=\{\(\) => void store\.enhanceSuggested\(readyIds\)\}/);
  assert.match(grid, /function doEnhanceSelected\(\)/);
  const store = readFileSync(new URL('./store.svelte.ts', import.meta.url), 'utf8');
  assert.equal([...store.matchAll(/enhanceSuggested\(/g)].length, 2, 'declared once, calls the api once');
  for (const f of ['routes/Home.svelte', 'routes/Convert.svelte', 'routes/Backups.svelte', 'lib/store.svelte.ts']) {
    const src = readFileSync(new URL(`../${f}`, import.meta.url), 'utf8');
    assert.ok(!/api\.(setEnhance|setScanEnhance)\(/.test(src), f);
  }
});

test('the suggestion setting is on by default and only decides whether a suggestion is shown', () => {
  assert.equal(defaultSettings().suggestEnhance, true);
  const rust = readFileSync(new URL('../../../crates/engine/src/settings.rs', import.meta.url), 'utf8');
  assert.match(rust, /suggest_enhance: true,/);
  const settings = readFileSync(new URL('../routes/Settings.svelte', import.meta.url), 'utf8');
  assert.match(settings, /updateSettings\(\{ suggestEnhance: next \}\)/);
  assert.equal(S.settings.suggestEnhance, 'Suggest enhancement for documents and receipts');
  // the editor reads it before asking, and nothing else
  const editor = readFileSync(new URL('../routes/Editor.svelte', import.meta.url), 'utf8');
  assert.match(editor, /setting: store\.settings\.suggestEnhance/);
});

// ------------------------------------------------------------------------------------------ the copy

test('the copy says what the task asks, in plain language, with honest limits', () => {
  assert.equal(S.enhance.bannerDoc, 'This looks like a document: Clean makes it easier to read');
  assert.equal(S.enhance.bannerReceipt, 'This looks like a receipt: Clean makes it easier to read');
  assert.deepEqual([S.enhance.preview, S.enhance.apply, S.enhance.notNow], ['Preview', 'Apply', 'Not now']);
  assert.deepEqual([S.enhance.scopeThis, S.enhance.scopeAll], ['This item', 'All items in this scan']);
  assert.equal(S.enhance.gridAll, 'Enhance all suggested');
  assert.deepEqual(Object.keys(S.enhance.looks), ['original', 'clean', 'document', 'receipt']);
  for (const l of Object.values(S.enhance.looks)) assert.ok(l.line.length > 15 && l.line.endsWith('.'));
  // honest limits: measured only on made-up pages; black and white can lose faint marks; keep Original if unsure
  assert.match(S.enhance.helpClean, /made-up pages/);
  assert.match(S.enhance.helpClean, /not been measured on real photos/);
  assert.match(S.enhance.helpBw, /faint marks/);
  assert.match(S.enhance.helpBw, /keep Original/);
  assert.match(S.settings.notes.join(' '), /keep Original if you are not sure/);
  assert.ok(!S.settings.notes.join(' ').includes('not available yet'), 'the old "not available" note is gone');
  assert.match(S.enhance.pngHint, /PNG/);
  assert.match(S.enhance.greyNote, /colour profile/);
  assert.match(S.enhance.heldNote, /accept it again/);
});

test('no line of the enhancement copy is empty, and none uses a dash for a pause', () => {
  const walk = (o: unknown, path: string, out: [string, string][]): void => {
    if (typeof o === 'string') out.push([path, o]);
    else if (o && typeof o === 'object') for (const [k, v] of Object.entries(o)) walk(v, `${path}.${k}`, out);
    else if (typeof o === 'function') {
      const f = o as (...a: number[]) => string;
      const r = f(2, 3, 1);
      if (typeof r === 'string') out.push([path + '()', r]);
    }
  };
  const lines: [string, string][] = [];
  walk(S.enhance, 'enhance', lines);
  assert.ok(lines.length > 60);
  for (const [p, t] of lines) {
    assert.ok(t.trim().length > 0, p);
    assert.ok(!t.includes('—') && !t.includes('–') || p.includes('strength'), `${p}: ${t}`);
  }
});

test('the batch summary says how many pages in how many images, and what it left alone', () => {
  assert.equal(batchLine({ images: 2, crops: 3, already: 0, none: 1, failed: 0 }), 'Enhanced 3 pages in 2 images. 1 had nothing to suggest.');
  assert.equal(batchLine({ images: 1, crops: 1, already: 0, none: 0, failed: 0 }), 'Enhanced 1 page in 1 image.');
  assert.match(batchLine({ images: 0, crops: 0, already: 2, none: 0, failed: 0 }), /Nothing new to enhance. 2 pages already have a look.$/);
  assert.match(batchLine({ images: 0, crops: 0, already: 1, none: 5, failed: 0 }), /1 page already has a look, and the rest have nothing to suggest/);
  assert.match(batchLine({ images: 0, crops: 0, already: 0, none: 4, failed: 0 }), /Nothing to enhance/);
});

// ------------------------------------------------------------------------------------------ the mock's model

const dims: [number, number] = [1000, 1400];
const quad = sampleEdit().quad;
function twoCrops() {
  let s = emptyState();
  s = addCrop(s, quad, 'auto', dims, { score: 0.99, forced: null, reasons: [] }).state;
  s = addCrop(s, [{ x: 0.0, y: 0.0 }, { x: 0.3, y: 0.0 }, { x: 0.3, y: 0.3 }, { x: 0.0, y: 0.3 }], 'auto', dims, { score: 0.99, forced: null, reasons: [] }).state;
  return s;
}

test('model: a crop\'s own look beats the default, and its picture key and the signature follow the effective look', () => {
  const s = twoCrops();
  const [a, b] = s.crops;
  const base = cropKey(a, s.enhance);
  const own = setCropEnhance(s, a.id, clean(70));
  assert.equal(effectiveEnhance(own, own.crops[0]).mode, 'clean');
  assert.equal(effectiveEnhance(own, own.crops[1]).mode, 'original');
  assert.notEqual(cropKey(own.crops[0], own.enhance), base, 'the picture of that crop is a new one');
  assert.equal(cropKey(own.crops[1], own.enhance), cropKey(b, s.enhance), 'the other crop keeps its picture');
  assert.notEqual(renderSignature(own), renderSignature(s), 'an accepted held scan has to be accepted again');
  const views = cropViews(own, { stem: 'x', ext: 'jpg', baseline: s });
  assert.deepEqual(views[0].enhanceOverride, { mode: 'clean', strength: 70 });
  assert.deepEqual(views[0].enhance, { mode: 'clean', strength: 70 });
  assert.equal(views[1].enhanceOverride, null);
  assert.equal(views[0].edited, false, 'a look is not a geometry edit');
  assert.equal(views[0].band, 'good');
});

test('model: strength 0 keys like Original, the scan scope drops every override, and clearing follows the default', () => {
  const s = twoCrops();
  assert.equal(renderSignature(setCropEnhance(s, s.crops[0].id, clean(0))), renderSignature(setCropEnhance(s, s.crops[0].id, { mode: 'original' })));
  assert.deepEqual(setCropEnhance(s, s.crops[0].id, clean(0)).crops[0].enhanceOverride, { mode: 'original' });
  const own = setCropEnhance(s, s.crops[0].id, doc(40));
  const all = setScanEnhance(own, clean(), true);
  assert.deepEqual(all.enhance, { mode: 'clean', strength: 100 });
  assert.ok(all.crops.every((c) => c.enhanceOverride === null));
  const keep = setScanEnhance(own, clean(), false);
  assert.equal(keep.crops[0].enhanceOverride?.mode, 'document', 'the plain default call keeps overrides');
  const back = setCropEnhance(all, all.crops[0].id, null);
  assert.equal(back.crops[0].enhanceOverride, null);
  assert.throws(() => setCropEnhance(s, 999, clean()));
});
