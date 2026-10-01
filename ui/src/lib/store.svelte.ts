// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Shared UI state: the live item list, settings, review decisions (UI-only), toasts and the saved summary.
// One instance, imported by the screens. Pure rules live in review.ts, decisions.ts and quad.ts.

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
import {
  classify,
  countsOf,
  saveCandidates,
  STRICTNESS_ORDER,
  type Classified,
  type Counts,
  type Decision,
  type Strictness,
} from './review.ts';
import { navigate, router } from './router.svelte.ts';
import { errorMessage, S } from './strings.ts';
import type { ErrorCode, ItemView, LaunchInfo, OpenSummary, SaveOutcome, Settings } from './types.ts';

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

export interface SaveSummary {
  saved: number;
  skipped: number;
  failed: SaveFailure[];
  copy: boolean;
}

const LS = {
  strictness: 'ac.strictness',
  theme: 'ac.theme',
  howDismissed: 'ac.howDismissed',
  subfolders: 'ac.subfolders',
  tileSize: 'ac.tileSize',
  sort: 'ac.sort',
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

class Store {
  ready = $state(false);
  fatal = $state<string | null>(null);
  launch = $state<LaunchInfo | null>(null);
  items = $state.raw<ItemView[]>([]);
  settings = $state.raw<Settings>({ saveAsCopy: false, retentionDays: 30, firstWriteAck: false });
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

  classified = $derived<Classified[]>(classify(this.items, this.strictness, this.decisions.map));
  counts = $derived<Counts>(countsOf(this.classified));
  byId = $derived(new Map(this.items.map((i) => [i.id, i])));
  saveSet = $derived(saveCandidates(this.classified));

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
      await this.refreshItems();
      await onItemUpdated((v) => this.upsert(v));
      await onItemsAdded((s) => void this.handleAdded(s));
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
    if (view.gen < cur.gen) return; // an older answer arriving late
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
  }

  decide(ids: number[], decision: Decision | null): void {
    this.setDecisions(applyDecision(this.decisions, ids, decision));
  }

  undoDecision(): void {
    this.setDecisions(undoDecision(this.decisions));
  }

  // ------------------------------------------------------------------------------------------ settings
  async updateSettings(patch: Partial<Settings>): Promise<boolean> {
    const before = this.settings;
    this.settings = { ...before, ...patch };
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
  async performSave(ids: number[], merge: SaveSummary | null = null): Promise<void> {
    if (this.saving || ids.length === 0) return;
    const copy = this.settings.saveAsCopy;
    this.saving = true;
    try {
      const outcomes: SaveOutcome[] = await api.saveItems(ids, copy ? 'copy' : 'replace', runName());
      await this.refreshItems();
      const failed: SaveFailure[] = outcomes
        .filter((o) => !o.ok)
        .map((o) => ({ id: o.id, name: this.byId.get(o.id)?.name ?? `#${o.id}`, error: o.error }));
      const saved = outcomes.filter((o) => o.ok).length + (merge?.saved ?? 0);
      this.summary = { saved, skipped: this.counts.skipped, failed, copy };
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
    this.announcement = this.announcement === text ? text + ' ' : text;
  }

  fail(e: unknown): void {
    console.error(e);
    this.toast(S.toasts.backendError, 'error');
  }
}

export const store = new Store();

export function saveCandidatesCount(): number {
  return store.saveSet.length;
}

export { LS };
