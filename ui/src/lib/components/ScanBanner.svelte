<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The scan-level banner (M10.41): "3 items found: held for review" with Accept split and Review items.
  // Nothing is written for a held scan, and by default every split waits for the person's OK (0.x rule).
  // Used in the editor (full) and on a grid tile (compact).
  import type { BannerState } from '../items.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    state,
    compact = false,
    busy = false,
    onaccept,
    onreview,
    onwithdraw,
    ondraw,
    ontreatasone,
    onskip,
    onstraight,
  }: {
    state: BannerState;
    compact?: boolean;
    busy?: boolean;
    onaccept?: () => void;
    onreview?: () => void;
    onwithdraw?: () => void;
    ondraw?: () => void;
    ontreatasone?: () => void;
    onskip?: () => void;
    /** "Back to straight" on a curved page. */
    onstraight?: () => void;
  } = $props();

  const tone = $derived(
    state.kind === 'held' || state.kind === 'curved' ? 'check' : state.kind === 'noItems' ? 'failed' : state.kind === 'accepted' || state.kind === 'curvedAccepted' ? 'good' : 'info',
  );
  const title = $derived(
    state.kind === 'curved'
      ? S.curved.held
      : state.kind === 'curvedAccepted'
        ? S.curved.accepted
        : state.kind === 'held'
      ? S.split.held(state.items)
      : state.kind === 'ready'
        ? S.split.ready(state.items)
        : state.kind === 'accepted'
          ? S.split.accepted(state.items)
          : state.kind === 'auto'
            ? S.split.auto(state.items)
            : state.kind === 'noItems'
              ? S.split.noItems
              : '',
  );
  const note = $derived(
    state.kind === 'curved'
      ? S.curved.heldNote
      : state.kind === 'curvedAccepted'
        ? S.curved.acceptedNote
        : state.kind === 'held'
      ? `${S.split.heldNeed(state.need)} ${S.split.heldNote}`
      : state.kind === 'ready'
        ? S.split.readyNote
        : state.kind === 'accepted'
          ? S.split.acceptedNote
          : state.kind === 'auto'
            ? S.split.autoNote
            : state.kind === 'noItems'
              ? S.split.noItemsNote
              : '',
  );
  const icon = $derived(tone === 'good' ? 'good' : tone === 'failed' ? 'failed' : tone === 'check' ? 'check' : 'info');
</script>

{#if state.kind !== 'none'}
  <div class="banner {tone}" class:compact role="status" data-banner={state.kind}>
    <span class="ico"><Icon name={icon} size={compact ? 16 : 22} stroke={1.9} /></span>
    <div class="text">
      <div class="title">{title}</div>
      {#if !compact}<div class="note">{note}</div>{/if}
    </div>
    <div class="actions">
      {#if state.kind === 'curved'}
        <button type="button" class="btn btn-primary" class:btn-sm={compact} disabled={busy} onclick={onaccept}>{S.curved.accept}</button>
        {#if onstraight}<button type="button" class="btn" class:btn-sm={compact} disabled={busy} onclick={onstraight}>{S.curved.backToStraight}</button>{/if}
      {:else if state.kind === 'curvedAccepted'}
        <button type="button" class="btn" class:btn-sm={compact} disabled={busy} onclick={onwithdraw}>{S.curved.withdraw}</button>
      {:else if state.kind === 'held' || state.kind === 'ready'}
        <button type="button" class="btn btn-primary" class:btn-sm={compact} disabled={busy} onclick={onaccept}>{S.split.accept}</button>
        <button type="button" class="btn" class:btn-sm={compact} onclick={onreview}>{S.split.review}</button>
      {:else if state.kind === 'accepted'}
        <button type="button" class="btn" class:btn-sm={compact} disabled={busy} onclick={onwithdraw}>{S.split.withdraw}</button>
      {:else if state.kind === 'auto'}
        <button type="button" class="btn" class:btn-sm={compact} onclick={onreview}>{S.split.review}</button>
      {:else if state.kind === 'noItems'}
        <button type="button" class="btn btn-primary" class:btn-sm={compact} onclick={ondraw}>{S.split.drawItems}</button>
        <button type="button" class="btn" class:btn-sm={compact} onclick={ontreatasone}>{S.split.treatAsOne}</button>
        {#if onskip}<button type="button" class="btn" class:btn-sm={compact} onclick={onskip}>{S.split.skip}</button>{/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .banner {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 14px;
    padding: 10px 16px;
    border: 1px solid var(--line);
    border-radius: 12px;
  }

  .banner.compact {
    padding: 6px 8px;
    gap: 6px 8px;
    border-radius: 8px;
  }

  .banner.check {
    background: var(--check-bg);
    border-color: var(--check-line);
    color: var(--check-text-strong);
  }

  .banner.good {
    background: var(--good-bg);
    border-color: var(--good-line);
    color: var(--good-fg);
  }

  .banner.failed {
    background: var(--fail-bg);
    border-color: var(--fail-line);
    color: var(--fail-fg);
  }

  .banner.info {
    background: var(--info-bg);
    border-color: var(--info-line);
    color: var(--info-text);
  }

  .ico {
    display: inline-flex;
    flex-shrink: 0;
  }

  .text {
    flex: 1;
    min-width: 200px;
  }

  .compact .text {
    min-width: 120px;
  }

  .title {
    font-weight: 600;
    font-size: 15px;
  }

  .compact .title {
    font-size: 12px;
  }

  .note {
    margin-top: 2px;
    font-size: 13px;
    line-height: 1.4;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .compact .actions {
    gap: 6px;
  }

  .actions .btn:not(.btn-primary) {
    color: var(--text);
  }

  /* On a short window the stage needs every pixel: the one-line title says it, the note repeats it. */
  @media (max-height: 760px) {
    .banner:not(.compact) .note {
      display: none;
    }

    .banner:not(.compact) {
      padding-block: 6px;
    }
  }
</style>
