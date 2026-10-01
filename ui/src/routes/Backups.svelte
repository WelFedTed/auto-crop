<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/backend.ts';
  import Icon from '../lib/components/Icon.svelte';
  import { formatBytes, formatDate, formatDateTime, isPast } from '../lib/format.ts';
  import { href, navigate, router } from '../lib/router.svelte.ts';
  import { routePath } from '../lib/routes.ts';
  import { store } from '../lib/store.svelte.ts';
  import { errorMessage, RETENTION_OPTIONS, S } from '../lib/strings.ts';
  import type { BackupFile, BackupRun, BackupsView, RestoreMode, RestoreOutcome } from '../lib/types.ts';

  let view = $state.raw<BackupsView | null>(null);
  let loading = $state(true);
  let failed = $state(false);
  let expanded = $state.raw<Set<string>>(new Set());
  /** Files whose restore needs a choice (changed since saved). */
  let asking = $state.raw<Set<string>>(new Set());
  let busy = $state.raw<Set<string>>(new Set());
  let notice = $state<{ text: string; kind: 'ok' | 'warn' } | null>(null);
  let purging = $state(false);

  const back = $derived(
    router.previous && router.previous.name !== 'backups' ? routePath(router.previous) : store.items.length > 0 ? '/grid' : '/',
  );

  async function load(first = false): Promise<void> {
    try {
      const v = await api.listBackups();
      view = v;
      failed = false;
      if (first && v.runs.length > 0) expanded = new Set([v.runs[0].id]);
    } catch (e) {
      console.error(e);
      failed = true;
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load(true);
  });

  function toggleRun(id: string): void {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  function mark(set: Set<string>, id: string, on: boolean): Set<string> {
    const next = new Set(set);
    if (on) next.add(id);
    else next.delete(id);
    return next;
  }

  async function restore(file: BackupFile, mode: RestoreMode): Promise<void> {
    busy = mark(busy, file.id, true);
    try {
      const o = await api.restoreFile(file.id, mode);
      if (o.needsChoice) {
        asking = mark(asking, file.id, true);
        return;
      }
      asking = mark(asking, file.id, false);
      if (o.ok) {
        notice = {
          kind: 'ok',
          text: mode === 'as_copy' ? S.backups.restoredAsCopy(o.restored ?? file.name) : S.backups.restoredOne(o.restored ?? file.name),
        };
        store.announce(notice.text);
        await Promise.all([load(), store.refreshItems()]);
      } else {
        notice = { kind: 'warn', text: `${file.name}: ${errorMessage(o.error)}` };
      }
    } catch (e) {
      console.error(e);
      notice = { kind: 'warn', text: S.toasts.backendError };
    } finally {
      busy = mark(busy, file.id, false);
    }
  }

  async function restoreRun(run: BackupRun): Promise<void> {
    busy = mark(busy, run.id, true);
    try {
      const outcomes: RestoreOutcome[] = await api.restoreRun(run.id);
      const ok = outcomes.filter((o) => o.ok).length;
      const choice = outcomes.filter((o) => o.needsChoice).length;
      const bad = outcomes.length - ok - choice;
      notice = { kind: bad > 0 || choice > 0 ? 'warn' : 'ok', text: S.backups.restoredRun(ok, choice, bad) };
      store.announce(notice.text);
      if (choice > 0) {
        const pending = new Set(asking);
        for (const f of run.files) if (f.changedSinceSaved && !f.restored) pending.add(f.id);
        asking = pending;
      }
      await Promise.all([load(), store.refreshItems()]);
    } catch (e) {
      console.error(e);
      notice = { kind: 'warn', text: S.toasts.backendError };
    } finally {
      busy = mark(busy, run.id, false);
    }
  }

  async function pin(run: BackupRun): Promise<void> {
    try {
      await api.pinRun(run.id, !run.pinned);
      notice = { kind: 'ok', text: run.pinned ? S.backups.unpinned : S.backups.pinned };
      await load();
    } catch (e) {
      console.error(e);
      store.toast(S.toasts.backendError, 'error');
    }
  }

  async function purge(): Promise<void> {
    purging = true;
    try {
      const n = await api.purgeNow();
      notice = { kind: 'ok', text: S.backups.purged(n) };
      store.announce(notice.text);
      await load();
    } catch (e) {
      console.error(e);
      store.toast(S.toasts.backendError, 'error');
    } finally {
      purging = false;
    }
  }

  async function openFolder(): Promise<void> {
    try {
      await api.openBackupsFolder();
    } catch (e) {
      console.error(e);
      store.toast(S.backups.openFailed, 'error');
    }
  }

  async function setRetention(value: string): Promise<void> {
    const days = value === 'null' ? null : Number(value);
    if (await store.updateSettings({ retentionDays: days })) {
      notice = { kind: 'ok', text: S.backups.retentionSaved };
      await load();
    }
  }

  const fileNote = (f: BackupFile): string => {
    if (f.restored) return S.backups.restored;
    const sizes = f.outputBytes === null ? formatBytes(f.originalBytes) : S.backups.sizes(formatBytes(f.originalBytes), formatBytes(f.outputBytes));
    return f.changedSinceSaved ? `${sizes} · ${S.backups.changedTag}` : `${sizes} · ${S.backups.unchangedTag}`;
  };

  const expiryText = (r: BackupRun): string =>
    r.pinned || !r.expiresAt ? S.backups.neverExpires : isPast(r.expiresAt) ? 'expired' : S.backups.expires(formatDate(r.expiresAt));
</script>

<div class="page">
  <header class="top">
    <a class="icon-btn" href={href(back)} aria-label={S.nav.close}><Icon name="back" size={20} /></a>
    <div class="titles">
      <h1 data-route-heading tabindex="-1">{S.backups.title}</h1>
      <p>{S.backups.intro}</p>
    </div>
  </header>

  <div class="tools">
    <div class="loc">
      <span class="lbl">{S.backups.location}</span>
      <span class="path mono">{view?.location ?? store.launch?.backupsLocation ?? ''}</span>
    </div>
    <button type="button" class="btn btn-sm" onclick={openFolder}>{S.backups.open}</button>
    <span class="grow"></span>
    <label class="keep">
      {S.backups.keepFor}
      <select class="select" value={String(store.settings.retentionDays)} onchange={(e) => setRetention(e.currentTarget.value)}>
        {#each RETENTION_OPTIONS as o (String(o.value))}
          <option value={String(o.value)}>{o.label}</option>
        {/each}
      </select>
    </label>
    {#if view}
      <span class="usage">
        {S.backups.using(formatBytes(view.usedBytes))}{#if view.freeBytes !== null} · {S.backups.freeOf(formatBytes(view.freeBytes))}{/if}
      </span>
    {/if}
    <button type="button" class="btn btn-sm" disabled={purging} onclick={purge}>{S.backups.purge}</button>
  </div>

  {#if notice}
    <div class="notice {notice.kind}" role="status">
      <Icon name={notice.kind === 'ok' ? 'good' : 'check'} size={18} stroke={2} />
      <span class="grow">{notice.text}</span>
      <button type="button" class="icon-btn" aria-label={S.nav.close} onclick={() => (notice = null)}><Icon name="close" size={16} /></button>
    </div>
  {/if}

  <div class="page-scroll list">
    {#if loading}
      <p class="empty">Loading…</p>
    {:else if failed}
      <p class="empty">{S.backups.loadFailed}</p>
    {:else if !view || view.runs.length === 0}
      <div class="empty">
        <p class="big">{S.backups.empty}</p>
        <p>{S.backups.emptyNote}</p>
      </div>
    {:else}
      {#each view.runs as run (run.id)}
        {@const open = expanded.has(run.id)}
        {@const expired = !run.pinned && isPast(run.expiresAt)}
        <section class="run" aria-label={run.name}>
          <div class="run-head">
            <button type="button" class="twist" aria-expanded={open} aria-label={`${open ? S.backups.collapse : S.backups.expand}: ${run.name}`} onclick={() => toggleRun(run.id)}>
              <span class:rot={open}><Icon name="next" size={18} stroke={1.9} /></span>
            </button>
            <div class="run-title">
              <span class="rname">{run.name}</span>
              <span class="rmeta">
                {formatDateTime(run.createdAt)} · {S.backups.files(run.fileCount)} · {formatBytes(run.totalBytes)} · {expiryText(run)}
              </span>
            </div>
            {#if run.pinned}<span class="badge good">{S.backups.kept}</span>{/if}
            {#if expired}<span class="badge failed">Expired</span>{/if}
            <button type="button" class="btn btn-sm" aria-pressed={run.pinned} onclick={() => pin(run)}>
              {run.pinned ? S.backups.unkeep : S.backups.keep}
            </button>
            <button type="button" class="btn btn-sm btn-primary" disabled={busy.has(run.id) || expired || run.files.every((f) => f.restored)} onclick={() => restoreRun(run)}>
              {S.backups.restoreRun}
            </button>
          </div>

          {#if open}
            <ul class="files">
              {#each run.files as f (f.id)}
                <li class:changed={f.changedSinceSaved && !f.restored}>
                  <div class="frow">
                    <span class="fthumb"><Icon name="image" size={20} /></span>
                    <div class="finfo">
                      <div class="fname mono">{f.name} <span class="fnote" class:warn={f.changedSinceSaved && !f.restored} class:okay={f.restored}>{fileNote(f)}</span></div>
                      <div class="fpath mono" title={f.displayPath}>{f.displayPath}</div>
                    </div>
                    {#if f.restored}
                      <span class="badge good"><Icon name="good" size={14} stroke={2} /> {S.backups.restored}</span>
                    {:else}
                      <button type="button" class="btn btn-sm btn-primary" disabled={busy.has(f.id) || expired} onclick={() => restore(f, 'auto')}>
                        {S.backups.restore}
                      </button>
                    {/if}
                  </div>
                  {#if asking.has(f.id) && !f.restored}
                    <div class="ask" role="group" aria-label={S.backups.changedSince}>
                      <span>{S.backups.changedSince}</span>
                      <span class="grow"></span>
                      <button type="button" class="btn btn-sm btn-primary" disabled={busy.has(f.id)} onclick={() => restore(f, 'as_copy')}>{S.backups.restoreAsCopy}</button>
                      <button type="button" class="btn btn-sm" disabled={busy.has(f.id)} onclick={() => restore(f, 'replace_anyway')}>{S.backups.replaceAnyway}</button>
                    </div>
                  {/if}
                </li>
              {/each}
            </ul>
          {/if}
        </section>
      {/each}
    {/if}
  </div>

  <footer class="foot">
    <span>{S.backups.note}</span>
    <button type="button" class="btn btn-sm" onclick={() => navigate('/settings')}>{S.nav.settings}</button>
  </footer>
</div>

<style>
  .top {
    flex-shrink: 0;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 18px 24px 10px;
  }

  .titles h1 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .titles p {
    margin: 2px 0 0;
    font-size: 13px;
    color: var(--text-3);
  }

  .tools {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px 14px;
    padding: 12px 24px;
    background: var(--surface-2);
    border-top: 1px solid var(--line-soft);
    border-bottom: 1px solid var(--line-soft);
  }

  .loc {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .lbl,
  .keep,
  .usage {
    font-size: 13px;
    color: var(--text-2);
  }

  .path {
    padding: 6px 10px;
    font-size: 12px;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 6px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 46vw;
  }

  .keep {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .grow {
    flex: 1;
  }

  .notice {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px 6px 24px;
    font-size: 14px;
  }

  .notice.ok {
    background: var(--good-bg);
    color: var(--good-fg);
    border-bottom: 1px solid var(--good-line);
  }

  .notice.warn {
    background: var(--check-bg);
    color: var(--check-text-strong);
    border-bottom: 1px solid var(--check-line);
  }

  .list {
    padding: 8px 24px;
  }

  .empty {
    padding: 48px 0;
    text-align: center;
    color: var(--text-3);
  }

  .empty p {
    margin: 4px 0;
  }

  .empty .big {
    font-size: 15px;
    color: var(--text-2);
  }

  .run {
    padding: 12px 0;
    border-bottom: 1px solid var(--line-soft);
  }

  .run-head {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 10px;
  }

  .twist {
    width: var(--ctl-h-sm);
    height: var(--ctl-h-sm);
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 0;
    border-radius: 8px;
    color: var(--text-2);
  }

  .twist .rot {
    display: inline-flex;
    transform: rotate(90deg);
  }

  .run-title {
    flex: 1;
    min-width: 200px;
  }

  .rname {
    font-weight: 600;
  }

  .rmeta {
    margin-left: 8px;
    color: var(--text-3);
    font-size: 13px;
  }

  .files {
    list-style: none;
    margin: 10px 0 0 32px;
    padding: 0;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
    overflow: hidden;
  }

  .files li {
    padding: 10px 12px;
    border-bottom: 1px solid var(--line-soft);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .files li:last-child {
    border-bottom: 0;
  }

  .files li.changed {
    background: var(--check-bg);
  }

  .frow {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .fthumb {
    width: 40px;
    height: 40px;
    border-radius: 6px;
    background: var(--tile-bg);
    color: var(--text-2);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  .finfo {
    flex: 1;
    min-width: 0;
  }

  .fname {
    font-size: 13px;
    font-weight: 500;
  }

  .fnote {
    margin-left: 6px;
    font-family: var(--font-sans);
    font-weight: 400;
    color: var(--text-3);
  }

  .fnote.warn {
    color: var(--check-fg);
  }

  .fnote.okay {
    color: var(--good-fg);
  }

  .fpath {
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .ask {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 10px;
    margin-left: 52px;
    font-size: 13px;
    color: var(--check-text-strong);
  }

  .foot {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 16px;
    padding: 10px 24px;
    background: var(--surface-2);
    border-top: 1px solid var(--line-soft);
    font-size: 13px;
    color: var(--text-3);
  }

  @media (max-width: 900px) {
    .top,
    .tools,
    .list,
    .foot {
      padding-left: 14px;
      padding-right: 14px;
    }

    .files {
      margin-left: 0;
    }

    .ask {
      margin-left: 0;
    }
  }
</style>
