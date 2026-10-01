<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import FirstWriteSheet from '../lib/components/FirstWriteSheet.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import Tile from '../lib/components/Tile.svelte';
  import {
    canAccept,
    isItemFlaggedFirstQueue,
    matchesFilter,
    sortClassified,
    STRICTNESS_ORDER,
    type Filter,
    type SortKey,
    type Strictness,
  } from '../lib/review.ts';
  import { href, navigate } from '../lib/router.svelte.ts';
  import { LS, lsGet, lsSet, store } from '../lib/store.svelte.ts';
  import { errorMessage, S } from '../lib/strings.ts';

  const FILTERS: Filter[] = ['needs', 'all', 'edited', 'skipped', 'failed', 'saved'];
  const TILE_MIN = [132, 160, 190, 230, 280];

  let userFilter = $state<Filter | null>(null);
  let sort = $state<SortKey>(lsGet(LS.sort) === 'name' ? 'name' : 'confidence');
  let tileSize = $state(Math.min(5, Math.max(1, Number(lsGet(LS.tileSize)) || 3)));
  let selected = $state.raw<Set<number>>(new Set());
  let confirmAccept = $state(false);
  let saveStage = $state<0 | 1>(0);
  let sheetOpen = $state(false);
  let saveBtn = $state<HTMLButtonElement | null>(null);

  const counts = $derived(store.counts);
  const filter = $derived<Filter>(userFilter ?? (counts.needs > 0 || counts.analysing > 0 ? 'needs' : 'all'));
  const shown = $derived(sortClassified(store.classified.filter((x) => matchesFilter(x, filter)), sort));
  const shownIds = $derived(shown.filter((x) => x.item.status === 'ready').map((x) => x.item.id));
  const selectedList = $derived(store.classified.filter((x) => selected.has(x.item.id)));
  const analysingDone = $derived(counts.total - counts.analysing);
  const anyBackup = $derived(store.items.some((i) => i.saved?.backupId));
  const strictCaption = $derived(counts.saved > 0 ? S.grid.strictnessSavedCaption : S.grid.strictnessCaption);

  // Counts shown on the filter chips.
  const chipCount = (f: Filter): number =>
    f === 'needs' ? counts.needs : f === 'all' ? counts.total : f === 'edited' ? counts.edited : f === 'skipped' ? counts.skipped : f === 'failed' ? counts.failed : counts.saved;

  // Drop selections that no longer exist (removed items).
  $effect(() => {
    const ids = new Set(store.items.map((i) => i.id));
    if ([...selected].some((id) => !ids.has(id))) selected = new Set([...selected].filter((id) => ids.has(id)));
  });

  // Tell screen-reader users once when analysis finishes (the strip itself is not announced per item).
  let wasAnalysing = false;
  $effect(() => {
    const now = counts.analysing > 0;
    if (wasAnalysing && !now) store.announce(`${S.grid.needReview(counts.needs)}, ${S.grid.confident(counts.good)}`);
    wasAnalysing = now;
  });

  onMount(() => {
    const id = store.lastOpened;
    if (id === null) return;
    store.lastOpened = null;
    void tick().then(() => {
      const link = document.querySelector<HTMLElement>(`[data-tile-id="${id}"] a`);
      if (link) {
        link.focus({ preventScroll: true });
        link.scrollIntoView({ block: 'nearest' });
      } else {
        document.querySelector<HTMLElement>('[data-route-heading]')?.focus();
      }
    });
  });

  function pickFilter(f: Filter): void {
    userFilter = f;
  }

  function setSort(v: string): void {
    sort = v === 'name' ? 'name' : 'confidence';
    lsSet(LS.sort, sort);
  }

  function setSize(v: number): void {
    tileSize = v;
    lsSet(LS.tileSize, String(v));
  }

  function toggle(id: number): void {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    selected = next;
    confirmAccept = false;
  }

  function selectShown(): void {
    selected = new Set(shownIds);
    confirmAccept = false;
  }

  function clearSelection(): void {
    selected = new Set();
    confirmAccept = false;
  }

  function open(id: number): void {
    store.queue = shownIds;
    store.lastOpened = id;
  }

  // ---- bulk actions --------------------------------------------------------------------------
  const acceptable = $derived(selectedList.filter((x) => x.tier === 'check' && canAccept(x.item, store.strictness) && x.decision !== 'accepted'));
  const unreviewed = $derived(acceptable.filter((x) => x.needs));
  const hasSkippedSelected = $derived(selectedList.some((x) => x.decision === 'skipped'));

  function askAccept(): void {
    if (acceptable.length === 0) {
      const failed = selectedList.some((x) => x.tier === 'failed' && !x.item.edited);
      store.toast(failed ? S.grid.failedNeedCrop : S.grid.goodAlready);
      return;
    }
    if (unreviewed.length > 0) confirmAccept = true;
    else doAccept();
  }

  function doAccept(): void {
    const ids = acceptable.map((x) => x.item.id);
    store.decide(ids, 'accepted');
    store.announce(S.grid.accepted(ids.length));
    clearSelection();
  }

  function doSkip(): void {
    const ids = selectedList.filter((x) => x.item.status === 'ready').map((x) => x.item.id);
    store.decide(ids, 'skipped');
    store.announce(S.grid.skipped(ids.length));
    clearSelection();
  }

  function doPutBack(): void {
    store.decide(selectedList.filter((x) => x.decision === 'skipped').map((x) => x.item.id), null);
    clearSelection();
  }

  async function doRemove(): Promise<void> {
    const ids = [...selected];
    clearSelection();
    await store.removeItems(ids);
    store.toast(S.grid.removed(ids.length));
  }

  // ---- save all ------------------------------------------------------------------------------
  function openSave(): void {
    if (store.saving) return;
    if (store.saveSet.length === 0) {
      store.toast(S.grid.nothingToSave);
      return;
    }
    saveStage = 1;
    void tick().then(() => saveBtn?.focus());
  }

  function confirmSave(): void {
    if (!store.settings.firstWriteAck) {
      sheetOpen = true;
      return;
    }
    void doSave();
  }

  async function chooseMode(copy: boolean): Promise<void> {
    const ok = await store.updateSettings({ saveAsCopy: copy, firstWriteAck: true });
    sheetOpen = false;
    if (ok) void doSave();
  }

  async function doSave(): Promise<void> {
    saveStage = 0;
    await store.performSave(store.saveSet.map((x) => x.item.id));
  }

  function reviewFlagged(): void {
    const ids = isItemFlaggedFirstQueue(store.classified);
    if (ids.length === 0) return;
    saveStage = 0;
    store.queue = ids;
    store.lastOpened = ids[0];
    navigate(`/item/${ids[0]}`);
  }

  function onKeydown(e: KeyboardEvent): void {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'SELECT' || t.tagName === 'TEXTAREA')) return;
    if (e.key === 'Escape' && selected.size > 0) clearSelection();
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'z' && !e.shiftKey) {
      e.preventDefault();
      store.undoDecision();
    }
  }
</script>

<svelte:window onkeydown={onKeydown} />

<div class="page">
  <div class="page-scroll">
  <div class="head">
    <div class="titlebar">
      <a class="icon-btn" href={href('/')} aria-label={S.nav.backToHome}><Icon name="back" size={20} /></a>
      <h1 data-route-heading tabindex="-1">Batch review</h1>
      <button
        type="button"
        class="icon-btn"
        aria-label={store.decisions.history.length ? S.grid.undoDecision : S.grid.nothingToUndo}
        title={store.decisions.history.length ? S.grid.undoDecision : S.grid.nothingToUndo}
        disabled={store.decisions.history.length === 0}
        onclick={() => store.undoDecision()}
      >
        <Icon name="undo" />
      </button>
      <span class="grow"></span>
      {#if anyBackup}
        <span class="backed"><Icon name="shield" size={16} /> {S.grid.backedUp}</span>
      {/if}
      <a class="btn" href={href('/backups')}><Icon name="backups" size={16} /> {S.nav.backups}</a>
      <a class="btn" href={href('/settings')}><Icon name="settings" size={16} /> {S.nav.settings}</a>
      <button type="button" class="btn btn-primary" disabled={store.saving} onclick={openSave}>{S.grid.saveAll}</button>
    </div>

    {#if saveStage === 1}
      <div class="confirm" role="group" aria-label="Confirm save">
        <div class="confirm-text">
          <b>{S.grid.willSave(store.saveSet.length)}</b>
          {S.grid.flaggedStay(counts.needs)}
        </div>
        {#if counts.needs > 0}
          <button type="button" class="btn" onclick={reviewFlagged}>{S.grid.reviewFlaggedFirst}</button>
        {/if}
        <button type="button" class="btn btn-primary" bind:this={saveBtn} onclick={confirmSave}>{S.grid.saveN(store.saveSet.length)}</button>
        <button type="button" class="icon-btn" aria-label={S.grid.cancel} onclick={() => (saveStage = 0)}><Icon name="close" /></button>
      </div>
    {/if}

    {#if store.summary}
      {@const sm = store.summary}
      <div class="summary" role="status">
        <div class="summary-row">
          <Icon name="good" size={20} stroke={2} />
          <div class="grow summary-text">
            <b>{sm.copy ? S.grid.savedCopiesSummary(sm.saved, sm.skipped, sm.failed.length) : S.grid.savedSummary(sm.saved, sm.skipped, sm.failed.length)}</b>
          </div>
          {#if !sm.copy && sm.saved > 0}
            <button type="button" class="btn btn-sm" onclick={() => navigate('/backups')}>{S.grid.restoreAllOriginals}</button>
            <a class="btn btn-sm" href={href('/backups')}>{S.grid.openBackups}</a>
          {/if}
          {#if sm.failed.length > 0}
            <button type="button" class="btn btn-sm btn-danger" disabled={store.saving} onclick={() => store.retryFailed()}>{S.grid.retryFailed}</button>
          {/if}
          <button type="button" class="btn-link" onclick={() => store.dismissSummary()}>{S.grid.dismiss}</button>
        </div>
        {#if sm.failed.length > 0}
          <ul class="failures">
            {#each sm.failed as f (f.id)}
              <li><span class="mono">{f.name}</span> {errorMessage(f.error)}</li>
            {/each}
          </ul>
        {/if}
      </div>
    {/if}

    <div class="status card">
      <div class="status-text">
        <h2>{S.grid.needReview(counts.needs)}</h2>
        <span class="sub">{S.grid.confident(counts.good)}{S.grid.savedNote}</span>
      </div>
      <div class="strict">
        <div class="seg" role="group" aria-label={S.grid.strictness}>
          {#each STRICTNESS_ORDER as s (s)}
            <button type="button" aria-pressed={store.strictness === s} onclick={() => store.setStrictness(s as Strictness)}>
              {S.grid[s]}
              {#if s === 'balanced'}<span class="tag">{S.grid.experimental}</span>{/if}
            </button>
          {/each}
        </div>
        <span class="caption">{strictCaption}</span>
      </div>
    </div>

    {#if counts.analysing > 0}
      <div class="progress" role="group" aria-label="Analysis progress">
        <Icon name="analysing" size={16} />
        <span class="progress-text">{S.grid.analysing(analysingDone, counts.total)}</span>
        <progress max={counts.total} value={analysingDone} aria-label={S.grid.analysing(analysingDone, counts.total)}></progress>
      </div>
    {/if}

    <div class="toolbar">
      <div class="filters" role="group" aria-label={S.grid.filterLabel}>
        {#each FILTERS as f (f)}
          <button type="button" class="chip" aria-pressed={filter === f} onclick={() => pickFilter(f)}>
            {S.grid.filters[f]} <span class="n">{chipCount(f)}</span>
          </button>
        {/each}
      </div>
      <div class="tools">
        <label class="tool">
          {S.grid.sort}
          <select class="select" value={sort} onchange={(e) => setSort(e.currentTarget.value)}>
            <option value="confidence">{S.grid.sortConfidence}</option>
            <option value="name">{S.grid.sortName}</option>
          </select>
        </label>
        <label class="tool">
          {S.grid.size}
          <input type="range" min="1" max="5" value={tileSize} oninput={(e) => setSize(Number(e.currentTarget.value))} />
        </label>
        <button type="button" class="btn btn-sm" onclick={selectShown} disabled={shownIds.length === 0}>{S.grid.selectShown}</button>
      </div>
    </div>

    {#if selected.size > 0}
      <div class="bulk" role="group" aria-label="Selection actions">
        <span class="grow">{confirmAccept ? S.grid.confirmAccept(unreviewed.length) : S.grid.selected(selected.size)}</span>
        {#if confirmAccept}
          <button type="button" class="bulk-btn solid" onclick={doAccept}>{S.grid.yesAccept}</button>
          <button type="button" class="bulk-btn" onclick={() => (confirmAccept = false)}>{S.grid.cancel}</button>
        {:else}
          <button type="button" class="bulk-btn solid" onclick={askAccept}>{S.grid.accept}</button>
          <button type="button" class="bulk-btn" onclick={doSkip}>{S.grid.skip}</button>
          {#if hasSkippedSelected}
            <button type="button" class="bulk-btn" onclick={doPutBack}>{S.grid.unskip}</button>
          {/if}
          <button type="button" class="bulk-btn" onclick={doRemove}>{S.grid.removeFromList}</button>
          <button type="button" class="bulk-btn link" onclick={clearSelection}>{S.grid.clear}</button>
        {/if}
      </div>
    {/if}
  </div>

  <div class="tiles-wrap">
    {#if store.items.length === 0}
      <div class="empty">
        <p class="big">{S.grid.noItemsTitle}</p>
        <p>{S.grid.noItemsNote}</p>
        <a class="btn" href={href('/')}>{S.nav.home}</a>
      </div>
    {:else if shown.length === 0}
      <div class="empty"><p class="big">{S.grid.empty[filter]}</p></div>
    {:else}
      <div class="tiles" style:--min="{TILE_MIN[tileSize - 1]}px">
        {#each shown as x (x.item.id)}
          <Tile {x} strictness={store.strictness} selected={selected.has(x.item.id)} ontoggle={toggle} onopen={open} />
        {/each}
      </div>
    {/if}
  </div>
  </div>

  {#if counts.total > 0}
  <footer class="foot">
    <div>
      {#if counts.needs > 0}
        <b>{S.grid.heldFooter(counts.needs)}</b> <span class="sub">{S.grid.heldNote}</span>
      {:else if counts.total > 0 && counts.analysing === 0}
        <b>{S.grid.nothingHeld(counts.total)}</b> <span class="sub">{S.grid.nothingHeldNote}</span>
      {/if}
    </div>
    {#if counts.needs > 0}
      <button type="button" class="btn btn-primary" onclick={reviewFlagged}>{S.grid.reviewFlagged} <Icon name="next" size={16} stroke={2} /></button>
    {/if}
  </footer>
  {/if}
</div>

<FirstWriteSheet
  open={sheetOpen}
  count={store.saveSet.length}
  copy={store.settings.saveAsCopy}
  days={store.settings.retentionDays}
  onchoose={chooseMode}
  oncancel={() => (sheetOpen = false)}
/>

<style>
  .head {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 12px 24px 0;
  }

  .titlebar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }

  h1 {
    margin: 0 12px 0 2px;
    font-size: 20px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .grow {
    flex: 1;
  }

  .titlebar .btn {
    margin-left: 0;
  }

  .titlebar > .btn:not(:last-child) {
    margin-right: 2px;
  }

  .backed {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-right: 10px;
    font-size: 13px;
    color: var(--good-fg);
  }

  .confirm {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
    padding: 10px 12px 10px 16px;
    background: var(--surface);
    border: 2px solid var(--accent);
    border-radius: 12px;
  }

  .confirm-text {
    flex: 1;
    min-width: 200px;
    font-size: 15px;
  }

  .summary {
    padding: 10px 12px 10px 16px;
    background: var(--good-bg);
    border: 1px solid var(--good-line);
    border-radius: 12px;
    color: var(--good-fg);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .summary-row {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }

  .summary-text {
    min-width: 180px;
    font-size: 15px;
  }

  .summary .btn {
    color: var(--text);
  }

  .summary .btn-danger {
    color: var(--fail-fg);
  }

  .summary .btn-link {
    color: var(--good-fg);
  }

  .failures {
    margin: 0;
    padding: 0 0 0 36px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
    color: var(--text);
  }

  .failures .mono {
    font-size: 12px;
    font-weight: 500;
    margin-right: 6px;
  }

  .status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 12px 24px;
    padding: 12px 16px;
  }

  .status-text {
    display: flex;
    align-items: baseline;
    flex-wrap: wrap;
    gap: 4px 12px;
  }

  .status h2 {
    margin: 0;
    font-size: 22px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .sub {
    color: var(--text-3);
    font-size: 15px;
  }

  .foot .sub {
    font-size: 14px;
  }

  .strict {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 4px;
  }

  .caption {
    font-size: 12px;
    color: var(--text-3);
  }

  .progress {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 16px;
    background: var(--info-bg);
    border: 1px solid var(--info-line);
    border-radius: 12px;
    color: var(--info-text);
  }

  .progress-text {
    min-width: 130px;
    font-weight: 500;
  }

  progress {
    flex: 1;
    height: 8px;
    accent-color: var(--accent);
  }

  .toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 16px;
  }

  .filters {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .chip {
    min-height: var(--ctl-h-sm);
    padding: 0 14px;
    border: 1px solid var(--line-strong);
    border-radius: 17px;
    background: var(--surface);
    color: var(--text);
    font-size: 13px;
    font-weight: 500;
  }

  .chip[aria-pressed='true'] {
    background: var(--text);
    border-color: var(--text);
    color: var(--bg);
    font-weight: 600;
  }

  .chip .n {
    opacity: 0.8;
    margin-left: 2px;
  }

  .tools {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 14px;
  }

  .tool {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text-2);
  }

  .tool input[type='range'] {
    width: 90px;
  }

  .bulk {
    position: sticky;
    top: 8px;
    z-index: 5;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 10px 8px 16px;
    background: var(--bulk-bg);
    color: var(--bulk-text);
    border-radius: 10px;
  }

  .bulk-btn {
    min-height: var(--ctl-h-sm);
    padding: 0 14px;
    border-radius: 8px;
    border: 1px solid rgba(255, 255, 255, 0.5);
    background: transparent;
    color: var(--bulk-text);
    font-size: 13px;
    font-weight: 500;
  }

  .bulk-btn.solid {
    background: #ffffff;
    border-color: #ffffff;
    color: #15181e;
    font-weight: 600;
  }

  .bulk-btn.link {
    border: 0;
    text-decoration: underline;
    padding: 0 10px;
  }

  .tiles-wrap {
    padding: 12px 24px;
  }

  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(var(--min), 1fr));
    gap: 12px;
    align-content: start;
  }

  .empty {
    padding: 48px 0;
    text-align: center;
    color: var(--text-3);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .empty p {
    margin: 0;
  }

  .empty .big {
    font-size: 15px;
  }

  .foot {
    flex-shrink: 0;
    min-height: 64px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 8px 16px;
    padding: 8px 24px;
    background: var(--surface);
    border-top: 1px solid var(--line);
  }

  @media (max-width: 900px) {
    .head,
    .tiles-wrap {
      padding-left: 14px;
      padding-right: 14px;
    }

    .foot {
      padding: 8px 14px;
    }

    .strict {
      align-items: flex-start;
    }
  }
</style>
