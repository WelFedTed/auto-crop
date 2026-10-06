// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Shared UI state: the live item list, settings, review decisions (UI-only), toasts and the saved summary.
// One instance, imported by the screens. Pure rules live in review.ts, items.ts, decisions.ts and quad.ts.

import { api, initBackend, isMock, onItemUpdated, onItemsAdded } from './backend.ts';
import {
  applyDecision,
  emptyDecisions,
  parseDecisions,
  pruneDecisions,
  serialiseDecisions,
  undoDecision,
  type DecisionState,
} from './decisions.ts';
import { afterRedo, afterUndo, emptyGridHistory, nextRedo, nextUndo, pushAction, reconcile, type GridHistory } from './grid-history.ts';
import { collisionNotice, plannedNames } from './items.ts';
import {
  classify,
  countsOf,
  openOnlyLeftAlone,
  saveCandidates,
  STRICTNESS_ORDER,
  type Classified,
  type Counts,
  type Decision,
  type Strictness,
} from './review.ts';
import { navigate, router } from './router.svelte.ts';
import { defaultSettings, patchedSettings } from './settings-defaults.ts';
import { ERRORS, errorMessage, noticeText, S } from './strings.ts';
import type { ErrorCode, ItemView, LaunchInfo, OpenSummary, SaveOutcome, SaveTarget, Settings, SplitPatch } from './types.ts';

export type ThemeChoice = 'system' | 'light' | 'dark';

export interface Toast {
  id: number;
  text: string;
  kind: 'info' | 'success' | 'error';
  action?: { label: string; run: () => void };
}

export interface SaveFailure {
  id: number;
  name: string;
  error: ErrorCode | null;
}

/** A line under the saved summary that is not a failure: a note on a file that WAS saved, or one left alone. */
export interface SaveNote {
  id: number;
  name: string;
  text: string;
}

export interface SaveSummary {
  saved: number;
  /** Files written (a split scan writes several). */
  files: number;
  skipped: number;
  failed: SaveFailure[];
  copy: boolean;
  /** Scans left unwritten because they wait for the person's OK (not an error). */
  held: number;
  /** Sources that are never replaced in place and were left as they are. */
  notReplaced: number;
  notes: SaveNote[];
}

/** What `saveScan` tells the editor about one scan. */
export interface ScanSave {
  outcome: SaveOutcome;
  planned: string[];
  collision: { wanted: string; got: string } | null;
}

const LS = {
  strictness: 'ac.strictness',
  theme: 'ac.theme',
  howDismissed: 'ac.howDismissed',
  subfolders: 'ac.subfolders',
  tileSize: 'ac.tileSize',
  sort: 'ac.sort',
  firstSplitAck: 'ac.firstSplitAck',
} as const;

const SS_DECISIONS = 'ac.decisions';

export function lsGet(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

export function lsSet(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // storage can be blocked; the UI works without it
  }
}

function ssGet(key: string): string | null {
  try {
    return sessionStorage.getItem(key);
  } catch {
    return null;
  }
}

function ssSet(key: string, value: string): void {
  try {
    sessionStorage.setItem(key, value);
  } catch {
    // ignore
  }
}

function loadStrictness(): Strictness {
  const v = lsGet(LS.strictness);
  return (STRICTNESS_ORDER as string[]).includes(v ?? '') ? (v as Strictness) : 'strict';
}

function loadTheme(): ThemeChoice {
  const v = lsGet(LS.theme);
  return v === 'light' || v === 'dark' ? v : 'system';
}

function runName(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `Batch ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** The error code of a rejected command (the shell rejects with the bare code, e.g. "ITEM_OP"), if it is one we know. */
export function codeOf(e: unknown): ErrorCode | null {
  const text = typeof e === 'string' ? e : e instanceof Error ? e.message : '';
  return text in ERRORS ? (text as ErrorCode) : null;
}

class Store {
  ready = $state(false);
  fatal = $state<string | null>(null);
  launch = $state<LaunchInfo | null>(null);
  items = $state.raw<ItemView[]>([]);
  settings = $state.raw<Settings>(defaultSettings());
  strictness = $state<Strictness>(loadStrictness());
  decisions = $state.raw<DecisionState>(emptyDecisions());
  theme = $state<ThemeChoice>(loadTheme());
  toasts = $state.raw<Toast[]>([]);
  announcement = $state('');
  summary = $state.raw<SaveSummary | null>(null);
  saving = $state(false);
  opening = $state(false);
  /** Ids in the order the editor walks them (set by the grid when a tile is opened). */
  queue = $state.raw<number[]>([]);
  /** Tile to focus when the grid comes back after the editor. */
  lastOpened = $state<number | null>(null);
  includeSubfolders = $state(lsGet(LS.subfolders) !== '0');
  howDismissed = $state(lsGet(LS.howDismissed) === '1');
  /** The first split sheet ("1 scan becomes 4 files") has been answered. */
  firstSplitAck = $state(lsGet(LS.firstSplitAck) === '1');
  gridHistory = $state.raw<GridHistory>(emptyGridHistory());

  /** The settings that change what the grid does with scans that have several items. */
  reviewOpts = $derived({ autoSaveSplits: this.settings.autoSaveSplits, saveAsCopy: this.settings.saveAsCopy });
  classified = $derived<Classified[]>(classify(this.items, this.strictness, this.decisions.map, this.reviewOpts));
  counts = $derived<Counts>(countsOf(this.classified));
  byId = $derived(new Map(this.items.map((i) => [i.id, i])));
  saveSet = $derived(saveCandidates(this.classified, this.reviewOpts));
  openOnlyAside = $derived(openOnlyLeftAlone(this.classified, this.reviewOpts));
  canUndoGrid = $derived(nextUndo(this.gridHistory) !== null);
  canRedoGrid = $derived(nextRedo(this.gridHistory) !== null);

  private toastSeq = 1;
  private seen = new Set<number>();

  // ------------------------------------------------------------------------------------------ boot
  async init(): Promise<void> {
    try {
      this.launch = await initBackend();
      this.settings = await api.getSettings();
      if (isMock) {
        // The mock restarts its ids on every page load, so stale decisions would hit other items.
        this.decisions = emptyDecisions();
        ssSet(SS_DECISIONS, '{}');
      } else {
        this.decisions = { map: parseDecisions(ssGet(SS_DECISIONS)), history: [] };
      }
      // Browser mock only: `?samples` opens the sample batch at start, to try the UI without clicking through Home.
      if (isMock && new URLSearchParams(location.search).has('samples')) await api.addSamples();
      // Listen first, list second: files opened from the command line start analysing before the window exists,
      // and an update that lands between the two must not be lost.
      await onItemUpdated((v) => this.upsert(v));
      await onItemsAdded((s) => void this.handleAdded(s));
      await this.refreshItems();
      // Opened from the command line or "Open with": the files are already in the batch, so start at the grid.
      if (!isMock && this.items.length > 0 && router.route.name === 'home') navigate('/grid');
      this.applyTheme();
      this.ready = true;
    } catch (e) {
      this.fatal = e instanceof Error ? e.message : String(e);
    }
  }

  // ------------------------------------------------------------------------------------------ items
  async refreshItems(): Promise<void> {
    const list = await api.listItems();
    for (const i of list) this.seen.add(i.id);
    this.items = list;
    this.pruneDecisions();
  }

  upsert(view: ItemView): void {
    const at = this.items.findIndex((i) => i.id === view.id);
    if (at < 0) {
      this.items = [...this.items, view];
      return;
    }
    const cur = this.items[at];
    // An older answer arriving late is dropped. The engine bumps `gen` on every committed change, and the
    // history position moves with undo and redo, so equal gens only replace on a newer position.
    if (view.gen < cur.gen) return;
    const next = this.items.slice();
    next[at] = view;
    this.items = next;
  }

  async handleAdded(summary: OpenSummary): Promise<void> {
    if (summary.added === 0 && summary.skipped === 0) return; // dialog cancelled
    const fresh = summary.ids.filter((id) => !this.seen.has(id));
    if (summary.ids.length > 0 && fresh.length === 0) return; // already handled (event and reply both arrive)
    await this.refreshItems();
    if (summary.added > 0) {
      this.toast(summary.skipped > 0 ? S.toasts.addedSkipped(summary.added, summary.skipped) : S.toasts.added(summary.added));
    } else {
      this.toast(`${S.toasts.noneAdded} ${errorMessage(summary.skippedReasons[0])}`, 'error');
    }
    if (summary.added > 0 && router.route.name === 'home') navigate('/grid');
  }

  async run(action: () => Promise<OpenSummary>): Promise<void> {
    if (this.opening) return;
    this.opening = true;
    try {
      await this.handleAdded(await action());
    } catch (e) {
      this.fail(e);
    } finally {
      this.opening = false;
    }
  }

  openFiles = () => this.run(() => api.pickFiles());
  openFolder = () => this.run(() => api.pickFolder(this.includeSubfolders));
  addSamples = () => this.run(() => api.addSamples());

  async removeItems(ids: number[]): Promise<void> {
    try {
      await api.removeItems(ids);
      const drop = new Set(ids);
      this.items = this.items.filter((i) => !drop.has(i.id));
      this.pruneDecisions();
    } catch (e) {
      this.fail(e);
    }
  }

  // ------------------------------------------------------------------------------------------ decisions
  private pruneDecisions(): void {
    const next = pruneDecisions(this.decisions, new Set(this.items.map((i) => i.id)));
    if (next !== this.decisions) this.setDecisions(next);
  }

  private setDecisions(next: DecisionState): void {
    this.decisions = next;
    ssSet(SS_DECISIONS, serialiseDecisions(next.map));
    this.gridHistory = reconcile(this.gridHistory, next.history.length);
  }

  decide(ids: number[], decision: Decision | null): void {
    const next = applyDecision(this.decisions, ids, decision);
    if (next === this.decisions) return;
    this.setDecisions(next);
    this.gridHistory = pushAction(this.gridHistory, { kind: 'decision' });
  }

  undoDecision(): void {
    this.setDecisions(undoDecision(this.decisions));
  }

  // ------------------------------------------------------------------------------------------ grid undo
  /** Undo in the review grid: the most recent decision or session command, whichever came last. */
  async undoGrid(): Promise<void> {
    const a = nextUndo(this.gridHistory);
    if (!a) return;
    if (a.kind === 'decision') {
      this.gridHistory = afterUndo(this.gridHistory);
      this.undoDecision();
      return;
    }
    try {
      const step = await api.sessionUndo();
      this.gridHistory = afterUndo(this.gridHistory, step?.label);
      for (const v of step?.items ?? []) this.upsert(v);
      if (step) this.announce(S.grid.sessionUndone(step.label));
    } catch (e) {
      this.fail(e);
    }
  }

  async redoGrid(): Promise<void> {
    if (!nextRedo(this.gridHistory)) return;
    try {
      const step = await api.sessionRedo();
      this.gridHistory = afterRedo(this.gridHistory, step?.label);
      for (const v of step?.items ?? []) this.upsert(v);
      if (step) this.announce(S.grid.sessionRedone(step.label));
    } catch (e) {
      this.fail(e);
    }
  }

  /** "Treat as one item" or "Split into items" for several scans: ONE undo step of the session. */
  async changeSplit(ids: number[], patch: SplitPatch): Promise<void> {
    if (ids.length === 0) return;
    const word = patch.policy === 'never' ? S.grid.oneSelected : patch.policy ? S.grid.splitSelected : 'Re-detect items';
    const label = `${word} (${ids.length} ${ids.length === 1 ? 'image' : 'images'})`;
    try {
      const results = await api.redetectMany(ids, patch);
      for (const r of results) if (r.view) this.upsert(r.view);
      const bad = results.filter((r) => !r.view).length;
      this.gridHistory = pushAction(this.gridHistory, { kind: 'session', label });
      this.announce(label);
      this.toast(bad > 0 ? `${label}. ${S.grid.splitFailed(bad)}` : label, bad > 0 ? 'error' : 'success', {
        label: S.grid.undoSession,
        run: () => void this.undoGrid(),
      });
    } catch (e) {
      this.fail(e);
    }
  }

  // ------------------------------------------------------------------------------------------ one scan
  async acceptScan(id: number, said: string = S.split.acceptedAnnounce): Promise<boolean> {
    try {
      this.upsert(await api.acceptScan(id));
      this.announce(said);
      return true;
    } catch (e) {
      this.fail(e);
      return false;
    }
  }

  async unacceptScan(id: number, said: string = S.split.withdrawnAnnounce): Promise<void> {
    try {
      this.upsert(await api.unacceptScan(id));
      this.announce(said);
    } catch (e) {
      this.fail(e);
    }
  }

  /** Saves one scan (the editor's Save as copy and Replace original). Never throws; failures come back as the outcome. */
  async saveScan(id: number, target: SaveTarget): Promise<ScanSave | null> {
    if (this.saving) return null;
    const before = this.byId.get(id);
    const planned = before ? plannedNames(before) : [];
    this.saving = true;
    try {
      const [outcome] = await api.saveItems([id], target, runName());
      await this.refreshItems();
      const written = outcome?.saved?.outputs.length ? outcome.saved.outputs : outcome?.saved ? [outcome.saved.output] : [];
      return {
        outcome,
        planned,
        // a plain one-to-one save keeps the name; only a set can be moved to another base name
        collision: outcome?.ok && written.length > 1 ? collisionNotice(planned, written) : null,
      };
    } catch (e) {
      this.fail(e);
      return null;
    } finally {
      this.saving = false;
    }
  }

  ackFirstSplit(): void {
    this.firstSplitAck = true;
    lsSet(LS.firstSplitAck, '1');
  }

  // ------------------------------------------------------------------------------------------ settings
  /** Sends the WHOLE settings object back with the change laid over it, so fields this UI does not show never reset. */
  async updateSettings(patch: Partial<Settings>): Promise<boolean> {
    const before = this.settings;
    this.settings = patchedSettings(before, patch);
    try {
      this.settings = await api.setSettings(this.settings);
      return true;
    } catch (e) {
      this.settings = before;
      this.toast(S.settings.saveFailed, 'error');
      console.error(e);
      return false;
    }
  }

  setStrictness(s: Strictness): void {
    this.strictness = s;
    lsSet(LS.strictness, s);
  }

  setTheme(t: ThemeChoice): void {
    this.theme = t;
    lsSet(LS.theme, t);
    this.applyTheme();
  }

  applyTheme(): void {
    const root = document.documentElement;
    if (this.theme === 'system') delete root.dataset.theme;
    else root.dataset.theme = this.theme;
  }

  setIncludeSubfolders(v: boolean): void {
    this.includeSubfolders = v;
    lsSet(LS.subfolders, v ? '1' : '0');
  }

  dismissHow(v = true): void {
    this.howDismissed = v;
    lsSet(LS.howDismissed, v ? '1' : '0');
  }

  // ------------------------------------------------------------------------------------------ saving
  /** Writes `ids` with the current mode. Callers have already confirmed (and shown the first-write sheet). */
  async performSave(ids: number[], merge: SaveSummary | null = null, target?: SaveTarget): Promise<void> {
    if (this.saving || ids.length === 0) return;
    const copy = target ? target === 'copy' : this.settings.saveAsCopy;
    const planned = new Map(ids.map((id) => [id, plannedNames(this.byId.get(id) ?? ({ crops: [] } as unknown as ItemView))]));
    this.saving = true;
    try {
      const outcomes: SaveOutcome[] = await api.saveItems(ids, copy ? 'copy' : 'replace', runName());
      await this.refreshItems();
      const nameOf = (id: number) => this.byId.get(id)?.name ?? `#${id}`;
      const failed: SaveFailure[] = [];
      const notes: SaveNote[] = [];
      let held = merge?.held ?? 0;
      let notReplaced = merge?.notReplaced ?? 0;
      let files = merge?.files ?? 0;
      for (const o of outcomes) {
        if (!o.ok) {
          // Held for review and never-replaced are not failures: nothing went wrong, the file just waits or stays.
          if (o.error === 'HELD_FOR_REVIEW') held++;
          else if (o.error === 'NOT_REPLACEABLE') {
            notReplaced++;
            notes.push({ id: o.id, name: nameOf(o.id), text: o.notices[0] ? noticeText(o.notices[0]) : errorMessage(o.error) });
          } else failed.push({ id: o.id, name: nameOf(o.id), error: o.error });
          continue;
        }
        const written = o.saved?.outputs.length ? o.saved.outputs : [o.saved?.output ?? ''];
        files += written.length;
        for (const n of o.notes) notes.push({ id: o.id, name: nameOf(o.id), text: errorMessage(n) });
        for (const n of o.notices) notes.push({ id: o.id, name: nameOf(o.id), text: noticeText(n) });
        const c = written.length > 1 ? collisionNotice(planned.get(o.id) ?? [], written) : null;
        if (c) notes.push({ id: o.id, name: nameOf(o.id), text: S.save.collision(c.wanted, c.got) });
      }
      const saved = outcomes.filter((o) => o.ok).length + (merge?.saved ?? 0);
      this.summary = { saved, files, skipped: this.counts.skipped, failed, copy: copy || !!merge?.copy, held, notReplaced, notes: [...(merge?.notes ?? []), ...notes] };
      this.announce(S.grid.savedSummary(saved, this.counts.skipped, failed.length));
    } catch (e) {
      this.fail(e);
    } finally {
      this.saving = false;
    }
  }

  retryFailed(): void {
    const s = this.summary;
    if (!s || s.failed.length === 0) return;
    void this.performSave(
      s.failed.map((f) => f.id),
      s,
    );
  }

  dismissSummary(): void {
    this.summary = null;
  }

  // ------------------------------------------------------------------------------------------ messages
  toast(text: string, kind: Toast['kind'] = 'info', action?: Toast['action']): void {
    const id = this.toastSeq++;
    this.toasts = [...this.toasts, { id, text, kind, action }];
    setTimeout(() => this.dismissToast(id), kind === 'error' || action ? 10000 : 5500);
  }

  dismissToast(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  announce(text: string): void {
    // Alternate a trailing no-break space so an identical message is announced again.
    this.announcement = this.announcement === text ? text + ' ' : text;
  }

  fail(e: unknown): void {
    console.error(e);
    const code = codeOf(e);
    this.toast(code ? errorMessage(code) : S.toasts.backendError, 'error');
  }
}

export const store = new Store();

export function saveCandidatesCount(): number {
  return store.saveSet.length;
}

export { LS };
