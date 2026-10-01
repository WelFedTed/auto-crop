// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// In-memory implementation of the `Api` contract, used automatically when the UI runs in a plain browser
// (no `window.__TAURI_INTERNALS__`). It generates synthetic receipt and document photos with canvas,
// simulates asynchronous analysis (items appear "analysing", then "ready"), keeps a real edit history,
// "saves" items into fake backup runs and serves the backups API from memory. Image URLs are blob: URLs.
//
// Deliberate demo behaviour worth knowing: the first save of six or more items fails exactly one item with
// SOURCE_CHANGED so the per-file failure row and "Retry failed" can be seen; the retry succeeds. Backups
// start with three runs, one of them already expired (Purge expired now removes it) and one file that
// "changed since saved" (Restore asks for Restore as copy or Replace anyway).

import {
  canvasFromFile,
  canvasToBlobUrl,
  defaultEdit,
  detectQuad,
  drawScene,
  failedPlaceholderEdit,
  insetQuad,
  makeSpec,
  mulberry32,
  renderResult,
  scaleToLongEdge,
  skinnyQuad,
} from './mock-scene.ts';
import type { SceneSpec } from './mock-scene.ts';
import { cloneEdit } from './quad.ts';
import type { BackendImpl, Unlisten } from './backend.ts';
import type {
  Api,
  BackupFile,
  BackupRun,
  BackupsView,
  Confidence,
  Edit,
  ErrorCode,
  Events,
  ImageKind,
  ItemView,
  LaunchInfo,
  OpenSummary,
  Reason,
  RestoreMode,
  RestoreOutcome,
  SaveOutcome,
  SaveTarget,
  Settings,
} from './types.ts';

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const DAY = 86_400_000;

interface HistoryEntry {
  edit: Edit;
  label: string;
  drawn: boolean;
}

interface MockItem {
  view: ItemView;
  src: HTMLCanvasElement | null;
  spec: SceneSpec | null;
  history: HistoryEntry[];
  cursor: number;
  failedUntouched: boolean;
}

const items = new Map<number, MockItem>();
let nextId = 1;
let settings: Settings = { saveAsCopy: false, retentionDays: 30, firstWriteAck: false };
let saveCount = 0;
let failedOnce = false;

// ---------------------------------------------------------------------------------------------- events
const listeners: { [K in keyof Events]: Set<(p: Events[K]) => void> } = {
  'items-added': new Set(),
  'item-updated': new Set(),
};

function emit<K extends keyof Events>(event: K, payload: Events[K]): void {
  for (const cb of listeners[event]) {
    try {
      (cb as (p: Events[K]) => void)(payload);
    } catch (e) {
      console.error('[mock] listener failed', e);
    }
  }
}

// ---------------------------------------------------------------------------------------------- images
const urlExact = new Map<string, string>();
const urlLatest = new Map<string, string>();
let placeholder = '';

function ensurePlaceholder(): string {
  if (!placeholder) {
    const c = document.createElement('canvas');
    c.width = 4;
    c.height = 4;
    const g = c.getContext('2d')!;
    g.fillStyle = '#c9ced7';
    g.fillRect(0, 0, 4, 4);
    // synchronous data is not available from toBlob; a 1x1 transparent GIF as a blob is built by hand
    const bytes = Uint8Array.from(
      atob('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'),
      (ch) => ch.charCodeAt(0),
    );
    placeholder = URL.createObjectURL(new Blob([bytes], { type: 'image/gif' }));
  }
  return placeholder;
}

function mockImageUrl(kind: ImageKind, id: number, gen: number): string {
  const exact = urlExact.get(`${id}/${kind}/${kind === 'src' ? 0 : gen}`);
  return exact ?? urlLatest.get(`${id}/${kind}`) ?? ensurePlaceholder();
}

async function publish(id: number, kind: ImageKind, gen: number, canvas: HTMLCanvasElement): Promise<void> {
  const url = await canvasToBlobUrl(canvas);
  const key = kind === 'src' ? `${id}/src/0` : `${id}/${kind}/${gen}`;
  const prev = urlExact.get(`${id}/${kind}/${gen - 2}`);
  urlExact.set(key, url);
  urlLatest.set(`${id}/${kind}`, url);
  if (prev && kind !== 'src') {
    // Old generations are only needed for a moment (an <img> that has not swapped yet).
    setTimeout(() => URL.revokeObjectURL(prev), 8000);
    urlExact.delete(`${id}/${kind}/${gen - 2}`);
  }
}

/** (Re)renders the result and thumbnail for the current state of an item and publishes blob URLs. */
async function renderItem(m: MockItem): Promise<void> {
  if (!m.src) return;
  const v = m.view;
  const edit = v.edit ?? defaultEdit(insetQuad(0.05));
  const result = renderResult(m.src, edit);
  const thumbSource = m.failedUntouched ? m.src : result;
  await publish(v.id, 'result', v.gen, result);
  await publish(v.id, 'thumb', v.gen, scaleToLongEdge(thumbSource, 256));
}

// ---------------------------------------------------------------------------------------------- items
function snapshot(m: MockItem): ItemView {
  const v = m.view;
  const cur = m.history[m.cursor];
  const next = m.history[m.cursor + 1];
  const auto = v.autoEdit;
  const editedNow = !!auto && (cur.drawn || !editsEqual(cur.edit, auto));
  return {
    ...v,
    edit: v.edit ? cloneEdit(cur.edit) : null,
    autoEdit: auto ? cloneEdit(auto) : null,
    confidence: v.confidence ? { ...v.confidence, reasons: v.confidence.reasons.map((r) => ({ ...r })) } : null,
    edited: v.status === 'ready' && editedNow,
    saved: v.saved ? { ...v.saved } : null,
    canUndo: m.cursor > 0,
    canRedo: !!next,
    undoLabel: m.cursor > 0 ? cur.label : null,
    redoLabel: next ? next.label : null,
  };
}

function editsEqual(a: Edit, b: Edit): boolean {
  const eps = 1e-6;
  return (
    a.quarterTurns === b.quarterTurns &&
    Math.abs(a.fineDeg - b.fineDeg) < eps &&
    a.quad.every((p, i) => Math.abs(p.x - b.quad[i].x) < eps && Math.abs(p.y - b.quad[i].y) < eps)
  );
}

function sync(m: MockItem): ItemView {
  m.view = { ...m.view, ...snapshot(m) };
  return snapshot(m);
}

function getItem(id: number): MockItem {
  const m = items.get(id);
  if (!m) throw new Error(`no item ${id}`);
  return m;
}

function newAnalysingItem(name: string): MockItem {
  const id = nextId++;
  const m: MockItem = {
    view: {
      id,
      name,
      width: 0,
      height: 0,
      status: 'analysing',
      error: null,
      edit: null,
      autoEdit: null,
      confidence: null,
      gen: 0,
      edited: false,
      saved: null,
      dirtySinceSave: false,
      canUndo: false,
      canRedo: false,
      undoLabel: null,
      redoLabel: null,
    },
    src: null,
    spec: null,
    history: [],
    cursor: 0,
    failedUntouched: false,
  };
  items.set(id, m);
  return m;
}

async function finishAnalysis(
  m: MockItem,
  src: HTMLCanvasElement,
  spec: SceneSpec | null,
  width: number,
  height: number,
  auto: Edit,
  confidence: Confidence,
): Promise<void> {
  m.src = src;
  m.spec = spec;
  m.history = [{ edit: cloneEdit(auto), label: 'Auto', drawn: false }];
  m.cursor = 0;
  m.failedUntouched = confidence.forced === 'failed' || confidence.score < 0.6;
  m.view = {
    ...m.view,
    width,
    height,
    status: 'ready',
    edit: cloneEdit(auto),
    autoEdit: cloneEdit(auto),
    confidence,
    gen: 1,
  };
  await publish(m.view.id, 'src', 1, src);
  await renderItem(m);
  emit('item-updated', sync(m));
}

async function failAnalysis(m: MockItem, error: ErrorCode): Promise<void> {
  m.view = { ...m.view, status: 'error', error, gen: 1 };
  emit('item-updated', sync(m));
}

// ---------------------------------------------------------------------------------------------- samples
interface SampleDef {
  name: string;
  kind: 'receipt' | 'document';
  score: number;
  forced: Confidence['forced'];
  reasons: Reason[];
  opts?: { aspect?: number; lowContrast?: boolean; invisible?: boolean; cutOffBottom?: boolean };
  noise?: number;
  weak?: 'top' | 'right' | 'bottom' | 'left';
  skinny?: boolean;
  error?: ErrorCode;
}

function sampleDefs(): SampleDef[] {
  const defs: SampleDef[] = [
    { name: 'IMG_0211.jpg', kind: 'receipt', score: 0.31, forced: 'failed', reasons: [{ code: 'NO_QUAD' }], opts: { invisible: true } },
    { name: 'IMG_0087.jpg', kind: 'receipt', score: 0.64, forced: null, reasons: [{ code: 'WEAK_EDGE', side: 'right' }], weak: 'right', noise: 0.012 },
    { name: 'IMG_0133.jpg', kind: 'document', score: 0.69, forced: null, reasons: [{ code: 'LOW_CONTRAST_EDGE' }], opts: { lowContrast: true }, noise: 0.015 },
    { name: 'IMG_0094.jpg', kind: 'document', score: 0.74, forced: null, reasons: [{ code: 'PARTIAL_FRAME' }], opts: { cutOffBottom: true } },
    { name: 'IMG_0178.jpg', kind: 'receipt', score: 0.79, forced: null, reasons: [{ code: 'ODD_ASPECT' }], opts: { aspect: 0.17 }, noise: 0.01 },
    { name: 'IMG_0302.jpg', kind: 'document', score: 0.84, forced: null, reasons: [{ code: 'WEAK_EDGE', side: 'top' }], weak: 'top', noise: 0.01 },
    { name: 'IMG_0045.jpg', kind: 'receipt', score: 0.88, forced: null, reasons: [{ code: 'WEAK_EDGE', side: 'left' }], weak: 'left', noise: 0.008 },
    { name: 'IMG_0120.jpg', kind: 'receipt', score: 0.91, forced: null, reasons: [{ code: 'WEAK_EDGE', side: 'bottom' }], weak: 'bottom', noise: 0.006 },
    { name: 'IMG_0156.jpg', kind: 'document', score: 0.93, forced: null, reasons: [{ code: 'LOW_CONTRAST_EDGE' }], opts: { lowContrast: true }, noise: 0.006 },
    { name: 'IMG_0388.jpg', kind: 'receipt', score: 0.5, forced: null, reasons: [], noise: 0.03 },
    { name: 'IMG_0402.jpg', kind: 'receipt', score: 0.97, forced: 'check', reasons: [{ code: 'ODD_ASPECT' }], opts: { aspect: 0.2 }, noise: 0.004 },
    { name: 'IMG_0415.jpg', kind: 'document', score: 0.8, forced: 'failed', reasons: [{ code: 'IMPLAUSIBLE_QUAD' }], skinny: true },
    { name: 'IMG_0999.png', kind: 'document', score: 0, forced: null, reasons: [], error: 'CORRUPT' },
  ];
  const goodScores = [0.955, 0.96, 0.962, 0.968, 0.97, 0.974, 0.978, 0.98, 0.983, 0.985, 0.988, 0.992];
  goodScores.forEach((score, i) => {
    defs.push({
      name: `IMG_${String(i + 1).padStart(4, '0')}.jpg`,
      kind: i % 3 === 2 ? 'document' : 'receipt',
      score,
      forced: null,
      reasons: [],
      noise: 0.003,
    });
  });
  // A deterministic shuffle so the grid does not arrive pre-sorted.
  const rnd = mulberry32(42);
  for (let i = defs.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [defs[i], defs[j]] = [defs[j], defs[i]];
  }
  return defs;
}

async function analyseSample(m: MockItem, d: SampleDef, seed: number): Promise<void> {
  if (d.error) {
    await failAnalysis(m, d.error);
    return;
  }
  const rnd = mulberry32(seed * 31 + 5);
  const spec = makeSpec(rnd, d.kind, { ...d.opts, seed });
  const src = drawScene(spec);
  let auto: Edit;
  if (d.forced === 'failed' && !d.skinny) auto = failedPlaceholderEdit();
  else if (d.skinny) auto = defaultEdit(skinnyQuad());
  else auto = defaultEdit(detectQuad(spec, rnd, d.noise ?? 0.005, d.weak));
  // Pretend the original is a 3.3x larger camera photo.
  await finishAnalysis(m, src, spec, Math.round(spec.w * 3.33), Math.round(spec.h * 3.33), auto, {
    score: d.score,
    forced: d.forced,
    reasons: d.reasons,
  });
}

async function runAnalysisQueue(jobs: (() => Promise<void>)[]): Promise<void> {
  for (let i = 0; i < jobs.length; i++) {
    await sleep(i < 3 ? 80 : 60 + Math.random() * 140);
    try {
      await jobs[i]();
    } catch (e) {
      console.error('[mock] analysis failed', e);
    }
  }
}

async function addSamples(): Promise<OpenSummary> {
  const defs = sampleDefs();
  const base = nextId;
  const created = defs.map((d) => newAnalysingItem(d.name));
  const ids = created.map((m) => m.view.id);
  const summary: OpenSummary = { added: ids.length, skipped: 0, ids, skippedReasons: [] };
  emit('items-added', summary);
  void runAnalysisQueue(created.map((m, i) => () => analyseSample(m, defs[i], base + i)));
  await sleep(40);
  return summary;
}

// ---------------------------------------------------------------------------------------------- user files
async function addFiles(files: File[]): Promise<OpenSummary> {
  const ok = files.filter((f) => /^image\/(jpeg|png)$/.test(f.type) || /\.(jpe?g|png)$/i.test(f.name));
  const skippedReasons: ErrorCode[] = files.filter((f) => !ok.includes(f)).map(() => 'UNSUPPORTED_FORMAT');
  const created = ok.map((f) => newAnalysingItem(f.name));
  const ids = created.map((m) => m.view.id);
  const summary: OpenSummary = { added: ids.length, skipped: skippedReasons.length, ids, skippedReasons };
  if (ids.length > 0) {
    emit('items-added', summary);
    void runAnalysisQueue(
      created.map((m, i) => async () => {
        try {
          const { canvas, width, height } = await canvasFromFile(ok[i]);
          const rnd = mulberry32(m.view.id * 97);
          const score = 0.62 + rnd() * 0.36;
          // Without a detector the mock proposes a slightly inset full frame.
          await finishAnalysis(m, canvas, null, width, height, defaultEdit(insetQuad(0.04)), {
            score,
            forced: null,
            reasons: score < 0.9 ? [{ code: 'WEAK_EDGE', side: (['top', 'right', 'bottom', 'left'] as const)[Math.floor(rnd() * 4)] }] : [],
          });
        } catch {
          await failAnalysis(m, 'UNREADABLE');
        }
      }),
    );
  }
  return summary;
}

function chooseFiles(directory: boolean): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    input.multiple = true;
    input.accept = 'image/jpeg,image/png';
    if (directory) (input as HTMLInputElement & { webkitdirectory: boolean }).webkitdirectory = true;
    input.style.display = 'none';
    document.body.appendChild(input);
    const done = (files: File[]) => {
      input.remove();
      resolve(files);
    };
    input.addEventListener('change', () => done(Array.from(input.files ?? [])));
    input.addEventListener('cancel', () => done([]));
    input.click();
  });
}

// ---------------------------------------------------------------------------------------------- edits
function pushHistory(m: MockItem, edit: Edit, label: string, drawn: boolean): void {
  m.history = m.history.slice(0, m.cursor + 1);
  m.history.push({ edit: cloneEdit(edit), label, drawn });
  m.cursor = m.history.length - 1;
}

async function commit(m: MockItem): Promise<ItemView> {
  m.view = { ...m.view, edit: cloneEdit(m.history[m.cursor].edit), gen: m.view.gen + 1 };
  if (m.view.saved) m.view = { ...m.view, dirtySinceSave: true };
  const now = sync(m);
  const c = now.confidence;
  m.failedUntouched = !!c && (c.forced === 'failed' || c.score < 0.6) && !now.edited;
  await renderItem(m);
  return sync(m);
}

const api: Api = {
  async launchInfo(): Promise<LaunchInfo> {
    return {
      token: 'mock',
      version: '0.0.0-mock',
      platform: navigator.userAgent.includes('Windows') ? 'windows' : navigator.userAgent.includes('Mac') ? 'macos' : 'linux',
      backupsLocation: '%LOCALAPPDATA%\\AutoCrop\\backups',
    };
  },
  async pickFiles() {
    return addFiles(await chooseFiles(false));
  },
  async pickFolder(_includeSubfolders: boolean) {
    return addFiles(await chooseFiles(true));
  },
  addSamples,
  async listItems() {
    return [...items.values()].map(snapshot);
  },
  async setEdit(id, edit, phase, label) {
    const m = getItem(id);
    if (m.view.status !== 'ready') throw new Error('item is not ready');
    if (phase === 'live') {
      m.view = { ...m.view, edit: cloneEdit(edit) };
      return { ...snapshot(m), edit: cloneEdit(edit) };
    }
    await sleep(30 + Math.random() * 50);
    pushHistory(m, edit, label, m.history[m.cursor].drawn);
    const v = await commit(m);
    emit('item-updated', v);
    return v;
  },
  async undo(id) {
    const m = getItem(id);
    await sleep(30);
    if (m.cursor > 0) m.cursor--;
    const v = await commit(m);
    emit('item-updated', v);
    return v;
  },
  async redo(id) {
    const m = getItem(id);
    await sleep(30);
    if (m.cursor < m.history.length - 1) m.cursor++;
    const v = await commit(m);
    emit('item-updated', v);
    return v;
  },
  async resetToAuto(id) {
    const m = getItem(id);
    await sleep(40);
    if (m.view.autoEdit) pushHistory(m, m.view.autoEdit, 'Reset to auto', false);
    const v = await commit(m);
    emit('item-updated', v);
    return v;
  },
  async drawCrop(id) {
    const m = getItem(id);
    await sleep(40);
    pushHistory(m, failedPlaceholderEdit(), 'Draw crop', true);
    const v = await commit(m);
    emit('item-updated', v);
    return v;
  },
  async removeItems(ids) {
    for (const id of ids) items.delete(id);
  },
  async saveItems(ids: number[], target: SaveTarget, runName: string): Promise<SaveOutcome[]> {
    await sleep(350);
    saveCount++;
    const outcomes: SaveOutcome[] = [];
    const runId = `run-${Date.now().toString(36)}`;
    const files: BackupFile[] = [];
    const failIndex = !failedOnce && ids.length >= 6 ? 2 : -1;
    for (let i = 0; i < ids.length; i++) {
      const m = items.get(ids[i]);
      await sleep(25);
      if (!m || m.view.status !== 'ready') {
        outcomes.push({ id: ids[i], ok: false, error: 'INTERNAL', saved: null });
        continue;
      }
      if (i === failIndex) {
        failedOnce = true;
        outcomes.push({ id: ids[i], ok: false, error: 'SOURCE_CHANGED', saved: null });
        continue;
      }
      const copy = target === 'copy';
      const bytes = 2_000_000 + ((m.view.id * 7919) % 3_000_000);
      let backupId: string | null = null;
      if (!copy) {
        backupId = `${runId}/${files.length}`;
        files.push({
          id: backupId,
          name: m.view.name,
          displayPath: `C:\\Users\\you\\Pictures\\Receipts\\${m.view.name}`,
          originalBytes: bytes,
          outputBytes: Math.round(bytes * 0.32),
          changedSinceSaved: false,
          restored: false,
        });
      }
      m.view = {
        ...m.view,
        saved: { backupId, output: m.view.name, copy },
        dirtySinceSave: false,
        gen: m.view.gen + 1,
      };
      const v = sync(m);
      outcomes.push({ id: m.view.id, ok: true, error: null, saved: v.saved });
      emit('item-updated', v);
    }
    if (files.length > 0) {
      const created = new Date();
      backups.unshift({
        id: runId,
        name: runName,
        createdAt: created.toISOString(),
        expiresAt: expiry(created.getTime()),
        fileCount: files.length,
        totalBytes: files.reduce((s, f) => s + f.originalBytes, 0),
        pinned: false,
        files,
      });
    }
    return outcomes;
  },
  async getSettings() {
    return { ...settings };
  },
  async setSettings(s) {
    settings = { ...s };
    for (const r of backups) if (!r.pinned) r.expiresAt = expiry(Date.parse(r.createdAt));
    return { ...settings };
  },
  async listBackups(): Promise<BackupsView> {
    await sleep(60);
    const used = backups.reduce((s, r) => s + r.totalBytes, 0);
    return {
      location: '%LOCALAPPDATA%\\AutoCrop\\backups',
      usedBytes: used,
      freeBytes: 212 * 1024 ** 3,
      runs: backups.map((r) => ({ ...r, files: r.files.map((f) => ({ ...f })) })),
    };
  },
  async restoreFile(fileId: string, mode: RestoreMode): Promise<RestoreOutcome> {
    await sleep(120);
    const found = findFile(fileId);
    if (!found) return { ok: false, needsChoice: false, error: 'ORIGINAL_EXPIRED', restored: null };
    const { run, file } = found;
    if (run.expiresAt && Date.parse(run.expiresAt) < Date.now()) {
      return { ok: false, needsChoice: false, error: 'ORIGINAL_EXPIRED', restored: null };
    }
    if (mode === 'auto' && file.changedSinceSaved) return { ok: false, needsChoice: true, error: null, restored: null };
    const dot = file.name.lastIndexOf('.');
    const copyName = dot > 0 ? `${file.name.slice(0, dot)} (restored)${file.name.slice(dot)}` : `${file.name} (restored)`;
    file.restored = true;
    file.changedSinceSaved = false;
    // If the file belongs to an item of this session, the original is back on disk.
    for (const m of items.values()) {
      if (m.view.saved?.backupId === file.id && mode !== 'as_copy') {
        m.view = { ...m.view, saved: null, dirtySinceSave: false, gen: m.view.gen + 1 };
        emit('item-updated', sync(m));
      }
    }
    return { ok: true, needsChoice: false, error: null, restored: mode === 'as_copy' ? copyName : file.name };
  },
  async restoreRun(runId: string): Promise<RestoreOutcome[]> {
    const run = backups.find((r) => r.id === runId);
    if (!run) return [];
    const out: RestoreOutcome[] = [];
    for (const f of run.files) {
      if (f.restored) continue;
      out.push(await api.restoreFile(f.id, 'auto'));
    }
    return out;
  },
  async pinRun(runId, pinned) {
    const run = backups.find((r) => r.id === runId);
    if (!run) return;
    run.pinned = pinned;
    run.expiresAt = pinned ? null : expiry(Date.parse(run.createdAt));
  },
  async purgeNow() {
    await sleep(100);
    const before = backups.length;
    for (let i = backups.length - 1; i >= 0; i--) {
      const r = backups[i];
      if (!r.pinned && r.expiresAt && Date.parse(r.expiresAt) < Date.now()) backups.splice(i, 1);
    }
    return before - backups.length;
  },
  async openBackupsFolder() {
    console.info('[mock] open backups folder');
  },
};

// ---------------------------------------------------------------------------------------------- backups
function expiry(createdMs: number): string | null {
  return settings.retentionDays === null ? null : new Date(createdMs + settings.retentionDays * DAY).toISOString();
}

function mkFiles(runId: string, names: string[], changed: number[] = []): BackupFile[] {
  return names.map((name, i) => ({
    id: `${runId}/${i}`,
    name,
    displayPath: `C:\\Users\\you\\Pictures\\${name.startsWith('scan') ? 'Scans' : 'Receipts\\September'}\\${name}`,
    originalBytes: 3_400_000 + i * 410_000,
    outputBytes: 1_100_000 + i * 90_000,
    changedSinceSaved: changed.includes(i),
    restored: false,
  }));
}

function seedBackups(): BackupRun[] {
  const now = Date.now();
  const a = mkFiles('seed-a', ['IMG_0007.jpg', 'IMG_0133.jpg', 'IMG_0094.jpg'], [1]);
  const b = mkFiles('seed-b', ['scan_01.jpg', 'scan_02.jpg']);
  const c = mkFiles('seed-c', ['IMG_5501.jpg', 'IMG_5502.jpg', 'IMG_5503.jpg', 'IMG_5504.jpg']);
  const total = (f: BackupFile[]) => f.reduce((s, x) => s + x.originalBytes, 0);
  return [
    {
      id: 'seed-a',
      name: 'Sep-receipts',
      createdAt: new Date(now - 2 * DAY).toISOString(),
      expiresAt: new Date(now + 28 * DAY).toISOString(),
      fileCount: a.length,
      totalBytes: total(a),
      pinned: false,
      files: a,
    },
    {
      id: 'seed-b',
      name: 'Flatbed',
      createdAt: new Date(now - 14 * DAY).toISOString(),
      expiresAt: null,
      fileCount: b.length,
      totalBytes: total(b),
      pinned: true,
      files: b,
    },
    {
      id: 'seed-c',
      name: 'Old receipts (expired)',
      createdAt: new Date(now - 40 * DAY).toISOString(),
      expiresAt: new Date(now - 10 * DAY).toISOString(),
      fileCount: c.length,
      totalBytes: total(c),
      pinned: false,
      files: c,
    },
  ];
}

const backups: BackupRun[] = seedBackups();

function findFile(fileId: string): { run: BackupRun; file: BackupFile } | null {
  for (const run of backups) {
    const file = run.files.find((f) => f.id === fileId);
    if (file) return { run, file };
  }
  return null;
}

// ---------------------------------------------------------------------------------------------- export
export const mockImpl: BackendImpl = {
  api,
  imageUrl: mockImageUrl,
  async on<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): Promise<Unlisten> {
    listeners[event].add(cb);
    return () => {
      listeners[event].delete(cb);
    };
  },
  dropFiles: addFiles,
};

export const mockStats = () => ({ saveCount, items: items.size });
