// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { test } from 'node:test';
import { defaultSettings, patchedSettings } from './settings-defaults.ts';
import {
  ERRORS,
  FALLBACK_HOLD,
  HOLD,
  NOTICES,
  S,
  errorMessage,
  formatList,
  holdAction,
  holdCause,
  holdTitle,
  noticeText,
  openOnlyShort,
  retentionText,
} from './strings.ts';
import type { CurveProblem } from './curve.ts';
import type { ReasonCode } from './types.ts';

// Every code in the contract must have copy. The map is typed against `ReasonCode`, so adding a code to
// types.ts without adding it here fails `npm run check`.
const CONTRACT_CODES: Record<ReasonCode, true> = {
  NO_QUAD: true,
  WEAK_EDGE: true,
  PARTIAL_FRAME: true,
  ODD_ASPECT: true,
  LOW_CONTRAST_EDGE: true,
  IMPLAUSIBLE_QUAD: true,
  TOUCHING_ITEMS: true,
  OVERLAPPING_ITEMS: true,
  ITEMS_TOO_CLOSE: true,
  SPLIT_UNSTABLE: true,
  TOO_MANY_ITEMS: true,
  BED_UNCERTAIN: true,
  ANALYSIS_LIMIT: true,
  NO_DOCUMENT: true,
};

const screaming = (camel: string) => camel.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();
const camel = (snake: string) => snake.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

/** The variant names of a Rust enum, read from its source (drift guard between the engine and the copy). */
function rustVariants(file: string, enumName: string): string[] {
  const src = readFileSync(new URL(`../../../${file}`, import.meta.url), 'utf8');
  const start = src.indexOf(`pub enum ${enumName} {`);
  assert.ok(start >= 0, `${enumName} not found in ${file}`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*)(?:\s*[,{(]|$)/gm)].map((m) => m[1]);
}

test('every contract reason code has a title, cause and action', () => {
  for (const code of Object.keys(CONTRACT_CODES) as ReasonCode[]) {
    assert.ok(holdTitle({ code }).length > 0, code);
    assert.ok(holdCause({ code }).length > 10, code);
    assert.ok(holdAction({ code }).length > 3, code);
    assert.ok(HOLD[code], `${code} has its own copy, not the fallback`);
  }
});

test('every reason code the engine can send has its own copy (read from crates/core)', () => {
  for (const v of rustVariants('crates/core/src/confidence.rs', 'ReasonCode')) {
    const code = screaming(v);
    assert.ok(HOLD[code], `no hold copy for ${code}`);
    assert.ok((CONTRACT_CODES as Record<string, true>)[code], `${code} is missing from the TypeScript ReasonCode`);
  }
});

test('every error code the engine can send has its own message (read from crates/core)', () => {
  const codes = rustVariants('crates/core/src/error.rs', 'ErrKind').map(screaming);
  assert.ok(codes.length >= 30, `found only ${codes.length} codes`);
  for (const code of codes) {
    assert.ok(code in ERRORS, `no message for ${code}`);
    if (code !== 'INTERNAL') assert.notEqual(ERRORS[code as keyof typeof ERRORS], ERRORS.INTERNAL, `${code} reuses the generic message`);
  }
});

test('the multi-item error codes read as plain language that says what to do', () => {
  assert.match(errorMessage('HELD_FOR_REVIEW'), /held for review/i);
  assert.match(errorMessage('HELD_FOR_REVIEW'), /Accept split/);
  assert.match(errorMessage('PLAN_STALE'), /nothing was written/i);
  assert.match(errorMessage('GROUP_COMMIT_FAILED'), /scan is untouched/i);
  assert.match(errorMessage('SAVED_SOURCE_IN_USE'), /set is complete/i);
  assert.match(errorMessage('ITEM_OP'), /nothing was changed/i);
  assert.match(errorMessage('NOT_REPLACEABLE'), /Save as copy/);
});

test('notices: every code the engine sends has a line, and an unknown code never shows raw', () => {
  for (const code of ['tiff.multi_page', 'format.write_unavailable', 'split.held', 'derived.user_edited']) {
    assert.ok(NOTICES[code].length > 20, code);
    assert.equal(noticeText(code), NOTICES[code]);
  }
  assert.match(NOTICES['tiff.multi_page'], /never replaced/);
  assert.match(NOTICES['format.write_unavailable'], /Save as copy/);
  assert.ok(!noticeText('some.future.code').includes('some.future'));
  assert.equal(openOnlyShort('tiff.multi_page'), 'Multi-page: copy only');
  assert.equal(openOnlyShort(null), '');
});

test('the formats line names what the shell reports, in a fixed order', () => {
  assert.equal(formatList(['jpg', 'jpeg', 'png']), 'JPG and PNG');
  assert.equal(formatList(['jpg', 'jpeg', 'png', 'tif', 'tiff', 'webp']), 'JPG, PNG, WebP and TIFF');
  assert.equal(formatList(['jpg', 'jpeg', 'png', 'tif', 'tiff', 'webp', 'heic', 'heif', 'avif']), 'JPG, PNG, WebP, TIFF, HEIC and AVIF');
  assert.equal(formatList(undefined), 'JPG and PNG');
  assert.equal(S.home.formats(['jpg', 'png', 'webp']), 'JPG · PNG · WebP');
});

test('settings: the defaults carry every field of the engine struct, and a patch keeps the rest', () => {
  const src = readFileSync(new URL('../../../crates/engine/src/settings.rs', import.meta.url), 'utf8');
  const start = src.indexOf('pub struct Settings {');
  const body = src.slice(start, src.indexOf('\n}', start));
  const fields = [...body.matchAll(/^ {4}pub ([a-z_]+):/gm)].map((m) => camel(m[1]));
  assert.ok(fields.includes('splitPolicy') && fields.includes('autoSaveSplits'));
  assert.deepEqual(Object.keys(defaultSettings()).sort(), fields.sort());
  const received = { ...defaultSettings(), someFutureField: 7 } as ReturnType<typeof defaultSettings>;
  const sent = patchedSettings(received, { saveAsCopy: true });
  assert.equal(sent.saveAsCopy, true);
  assert.equal(sent.splitPolicy, 'auto');
  assert.equal((sent as unknown as Record<string, unknown>).someFutureField, 7, 'a field the UI does not know survives');
});

test('PLAN 6.7 titles', () => {
  assert.equal(holdTitle({ code: 'NO_QUAD' }), "Couldn't find the edges");
  assert.equal(holdTitle({ code: 'PARTIAL_FRAME' }), 'Content may be cut off');
  assert.equal(holdTitle({ code: 'ODD_ASPECT' }), 'Unusual shape for a photo');
  assert.equal(holdTitle({ code: 'LOW_CONTRAST_EDGE' }), 'Edge hard to see');
  assert.equal(holdTitle({ code: 'BATCH_OUTLIER' as ReasonCode }), 'Differs from the rest of the batch');
});

test('WEAK_EDGE names the side', () => {
  assert.equal(holdTitle({ code: 'WEAK_EDGE', side: 'left' }), 'Edge unclear on the left side');
  assert.equal(holdTitle({ code: 'WEAK_EDGE', side: 'top' }), 'Edge unclear on the top side');
  assert.ok(!holdTitle({ code: 'WEAK_EDGE' }).includes('{side}'));
});

test('unknown codes never show a blank or a raw code', () => {
  const unknown = { code: 'FUTURE_CODE' as ReasonCode };
  assert.equal(holdTitle(unknown), 'Held for review');
  assert.ok(holdCause(unknown).length > 0);
  assert.ok(FALLBACK_HOLD.check.title.length > 0 && FALLBACK_HOLD.failed.title.length > 0);
});

test('the registry has no empty copy', () => {
  for (const [code, c] of Object.entries(HOLD)) {
    assert.ok(c.title && c.cause && c.action, code);
  }
});

test('typed error messages are complete and SOURCE_CHANGED matches PLAN 6.7', () => {
  for (const [code, text] of Object.entries(ERRORS)) assert.ok(text.length > 10, code);
  assert.equal(
    errorMessage('SOURCE_CHANGED'),
    'This file changed while Auto Crop was working, so it was left as it is. Its backup is kept.',
  );
  assert.equal(errorMessage('VERIFY_FAILED'), "Couldn't verify the new file. The original is unchanged.");
  assert.ok(errorMessage('ORIGINAL_EXPIRED').toLowerCase().includes('expired'));
  assert.ok(errorMessage('DISK_FULL').toLowerCase().includes('space'));
  assert.equal(errorMessage(null), ERRORS.INTERNAL);
});

test('retention text', () => {
  assert.equal(retentionText(30), '30 days');
  assert.equal(retentionText(null), 'until you delete them');
});

// ------------------------------------------------------------------------------------------ notices and curves

/** Every notice code the engine can put on a result: its `NOTICE_*` constants and the literals it pushes or returns. */
function engineNoticeCodes(): string[] {
  const dir = new URL('../../../crates/engine/src/', import.meta.url);
  const found = new Set<string>();
  for (const f of readdirSync(dir).filter((n) => n.endsWith('.rs'))) {
    const src = readFileSync(new URL(f, dir), 'utf8');
    for (const m of src.matchAll(/pub const NOTICE_[A-Z_]+: &str = "([a-z_]+\.[a-z_]+)"/g)) found.add(m[1]);
    for (const m of src.matchAll(/notices(?:\.push\(|\s*=\s*vec!\[)"([a-z_]+\.[a-z_]+)"/g)) found.add(m[1]);
    for (const m of src.matchAll(/\(ErrKind::HeldForReview, vec!\["([a-z_]+\.[a-z_]+)"/g)) found.add(m[1]);
    // fsplan's refusals: `Some("tiff.multi_page")` and the `_ => "tiff.multi_page"` arm
    if (f === 'fsplan.rs') for (const m of src.matchAll(/(?:Some\(|=>\s*)"((?:tiff|format)\.[a-z_]+)"/g)) found.add(m[1]);
  }
  return [...found].sort();
}

test('every notice code the engine can send has its own line (read from crates/engine)', () => {
  const codes = engineNoticeCodes();
  for (const known of ['curved.held', 'split.held', 'tiff.multi_page', 'format.write_unavailable', 'derived.user_edited', 'jpeg.lossless', 'sync.root']) {
    assert.ok(codes.includes(known), `the scan of the engine sources missed ${known}: ${codes.join(', ')}`);
  }
  for (const code of codes) {
    assert.ok(code in NOTICES, `no copy for the notice ${code}`);
    assert.ok(NOTICES[code].length > 20, code);
    assert.notEqual(noticeText(code), noticeText('some.future.code'), `${code} falls back to the generic line`);
  }
});

test('curved pages: the held notice says what to do, and the limits note is honest', () => {
  assert.match(NOTICES['curved.held'], /held for review/);
  assert.match(NOTICES['curved.held'], /Accept the page/);
  assert.match(NOTICES['curved.held'], /Save as copy/);
  assert.equal(S.curved.limits, 'Fixes bowed edges and perspective. Wrinkles inside the page stay.');
  assert.match(S.curved.limitsMore, /four edges/);
  assert.equal(S.curved.held, 'Curved page: review, then accept');
  assert.match(S.save.acceptFirstCurved, /Save as copy/);
  assert.match(S.curved.itemOp, /Back to straight/);
  assert.match(S.curved.backToStraightHint, /Undo/);
});

test('every way the engine can refuse a curve set has its own plain sentence (read from crates/core)', () => {
  // CurveError variant -> the problem the UI names it with (curve.ts `validateCurves`)
  const problemOf: Record<string, CurveProblem> = {
    TooFewPoints: 'points',
    TooManyPoints: 'points',
    NonFinite: 'nonFinite',
    CoincidentPoints: 'coincident',
    OutOfRange: 'range',
    CornerMismatch: 'corners',
    SelfIntersecting: 'crossing',
    NoArea: 'noArea',
  };
  for (const v of rustVariants('crates/core/src/curve.rs', 'CurveError')) {
    const problem = problemOf[v];
    assert.ok(problem, `CurveError::${v} has no mapping in this test: give it copy and add it here`);
    assert.ok(S.curved.problems[problem].length > 15, `${v} -> ${problem}`);
  }
  for (const text of Object.values(S.curved.problems)) assert.ok(!/DEGENERATE/.test(text));
});

test('the curved copy has no empty line', () => {
  const walk = (o: unknown, path: string): void => {
    if (typeof o === 'string') assert.ok(o.trim().length > 0, path);
    else if (typeof o === 'function') {
      const out = (o as (...a: never[]) => unknown)(...(['Top edge', 3, 5, '1.0', '2.0'] as never[]));
      assert.ok(typeof out === 'string' && out.trim().length > 0, `${path}()`);
    } else if (o && typeof o === 'object') for (const [k, v] of Object.entries(o)) walk(v, `${path}.${k}`);
  };
  walk(S.curved, 'S.curved');
});
