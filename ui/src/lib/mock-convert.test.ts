// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { DEFAULT_OPTIONS } from './convert.ts';
import { createConvertMock } from './mock-convert.ts';
import type { BackupRun, ConvertProgress } from './types.ts';

function setup() {
  const progress: ConvertProgress[] = [];
  const runs: BackupRun[] = [];
  const mock = createConvertMock({
    emitAdded: () => {},
    emitProgress: (p) => progress.push(p),
    addBackupRun: (r) => runs.push(r),
    expiresAt: () => null,
  });
  return { mock, progress, runs };
}

const file = (name: string, bytes = 1_000_000) => ({ name, bytes });

test('the mock reads the format from the extension, skips what is not a picture, and says what is a duplicate', async () => {
  const { mock } = setup();
  const files = [new File(['x'], 'a.HEIC'), new File(['x'], 'b.png'), new File(['x'], 'notes.txt'), new File(['x'], 'a.HEIC')];
  const a = await mock.dropFiles(files);
  assert.equal(a.added, 2);
  assert.equal(a.duplicates, 1);
  const list = await mock.api.convertList();
  assert.deepEqual(list.map((f) => [f.name, f.format]), [['a.HEIC', 'heic'], ['b.png', 'png']]);
});

test('the preview follows the options: same format is left alone unless re-encoding, no HEIC decoder means cannot', async () => {
  const { mock } = setup();
  mock.samples(false);
  const jpg = await mock.api.convertPreview(DEFAULT_OPTIONS);
  assert.equal(jpg.total, 15);
  assert.equal(jpg.sameFormat, 1); // already.jpg
  assert.equal(jpg.cannot, 1); // mystery_unknown.jpg
  assert.equal(jpg.willConvert, 13);
  assert.equal(jpg.heifAvailable, true);
  assert.equal(jpg.byFormat.find((f) => f.format === 'heic')?.count, 5);
  assert.deepEqual(jpg.cannotFiles.map((c) => c.code), ['UNSUPPORTED_FORMAT']);
  const again = await mock.api.convertPreview({ ...DEFAULT_OPTIONS, reencode: true });
  assert.equal(again.sameFormat, 0);
  assert.equal(again.willConvert, 14);
  const png = await mock.api.convertPreview({ ...DEFAULT_OPTIONS, target: 'png' });
  assert.equal(png.sameFormat, 3); // the three PNGs
  const folder = await mock.api.convertPreview({ ...DEFAULT_OPTIONS, placement: 'folder' });
  assert.equal(folder.folder, null);
  assert.equal(await mock.api.convertPickOutputFolder(), 'Converted photos');
  assert.equal((await mock.api.convertPreview({ ...DEFAULT_OPTIONS, placement: 'folder' })).folder, 'Converted photos');
});

test('a run emits one progress event per file and returns results with notices, skips, failures and backups', async () => {
  const { mock, progress, runs } = setup();
  await mock.dropFiles([
    new File([new Uint8Array(2_000_000)], 'IMG_1.HEIC'),
    new File([new Uint8Array(500_000)], 'burst_2.heic'),
    new File([new Uint8Array(400_000)], 'pic.jpg'),
    new File([new Uint8Array(300_000)], 'bad_corrupt.png'),
  ]);
  const s = await mock.api.convertRun(DEFAULT_OPTIONS, 'Convert test');
  assert.equal(progress.length, 4);
  assert.deepEqual(progress.map((p) => [p.done, p.total]), [[1, 4], [2, 4], [3, 4], [4, 4]]);
  assert.equal(s.converted, 1);
  assert.equal(s.skipped, 2);
  assert.equal(s.failed, 1);
  assert.equal(s.cancelled, false);
  const heic = s.results[0];
  assert.equal(heic.status, 'converted');
  assert.equal(heic.output, 'IMG_1.jpg');
  assert.ok(heic.notices.includes('colour.converted_to_srgb'));
  assert.ok(heic.backupId);
  assert.equal(s.results[1].code, 'NOT_REPLACEABLE'); // several images: never replaced
  assert.deepEqual(s.results[1].notices, ['heic.multi_image']);
  assert.equal(s.results[2].code, 'SAME_FORMAT');
  assert.equal(s.results[3].code, 'CORRUPT');
  // the backup is a conversion in Backups: one original, one derived file
  assert.equal(runs.length, 1);
  assert.equal(runs[0].files[0].id, heic.backupId);
  assert.equal(runs[0].files[0].kind, 'OneToN');
  assert.deepEqual(runs[0].files[0].derived.map((d) => d.name), ['IMG_1.jpg']);
  // converted files are done: not converted again
  assert.equal((await mock.api.convertPreview(DEFAULT_OPTIONS)).total, 3);
});

test('keeping the originals converts a multi-image file and leaves no backup; cancel stops the rest', async () => {
  const { mock, runs } = setup();
  await mock.dropFiles([file('burst_1.heic'), file('b.heic'), file('c.heic')].map((f) => new File([new Uint8Array(f.bytes)], f.name)));
  const p = mock.api.convertRun({ ...DEFAULT_OPTIONS, placement: 'keep' }, 'x');
  setTimeout(() => void mock.api.convertCancel(), 100);
  const s = await p;
  assert.equal(runs.length, 0);
  assert.equal(s.cancelled, true);
  assert.equal(s.results[0].status, 'converted');
  assert.deepEqual(s.results[0].notices.filter((n) => n === 'heic.multi_image'), ['heic.multi_image']);
  assert.ok(s.results.slice(1).every((r) => r.code === 'CANCELLED'));
});
