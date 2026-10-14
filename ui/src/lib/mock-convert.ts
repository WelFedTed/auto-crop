// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The Convert half of the in-memory mock (mock.ts): a list of fake files (the format is read from the extension), a
// preview that follows the options the way crates/engine/src/convert_batch.rs does, and a run that emits
// `convert-progress` with small delays, can be cancelled, and returns results with the notices, skips and failures the
// real engine produces. Nothing is read from or written to disk; sizes are made up (or the dropped file's own).
//
// Demo behaviour worth knowing (by file name): `corrupt` fails with CORRUPT, `locked` with FILE_IN_USE, `burst`
// is a HEIC with several images, `live` a Live Photo, `anim` an animated GIF, `multipage` a multi-page TIFF, `unknown`
// is not recognised, `alpha` has transparency. `?noheif=1` makes this build one without the HEIC decoder.
// `?convertsamples=1` starts with a sample list; `window.__autoCropMock.convertSamples()` adds it later.

import type { BackupFile, BackupRun, ConvertAdded, ConvertCannot, ConvertFileResult, ConvertFileView, ConvertOptions, ConvertPreview, ConvertProgress, ConvertSummary, Api } from './types.ts';

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

export interface ConvertMockDeps {
  emitAdded(a: ConvertAdded): void;
  emitProgress(p: ConvertProgress): void;
  /** Puts a backup run into the Backups view. */
  addBackupRun(run: BackupRun): void;
  expiresAt(createdMs: number): string | null;
}

interface Entry {
  id: number;
  name: string;
  format: string;
  bytes: number;
  done: boolean;
}

const EXT_FORMAT: Record<string, string> = {
  jpg: 'jpeg',
  jpeg: 'jpeg',
  png: 'png',
  tif: 'tiff',
  tiff: 'tiff',
  webp: 'webp',
  heic: 'heic',
  heif: 'heic',
  avif: 'avif',
  gif: 'gif',
  bmp: 'bmp',
  jxl: 'jxl',
};

export const CONVERT_ACCEPT = '.jpg,.jpeg,.png,.tif,.tiff,.webp,.heic,.heif,.avif,.gif,.bmp,image/*';

/** The format name the engine reports for each Convert target, and the extension its files get. */
const TARGET_FORMAT: Record<ConvertOptions['target'], string> = { jpg: 'jpeg', png: 'png', tiff: 'tiff', webp: 'webp', pdf: 'pdf' };
const TARGET_EXT: Record<ConvertOptions['target'], string> = { jpg: 'jpg', png: 'png', tiff: 'tif', webp: 'webp', pdf: 'pdf' };

const extOf = (name: string) => name.slice(name.lastIndexOf('.') + 1).toLowerCase();
const stemOf = (name: string) => (name.lastIndexOf('.') > 0 ? name.slice(0, name.lastIndexOf('.')) : name);

function sniff(name: string): string | null {
  const f = EXT_FORMAT[extOf(name)];
  if (!f) return null;
  return /unknown/i.test(name) ? 'unknown' : f;
}

const heifAvailable = () => typeof location === 'undefined' || !new URLSearchParams(location.search).has('noheif');

const SAMPLES: [string, number][] = [
  ['IMG_4021.HEIC', 2_400_000],
  ['IMG_4022.HEIC', 3_100_000],
  ['IMG_4023.heic', 2_800_000],
  ['IMG_4024_burst.HEIC', 5_200_000],
  ['IMG_4025_live.HEIC', 2_900_000],
  ['Screenshot 2026-09-30.png', 1_900_000],
  ['logo_alpha.png', 340_000],
  ['Holiday.webp', 640_000],
  ['ledger_multipage.tif', 8_400_000],
  ['animated_cat.gif', 1_100_000],
  ['scan_corrupt.png', 720_000],
  ['report_locked.bmp', 3_000_000],
  ['Camera.avif', 410_000],
  ['already.jpg', 3_500_000],
  ['mystery_unknown.jpg', 90_000],
];

/** Output size relative to the input, by format pair and quality. */
function bytesOutOf(from: string, to: string, bytes: number, o: ConvertOptions): number {
  const q = o.quality.kind === 'fixed' ? Math.pow(o.quality.value / 85, 1.5) : { small: 0.7, balanced: 1, best: 1.6 }[o.quality.preset];
  const jpg: Record<string, number> = { heic: 1.5, png: 0.22, webp: 1.3, tiff: 0.1, bmp: 0.05, gif: 0.8, avif: 1.6, jpeg: 0.9, jxl: 1.1 };
  const png: Record<string, number> = { heic: 4, jpeg: 3.2, bmp: 0.3, gif: 1.2, webp: 2.5, tiff: 0.7, avif: 4, png: 0.97, jxl: 2 };
  // JPG and a PDF page (a JPG inside) follow the quality; the lossless targets do not: a TIFF is a little larger
  // than the PNG, a lossless WebP a little smaller.
  const lossless = (png[from] ?? 1) * (to === 'tiff' ? 1.25 : to === 'webp' ? 0.8 : 1);
  const r = to === 'jpeg' || to === 'pdf' ? (jpg[from] ?? 1) * q : lossless;
  return Math.max(2_000, Math.round(bytes * r));
}

export function createConvertMock(deps: ConvertMockDeps) {
  let files: Entry[] = [];
  let nextId = 1;
  let folderName: string | null = null;
  let running = false;
  let cancel = false;
  let runCount = 0;

  const view = (e: Entry): ConvertFileView => ({ id: e.id, name: e.name, format: e.format, bytes: e.bytes });

  function add(list: { name: string; bytes: number }[]): ConvertAdded {
    const out: ConvertAdded = { added: 0, duplicates: 0, linksSkipped: 0, truncated: false, ids: [] };
    for (const f of list) {
      const format = sniff(f.name);
      if (!format) continue; // the walk only takes picture extensions
      if (files.some((x) => x.name === f.name && x.bytes === f.bytes)) {
        out.duplicates++;
        continue;
      }
      const e: Entry = { id: nextId++, name: f.name, format, bytes: f.bytes, done: false };
      files.push(e);
      out.ids.push(e.id);
      out.added++;
    }
    return out;
  }

  /** What a run does with a file: convert, leave (same format) or cannot. */
  function plan(e: Entry, o: ConvertOptions): { kind: 'convert' } | { kind: 'same' } | { kind: 'cannot'; code: string } {
    if (e.format === 'unknown') return { kind: 'cannot', code: 'UNSUPPORTED_FORMAT' };
    if ((e.format === 'heic' || e.format === 'avif') && !heifAvailable()) return { kind: 'cannot', code: 'HEIF_UNAVAILABLE' };
    const target = TARGET_FORMAT[o.target];
    if (e.format === target && !o.reencode) return { kind: 'same' };
    return { kind: 'convert' };
  }

  function preview(o: ConvertOptions): ConvertPreview {
    const live = files.filter((f) => !f.done);
    const counts = new Map<string, number>();
    const cannotFiles: ConvertCannot[] = [];
    let will = 0;
    let same = 0;
    let cannot = 0;
    let heifMissing = 0;
    let bytes = 0;
    for (const f of live) {
      counts.set(f.format, (counts.get(f.format) ?? 0) + 1);
      const p = plan(f, o);
      if (p.kind === 'convert') {
        will++;
        bytes += f.bytes;
      } else if (p.kind === 'same') same++;
      else {
        cannot++;
        if (p.code === 'HEIF_UNAVAILABLE') heifMissing++;
        if (cannotFiles.length < 50) cannotFiles.push({ id: f.id, name: f.name, format: f.format, code: p.code });
      }
    }
    return {
      total: live.length,
      willConvert: will,
      sameFormat: same,
      cannot,
      byFormat: [...counts].map(([format, count]) => ({ format, count })).sort((a, b) => b.count - a.count),
      cannotFiles,
      heifAvailable: heifAvailable(),
      heifUnavailable: heifMissing,
      bytesToConvert: bytes,
      placement: o.placement,
      folder: o.placement === 'folder' ? folderName : null,
    };
  }

  function chooseFiles(directory: boolean): Promise<File[]> {
    return new Promise((resolve) => {
      const input = document.createElement('input');
      input.type = 'file';
      input.multiple = true;
      input.accept = CONVERT_ACCEPT;
      if (directory) (input as HTMLInputElement & { webkitdirectory: boolean }).webkitdirectory = true;
      input.style.display = 'none';
      document.body.appendChild(input);
      const done = (list: File[]) => {
        input.remove();
        resolve(list);
      };
      input.addEventListener('change', () => done(Array.from(input.files ?? [])));
      input.addEventListener('cancel', () => done([]));
      input.click();
    });
  }

  const fromFiles = (list: File[]) => add(list.map((f) => ({ name: f.name, bytes: f.size })));

  async function run(o: ConvertOptions, runName: string): Promise<ConvertSummary> {
    if (running) throw 'INTERNAL';
    running = true;
    cancel = false;
    const queue = files.filter((f) => !f.done);
    const runId = `conv-${Date.now().toString(36)}-${++runCount}`;
    const taken = new Set(files.map((f) => f.name.toLowerCase()));
    const target = TARGET_FORMAT[o.target];
    const ext = TARGET_EXT[o.target];
    const results: ConvertFileResult[] = [];
    let backup: BackupRun | null = null;
    let seed = 7;
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    try {
      for (let i = 0; i < queue.length; i++) {
        const e = queue[i];
        const base = (patch: Partial<ConvertFileResult>): ConvertFileResult => ({
          id: e.id,
          name: e.name,
          status: 'skipped',
          code: null,
          notices: [],
          output: null,
          from: e.format,
          to: null,
          bytesIn: e.bytes,
          bytesOut: 0,
          width: null,
          height: null,
          backupId: null,
          ...patch,
        });
        let r: ConvertFileResult;
        if (cancel) {
          r = base({ status: 'skipped', code: 'CANCELLED' });
        } else {
          await sleep(160 + Math.round(rnd() * 200));
          const p = plan(e, o);
          const replace = o.placement === 'replace';
          const name = e.name;
          const multi = /burst/i.test(name) ? 'heic.multi_image' : /multipage/i.test(name) ? 'tiff.multi_page' : /anim/i.test(name) ? 'anim.first_frame_only' : null;
          if (p.kind === 'cannot') {
            r = base({ status: 'skipped', code: p.code, notices: p.code === 'HEIF_UNAVAILABLE' ? ['heif.unavailable'] : [] });
          } else if (p.kind === 'same') {
            r = base({ status: 'skipped', code: 'SAME_FORMAT', notices: ['convert.same_format'] });
          } else if (/corrupt/i.test(name)) {
            r = base({ status: 'failed', code: 'CORRUPT' });
          } else if (/locked/i.test(name)) {
            r = base({ status: 'failed', code: 'FILE_IN_USE' });
          } else if (multi && replace) {
            r = base({ status: 'skipped', code: 'NOT_REPLACEABLE', notices: [multi] });
          } else {
            const notices: string[] = [];
            if (multi) notices.push(multi);
            if (e.format === 'heic' && /live/i.test(name)) notices.push('livephoto.video_untouched');
            if (e.format === 'png' && (o.target === 'jpg' || o.target === 'pdf') && (/alpha/i.test(name) || e.id % 2 === 1)) notices.push('alpha.flattened');
            if (e.format === 'heic' && o.colour === 'srgb') notices.push('colour.converted_to_srgb');
            if (e.format === 'heic' && e.id % 5 === 3) notices.push('colour.nclx_not_srgb');
            if (e.format === 'avif') notices.push('depth.reduced_to_8');
            if (/sync/i.test(name)) notices.push('sync.root');
            // The new name: the source's own when it is replaced in place with another extension? No: a new extension is
            // a new name, so a taken one gets "(2)".
            let out = `${stemOf(name)}.${ext}`;
            const sameName = out.toLowerCase() === name.toLowerCase();
            if (!(replace && sameName)) {
              let n = 2;
              while (taken.has(out.toLowerCase())) out = `${stemOf(name)} (${n++}).${ext}`;
            }
            taken.add(out.toLowerCase());
            const w = e.format === 'heic' ? 4032 : 1000 + ((e.id * 313) % 3000);
            const h = e.format === 'heic' ? 3024 : Math.round(w * 0.75);
            let backupId: string | null = null;
            if (replace) {
              if (!backup) {
                const created = new Date();
                backup = { id: runId, name: runName, createdAt: created.toISOString(), expiresAt: deps.expiresAt(created.getTime()), fileCount: 0, totalBytes: 0, pinned: false, files: [] };
                deps.addBackupRun(backup);
              }
              backupId = `${runId}/${backup.files.length}`;
              const bytesOut = bytesOutOf(e.format, target, e.bytes, o);
              const f: BackupFile = {
                id: backupId,
                name,
                displayPath: `C:\\Users\\you\\Pictures\\Camera Roll\\${name}`,
                originalBytes: e.bytes,
                outputBytes: bytesOut,
                changedSinceSaved: false,
                restored: false,
                kind: 'OneToN',
                derived: [{ name: out, bytes: bytesOut, state: 'unchanged' }],
              };
              backup.files.push(f);
              backup.fileCount = backup.files.length;
              backup.totalBytes += e.bytes;
            }
            r = base({
              status: 'converted',
              notices,
              output: out,
              to: target,
              bytesOut: bytesOutOf(e.format, target, e.bytes, o),
              width: w,
              height: h,
              backupId,
            });
            e.done = true;
          }
        }
        results.push(r);
        deps.emitProgress({ done: results.length, total: queue.length, result: r });
      }
    } finally {
      running = false;
    }
    const sum = (s: ConvertFileResult['status']) => results.filter((x) => x.status === s).length;
    const conv = results.filter((x) => x.status === 'converted');
    return {
      runId,
      converted: sum('converted'),
      skipped: sum('skipped'),
      failed: sum('failed'),
      cancelled: cancel,
      bytesIn: conv.reduce((s, x) => s + x.bytesIn, 0),
      bytesOut: conv.reduce((s, x) => s + x.bytesOut, 0),
      results,
    };
  }

  const api: Pick<
    Api,
    | 'convertMode'
    | 'convertPickFiles'
    | 'convertPickFolder'
    | 'convertPickOutputFolder'
    | 'convertList'
    | 'convertRemove'
    | 'convertClear'
    | 'convertPreview'
    | 'convertRun'
    | 'convertCancel'
    | 'convertOpenFolder'
  > = {
    async convertMode() {},
    async convertPickFiles() {
      return fromFiles(await chooseFiles(false));
    },
    async convertPickFolder() {
      return fromFiles(await chooseFiles(true));
    },
    async convertPickOutputFolder() {
      await sleep(80);
      folderName = folderName ?? 'Converted photos';
      return folderName;
    },
    async convertList() {
      return files.map(view);
    },
    async convertRemove(ids) {
      const drop = new Set(ids);
      files = files.filter((f) => !drop.has(f.id));
    },
    async convertClear() {
      files = [];
    },
    async convertPreview(options) {
      await sleep(40);
      return preview(options);
    },
    convertRun: run,
    async convertCancel() {
      cancel = true;
    },
    async convertOpenFolder(id) {
      console.info('[mock] open the folder of', id === null ? 'the first converted file' : `file ${id}`);
    },
  };

  return {
    api,
    dropFiles: async (list: File[]) => fromFiles(list),
    /** The sample list; `announce` emits `convert-added` as a window drop would. */
    samples(announce: boolean): ConvertAdded {
      const a = add(SAMPLES.map(([name, bytes]) => ({ name, bytes })));
      if (announce) deps.emitAdded(a);
      return a;
    },
  };
}
