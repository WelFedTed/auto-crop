// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// State of the Convert screen: the file list the engine holds, the options (remembered in localStorage), the live
// preview, and the run. One instance, so a run keeps going (and its results stay) while the person looks at Backups.
// Pure rules live in convert.ts.

import { api, onConvertAdded, onConvertProgress } from './backend.ts';
import {
  announceDue,
  clampQuality,
  countResults,
  CUSTOM_QUALITY_START,
  parseOptions,
  restoreFileId,
  sanitiseOptions,
  serialiseOptions,
  whyOff,
  type ResultCounts,
  type WhyOff,
} from './convert.ts';
import { lsGet, lsSet, store } from './store.svelte.ts';
import { convertCodeText, errorMessage, S } from './strings.ts';
import type {
  ConvertAdded,
  ConvertFileResult,
  ConvertFileView,
  ConvertOptions,
  ConvertPreview,
  ConvertSummary,
  DerivedAction,
  RestoreMode,
} from './types.ts';

const LS_OPTIONS = 'ac.convertOptions';

export type Phase = 'idle' | 'running' | 'done';

function runName(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `Convert ${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** What the person is asked after pressing Restore original: Keep or Remove the converted file. */
export interface RestoreAsk {
  result: ConvertFileResult;
}

class ConvertState {
  files = $state.raw<ConvertFileView[]>([]);
  options = $state.raw<ConvertOptions>(parseOptions(lsGet(LS_OPTIONS)));
  preview = $state.raw<ConvertPreview | null>(null);
  checking = $state(false);
  previewFailed = $state(false);
  adding = $state(false);

  phase = $state<Phase>('idle');
  total = $state(0);
  done = $state(0);
  cancelling = $state(false);
  /** Results as they arrive; the summary replaces them at the end. */
  live = $state.raw<ConvertFileResult[]>([]);
  summary = $state.raw<ConvertSummary | null>(null);
  /** The run ended because the command failed, not because it finished: the results shown are those that arrived. */
  interrupted = $state(false);

  /** File ids whose original was restored, whose restore is running, and whose restore needs a choice. */
  restored = $state.raw<Set<number>>(new Set());
  restoring = $state.raw<Set<number>>(new Set());
  asking = $state.raw<Map<number, DerivedAction>>(new Map());
  /** The Keep or Remove sheet. */
  ask = $state.raw<RestoreAsk | null>(null);

  results = $derived<ConvertFileResult[]>(this.summary?.results ?? this.live);
  counts = $derived<ResultCounts>(countResults(this.results));
  why = $derived<WhyOff>(whyOff(this.files.length, this.preview, this.options, this.checking));
  canRun = $derived(this.why === null && this.phase === 'idle');

  private timer: ReturnType<typeof setTimeout> | undefined;
  private seq = 0;
  private seen = new Set<number>();
  private listening = false;

  // ------------------------------------------------------------------------------------------ screen lifetime
  async enter(): Promise<void> {
    try {
      await this.listen();
      await api.convertMode(true);
      await this.refresh();
    } catch (e) {
      store.fail(e);
    }
  }

  leave(): void {
    void api.convertMode(false).catch((e) => console.error(e));
  }

  /** Listens once for the life of the page: a run goes on while another screen shows. */
  private async listen(): Promise<void> {
    if (this.listening) return;
    this.listening = true;
    await onConvertAdded((a) => void this.handleAdded(a));
    await onConvertProgress((p) => this.progress(p.done, p.total, p.result));
  }

  // ------------------------------------------------------------------------------------------ the list
  async refresh(): Promise<void> {
    this.files = await api.convertList();
    for (const f of this.files) this.seen.add(f.id);
    this.schedulePreview();
  }

  async handleAdded(a: ConvertAdded): Promise<void> {
    if (a.added === 0 && a.duplicates === 0 && !a.truncated) return; // a dialog that was cancelled
    const fresh = a.ids.filter((id) => !this.seen.has(id));
    if (a.ids.length > 0 && fresh.length === 0) return; // the event and the reply both arrive
    if (this.phase === 'done' && a.added > 0) this.resetResults();
    try {
      await this.refresh();
    } catch (e) {
      store.fail(e);
      return;
    }
    store.announce(S.convert.added(a.added, a.duplicates, a.truncated));
    if (a.added === 0 || a.duplicates > 0 || a.truncated) store.toast(S.convert.added(a.added, a.duplicates, a.truncated));
  }

  private async add(action: () => Promise<ConvertAdded>): Promise<void> {
    if (this.adding || this.phase === 'running') return;
    this.adding = true;
    try {
      await this.handleAdded(await action());
    } catch (e) {
      store.fail(e);
    } finally {
      this.adding = false;
    }
  }

  pickFiles = () => this.add(() => api.convertPickFiles());
  pickFolder = () => this.add(() => api.convertPickFolder(store.includeSubfolders));

  async remove(id: number): Promise<void> {
    try {
      await api.convertRemove([id]);
      this.files = this.files.filter((f) => f.id !== id);
      this.schedulePreview();
      store.announce(S.convert.removed(1));
    } catch (e) {
      store.fail(e);
    }
  }

  async clear(): Promise<void> {
    try {
      await api.convertClear();
      this.files = [];
      this.schedulePreview();
      store.announce(S.convert.cleared);
    } catch (e) {
      store.fail(e);
    }
  }

  // ------------------------------------------------------------------------------------------ options and preview
  setOptions(patch: Partial<ConvertOptions>): void {
    this.options = sanitiseOptions({ ...this.options, ...patch });
    lsSet(LS_OPTIONS, serialiseOptions(this.options));
    this.schedulePreview();
  }

  setPreset(preset: 'small' | 'balanced' | 'best'): void {
    this.setOptions({ quality: { kind: 'preset', preset } });
  }

  setCustomQuality(value: number | null): void {
    const q = this.options.quality;
    this.setOptions({ quality: { kind: 'fixed', value: clampQuality(value ?? (q.kind === 'fixed' ? q.value : CUSTOM_QUALITY_START)) } });
  }

  async pickOutputFolder(): Promise<void> {
    try {
      await api.convertPickOutputFolder();
      this.schedulePreview(true);
    } catch (e) {
      console.error(e);
      store.toast(S.convert.chooseFolderFailed, 'error');
    }
  }

  schedulePreview(now = false): void {
    clearTimeout(this.timer);
    this.seq++; // an answer already on its way is stale
    if (this.files.length === 0) {
      this.preview = null;
      this.checking = false;
      this.previewFailed = false;
      return;
    }
    this.checking = true;
    this.timer = setTimeout(() => void this.refreshPreview(), now ? 0 : 150);
  }

  private async refreshPreview(): Promise<void> {
    const seq = ++this.seq;
    try {
      const p = await api.convertPreview(this.options);
      if (seq !== this.seq) return;
      this.preview = p;
      this.previewFailed = false;
    } catch (e) {
      console.error(e);
      if (seq === this.seq) this.previewFailed = true;
    } finally {
      if (seq === this.seq) this.checking = false;
    }
  }

  // ------------------------------------------------------------------------------------------ the run
  async run(): Promise<void> {
    if (!this.canRun) return;
    this.phase = 'running';
    this.live = [];
    this.summary = null;
    this.interrupted = false;
    this.cancelling = false;
    this.done = 0;
    this.total = this.files.length;
    this.restored = new Set();
    this.asking = new Map();
    try {
      const s = await api.convertRun(this.options, runName());
      this.summary = s;
      this.total = Math.max(this.total, s.results.length);
      this.done = s.results.length;
      store.announce(S.convert.done(s.converted, s.skipped, s.failed));
    } catch (e) {
      console.error(e);
      this.interrupted = true;
      store.toast(S.convert.runFailed, 'error');
    } finally {
      this.phase = 'done';
      this.cancelling = false;
    }
  }

  /** One finished file (`convert-progress`). */
  progress(done: number, total: number, result: ConvertFileResult): void {
    if (this.phase !== 'running') return;
    this.live = [...this.live, result];
    const before = this.done;
    this.done = done;
    this.total = total;
    if (announceDue(before, done, total) && done < total) store.announce(S.convert.progressAnnounce(done, total));
  }

  async cancel(): Promise<void> {
    if (this.phase !== 'running' || this.cancelling) return;
    this.cancelling = true;
    try {
      await api.convertCancel();
    } catch (e) {
      console.error(e);
    }
  }

  /** Back to the list: the converted files leave it; skipped and failed ones stay for another try. */
  async more(): Promise<void> {
    const ids = this.results.filter((r) => r.status === 'converted').map((r) => r.id);
    this.resetResults();
    try {
      if (ids.length > 0) await api.convertRemove(ids);
      await this.refresh();
    } catch (e) {
      store.fail(e);
    }
  }

  private resetResults(): void {
    this.phase = 'idle';
    this.summary = null;
    this.live = [];
    this.interrupted = false;
    this.done = 0;
    this.total = 0;
    this.ask = null;
  }

  async openFolder(id: number | null): Promise<void> {
    try {
      await api.convertOpenFolder(id);
    } catch (e) {
      console.error(e);
      store.toast(S.convert.openFailed, 'error');
    }
  }

  // ------------------------------------------------------------------------------------------ restore
  private mark<T>(set: Set<T>, v: T, on: boolean): Set<T> {
    const next = new Set(set);
    if (on) next.add(v);
    else next.delete(v);
    return next;
  }

  /** `Restore original` pressed: ask what happens to the converted file. */
  askRestore(result: ConvertFileResult): void {
    this.ask = { result };
  }

  cancelAsk(): void {
    this.ask = null;
  }

  /** Restores the original of `result`; `mode` is `auto` unless the file changed and the person chose. */
  async restore(result: ConvertFileResult, action: DerivedAction, mode: RestoreMode = 'auto'): Promise<void> {
    if (!result.backupId) return;
    this.restoring = this.mark(this.restoring, result.id, true);
    try {
      const o = await api.restoreFileDerived(restoreFileId(result.backupId), mode, action);
      this.ask = null;
      if (o.needsChoice) {
        const next = new Map(this.asking);
        next.set(result.id, action);
        this.asking = next;
      } else if (o.ok) {
        const next = new Map(this.asking);
        next.delete(result.id);
        this.asking = next;
        this.restored = this.mark(this.restored, result.id, true);
        const text = mode === 'as_copy' ? S.convert.restoredAsCopy(o.restored ?? result.name) : S.convert.restoredOne(o.restored ?? result.name);
        store.announce(text);
        store.toast(text, 'success');
      } else {
        store.toast(S.convert.restoreFailed(result.name, errorMessage(o.error)), 'error');
      }
    } catch (e) {
      this.ask = null;
      store.fail(e);
    } finally {
      this.restoring = this.mark(this.restoring, result.id, false);
    }
  }

  /** The line under a skipped or failed result. */
  reasonOf(r: ConvertFileResult): string {
    return convertCodeText(r.code);
  }
}

export const convert = new ConvertState();
