// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// In-memory implementation of the `Api` contract, used automatically when the UI runs in a plain browser
// (no `window.__TAURI_INTERNALS__`). It generates synthetic receipt, document and multi-item scanner-bed
// photos with canvas, simulates asynchronous analysis (items appear "analysing", then "ready"), keeps a real
// edit history of a multi-crop edit state (scan-model.ts, the same rules as crates/core/src/items.rs), applies
// the engine's hold rule when "saving", and serves the backups API from memory. Image URLs are blob: URLs.
//
// Deliberate demo behaviour worth knowing:
//  * the first save of six or more items fails exactly one item with SOURCE_CHANGED so the per-file failure row
//    and "Retry failed" can be seen; the retry succeeds;
//  * `scan_locked.jpg` saves its set but cannot be removed (SAVED_SOURCE_IN_USE); `scan_two.jpg` collides with
//    a file called scan_two_01.jpg, so its set is saved as `scan_two (2)_01.jpg`;
//  * `ledger_pages.tif` (multi-page) and `holiday.webp` are open-only: Replace says NOT_REPLACEABLE;
//  * backups start with four runs: one expired, one file "changed since saved", and one split scan with
//    derived files in every state;
//  * `window.__autoCropMock.failNextSave(code)` forces the next save to fail with that code (for testing).

import {
  bedDetections,
  bedLayout,
  bedSingle,
  canvasFromFile,
  canvasToBlobUrl,
  canvasToBlobUrlSync,
  defaultEdit,
  detectQuad,
  drawBed,
  drawCurvedScene,
  drawScene,
  failedPlaceholderEdit,
  insetQuad,
  makeSpec,
  mulberry32,
  renderCurvedResult,
  renderResult,
  scaleToLongEdge,
  skinnyQuad,
} from './mock-scene.ts';
import { curveQuery, validateCurves } from './curve.ts';
import { curvedDetectedQuad } from './curved-model.ts';
import { gestureKey, pushEntry } from './history.ts';
import type { BedSpec, SceneSpec } from './mock-scene.ts';
import { cloneEdit, type Quad } from './quad.ts';
import {
  ItemOpError,
  addCrop,
  angleCrop,
  clearCurves as clearCurvesState,
  cloneState,
  cropViews,
  curveCrop,
  cutCrop,
  editCrop,
  emptyState,
  flipCrop,
  hasCurved,
  included,
  isEdited,
  mergeCrops,
  moveCrop,
  outputName,
  outputNames,
  redetect as redetectState,
  renderSignature,
  revertCrop,
  setCurves as setCurvesState,
  setInclude,
  splitView,
  toEdit,
  triage,
  turnCrop,
  useReadingOrder,
  type Dims,
  type ModelCrop,
  type ScanState,
} from './scan-model.ts';
import { defaultSettings } from './settings-defaults.ts';
import type { BackendImpl, Unlisten } from './backend.ts';
import type {
  Api,
  BackupFile,
  BackupRun,
  BackupsView,
  Confidence,
  CropImageKind,
  CurvePreviewKind,
  CurveSet,
  DerivedFile,
  Edit,
  ErrorCode,
  Events,
  ImageKind,
  ItemView,
  LaunchInfo,
  OpenSummary,
  Reason,
  RedetectResult,
  RestoreMode,
  RestoreOutcome,
  SaveOutcome,
  SaveTarget,
  SessionStep,
  Settings,
  SplitPatch,
} from './types.ts';

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
const DAY = 86_400_000;

interface HistoryEntry {
  state: ScanState;
  label: string;
  /** `<gesture>:<crop>`: an edit under the same id as the entry at the cursor is merged into it (one undo step). */
  gesture?: string;
}

interface MockItem {
  id: number;
  name: string;
  stem: string;
  ext: string;
  status: ItemView['status'];
  error: ErrorCode | null;
  width: number;
  height: number;
  gen: number;
  src: HTMLCanvasElement | null;
  spec: SceneSpec | null;
  bed: BedSpec | null;
  hist: HistoryEntry[];
  cursor: number;
  auto: ScanState | null;
  /** The render signature the person accepted (a held split is saved only while the state still has it). */
  accepted: string | null;
  saved: { backupId: string | null; output: string; outputs: string[]; copy: boolean; sig: string } | null;
  groupSaved: boolean;
  openOnly: string | null;
}

const items = new Map<number, MockItem>();
let nextId = 1;
let settings: Settings = defaultSettings();
let saveCount = 0;
let failedOnce = false;
let forcedFailure: ErrorCode | null = null;
/** File names that "already exist" next to the scans, to show a collision. */
const existingNames = new Set<string>(['scan_two_01.jpg']);

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
const urlCrop = new Map<string, string>();
let placeholder = '';

function ensurePlaceholder(): string {
  if (!placeholder) {
    // a 1x1 transparent GIF as a blob (synchronous data is not available from toBlob)
    const bytes = Uint8Array.from(atob('R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7'), (ch) => ch.charCodeAt(0));
    placeholder = URL.createObjectURL(new Blob([bytes], { type: 'image/gif' }));
  }
  return placeholder;
}

function mockImageUrl(kind: ImageKind, id: number, gen: number): string {
  const exact = urlExact.get(`${id}/${kind}/${kind === 'src' ? 0 : gen}`);
  return exact ?? urlLatest.get(`${id}/${kind}`) ?? ensurePlaceholder();
}

function mockCropImageUrl(kind: CropImageKind, id: number, crop: number, renderKey: string): string {
  return urlCrop.get(`${id}/${crop}/${kind}/${renderKey}`) ?? ensurePlaceholder();
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

function flipped(c: HTMLCanvasElement): HTMLCanvasElement {
  const out = document.createElement('canvas');
  out.width = c.width;
  out.height = c.height;
  const g = out.getContext('2d')!;
  g.translate(c.width, 0);
  g.scale(-1, 1);
  g.drawImage(c, 0, 0);
  return out;
}

function renderCrop(src: HTMLCanvasElement, c: ModelCrop): HTMLCanvasElement {
  // A curved page is flattened through its curves (turns and mirror included); a quad goes through the homography.
  if (c.curves) return renderCurvedResult(src, { ...c.curves, quarterTurns: c.quarterTurns, mirror: c.mirror });
  const out = renderResult(src, toEdit(c));
  return c.mirror ? flipped(out) : out;
}

const current = (m: MockItem): ScanState => m.hist[m.cursor].state;

/** Whole-scan and per-crop images for the current state. Per-crop images are cached by render key. */
async function renderItem(m: MockItem): Promise<void> {
  if (!m.src) return;
  const state = current(m);
  const inc = included(state);
  const first = inc[0] ?? state.crops[0];
  const split = inc.length >= 2;
  const failedUntouched = !!first && first.confidence !== null && bandFailed(first) && !isEdited(state, m.auto);
  const firstResult = first ? renderCrop(m.src, first) : m.src;
  await publish(m.id, 'result', m.gen, firstResult);
  await publish(m.id, 'thumb', m.gen, scaleToLongEdge(split || failedUntouched || !first ? m.src : firstResult, 256));
  const views = cropViews(state, { stem: m.stem, ext: m.ext, baseline: m.auto });
  for (const v of views) {
    const c = state.crops.find((x) => x.id === v.id)!;
    const rk = `${m.id}/${v.id}/result/${v.renderKey}`;
    if (urlCrop.has(rk)) continue;
    const res = c === first ? firstResult : renderCrop(m.src, c);
    urlCrop.set(rk, await canvasToBlobUrl(res));
    urlCrop.set(`${m.id}/${v.id}/thumb/${v.renderKey}`, await canvasToBlobUrl(scaleToLongEdge(res, 256)));
  }
}

function bandFailed(c: ModelCrop): boolean {
  return !!c.confidence && (c.confidence.forced === 'failed' || c.confidence.score < 0.6);
}

// ---------------------------------------------------------------------------------------------- views
function viewOf(m: MockItem, shown?: ScanState): ItemView {
  const state = shown ?? (m.hist.length ? current(m) : emptyState());
  const ready = m.status === 'ready';
  const sig = ready ? renderSignature(state) : '';
  const crops = m.hist.length ? cropViews(state, { stem: m.stem, ext: m.ext, baseline: m.auto, accepted: m.accepted === sig }) : [];
  const first = crops.find((c) => c.include) ?? crops[0] ?? null;
  const cur = m.hist[m.cursor];
  const next = m.hist[m.cursor + 1];
  return {
    id: m.id,
    name: m.name,
    width: m.width,
    height: m.height,
    status: m.status,
    error: m.error,
    edit: first?.edit ? cloneEdit(first.edit) : null,
    autoEdit: first?.autoEdit ? cloneEdit(first.autoEdit) : null,
    confidence: first?.confidence ? { ...first.confidence, reasons: first.confidence.reasons.map((r) => ({ ...r })) } : null,
    gen: m.gen,
    edited: ready && isEdited(state, m.auto),
    saved: m.saved ? { backupId: m.saved.backupId, output: m.saved.output, copy: m.saved.copy, outputs: [...m.saved.outputs] } : null,
    dirtySinceSave: !!m.saved && m.saved.sig !== sig,
    canUndo: ready && m.cursor > 0,
    canRedo: ready && !!next,
    undoLabel: ready && m.cursor > 0 ? cur.label : null,
    redoLabel: ready && next ? next.label : null,
    crops,
    split: ready ? splitView(state, m.accepted === sig, m.groupSaved) : null,
    historyPosition: m.cursor,
    openOnly: ready ? m.openOnly : null,
  };
}

function getItem(id: number): MockItem {
  const m = items.get(id);
  if (!m) throw new Error(`no item ${id}`);
  return m;
}

function ready(id: number): MockItem {
  const m = getItem(id);
  if (m.status !== 'ready') throw 'INTERNAL';
  return m;
}

function newAnalysingItem(name: string): MockItem {
  const id = nextId++;
  const dot = name.lastIndexOf('.');
  const m: MockItem = {
    id,
    name,
    stem: dot > 0 ? name.slice(0, dot) : name,
    ext: dot > 0 ? name.slice(dot + 1).toLowerCase().replace('jpeg', 'jpg') : 'jpg',
    status: 'analysing',
    error: null,
    width: 0,
    height: 0,
    gen: 0,
    src: null,
    spec: null,
    bed: null,
    hist: [],
    cursor: 0,
    auto: null,
    accepted: null,
    saved: null,
    groupSaved: false,
    openOnly: null,
  };
  items.set(id, m);
  return m;
}

async function finishAnalysis(m: MockItem, src: HTMLCanvasElement, width: number, height: number, auto: ScanState): Promise<void> {
  m.src = src;
  m.auto = cloneState(auto);
  m.hist = [{ state: cloneState(auto), label: 'Auto' }];
  m.cursor = 0;
  m.width = width;
  m.height = height;
  m.status = 'ready';
  m.gen = 1;
  await publish(m.id, 'src', 1, src);
  await renderItem(m);
  emit('item-updated', viewOf(m));
}

async function failAnalysis(m: MockItem, error: ErrorCode): Promise<void> {
  m.status = 'error';
  m.error = error;
  m.gen = 1;
  emit('item-updated', viewOf(m));
}

const dimsOf = (m: MockItem): Dims => [m.src?.width ?? 1000, m.src?.height ?? 1000];

// ---------------------------------------------------------------------------------------------- detection
function documentState(edit: Edit, confidence: Confidence): ScanState {
  let s = emptyState(settings.splitPolicy, settings.splitProfile);
  s = addCrop(s, edit.quad, 'auto', [1000, 1000], confidence).state;
  return s;
}

function bedCrops(spec: BedSpec): { quad: Quad; confidence: Confidence }[] {
  return bedDetections(spec).map((d) => ({
    quad: d.quad,
    confidence: { score: d.score, forced: null, reasons: d.reasons.map((r) => ({ ...r })) as Reason[] },
  }));
}

/** The initial crops of a bed scan under `policy`: several, or one around everything. */
function bedState(spec: BedSpec, policy: ScanState['policy'], profile: ScanState['profile'], dims: Dims): ScanState {
  let s = emptyState(policy, profile);
  if (policy === 'never') {
    s = addCrop(s, bedSingle(spec), 'auto', dims, { score: 0.97, forced: null, reasons: [] }).state;
    return s;
  }
  for (const d of bedCrops(spec)) s = addCrop(s, d.quad, 'auto', dims, d.confidence).state;
  // Candidates the detector looked at and rejected: excluded, restorable as items.
  for (const dust of spec.dust) {
    const r = addCrop(s, dust, 'auto', dims, { score: 0.41, forced: null, reasons: [] });
    s = setInclude(r.state, r.id, false);
  }
  return s;
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
  bed?: 'albums' | 'receipts' | 'two' | 'locked';
  openOnly?: string;
  /** The curved sample: a receipt whose four edges are bent (curved-model.ts). */
  curved?: boolean;
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
  const none = { score: 0.98, forced: null, reasons: [] as Reason[], kind: 'document' as const };
  const multi: SampleDef[] = [
    { name: 'scan_albums.jpg', ...none, bed: 'albums' },
    { name: 'scan_receipts.jpg', ...none, bed: 'receipts' },
    { name: 'scan_two.jpg', ...none, bed: 'two' },
    { name: 'scan_locked.jpg', ...none, bed: 'locked' },
    { name: 'ledger_pages.tif', kind: 'document', score: 0.98, forced: null, reasons: [], noise: 0.004, openOnly: 'tiff.multi_page' },
    { name: 'holiday.webp', kind: 'receipt', score: 0.98, forced: null, reasons: [], noise: 0.004, openOnly: 'format.write_unavailable' },
  ];
  // The curved page first: it is the one to try the curved-edges editor on. The detector, like the real one, finds a
  // rough quad and flags the top and bottom edges.
  const curved: SampleDef = {
    name: 'receipt_curved.jpg',
    kind: 'receipt',
    score: 0.68,
    forced: 'check',
    reasons: [{ code: 'WEAK_EDGE', side: 'top' }, { code: 'WEAK_EDGE', side: 'bottom' }],
    curved: true,
  };
  return [curved, ...multi, ...defs];
}

async function analyseSample(m: MockItem, d: SampleDef, seed: number): Promise<void> {
  if (d.error) {
    await failAnalysis(m, d.error);
    return;
  }
  if (d.curved) {
    const src = drawCurvedScene();
    await finishAnalysis(
      m,
      src,
      src.width * 2,
      src.height * 2,
      documentState(defaultEdit(curvedDetectedQuad()), { score: d.score, forced: d.forced, reasons: d.reasons }),
    );
    return;
  }
  if (d.bed) {
    const spec = bedLayout(d.bed);
    m.bed = spec;
    const src = drawBed(spec);
    const dims: Dims = [src.width, src.height];
    await finishAnalysis(m, src, Math.round(src.width * 2.9), Math.round(src.height * 2.9), bedState(spec, settings.splitPolicy, settings.splitProfile, dims));
    return;
  }
  const rnd = mulberry32(seed * 31 + 5);
  const spec = makeSpec(rnd, d.kind, { ...d.opts, seed });
  const src = drawScene(spec);
  let edit: Edit;
  if (d.forced === 'failed' && !d.skinny) edit = failedPlaceholderEdit();
  else if (d.skinny) edit = defaultEdit(skinnyQuad());
  else edit = defaultEdit(detectQuad(spec, rnd, d.noise ?? 0.005, d.weak));
  m.spec = spec;
  m.openOnly = d.openOnly ?? null;
  // Pretend the original is a 3.3x larger camera photo.
  await finishAnalysis(
    m,
    src,
    Math.round(spec.w * 3.33),
    Math.round(spec.h * 3.33),
    documentState(edit, { score: d.score, forced: d.forced, reasons: d.reasons }),
  );
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
  const ids = created.map((m) => m.id);
  const summary: OpenSummary = { added: ids.length, skipped: 0, ids, skippedReasons: [] };
  emit('items-added', summary);
  void runAnalysisQueue(created.map((m, i) => () => analyseSample(m, defs[i], base + i)));
  await sleep(40);
  return summary;
}

// ---------------------------------------------------------------------------------------------- user files
const INPUT = ['jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff'];

async function addFiles(files: File[]): Promise<OpenSummary> {
  const extOf = (f: File) => f.name.slice(f.name.lastIndexOf('.') + 1).toLowerCase();
  const ok = files.filter((f) => INPUT.includes(extOf(f)));
  const skippedReasons: ErrorCode[] = files.filter((f) => !ok.includes(f)).map(() => 'UNSUPPORTED_FORMAT');
  const created = ok.map((f) => newAnalysingItem(f.name));
  const ids = created.map((m) => m.id);
  const summary: OpenSummary = { added: ids.length, skipped: skippedReasons.length, ids, skippedReasons };
  if (ids.length > 0) {
    emit('items-added', summary);
    void runAnalysisQueue(
      created.map((m, i) => async () => {
        try {
          const e = extOf(ok[i]);
          const rnd = mulberry32(m.id * 97);
          const score = 0.62 + rnd() * 0.36;
          const confidence: Confidence = {
            score,
            forced: null,
            reasons: score < 0.9 ? [{ code: 'WEAK_EDGE', side: (['top', 'right', 'bottom', 'left'] as const)[Math.floor(rnd() * 4)] }] : [],
          };
          // The browser cannot decode TIFF: a stand-in scene is shown, marked open-only like the real one.
          const { canvas, width, height } =
            e === 'tif' || e === 'tiff'
              ? (() => {
                  const c = drawScene(makeSpec(rnd, 'document', { seed: m.id }));
                  return { canvas: c, width: c.width * 3, height: c.height * 3 };
                })()
              : await canvasFromFile(ok[i]);
          if (e === 'webp') m.openOnly = 'format.write_unavailable';
          if (e === 'tif' || e === 'tiff') m.openOnly = 'format.write_unavailable';
          // Without a detector the mock proposes a slightly inset full frame.
          await finishAnalysis(m, canvas, width, height, documentState(defaultEdit(insetQuad(0.04)), confidence));
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
    input.accept = 'image/jpeg,image/png,image/webp,image/tiff';
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
/** The 1-based place of a crop for a history label: its output rank, or its place in the list while excluded. */
function who(state: ScanState, id: number): string {
  const inc = included(state);
  const r = inc.findIndex((c) => c.id === id);
  if (r >= 0) return String(r + 1);
  const i = state.crops.findIndex((c) => c.id === id);
  return i >= 0 ? String(i + 1) : `#${id}`;
}

function labelled(label: string, state: ScanState, id: number): string {
  return state.crops.length > 1 ? `${label} (item ${who(state, id)})` : label;
}

function refuse(e: unknown): never {
  if (e instanceof ItemOpError) throw e.code;
  throw e;
}

/** Applies `fn` to a copy of the current state and commits the result as ONE undo step. A refusal changes nothing. */
async function commitOp(m: MockItem, label: (before: ScanState) => string, fn: (s: ScanState) => ScanState, gesture?: string): Promise<ItemView> {
  const before = current(m);
  let next: ScanState;
  try {
    next = fn(before);
  } catch (e) {
    refuse(e);
  }
  await sleep(15 + Math.random() * 25);
  const text = label(before);
  const pushed = pushEntry(m.hist, m.cursor, { state: next, label: text.slice(0, 60), gesture });
  m.hist = pushed.hist;
  m.cursor = pushed.cursor;
  return bump(m);
}

async function bump(m: MockItem): Promise<ItemView> {
  m.gen++;
  await renderItem(m);
  const v = viewOf(m);
  emit('item-updated', v);
  return v;
}

async function seek(m: MockItem, position: number): Promise<ItemView> {
  m.cursor = Math.min(Math.max(0, position), m.hist.length - 1);
  return bump(m);
}

// ---- session history: one entry for a change made to many images (M10.19)
interface SessionCmd {
  label: string;
  moves: { id: number; before: number; after: number }[];
}
let sessionLog: SessionCmd[] = [];
let sessionCursor = 0;

async function sessionStep(undo: boolean): Promise<SessionStep | null> {
  if (undo ? sessionCursor === 0 : sessionCursor >= sessionLog.length) return null;
  const cmd = undo ? sessionLog[--sessionCursor] : sessionLog[sessionCursor++];
  const out: ItemView[] = [];
  for (const mv of cmd.moves) {
    const m = items.get(mv.id);
    if (m && m.status === 'ready') out.push(await seek(m, undo ? mv.before : mv.after));
  }
  return { label: cmd.label, items: out };
}

function detectionsFor(m: MockItem, policy: ScanState['policy']): { quad: Quad; confidence: Confidence | null }[] {
  if (m.bed) {
    if (policy === 'never') return [{ quad: bedSingle(m.bed), confidence: { score: 0.97, forced: null, reasons: [] } }];
    return bedCrops(m.bed);
  }
  // A single paper: the same crop whatever the policy.
  const base = m.auto?.crops[0];
  return base ? [{ quad: base.quad, confidence: base.confidence }] : [];
}

async function redetectOne(m: MockItem, patch: SplitPatch): Promise<ItemView> {
  const cur = current(m);
  const policy = patch.policy ?? cur.policy;
  const profile = patch.profile ?? cur.profile;
  const label = patch.policy === 'never' ? 'Treat as one item' : patch.policy ? 'Split into items' : 'Re-detect items';
  return commitOp(
    m,
    () => label,
    (s) => {
      const next = redetectState(s, detectionsFor(m, policy), dimsOf(m));
      next.policy = policy;
      next.profile = profile;
      return next;
    },
  );
}

const firstCrop = (s: ScanState): ModelCrop | undefined => included(s)[0] ?? s.crops[0];

const api: Api = {
  async launchInfo(): Promise<LaunchInfo> {
    return {
      token: 'mock',
      version: '0.0.0-mock',
      platform: navigator.userAgent.includes('Windows') ? 'windows' : navigator.userAgent.includes('Mac') ? 'macos' : 'linux',
      backupsLocation: '%LOCALAPPDATA%\\AutoCrop\\backups',
      inputExtensions: [...INPUT],
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
    return [...items.values()].map((m) => viewOf(m));
  },
  async setEdit(id, edit, phase, label) {
    const m = ready(id);
    const c = firstCrop(current(m));
    if (!c) throw 'NO_CROP';
    return api.setCropEdit(m.id, c.id, edit, phase, label, null);
  },
  async undo(id) {
    const m = ready(id);
    await sleep(25);
    return seek(m, m.cursor - 1);
  },
  async redo(id) {
    const m = ready(id);
    await sleep(25);
    return seek(m, m.cursor + 1);
  },
  async resetToAuto(id) {
    const m = ready(id);
    return commitOp(
      m,
      () => 'Reset to auto',
      () => cloneState(m.auto!),
    );
  },
  async drawCrop(id) {
    const m = ready(id);
    return commitOp(
      m,
      () => 'Draw crop',
      (s) => {
        const quad = failedPlaceholderEdit().quad;
        const c = firstCrop(s);
        if (!c) return addCrop(s, quad, 'manual', dimsOf(m)).state;
        const next = editCrop(s, c.id, { quad, quarterTurns: 0, fineDeg: 0 }, dimsOf(m));
        next.crops.find((x) => x.id === c.id)!.origin = 'manual';
        return next;
      },
    );
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
    const copy = target === 'copy';
    for (let i = 0; i < ids.length; i++) {
      const m = items.get(ids[i]);
      await sleep(25);
      const fail = (error: ErrorCode, notices: string[] = []) => outcomes.push({ id: ids[i], ok: false, error, saved: null, notes: [], notices });
      if (!m || m.status !== 'ready') {
        fail('INTERNAL');
        continue;
      }
      if (forcedFailure) {
        const code = forcedFailure;
        forcedFailure = null;
        fail(code);
        continue;
      }
      if (i === failIndex) {
        failedOnce = true;
        fail('SOURCE_CHANGED');
        continue;
      }
      const state = current(m);
      const inc = included(state);
      if (inc.length === 0) {
        fail('NO_CROP');
        continue;
      }
      if (!copy && m.openOnly) {
        fail('NOT_REPLACEABLE', [m.openOnly]);
        continue;
      }
      const split = inc.length >= 2;
      const curved = hasCurved(state);
      const sig = renderSignature(state);
      // A split scan and a curved page are held: Replace needs the person's acceptance of this exact state (a curved
      // page is never auto-saved). A copy overwrites nothing and needs none.
      if ((split || curved) && !copy) {
        const approved = !curved && settings.autoSaveSplits && triage(state).kind === 'approved';
        if (!(m.accepted === sig || approved)) {
          fail('HELD_FOR_REVIEW', [split ? 'split.held' : 'curved.held']);
          continue;
        }
      }
      // Output names: a copy of an open-only source is a PNG; a taken name moves the WHOLE set to `name (2)`.
      const ext = copy && m.openOnly ? 'png' : m.ext;
      let stem = m.stem;
      const plan = (s: string) => (split ? outputNames(s, ext, inc.length) : [outputName(s, ext, 1, 1)]);
      const ownOld = new Set(m.saved?.outputs ?? []);
      const notices: string[] = [];
      if (plan(stem).some((n) => existingNames.has(n) && !ownOld.has(n))) {
        stem = `${stem} (2)`;
      }
      const outputs = plan(stem);
      for (const n of outputs) existingNames.add(n);
      const bytes = 2_000_000 + ((m.id * 7919) % 3_000_000);
      let backupId: string | null = null;
      if (!copy) {
        backupId = `${runId}/${files.length}`;
        const derived: DerivedFile[] = split ? outputs.map((name, k) => ({ name, bytes: Math.round((bytes * 0.3) / inc.length) + k * 1000, state: 'unchanged' })) : [];
        files.push({
          id: backupId,
          name: m.name,
          displayPath: `C:\\Users\\you\\Pictures\\Receipts\\${m.name}`,
          originalBytes: bytes,
          outputBytes: Math.round(bytes * 0.32),
          changedSinceSaved: false,
          restored: false,
          kind: split ? 'OneToN' : 'OneToOne',
          derived,
        });
      }
      m.saved = { backupId, output: outputs[0], outputs: split ? outputs : [], copy, sig };
      m.groupSaved = split;
      m.gen++;
      const notes: ErrorCode[] = split && !copy && m.name === 'scan_locked.jpg' ? ['SAVED_SOURCE_IN_USE'] : [];
      const v = viewOf(m);
      outcomes.push({ id: m.id, ok: true, error: null, saved: v.saved, notes, notices });
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
    // The engine takes the whole object: anything missing resets to its default.
    settings = { ...defaultSettings(), ...s };
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
      runs: backups.map((r) => ({ ...r, files: r.files.map((f) => ({ ...f, derived: f.derived.map((d) => ({ ...d })) })) })),
    };
  },
  async restoreFile(fileId: string, mode: RestoreMode): Promise<RestoreOutcome> {
    return api.restoreFileDerived(fileId, mode, 'keep');
  },
  async restoreRun(runId: string): Promise<RestoreOutcome[]> {
    return api.restoreRunDerived(runId, 'keep');
  },
  async restoreFileDerived(fileId, mode, derived): Promise<RestoreOutcome> {
    await sleep(120);
    const found = findFile(fileId);
    if (!found) return { ok: false, needsChoice: false, error: 'ORIGINAL_EXPIRED', restored: null, derived: [] };
    const { run, file } = found;
    if (run.expiresAt && Date.parse(run.expiresAt) < Date.now()) {
      return { ok: false, needsChoice: false, error: 'ORIGINAL_EXPIRED', restored: null, derived: [] };
    }
    if (mode === 'auto' && file.changedSinceSaved) return { ok: false, needsChoice: true, error: null, restored: null, derived: [] };
    const dot = file.name.lastIndexOf('.');
    const copyName = dot > 0 ? `${file.name.slice(0, dot)} (restored)${file.name.slice(dot)}` : `${file.name} (restored)`;
    if (derived === 'remove') {
      // Only files still exactly as saved are moved (to the backup store, never deleted); edited or missing ones stay.
      for (const d of file.derived) if (d.state === 'unchanged') d.state = 'removed';
    }
    file.restored = true;
    file.changedSinceSaved = false;
    // If the file belongs to an item of this session, the original is back on disk.
    for (const m of items.values()) {
      if (m.saved?.backupId === file.id && mode !== 'as_copy') {
        m.saved = null;
        m.groupSaved = false;
        m.gen++;
        emit('item-updated', viewOf(m));
      }
    }
    return { ok: true, needsChoice: false, error: null, restored: mode === 'as_copy' ? copyName : file.name, derived: file.derived.map((d) => ({ ...d })) };
  },
  async restoreRunDerived(runId, derived): Promise<RestoreOutcome[]> {
    const run = backups.find((r) => r.id === runId);
    if (!run) return [];
    const out: RestoreOutcome[] = [];
    for (const f of run.files) {
      if (f.restored) continue;
      out.push(await api.restoreFileDerived(f.id, 'auto', derived));
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

  // ---- multi-item operations ------------------------------------------------------------------------
  async setCropEdit(id, crop, edit, phase, label, gesture) {
    const m = ready(id);
    if (phase === 'live') {
      const c = current(m).crops.find((x) => x.id === crop);
      if (!c || c.curves) throw 'ITEM_OP';
      return { ...viewOf(m), edit: cloneEdit(edit) };
    }
    const dims = dimsOf(m);
    return commitOp(
      m,
      (b) => labelled(label, b, crop),
      (s) => editCrop(s, crop, edit, dims),
      gestureKey(gesture, crop),
    );
  },
  async addCrop(id, quad, at) {
    const m = ready(id);
    const dims = dimsOf(m);
    return commitOp(
      m,
      () => 'Add item',
      (s) => {
        let q: Quad;
        if (quad) q = quad;
        else if (at) {
          const x0 = Math.min(0.8, Math.max(0, at.x - 0.1));
          const x1 = Math.max(0.2, Math.min(1, at.x + 0.1));
          const y0 = Math.min(0.8, Math.max(0, at.y - 0.1));
          const y1 = Math.max(0.2, Math.min(1, at.y + 0.1));
          q = [
            { x: x0, y: y0 },
            { x: x1, y: y0 },
            { x: x1, y: y1 },
            { x: x0, y: y1 },
          ];
        } else q = insetQuad(0.2);
        return addCrop(s, q, 'manual', dims).state;
      },
    );
  },
  async removeCrop(id, crop) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Remove', b, crop), (s) => setInclude(s, crop, false));
  },
  async restoreCrop(id, crop) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Restore', b, crop), (s) => setInclude(s, crop, true));
  },
  async mergeCrops(id, crops) {
    const m = ready(id);
    const dims = dimsOf(m);
    return commitOp(m, () => `Merge ${new Set(crops).size} items`, (s) => mergeCrops(s, crops, dims).state);
  },
  async cutCrop(id, crop, cut) {
    const m = ready(id);
    const dims = dimsOf(m);
    return commitOp(m, (b) => labelled('Cut', b, crop), (s) => cutCrop(s, crop, cut, dims).state);
  },
  async moveCrop(id, crop, toIndex) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Move', b, crop), (s) => moveCrop(s, crop, toIndex));
  },
  async useReadingOrder(id) {
    const m = ready(id);
    return commitOp(m, () => 'Reading order', (s) => useReadingOrder(s));
  },
  async turnCrop(id, crop, clockwise) {
    const m = ready(id);
    return commitOp(m, (b) => labelled(clockwise ? 'Turn right' : 'Turn left', b, crop), (s) => turnCrop(s, crop, clockwise));
  },
  async setCropAngle(id, crop, deg, gesture) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Straighten', b, crop), (s) => angleCrop(s, crop, deg), gestureKey(gesture, crop));
  },
  async flipCrop(id, crop) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Flip', b, crop), (s) => flipCrop(s, crop));
  },
  async revertCrop(id, crop, to) {
    const m = ready(id);
    const baseline = to.kind === 'auto' ? m.auto : (m.hist[to.position]?.state ?? null);
    if (!baseline) throw 'ITEM_OP';
    return commitOp(m, (b) => labelled('Revert', b, crop), (s) => revertCrop(s, crop, baseline));
  },
  async redetect(id, patch) {
    return redetectOne(ready(id), patch);
  },
  async redetectMany(ids, patch): Promise<RedetectResult[]> {
    const label = patch.policy === 'never' ? 'Treat as one item' : patch.policy ? 'Split into items' : 'Re-detect items';
    const cmd: SessionCmd = { label: `${label} (${ids.length} images)`, moves: [] };
    const out: RedetectResult[] = [];
    for (const id of ids) {
      try {
        const m = ready(id);
        const before = m.cursor;
        const view = await redetectOne(m, patch);
        cmd.moves.push({ id, before, after: m.cursor });
        out.push({ id, view, error: null });
      } catch (e) {
        out.push({ id, view: null, error: typeof e === 'string' ? (e as ErrorCode) : 'INTERNAL' });
      }
    }
    sessionLog = sessionLog.slice(0, sessionCursor);
    sessionLog.push(cmd);
    sessionCursor = sessionLog.length;
    return out;
  },
  sessionUndo: () => sessionStep(true),
  sessionRedo: () => sessionStep(false),
  async acceptScan(id) {
    const m = ready(id);
    m.accepted = renderSignature(current(m));
    const v = viewOf(m);
    emit('item-updated', v);
    return v;
  },
  async unacceptScan(id) {
    const m = ready(id);
    m.accepted = null;
    const v = viewOf(m);
    emit('item-updated', v);
    return v;
  },

  // ---- curved pages -------------------------------------------------------------------------------------
  async curveFromQuad(id, crop) {
    const m = ready(id);
    const dims = dimsOf(m);
    return commitOp(m, (b) => labelled('Curve edges', b, crop), (s) => curveCrop(s, crop, dims));
  },
  async setCurves(id, crop, curves, phase, label, gesture) {
    const m = ready(id);
    if (phase === 'live') {
      // Validates and shows what the view would be; nothing is recorded.
      let next: ScanState;
      try {
        next = setCurvesState(current(m), crop, curves);
      } catch (e) {
        refuse(e);
      }
      return viewOf(m, next);
    }
    return commitOp(m, (b) => labelled(label || 'Bend edges', b, crop), (s) => setCurvesState(s, crop, curves), gestureKey(gesture, crop));
  },
  async clearCurves(id, crop) {
    const m = ready(id);
    return commitOp(m, (b) => labelled('Straighten edges', b, crop), (s) => clearCurvesState(s, crop));
  },
};

// ---------------------------------------------------------------------------------------------- curve previews
const previews = new Map<string, string>();

/** The mock's `preview_curves`: renders the candidate through the canvas flattener; nothing is committed. */
function mockCurvePreviewUrl(kind: CurvePreviewKind, id: number, _crop: number, curves: CurveSet): string {
  const m = items.get(id);
  if (!m?.src || validateCurves(curves) !== null) return ensurePlaceholder();
  const key = `${id}/${kind}/${curveQuery(curves)}`;
  const hit = previews.get(key);
  if (hit) return hit;
  const url = canvasToBlobUrlSync(renderCurvedResult(m.src, curves, kind === 'thumb' ? 256 : 760));
  previews.set(key, url);
  if (previews.size > 40) {
    const oldest = previews.keys().next().value as string;
    const gone = previews.get(oldest);
    previews.delete(oldest);
    if (gone) setTimeout(() => URL.revokeObjectURL(gone), 8000);
  }
  return url;
}

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
    kind: 'OneToOne',
    derived: [],
  }));
}

function seedBackups(): BackupRun[] {
  const now = Date.now();
  const a = mkFiles('seed-a', ['IMG_0007.jpg', 'IMG_0133.jpg', 'IMG_0094.jpg'], [1]);
  const b = mkFiles('seed-b', ['scan_01.jpg', 'scan_02.jpg']);
  const c = mkFiles('seed-c', ['IMG_5501.jpg', 'IMG_5502.jpg', 'IMG_5503.jpg', 'IMG_5504.jpg']);
  const album: BackupFile = {
    id: 'seed-d/0',
    name: 'album_page.jpg',
    displayPath: 'C:\\Users\\you\\Pictures\\Scans\\album_page.jpg',
    originalBytes: 9_800_000,
    outputBytes: 4_100_000,
    changedSinceSaved: false,
    restored: false,
    kind: 'OneToN',
    derived: [
      { name: 'album_page_01.jpg', bytes: 1_200_000, state: 'unchanged' },
      { name: 'album_page_02.jpg', bytes: 1_150_000, state: 'unchanged' },
      { name: 'album_page_03.jpg', bytes: 980_000, state: 'changed' },
      { name: 'album_page_04.jpg', bytes: 1_020_000, state: 'missing' },
    ],
  };
  const total = (f: BackupFile[]) => f.reduce((s, x) => s + x.originalBytes, 0);
  return [
    {
      id: 'seed-d',
      name: 'Album scans',
      createdAt: new Date(now - 3_600_000).toISOString(),
      expiresAt: new Date(now + 30 * DAY).toISOString(),
      fileCount: 1,
      totalBytes: album.originalBytes,
      pinned: false,
      files: [album],
    },
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

// ---------------------------------------------------------------------------------------------- test hooks
if (typeof window !== 'undefined') {
  (window as unknown as Record<string, unknown>).__autoCropMock = {
    failNextSave: (code: ErrorCode) => {
      forcedFailure = code;
    },
  };
}

// ---------------------------------------------------------------------------------------------- export
export const mockImpl: BackendImpl = {
  api,
  imageUrl: mockImageUrl,
  cropImageUrl: mockCropImageUrl,
  curvePreviewUrl: mockCurvePreviewUrl,
  async on<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): Promise<Unlisten> {
    listeners[event].add(cb);
    return () => {
      listeners[event].delete(cb);
    };
  },
  dropFiles: addFiles,
};

export const mockStats = () => ({ saveCount, items: items.size });
