// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The output formats on the front end (ROADMAP M11.42, M11.43): the Convert targets, the editor's Save format and what
// the copy says about the sources a lossless writer cannot give back. Drift guards read the Rust sources, so a format
// or a notice added in the engine cannot be forgotten here.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { DEFAULT_OPTIONS, TARGETS, parseOptions, sanitiseOptions, serialiseOptions, usesQuality } from './convert.ts';
import { SAVE_FORMATS, changesFormat, plannedNames, saveGate } from './items.ts';
import { NOTICES, S, formatName, noticeText, openOnlyShort } from './strings.ts';
import { defaultSettings } from './settings-defaults.ts';
import { defaultSettings as kitSettings } from './testkit.ts';
import type { ItemView } from './types.ts';

const rust = (f: string) => readFileSync(new URL(`../../../${f}`, import.meta.url), 'utf8');

/** The variants of `pub enum <name>` in a Rust file, as their serde lower-case names. */
function lowerVariants(file: string, name: string): string[] {
  const src = rust(file);
  const start = src.indexOf(`pub enum ${name} {`);
  assert.ok(start >= 0, `${name} not found in ${file}`);
  const body = src.slice(start, src.indexOf('\n}', start));
  return [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*)(?:\s*[,{(]|$)/gm)].map((m) => m[1].toLowerCase());
}

test('the Convert targets are the engine ConvertTarget variants, in the order the screen lists them', () => {
  const engine = lowerVariants('crates/engine/src/convert.rs', 'ConvertTarget');
  assert.deepEqual([...TARGETS].sort(), engine.sort());
  assert.deepEqual([...TARGETS], ['jpg', 'png', 'tiff', 'webp', 'pdf']);
});

test('only the JPG, and the JPG inside a PDF page, have a quality to choose', () => {
  assert.deepEqual(TARGETS.filter(usesQuality), ['jpg', 'pdf']);
  for (const t of ['png', 'tiff', 'webp'] as const) {
    assert.match(S.convert.losslessNoQuality(S.convert.targetPng), /lossless/);
    assert.ok(!usesQuality(t), t);
  }
  assert.match(S.convert.targetWebp, /lossless/i, 'the WebP target says it is lossless: there is no lossy writer');
});

test('stored Convert options keep a new target and fall back from an unknown one', () => {
  for (const t of TARGETS) assert.equal(parseOptions(serialiseOptions({ ...DEFAULT_OPTIONS, target: t })).target, t);
  assert.equal(sanitiseOptions({ target: 'avif' }).target, DEFAULT_OPTIONS.target);
  assert.equal(sanitiseOptions({ target: 'jxl' }).target, 'jpg');
});

test('the Save formats are the engine SaveFormat variants and the default is the source own', () => {
  const engine = lowerVariants('crates/engine/src/output.rs', 'SaveFormat');
  assert.deepEqual([...SAVE_FORMATS].sort(), engine.sort());
  assert.equal(defaultSettings().saveFormat, 'auto');
  assert.equal(kitSettings().saveFormat, 'auto');
  for (const f of SAVE_FORMATS) {
    const label = { auto: S.save.formatAuto, jpeg: S.save.formatJpeg, png: S.save.formatPng, tiff: S.save.formatTiff, webp: S.save.formatWebp, pdf: S.save.formatPdf }[f];
    assert.ok(label.length > 1, f);
  }
  assert.match(S.save.formatWebp, /lossless/i);
});

const item = (name: string, patch: Partial<ItemView> = {}): ItemView =>
  ({
    id: 1,
    name,
    width: 100,
    height: 80,
    status: 'ready',
    openOnly: null,
    split: null,
    crops: [{ id: 1, order: 1, include: true, edit: { quad: [], quarterTurns: 0, fineDeg: 0 }, outputName: name.replace(/\.[^.]+$/, '') + '.' + (name.split('.').pop() ?? '') }],
    ...patch,
  }) as unknown as ItemView;

test('the planned names take the extension of the chosen format, and an unchanged choice leaves them alone', () => {
  const it = item('scan.png');
  assert.deepEqual(plannedNames(it), ['scan.png']);
  assert.deepEqual(plannedNames(it, 'auto'), ['scan.png']);
  assert.deepEqual(plannedNames(it, 'tiff'), ['scan.tif']);
  assert.deepEqual(plannedNames(it, 'webp'), ['scan.webp']);
  assert.deepEqual(plannedNames(it, 'pdf'), ['scan.pdf']);
  assert.deepEqual(plannedNames(it, 'jpeg'), ['scan.jpg']);
  // a stem with dots keeps them
  assert.deepEqual(plannedNames(item('a.b.c.png'), 'tiff'), ['a.b.c.tif']);
});

test('another Save format turns Replace original off (a copy is the only way), the same format does not', () => {
  assert.equal(changesFormat(item('a.png'), 'auto'), false);
  assert.equal(changesFormat(item('a.png'), undefined), false);
  assert.equal(changesFormat(item('a.png'), 'png'), false);
  assert.equal(changesFormat(item('a.JPEG'), 'jpeg'), false);
  assert.equal(changesFormat(item('a.tiff'), 'tiff'), false);
  assert.equal(changesFormat(item('a.png'), 'tiff'), true);
  assert.equal(changesFormat(item('a.heic'), 'jpeg'), true);
  assert.equal(changesFormat(item('noext'), 'png'), true);
  const s = defaultSettings();
  assert.deepEqual(saveGate(item('a.png'), { ...s, saveFormat: 'pdf' }), { replace: 'format-change' });
  assert.deepEqual(saveGate(item('a.png'), { ...s, saveFormat: 'png' }), { replace: 'ok' });
  assert.deepEqual(saveGate(item('a.png'), s), { replace: 'ok' });
  // a source that is never replaced says so first
  assert.deepEqual(saveGate(item('a.png', { openOnly: 'webp.lossy' }), { ...s, saveFormat: 'pdf' }), { replace: 'open-only', reason: 'webp.lossy' });
});

test('every reason a lossless writer cannot give a source back has its own line, badge and Save-as-copy advice', () => {
  const src = rust('crates/engine/src/fsplan.rs');
  const start = src.indexOf('pub const LOSSY_DECODE_NOTICES');
  const codes = [...src.slice(start, src.indexOf('];', start)).matchAll(/"([a-z_0-9]+\.[a-z_0-9]+)"/g)].map((m) => m[1]);
  assert.ok(codes.length >= 6, codes.join(','));
  for (const known of ['webp.lossy', 'tiff.low_depth', 'tiff.exif_not_carried']) assert.ok(codes.includes(known), known);
  for (const code of codes) {
    assert.ok(code in NOTICES, `no copy for ${code}`);
    assert.notEqual(noticeText(code), noticeText('some.future.code'), code);
  }
  for (const code of ['webp.lossy', 'tiff.low_depth', 'tiff.exif_not_carried']) {
    assert.match(NOTICES[code], /Save as copy/, code);
    assert.ok(openOnlyShort(code).length > 5);
  }
  assert.match(NOTICES['webp.lossy'], /PNG/);
  assert.match(NOTICES['format.change_needs_copy'], /copy/);
  assert.equal(formatName('pdf'), 'PDF');
  assert.equal(formatName('tiff'), 'TIFF');
});
