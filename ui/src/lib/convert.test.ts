// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  announceDue,
  chipNotices,
  clampQuality,
  countResults,
  DEFAULT_OPTIONS,
  isConversionBackup,
  parseOptions,
  restoreFileId,
  sanitiseOptions,
  serialiseOptions,
  stepOf,
  whyOff,
} from './convert.ts';
import type { ConvertFileResult, ConvertPreview } from './types.ts';

const preview = (patch: Partial<ConvertPreview> = {}): ConvertPreview => ({
  total: 3,
  willConvert: 2,
  sameFormat: 1,
  cannot: 0,
  byFormat: [],
  cannotFiles: [],
  heifAvailable: true,
  heifUnavailable: 0,
  bytesToConvert: 1000,
  placement: 'replace',
  folder: null,
  ...patch,
});

const result = (patch: Partial<ConvertFileResult> = {}): ConvertFileResult => ({
  id: 1,
  name: 'IMG_0001.heic',
  status: 'converted',
  code: null,
  notices: [],
  output: 'IMG_0001.jpg',
  from: 'heic',
  to: 'jpeg',
  bytesIn: 2_000_000,
  bytesOut: 1_000_000,
  width: 4032,
  height: 3024,
  backupId: null,
  ...patch,
});

test('the defaults are the engine defaults: JPG, sRGB, Balanced, GPS kept, replace, no re-encode', () => {
  assert.deepEqual(DEFAULT_OPTIONS, {
    target: 'jpg',
    colour: 'srgb',
    quality: { kind: 'preset', preset: 'balanced' },
    stripLocation: false,
    placement: 'replace',
    reencode: false,
  });
});

test('stored options are checked field by field and fall back to the defaults', () => {
  assert.deepEqual(parseOptions(null), DEFAULT_OPTIONS);
  assert.deepEqual(parseOptions('not json'), DEFAULT_OPTIONS);
  assert.deepEqual(parseOptions('[1,2]'), DEFAULT_OPTIONS);
  const o = parseOptions(JSON.stringify({ target: 'png', colour: 'preserve', quality: { kind: 'fixed', value: 250 }, stripLocation: true, placement: 'folder', reencode: true }));
  assert.deepEqual(o, { target: 'png', colour: 'preserve', quality: { kind: 'fixed', value: 100 }, stripLocation: true, placement: 'folder', reencode: true });
  const bad = sanitiseOptions({ target: 'gif', colour: 3, quality: { kind: 'preset', preset: 'huge' }, placement: 'cloud', stripLocation: 'yes' });
  assert.deepEqual(bad, DEFAULT_OPTIONS);
});

test('options survive a round trip through storage', () => {
  const o = { ...DEFAULT_OPTIONS, target: 'png' as const, placement: 'keep' as const, quality: { kind: 'fixed' as const, value: 72 } };
  assert.deepEqual(parseOptions(serialiseOptions(o)), o);
});

test('quality is a whole number from 1 to 100', () => {
  assert.equal(clampQuality(0), 1);
  assert.equal(clampQuality(101), 100);
  assert.equal(clampQuality(72.6), 73);
  assert.equal(clampQuality(Number.NaN), 85);
});

test('the Convert button says why it is off, in a fixed order', () => {
  const o = DEFAULT_OPTIONS;
  assert.equal(whyOff(0, null, o, false), 'files');
  assert.equal(whyOff(3, null, o, false), 'checking');
  assert.equal(whyOff(3, preview(), o, true), 'checking');
  assert.equal(whyOff(3, preview({ willConvert: 0, sameFormat: 3 }), o, false), 'nothing');
  assert.equal(whyOff(3, preview(), { ...o, placement: 'folder' }, false), 'folder');
  assert.equal(whyOff(3, preview({ folder: 'Converted' }), { ...o, placement: 'folder' }, false), null);
  assert.equal(whyOff(3, preview(), o, false), null);
  // a folder that was chosen earlier does not matter while the placement is something else
  assert.equal(whyOff(3, preview({ folder: null }), { ...o, placement: 'keep' }, false), null);
});

test('progress is announced at each new tenth and at the end, never twice', () => {
  assert.equal(stepOf(0, 12), 0);
  assert.equal(stepOf(6, 12), 5);
  assert.equal(stepOf(12, 12), 10);
  assert.equal(announceDue(0, 1, 100), false);
  assert.equal(announceDue(9, 10, 100), true);
  assert.equal(announceDue(10, 11, 100), false);
  assert.equal(announceDue(99, 100, 100), true);
  assert.equal(announceDue(100, 100, 100), false);
  // few files: every file crosses a step
  assert.equal(announceDue(0, 1, 3), true);
  // the number of announcements of a whole run is at most 11
  let n = 0;
  for (let d = 1; d <= 250; d++) if (announceDue(d - 1, d, 250)) n++;
  assert.ok(n <= 11 && n >= 10, String(n));
});

test('a backup id is the file id of its only file', () => {
  assert.equal(restoreFileId('run-1'), 'run-1/0');
  assert.equal(restoreFileId('run-1/3'), 'run-1/3');
});

test('a conversion in Backups is told from a split scan', () => {
  const file = (kind: 'OneToOne' | 'OneToN', name: string, derived: string[]) => ({ kind, name, derived: derived.map((d) => ({ name: d, bytes: 1, state: 'unchanged' as const })) });
  assert.equal(isConversionBackup(file('OneToN', 'IMG_0001.heic', ['IMG_0001.jpg'])), true);
  assert.equal(isConversionBackup(file('OneToN', 'IMG_0001.heic', ['IMG_0001 (2).jpg'])), true);
  assert.equal(isConversionBackup(file('OneToN', 'a.jpg', ['a.jpg'])), true);
  assert.equal(isConversionBackup(file('OneToN', 'scan.jpg', ['scan_01.jpg', 'scan_02.jpg'])), false);
  assert.equal(isConversionBackup(file('OneToN', 'scan.jpg', ['scan_01.jpg'])), false);
  assert.equal(isConversionBackup(file('OneToOne', 'a.jpg', [])), false);
});

test('results are counted by status, and sizes come from the converted files only', () => {
  const c = countResults([
    result({ id: 1 }),
    result({ id: 2, bytesIn: 1_000, bytesOut: 500 }),
    result({ id: 3, status: 'skipped', code: 'SAME_FORMAT', bytesIn: 9_999_999, bytesOut: 0 }),
    result({ id: 4, status: 'failed', code: 'CORRUPT' }),
  ]);
  assert.deepEqual(c, { converted: 2, skipped: 1, failed: 1, bytesIn: 2_001_000, bytesOut: 1_000_500 });
});

test('a notice the status line already says gets no chip', () => {
  assert.deepEqual(chipNotices(result({ notices: ['alpha.flattened', 'alpha.flattened', 'colour.converted_to_srgb'] })), ['alpha.flattened', 'colour.converted_to_srgb']);
  assert.deepEqual(chipNotices(result({ status: 'skipped', code: 'SAME_FORMAT', notices: ['convert.same_format'] })), []);
  assert.deepEqual(chipNotices(result({ status: 'skipped', code: 'HEIF_UNAVAILABLE', notices: ['heif.unavailable'] })), []);
  assert.deepEqual(chipNotices(result({ status: 'skipped', code: 'NOT_REPLACEABLE', notices: ['heic.multi_image'] })), ['heic.multi_image']);
});
