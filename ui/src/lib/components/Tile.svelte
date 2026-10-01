<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { imageUrl } from '../backend.ts';
  import { needsDrawCrop, reasonLine, tileLabel, type Classified, type Strictness } from '../review.ts';
  import { href } from '../router.svelte.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';
  import TierBadge from './TierBadge.svelte';

  let {
    x,
    strictness,
    selected,
    ontoggle,
    onopen,
  }: {
    x: Classified;
    strictness: Strictness;
    selected: boolean;
    ontoggle: (id: number) => void;
    onopen: (id: number) => void;
  } = $props();

  const item = $derived(x.item);
  const label = $derived(tileLabel(x));
  const reason = $derived(reasonLine(item, x.tier));
  const draw = $derived(needsDrawCrop(item, strictness));
  const meta = $derived.by(() => {
    if (x.decision === 'skipped') return S.grid.meta.skipped;
    if (x.decision === 'accepted') return S.grid.meta.accepted;
    if (item.edited) return S.grid.meta.edited;
    if (item.saved) return S.grid.meta.saved;
    return '';
  });
  let broken = $state(false);
</script>

<article class="tile" class:selected data-tile-id={item.id}>
  <div class="media">
    {#if x.tier === 'analysing'}
      <div class="thumb skeleton" role="img" aria-label={label}>
        <span class="busy"><Icon name="analysing" size={16} /> {S.grid.meta.analysing}</span>
      </div>
    {:else if item.status === 'error'}
      <div class="thumb err" role="img" aria-label={label}>
        <Icon name="failed" size={28} />
      </div>
    {:else}
      <a class="thumb" href={href(`/item/${item.id}`)} aria-label={label} onclick={() => onopen(item.id)}>
        {#if !broken}
          <img src={imageUrl('thumb', item.id, item.gen)} alt="" loading="lazy" draggable="false" onerror={() => (broken = true)} />
        {/if}
        {#if draw}
          <span class="banner"><Icon name="crop" size={14} stroke={2} /> {S.grid.drawCrop}</span>
        {/if}
      </a>
    {/if}
    {#if x.tier !== 'analysing'}
      <button
        type="button"
        class="check"
        role="checkbox"
        aria-checked={selected}
        aria-label={S.grid.selectItem(item.name)}
        onclick={() => ontoggle(item.id)}
      >
        {#if selected}<Icon name="check2" size={14} stroke={3} />{/if}
      </button>
    {/if}
  </div>
  <div class="body">
    <div class="line">
      <span class="name mono" title={item.name}>{item.name}</span>
      {#if meta}<span class="meta">{meta}</span>{/if}
    </div>
    {#if x.tier !== 'analysing'}
      {#if item.status === 'error'}
        <TierBadge tier="failed" label={S.tier.failed} />
      {:else}
        <TierBadge tier={x.tier} />
      {/if}
      {#if reason}
        <div class="reason">{reason}</div>
      {/if}
    {/if}
  </div>
</article>

<style>
  .tile {
    display: flex;
    flex-direction: column;
    height: fit-content;
    overflow: hidden;
    background: var(--surface);
    border: 1px solid var(--line);
    border-radius: 12px;
  }

  .tile.selected {
    border: 2px solid var(--accent);
    margin: -1px;
  }

  .media {
    position: relative;
  }

  .thumb {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: center;
    aspect-ratio: 1 / 1.05;
    overflow: hidden;
    background: var(--tile-bg);
    color: var(--text-3);
    text-decoration: none;
  }

  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  a.thumb:focus-visible {
    outline-offset: -3px;
  }

  .busy {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 10px;
    border-radius: 10px;
    background: var(--surface);
    font-size: 12px;
    font-weight: 500;
    color: var(--text-2);
  }

  .err {
    color: var(--fail-fg);
  }

  .banner {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    height: 30px;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    background: var(--check-bg);
    color: var(--check-fg);
    font-size: 12px;
    font-weight: 600;
  }

  .check {
    position: absolute;
    left: 6px;
    top: 6px;
    width: 32px;
    height: 32px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 0;
    color: #fff;
  }

  /* the visible box is 24 px; the button around it is the 32 px (44 px on touch) target */
  .check::before {
    content: '';
    position: absolute;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.92);
    border: 2px solid #5b6372;
  }

  .check[aria-checked='true']::before {
    background: var(--accent);
    border-color: #fff;
  }

  .check :global(svg) {
    position: relative;
  }

  :global(:root[data-input='touch']) .check {
    width: 44px;
    height: 44px;
    left: 2px;
    top: 2px;
  }

  .body {
    padding: 9px 11px 11px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }

  .line {
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
  }

  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-2);
  }

  .meta {
    flex-shrink: 0;
    font-size: 11px;
    font-weight: 500;
    color: var(--text-2);
  }

  .reason {
    font-size: 12px;
    line-height: 1.35;
    color: var(--text);
    min-height: 32px;
  }
</style>
