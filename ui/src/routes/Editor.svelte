<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { api, imageUrl } from '../lib/backend.ts';
  import CropStage from '../lib/components/CropStage.svelte';
  import Icon from '../lib/components/Icon.svelte';
  import ResultInset from '../lib/components/ResultInset.svelte';
  import Ruler from '../lib/components/Ruler.svelte';
  import TierBadge from '../lib/components/TierBadge.svelte';
  import { cloneEdit, normaliseAngle, parsePercent, setCornerPercent, toPercent, turnQuarter, type Quad } from '../lib/quad.ts';
  import { canAccept, defaultQueue, needsDrawCrop, nextToReview, reasonLine } from '../lib/review.ts';
  import { href, navigate } from '../lib/router.svelte.ts';
  import { store } from '../lib/store.svelte.ts';
  import { holdAction, holdCause, holdTitle, S } from '../lib/strings.ts';
  import type { Edit, ItemView } from '../lib/types.ts';

  let { id }: { id: number } = $props();

  const item = $derived<ItemView | undefined>(store.byId.get(id));
  const cls = $derived(store.classified.find((x) => x.item.id === id));
  const tier = $derived(cls?.tier ?? 'analysing');
  const decision = $derived(cls?.decision ?? null);

  // ---- queue -------------------------------------------------------------------------------
  // The editor is re-created for every item (App keys it by id), so reading `id` once here is intended.
  const startId = untrack(() => id);
  if (!store.queue.includes(startId)) store.queue = defaultQueue(store.classified);
  const queue = store.queue;
  const position = queue.indexOf(startId) + 1;

  function adjacent(dir: 1 | -1): number | null {
    let at = queue.indexOf(id);
    for (;;) {
      at += dir;
      if (at < 0 || at >= queue.length) return null;
      const it = store.byId.get(queue[at]);
      if (it && it.status === 'ready') return queue[at];
    }
  }

  const prevId = $derived(adjacent(-1));
  const nextId = $derived(adjacent(1));

  function goTo(target: number): void {
    store.lastOpened = target;
    navigate(`/item/${target}`);
  }

  function leave(): void {
    store.lastOpened = id;
    navigate('/grid');
  }

  function goNextToReview(): void {
    const needs = new Set(store.classified.filter((x) => x.needs).map((x) => x.item.id));
    const target = nextToReview(queue, id, needs);
    if (target === null) {
      store.toast(S.editor.noMoreFlagged);
      leave();
    } else goTo(target);
  }

  // ---- edits: optimistic draft plus one serial chain of calls --------------------------------
  let draft = $state.raw<Edit | null>(null);
  let previewQuad = $state.raw<Quad | null>(null);
  let liveAngle = $state<number | null>(null);
  let inflight = $state(0);
  let chain: Promise<unknown> = Promise.resolve();
  let sayTimer: ReturnType<typeof setTimeout> | undefined;

  const edit = $derived<Edit | null>(draft ?? item?.edit ?? null);
  const shownEdit = $derived<Edit | null>(
    edit ? { quad: previewQuad ?? edit.quad, quarterTurns: edit.quarterTurns, fineDeg: liveAngle ?? edit.fineDeg } : null,
  );
  const busy = $derived(previewQuad !== null || liveAngle !== null || inflight > 0);

  function say(text: string): void {
    clearTimeout(sayTimer);
    sayTimer = setTimeout(() => store.announce(text), 150);
  }

  function enqueue(work: () => Promise<ItemView | void>, optimistic: Edit | null): void {
    if (optimistic) draft = cloneEdit(optimistic);
    inflight++;
    chain = chain.then(async () => {
      try {
        const v = await work();
        if (v) store.upsert(v);
      } catch (e) {
        console.error(e);
        store.toast(S.editor.editFailed, 'error');
      } finally {
        inflight--;
        if (inflight === 0) draft = null;
      }
    });
  }

  function commitEdit(next: Edit, label: string, announce?: string): void {
    enqueue(() => api.setEdit(id, next, 'end', label), next);
    if (announce) say(announce);
  }

  function commitQuad(quad: Quad, label: string, announce: string): void {
    if (!edit) return;
    commitEdit({ ...cloneEdit(edit), quad }, label, announce);
  }

  function commitAngle(deg: number, label = S.editor.labels.rotate): void {
    if (!edit) return;
    const v = normaliseAngle(deg);
    commitEdit({ ...cloneEdit(edit), fineDeg: v }, label, S.editor.angleSpoken(v));
  }

  function turn(dir: 1 | -1): void {
    if (!edit) return;
    commitEdit({ ...cloneEdit(edit), quarterTurns: turnQuarter(edit.quarterTurns, dir) }, S.editor.labels.rotate90, dir > 0 ? S.editor.rotateRight90 : S.editor.rotateLeft90);
  }

  function setCorner(i: number, axis: 'x' | 'y', text: string): void {
    if (!edit) return;
    const v = parsePercent(text);
    if (v === null) return;
    commitEdit({ ...cloneEdit(edit), quad: setCornerPercent(edit.quad, i, axis, v) }, S.editor.labels.editCorner, `${S.editor.corners[i]} ${axis} ${v.toFixed(1)}%`);
  }

  function doUndo(): void {
    if (!item?.canUndo) return;
    const label = item.undoLabel ?? '';
    const wasSaved = !!item.saved;
    enqueue(async () => {
      const v = await api.undo(id);
      store.announce(S.editor.undone(label));
      if (wasSaved) store.toast(S.editor.undoSavedToast, 'info', { label: S.editor.restoreOriginalFile, run: () => navigate('/backups') });
      return v;
    }, null);
  }

  function doRedo(): void {
    if (!item?.canRedo) return;
    const label = item.redoLabel ?? '';
    enqueue(async () => {
      const v = await api.redo(id);
      store.announce(S.editor.redone(label));
      return v;
    }, null);
  }

  function doReset(): void {
    enqueue(() => api.resetToAuto(id), null);
    store.announce(S.editor.resetToAuto);
  }

  function doDrawCrop(): void {
    enqueue(() => api.drawCrop(id), null);
  }

  // ---- decisions ------------------------------------------------------------------------------
  function accept(): boolean {
    if (!item) return false;
    if (!canAccept(item, store.strictness)) {
      store.toast(S.editor.cannotAcceptFailed);
      return false;
    }
    store.decide([id], 'accepted');
    return true;
  }

  function toggleAccepted(): void {
    if (decision === 'accepted') store.decide([id], null);
    else if (accept()) store.announce(S.editor.acceptedToast);
  }

  function acceptAndNext(): void {
    if (accept()) goNextToReview();
  }

  function skip(): void {
    if (decision === 'skipped') {
      store.decide([id], null);
      return;
    }
    store.decide([id], 'skipped');
    store.announce(S.editor.skipped);
    goNextToReview();
  }

  // ---- compare (hold or toggle) ------------------------------------------------------------------
  let compareLatched = $state(false);
  let compareHeld = $state(false);
  let compareDownAt = 0;
  const compare = $derived(compareLatched || compareHeld);

  function compareDown(): void {
    compareHeld = true;
    compareDownAt = performance.now();
  }

  function compareUp(): void {
    if (!compareHeld) return;
    compareHeld = false;
    if (performance.now() - compareDownAt < 350) compareLatched = !compareLatched;
  }

  // ---- keyboard -------------------------------------------------------------------------------------
  let stage = $state<{ zoomBy: (f: number) => void; fit: () => void; actualSize: () => void } | null>(null);

  function typing(t: EventTarget | null): boolean {
    const el = t as HTMLElement | null;
    if (!el) return false;
    return el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA' || el.isContentEditable;
  }

  function onKeydown(e: KeyboardEvent): void {
    if (e.defaultPrevented) return;
    const t = e.target as HTMLElement | null;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'z' && !typing(t)) {
      e.preventDefault();
      if (e.shiftKey) doRedo();
      else doUndo();
      return;
    }
    if (mod && e.key.toLowerCase() === 'y' && !typing(t)) {
      e.preventDefault();
      doRedo();
      return;
    }
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      acceptAndNext();
      return;
    }
    if (typing(t) || mod || e.altKey) return;
    if (e.key === 'Escape') {
      leave();
      return;
    }
    const onControl = !!t?.closest('[data-handle], [role="slider"]');
    if (!onControl && e.key === 'ArrowLeft' && prevId !== null) {
      e.preventDefault();
      goTo(prevId);
    } else if (!onControl && e.key === 'ArrowRight' && nextId !== null) {
      e.preventDefault();
      goTo(nextId);
    } else if (e.key === 'x' || e.key === 'X') skip();
    else if (e.key === 'r') turn(1);
    else if (e.key === 'R') turn(-1);
    else if (e.key === 'b' || e.key === 'B') {
      if (!e.repeat) compareHeld = true;
    } else if (e.key === '+' || e.key === '=') stage?.zoomBy(1.25);
    else if (e.key === '-') stage?.zoomBy(1 / 1.25);
    else if (e.key === '0' || e.key === 'z' || e.key === 'Z') stage?.fit();
    else if (e.key === '1') stage?.actualSize();
  }

  function onKeyup(e: KeyboardEvent): void {
    if ((e.key === 'b' || e.key === 'B') && compareHeld) compareHeld = false;
  }

  onMount(() => {
    // Focus moves to the editor heading on entry (WCAG 2.4.3).
    document.querySelector<HTMLElement>('[data-route-heading]')?.focus({ preventScroll: true });
    return () => clearTimeout(sayTimer);
  });

  // ---- derived view state ---------------------------------------------------------------------------
  const failedBanner = $derived(!!item && needsDrawCrop(item, store.strictness));
  const canEdit = $derived(!!item && item.status === 'ready' && !!edit && !failedBanner);
  const reasons = $derived(item?.confidence?.reasons ?? []);
  const headline = $derived(item ? reasonLine(item, tier) : null);
  const statusWord = $derived.by(() => {
    if (!cls) return '';
    if (decision === 'accepted') return 'accepted';
    if (decision === 'skipped') return 'skipped';
    return cls.needs ? 'needs review' : tier;
  });
  const srcUrl = $derived(item ? imageUrl('src', item.id, item.gen) : '');
  const resultUrl = $derived(item ? imageUrl('result', item.id, item.gen) : '');
  const aspect = $derived(item && item.height > 0 ? item.width / item.height : 0.75);
</script>

<svelte:window onkeydown={onKeydown} onkeyup={onKeyup} />

{#if !item}
  <div class="page missing">
    <h1 data-route-heading tabindex="-1">{S.editor.notFound}</h1>
    <a class="btn btn-primary" href={href('/grid')}>{S.nav.backToGrid}</a>
  </div>
{:else}
  <div class="editor">
    <h1 class="sr-only" data-route-heading tabindex="-1">{S.editor.heading(item.name, position, queue.length, statusWord)}</h1>

    <header class="bar">
      <a class="btn back" href={href('/grid')} onclick={() => (store.lastOpened = id)}><Icon name="back" size={18} /> {S.nav.backToGrid}</a>
      <span class="name mono" title={item.name}>{item.name}</span>
      {#if position > 0}<span class="pos">{S.editor.positionOf(position, queue.length)}</span>{/if}
      <span class="chips">
        {#if item.status === 'ready'}
          <TierBadge tier={tier} label={headline ? `${S.tier[tier === 'analysing' ? 'check' : tier]}: ${headline}` : undefined} />
          {#each reasons.slice(1) as r, i (i)}
            <span class="badge neutral">{holdTitle(r)}</span>
          {/each}
        {/if}
        {#if item.edited}<span class="badge accent">{S.editor.editedChip}</span>{/if}
        {#if decision === 'skipped'}<span class="badge neutral">{S.editor.skippedChip}</span>{/if}
        {#if item.saved}<span class="badge good">{S.editor.savedChip}</span>{/if}
      </span>
      <span class="grow"></span>
      <button
        type="button"
        class="icon-btn"
        disabled={!item.canUndo}
        aria-label={item.canUndo ? `${S.editor.undo} ${item.undoLabel ?? ''}` : S.editor.nothingToUndo}
        title={item.canUndo ? `${S.editor.undo} ${item.undoLabel ?? ''}` : S.editor.nothingToUndo}
        onclick={doUndo}><Icon name="undo" /></button
      >
      <button
        type="button"
        class="icon-btn"
        disabled={!item.canRedo}
        aria-label={item.canRedo ? `${S.editor.redo} ${item.redoLabel ?? ''}` : S.editor.nothingToRedo}
        title={item.canRedo ? `${S.editor.redo} ${item.redoLabel ?? ''}` : S.editor.nothingToRedo}
        onclick={doRedo}><Icon name="redo" /></button
      >
      <button
        type="button"
        class="btn"
        aria-pressed={compare}
        title={S.editor.compareHint}
        disabled={!canEdit}
        onpointerdown={compareDown}
        onpointerup={compareUp}
        onpointercancel={() => (compareHeld = false)}
        onpointerleave={() => (compareHeld = false)}
        onclick={(e) => {
          if (e.detail === 0) compareLatched = !compareLatched; // keyboard activation
        }}><Icon name="compare" size={16} /> {S.editor.compare}</button
      >
      <button type="button" class="btn btn-primary" disabled={!canEdit && decision !== 'accepted'} aria-pressed={decision === 'accepted'} onclick={toggleAccepted}>
        {#if decision === 'accepted'}<Icon name="check2" size={16} stroke={2.5} /> {S.editor.accepted}{:else}{S.editor.accept}{/if}
      </button>
    </header>

    <div class="body">
      <div class="work">
        {#if item.status === 'analysing'}
          <div class="placeholder"><span class="skeleton fill"></span><p>{S.editor.analysing}</p></div>
        {:else if item.status === 'error'}
          <div class="placeholder"><p>{S.editor.loadError}</p></div>
        {:else}
          <CropStage
            bind:this={stage}
            src={srcUrl}
            name={item.name}
            fallbackAspect={aspect}
            quad={edit?.quad ?? null}
            showOverlay={canEdit && !compare}
            onpreview={(q) => (previewQuad = q)}
            oncommit={commitQuad}
          >
            {#if prevId !== null}
              <button type="button" class="navbtn left" data-nostage aria-label={S.editor.previous} onclick={() => goTo(prevId!)}><Icon name="back" size={22} stroke={2} /></button>
            {:else}
              <span class="navbtn left off" aria-hidden="true"><Icon name="back" size={22} stroke={2} /></span>
            {/if}
            {#if nextId !== null}
              <button type="button" class="navbtn right" data-nostage aria-label={S.editor.next} onclick={() => goTo(nextId!)}><Icon name="next" size={22} stroke={2} /></button>
            {:else}
              <span class="navbtn right off" aria-hidden="true"><Icon name="next" size={22} stroke={2} /></span>
            {/if}

            {#if canEdit && shownEdit}
              <ResultInset {srcUrl} {resultUrl} edit={shownEdit} {busy} {compare} name={item.name} />
            {/if}

            {#if failedBanner}
              <div class="banner" role="alert" data-nostage>
                <div class="banner-head">
                  <Icon name="failed" size={22} stroke={1.9} />
                  <div>
                    <div class="banner-title">{S.editor.failedBannerTitle}</div>
                    <div class="banner-body">{S.editor.failedBannerBody}</div>
                  </div>
                </div>
                <div class="banner-actions">
                  <button type="button" class="btn btn-primary btn-lg" onclick={doDrawCrop}><Icon name="crop" size={18} stroke={1.9} /> {S.editor.drawCrop}</button>
                  <button type="button" class="btn btn-lg" onclick={skip}>{S.editor.skip} <span class="kbd mono">X</span></button>
                  <span class="banner-note">{S.editor.failedDrawNote}</span>
                </div>
              </div>
            {/if}
          </CropStage>

          <div class="rulerbar">
            <button type="button" class="cbtn" aria-label={S.editor.rotateLeft90} disabled={!canEdit} onclick={() => turn(-1)}><Icon name="rotateLeft" /> {S.editor.left90}</button>
            <button type="button" class="cbtn sq" aria-label={S.editor.nudgeLeft} disabled={!canEdit} onclick={() => edit && commitAngle(edit.fineDeg - 0.1)}>−0.1</button>
            <Ruler value={edit?.fineDeg ?? 0} auto={item.autoEdit?.fineDeg ?? null} disabled={!canEdit} oncommit={(v) => commitAngle(v)} onlive={(v) => (liveAngle = v)} />
            <button type="button" class="cbtn sq" aria-label={S.editor.nudgeRight} disabled={!canEdit} onclick={() => edit && commitAngle(edit.fineDeg + 0.1)}>+0.1</button>
            <button type="button" class="cbtn" aria-label={S.editor.rotateRight90} disabled={!canEdit} onclick={() => turn(1)}>{S.editor.right90} <Icon name="rotateRight" /></button>
            <button type="button" class="cbtn auto" title={S.editor.autoAngle} disabled={!canEdit} onclick={() => commitAngle(item.autoEdit?.fineDeg ?? 0, S.editor.labels.autoAngle)}>{S.editor.auto}</button>
          </div>
        {/if}
      </div>

      <aside class="inspector" aria-label="Inspector">
        <section>
          <div class="sec-head"><h2>{S.editor.cropHeading}</h2><span class="note">{item.edited ? S.editor.outlineEdited : S.editor.outlineAuto}</span></div>
          {#if edit && !failedBanner}
            <div class="corners">
              <span class="cg-title">{S.editor.cornerPositions}</span>
              <div class="grid">
                {#each edit.quad as p, i (i)}
                  <span class="cn">{S.editor.cornersShort[i]}</span>
                  <input
                    class="input mono"
                    type="number"
                    step="0.1"
                    min="0"
                    max="100"
                    disabled={!canEdit}
                    aria-label={S.editor.cornerInput(S.editor.corners[i], 'x')}
                    value={toPercent(previewQuad?.[i].x ?? p.x)}
                    onchange={(e) => setCorner(i, 'x', e.currentTarget.value)}
                  />
                  <input
                    class="input mono"
                    type="number"
                    step="0.1"
                    min="0"
                    max="100"
                    disabled={!canEdit}
                    aria-label={S.editor.cornerInput(S.editor.corners[i], 'y')}
                    value={toPercent(previewQuad?.[i].y ?? p.y)}
                    onchange={(e) => setCorner(i, 'y', e.currentTarget.value)}
                  />
                {/each}
              </div>
            </div>
          {/if}
          <div class="btn-row">
            <button type="button" class="btn" disabled={!item.autoEdit || failedBanner} onclick={doReset}>{S.editor.resetToAuto}</button>
            {#if failedBanner}
              <button type="button" class="btn btn-primary" onclick={doDrawCrop}><Icon name="crop" size={16} stroke={1.9} /> {S.editor.drawCrop}</button>
            {/if}
          </div>
        </section>

        <section>
          <h2>{S.editor.whyHeading}</h2>
          {#if reasons.length > 0}
            <ul class="why">
              {#each reasons as r, i (i)}
                <li>
                  <div class="why-title">{holdTitle(r)}</div>
                  <div class="why-cause">{holdCause(r)}</div>
                  <div class="why-action">{holdAction(r)}</div>
                </li>
              {/each}
            </ul>
          {:else if tier === 'check' || tier === 'failed'}
            <p class="plain">{headline}</p>
          {:else}
            <p class="plain">{S.editor.whyNone}</p>
          {/if}
        </section>

        {#if item.saved}
          <section class="savedline">
            <div class="sv">{S.editor.savedState(item.saved.output, item.saved.copy)}</div>
            {#if item.dirtySinceSave}<div class="sv-note">{S.editor.dirtySinceSave}</div>{/if}
            {#if item.saved.backupId}
              <a class="btn btn-sm" href={href('/backups')}>{S.editor.restoreOriginal}</a>
            {/if}
          </section>
        {/if}

        <div class="foot-note"><Icon name="info" size={16} /> {S.editor.editingFromOriginal}</div>
      </aside>
    </div>

    <div class="actions">
      <button type="button" class="btn btn-lg" disabled={item.status !== 'ready'} aria-pressed={decision === 'skipped'} onclick={skip}>
        {decision === 'skipped' ? S.editor.unskip : S.editor.skip} <span class="kbd mono">X</span>
      </button>
      <button type="button" class="btn btn-lg" disabled={!canEdit || !item.autoEdit} onclick={doReset}>{S.editor.resetToAuto}</button>
      <span class="grow"></span>
      <span class="help">{S.editor.help}</span>
      <button type="button" class="btn btn-primary btn-lg" disabled={!canEdit} onclick={acceptAndNext}>
        {S.editor.acceptAndNext} <Icon name="next" size={18} stroke={2} />
      </button>
    </div>
  </div>
{/if}

<style>
  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .missing {
    align-items: center;
    justify-content: center;
    gap: 16px;
  }

  .bar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px 10px;
    padding: 8px 16px;
    min-height: 56px;
    background: var(--surface);
    border-bottom: 1px solid var(--line);
  }

  .back {
    border-color: transparent;
    background: transparent;
    padding: 0 12px 0 8px;
    gap: 4px;
  }

  .name {
    font-weight: 500;
    font-size: 14px;
  }

  .pos {
    font-size: 13px;
    color: var(--text-3);
  }

  .chips {
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .grow {
    flex: 1;
  }

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }

  .work {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--canvas);
  }

  .placeholder {
    flex: 1;
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--canvas-text-2);
  }

  .placeholder .fill {
    position: absolute;
    inset: 24px;
    border-radius: 12px;
    opacity: 0.4;
  }

  .placeholder p {
    position: relative;
  }

  .navbtn {
    position: absolute;
    top: 36%;
    width: 48px;
    height: 48px;
    margin-top: -24px;
    display: flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    border: 1px solid rgba(255, 255, 255, 0.3);
    background: rgba(255, 255, 255, 0.12);
    color: #fff;
  }

  .navbtn:hover:not(.off) {
    background: rgba(255, 255, 255, 0.22);
  }

  .navbtn:focus-visible {
    outline-color: #ffffff;
  }

  .navbtn.off {
    background: rgba(255, 255, 255, 0.05);
    border-color: rgba(255, 255, 255, 0.12);
    color: rgba(255, 255, 255, 0.35);
  }

  .navbtn.left {
    left: 12px;
  }

  .navbtn.right {
    right: 12px;
  }

  .banner {
    position: absolute;
    left: 50%;
    bottom: 24px;
    transform: translateX(-50%);
    width: min(820px, calc(100% - 32px));
    padding: 16px 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 14px;
    color: var(--check-text-strong);
    box-shadow: 0 8px 28px rgba(0, 0, 0, 0.4);
    cursor: default;
  }

  .banner-head {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    color: var(--check-fg);
  }

  .banner-title {
    font-size: 16px;
    font-weight: 600;
    color: var(--check-text-strong);
  }

  .banner-body {
    margin-top: 4px;
    font-size: 13px;
    line-height: 1.45;
    color: var(--check-text-strong);
  }

  .banner-actions {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
  }

  .banner-note {
    font-size: 12px;
    color: var(--check-text-strong);
  }

  .kbd {
    margin-left: 4px;
    font-size: 12px;
    color: var(--text-3);
  }

  .rulerbar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 16px;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
  }

  .cbtn {
    min-height: max(44px, var(--ctl-h));
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--canvas-btn);
    border: 1px solid var(--canvas-btn-line);
    border-radius: 8px;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
  }

  .cbtn.sq {
    width: 48px;
    padding: 0;
    justify-content: center;
    font-size: 12px;
  }

  .cbtn.auto {
    background: transparent;
    border-color: var(--canvas-accent);
    color: #bfd0ff;
    font-weight: 600;
  }

  .cbtn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.18);
  }

  .cbtn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .cbtn:focus-visible {
    outline-color: #ffffff;
  }

  .inspector {
    width: 320px;
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    overflow: auto;
    background: var(--surface);
    border-left: 1px solid var(--line);
  }

  .inspector section {
    padding: 16px 18px 14px;
    border-bottom: 1px solid var(--line-soft);
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  h2 {
    margin: 0;
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.06em;
    color: var(--text-2);
  }

  .note {
    font-size: 12px;
    color: var(--text-3);
  }

  .cg-title {
    font-size: 13px;
    font-weight: 500;
  }

  .corners {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .grid {
    display: grid;
    grid-template-columns: 34px repeat(2, minmax(0, 1fr));
    gap: 6px;
    align-items: center;
  }

  .cn {
    font-size: 12px;
    color: var(--text-2);
  }

  .grid .input {
    width: 100%;
    min-width: 0;
    min-height: var(--ctl-h-sm);
    padding: 0 6px;
  }

  .btn-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .why {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .why-title {
    font-weight: 600;
    font-size: 13px;
  }

  .why-cause {
    margin-top: 2px;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.4;
  }

  .why-action {
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-3);
  }

  .plain {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
  }

  .savedline .sv {
    font-size: 13px;
    font-weight: 500;
    color: var(--good-fg);
  }

  .savedline .sv-note {
    font-size: 12px;
    color: var(--check-fg);
  }

  .savedline .btn {
    align-self: flex-start;
  }

  .foot-note {
    margin: auto 18px 14px;
    padding-top: 12px;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .actions {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    min-height: 64px;
    padding: 8px 16px;
    background: var(--surface);
    border-top: 1px solid var(--line);
  }

  .help {
    font-size: 12px;
    color: var(--text-3);
  }

  @media (max-width: 900px) {
    .editor {
      overflow: auto;
    }

    .body {
      flex: none;
      flex-direction: column;
    }

    .work {
      min-height: 62vh;
    }

    .inspector {
      width: auto;
      border-left: 0;
      border-top: 1px solid var(--line);
      overflow: visible;
    }

    .help {
      display: none;
    }

    .rulerbar {
      flex-wrap: wrap;
    }
  }
</style>
