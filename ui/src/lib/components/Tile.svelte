<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { cropImageUrl, imageUrl } from '../backend.ts';
  import { bannerState, includedCrops, isCurvedScan, isSplitScan } from '../items.ts';
  import { needsDrawCrop, reasonLine, tileLabel, type Classified, type Strictness } from '../review.ts';
  import { href } from '../router.svelte.ts';
  import { S, noticeText } from '../strings.ts';
  import Icon from './Icon.svelte';
  import ScanBanner from './ScanBanner.svelte';
  import TierBadge from './TierBadge.svelte';

  let {
    x,
    strictness,
    selected,
    autoSaveSplits = false,
    ontoggle,
    onopen,
    onaccept,
    onreview,
  }: {
    x: Classified;
    strictness: Strictness;
    selected: boolean;
    autoSaveSplits?: boolean;
    ontoggle: (id: number) => void;
    onopen: (id: number) => void;
    /** "Accept split" on the tile's banner. */
    onaccept?: (id: number) => void;
    /** "Review items": open the editor on this scan. */
    onreview?: (id: number) => void;
  } = $props();

  const item = $derived(x.item);
  const split = $derived(isSplitScan(item));
  // A curved page is one file but waits for the person's OK like a split: it gets the same compact banner.
  const curvedOnly = $derived(!split && isCurvedScan(item));
  const banner = $derived(split || curvedOnly ? bannerState(item, { autoSaveSplits }) : ({ kind: 'none' } as const));
  const subs = $derived(split ? includedCrops(item) : []);
  let expanded = $state(false);
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
    {#if split}
      <span class="count" title={S.split.countBadgeLabel(item.split?.included ?? 0)} aria-hidden="true">{S.split.countBadge(item.split?.included ?? 0)}</span>
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
      {#if item.openOnly}
        <span class="badge neutral" title={noticeText(item.openOnly)} data-open-only>{S.grid.openOnlyBadge}</span>
      {/if}
      {#if reason && !curvedOnly}
        <div class="reason">{reason}</div>
      {/if}
    {/if}
  </div>
  {#if (split || curvedOnly) && banner.kind !== 'none'}
    <div class="splitbar">
      <ScanBanner state={banner} compact onaccept={() => onaccept?.(item.id)} onreview={() => onreview?.(item.id)} />
      {#if split}
      <button type="button" class="btn-link expand" aria-expanded={expanded} onclick={() => (expanded = !expanded)}>
        {expanded ? S.split.collapse : S.split.expand}
      </button>
      {/if}
      {#if split && expanded}
        <ul class="subs" aria-label={S.split.subtiles}>
          {#each subs as c (c.id)}
            <li class="sub-{c.band ?? 'check'}">
              <img src={cropImageUrl('thumb', item.id, c.id, c.renderKey)} alt="" loading="lazy" draggable="false" />
              <span class="sn">{c.order}</span>
              <span class="sb">{S.tier[c.band ?? 'check']}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
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

  .count {
    position: absolute;
    right: 6px;
    top: 6px;
    min-width: 34px;
    height: 24px;
    padding: 0 8px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 12px;
    background: var(--bulk-bg);
    color: var(--bulk-text);
    border: 2px solid #ffffff;
    font-size: 12px;
    font-weight: 700;
  }

  .splitbar {
    padding: 0 8px 10px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }

  .splitbar :global(.banner) {
    align-self: stretch;
  }

  .expand {
    padding: 0 2px;
  }

  .subs {
    list-style: none;
    margin: 0;
    padding: 0;
    align-self: stretch;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(64px, 1fr));
    gap: 6px;
  }

  .subs li {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    padding: 4px;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--surface-2);
    font-size: 11px;
  }

  .subs img {
    width: 100%;
    height: 52px;
    object-fit: contain;
    background: var(--tile-bg);
    border-radius: 4px;
  }

  .sn {
    position: absolute;
    left: 6px;
    top: 6px;
    min-width: 18px;
    height: 18px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 9px;
    background: var(--surface);
    border: 1px solid var(--line-strong);
    font-weight: 700;
  }

  .subs li.sub-good .sb {
    color: var(--good-fg);
  }

  .subs li.sub-check .sb {
    color: var(--check-fg);
  }

  .subs li.sub-failed .sb {
    color: var(--fail-fg);
  }

  .sb {
    font-weight: 600;
  }
</style>
