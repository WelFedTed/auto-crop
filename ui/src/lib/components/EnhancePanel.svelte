<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The Enhance section of the editor (docs/dev/enhance-engine.md, PLAN 5, B13): the looks with a plain line each, a
  // strength slider with a live preview while it moves, a compare control (hold to see the original, or a split view),
  // and, for a scan with several items, the scope (this item or all of them). The panel only ASKS: every choice goes up
  // through `onlook` and the Editor records it as ONE undo step. Nothing here applies a look by itself.
  import { onDestroy } from 'svelte';
  import { enhancePreviewUrl } from '../backend.ts';
  import { FULL, LOOKS, isEnhanced, modeOf, sameLook, strengthFor, strengthOf } from '../enhance.ts';
  import { S } from '../strings.ts';
  import type { Enhance, EnhanceMode } from '../types.ts';
  import Icon from './Icon.svelte';
  import SwapImage from './SwapImage.svelte';

  type Scope = 'this' | 'all';

  let {
    id,
    cropId,
    pageKey,
    look,
    committed,
    resultUrl,
    previewing = null,
    suggestedMode = null,
    fitsMode = null,
    multi = false,
    scope = 'this',
    total = 1,
    on = 0,
    customised = false,
    held = false,
    busy = false,
    onlook,
    onscope,
    onfollow,
  }: {
    id: number;
    cropId: number;
    /** Changes when the crop's page changes: an address that is the same for another page is not the same picture. */
    pageKey: string;
    /** What the controls show: the committed look, or the one just chosen while its commit is in flight. */
    look: Enhance;
    /** The look the committed picture (`resultUrl`) already has. */
    committed: Enhance;
    /** The committed picture of this crop, already enhanced with its effective look. */
    resultUrl: string;
    /** The suggestion's look while its Preview is on (nothing is recorded). */
    previewing?: { mode: EnhanceMode; strength: number } | null;
    /** The engine suggested this look for the page. */
    suggestedMode?: EnhanceMode | null;
    /** The black-and-white look that fits this page, an option. */
    fitsMode?: EnhanceMode | null;
    multi?: boolean;
    scope?: Scope;
    /** Included items on this scan, and how many of them are enhanced. */
    total?: number;
    on?: number;
    /** This item has a look of its own that differs from the scan's. */
    customised?: boolean;
    /** The scan waits for the person's OK (several files, or a curved page). */
    held?: boolean;
    busy?: boolean;
    onlook: (mode: EnhanceMode, strength: number, source: 'press' | 'drag' | 'key') => void;
    onscope: (scope: Scope) => void;
    onfollow: () => void;
  } = $props();

  const lookMode = $derived(modeOf(look));
  const lookPct = $derived(strengthOf(look));
  const sliderOn = $derived(lookMode !== 'original');

  // ---- the slider: a live value while it moves, committed when it lands ---------------------------------------------
  let live = $state<number | null>(null);
  let liveShown = $state<number | null>(null);
  let dragging = false;
  let liveTimer: ReturnType<typeof setTimeout> | undefined;
  let liveLast = 0;
  const sliderValue = $derived(live ?? lookPct);

  function onSlide(e: Event & { currentTarget: HTMLInputElement }): void {
    const v = Number(e.currentTarget.value);
    live = v;
    // The preview is asked for a few times a second, not for every pixel of the drag.
    clearTimeout(liveTimer);
    const wait = Math.max(0, 90 - (performance.now() - liveLast));
    liveTimer = setTimeout(() => {
      liveLast = performance.now();
      liveShown = live;
    }, wait);
  }

  function onLand(e: Event & { currentTarget: HTMLInputElement }): void {
    const v = Number(e.currentTarget.value);
    const source = dragging ? 'drag' : 'key';
    dragging = false;
    clearTimeout(liveTimer);
    live = null;
    liveShown = null;
    if (v !== lookPct) onlook(lookMode, v, source);
  }

  onDestroy(() => clearTimeout(liveTimer));

  // ---- what is on show -------------------------------------------------------------------------------------------------
  const shown = $derived.by(() => {
    if (previewing) return previewing;
    if (liveShown !== null && sliderOn) return { mode: lookMode, strength: liveShown };
    return { mode: lookMode, strength: lookPct };
  });
  const settled = $derived(!previewing && liveShown === null && sameLook(look, committed));
  const enhancedUrl = $derived(
    settled ? resultUrl : enhancePreviewUrl(liveShown !== null ? 'thumb' : 'result', id, cropId, shown.mode, shown.strength),
  );
  const enhancedOn = $derived(shown.mode !== 'original' && shown.strength > 0);

  let mainLoading = $state(false);
  let mainFailed = $state(false);
  /** The natural size of the picture on show and the size of its box: the split line is placed on the picture, not the box. */
  let natural = $state<[number, number]>([0, 0]);
  let boxW = $state(0);
  let boxH = $state(0);

  // ---- compare ---------------------------------------------------------------------------------------------------------
  let holding = $state(false);
  let latched = $state(false);
  let split = $state(false);
  let splitAt = $state(50);
  let downAt = 0;
  const showOriginal = $derived(enhancedOn && (holding || latched));
  const wantOriginal = $derived(enhancedOn && (holding || latched || split));
  /** The original, once asked for, stays loaded for this page (showing it again never fetches again). */
  let origUrl = $state('');
  let origPage = $state('');

  $effect(() => {
    if (origPage !== pageKey) {
      origPage = pageKey;
      origUrl = '';
    }
    // Two previews of the same kind supersede each other in the engine: the original waits until the look has arrived.
    if (wantOriginal && !mainLoading && origUrl === '') origUrl = enhancePreviewUrl('result', id, cropId, 'original', 0);
  });

  function holdDown(): void {
    holding = true;
    downAt = performance.now();
  }

  function holdUp(): void {
    if (!holding) return;
    holding = false;
    if (performance.now() - downAt < 350) latched = !latched;
  }

  function holdKeyDown(e: KeyboardEvent): void {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    if (!e.repeat) holdDown();
  }

  function holdKeyUp(e: KeyboardEvent): void {
    if (e.key !== ' ' && e.key !== 'Enter') return;
    e.preventDefault();
    holdUp();
  }

  $effect(() => {
    // Nothing to compare once the look is off: the toggles let go.
    if (!enhancedOn) {
      holding = false;
      latched = false;
    }
  });

  // ---- words -------------------------------------------------------------------------------------------------------------
  const lookName = (m: EnhanceMode): string => S.enhance.looks[m].name;
  const status = $derived.by(() => {
    if (showOriginal) return S.enhance.statusHeld;
    if (previewing) return S.enhance.statusPreview(lookName(previewing.mode), previewing.strength);
    if (!isEnhanced(look)) return S.enhance.statusOriginal;
    return S.enhance.statusApplied(lookName(lookMode), lookPct);
  });
  const caption = $derived(
    showOriginal || (!enhancedOn && !mainLoading) ? S.enhance.capOriginal : mainFailed ? S.enhance.failed : mainLoading ? S.enhance.capLoading : previewing ? S.enhance.capPreview : S.enhance.capEnhanced,
  );
  const splitText = $derived(`${S.enhance.splitSlider}: ${splitAt}%`);
  /** Where the split line is, as a percentage of the BOX: the picture is centred in it at its own aspect ratio. */
  const cutAt = $derived.by(() => {
    const [nw, nh] = natural;
    if (nw <= 0 || nh <= 0 || boxW <= 0 || boxH <= 0) return splitAt;
    const iw = nw * Math.min(boxW / nw, boxH / nh);
    return ((boxW - iw) / 2 + (splitAt / 100) * iw) / boxW * 100;
  });

  function choose(mode: EnhanceMode): void {
    if (mode === lookMode && !previewing) return;
    onlook(mode, strengthFor(look, mode), 'press');
  }

</script>

<section class="enh" data-enhance-panel aria-labelledby="enh-h">
  <div class="sec-head">
    <h2 id="enh-h">{S.enhance.heading.toUpperCase()}</h2>
    <span class="note" data-enhance-current>{lookName(lookMode)}{sliderOn ? ` ${lookPct}%` : ''}</span>
  </div>
  <p class="plain">{S.enhance.lead}</p>

  {#if multi}
    <div class="scope">
      <span class="cg-title" id="enh-scope-l">{S.enhance.scopeLabel}</span>
      <div class="seg wrap" role="group" aria-labelledby="enh-scope-l" data-enhance-scope>
        <button type="button" aria-pressed={scope === 'this'} onclick={() => onscope('this')}>{S.enhance.scopeThis}</button>
        <button type="button" aria-pressed={scope === 'all'} onclick={() => onscope('all')}>{S.enhance.scopeAll}</button>
      </div>
      {#if scope === 'all'}<p class="plain small">{S.enhance.scopeAllNote(total)}</p>{/if}
    </div>
    <p class="count" data-enhance-count><Icon name="sparkle" size={15} /> {S.enhance.onCount(on, total)}</p>
    {#if customised && scope === 'this'}
      <p class="plain small own" data-enhance-own>
        {S.enhance.customised}
        <button type="button" class="btn-link" disabled={busy} onclick={onfollow}>{S.enhance.followScan}</button>
      </p>
    {/if}
  {/if}

  <div class="looks" role="group" aria-label={S.enhance.looksLabel} data-enhance-looks>
    {#each LOOKS as m (m)}
      <button type="button" class="look" aria-pressed={lookMode === m} data-look={m} onclick={() => choose(m)}>
        <span class="ln">
          {S.enhance.looks[m].name}
          {#if suggestedMode === m}<span class="tag good" data-tag="suggested">{S.enhance.suggestedTag}</span>{/if}
          {#if fitsMode === m}<span class="tag" data-tag="fits">{S.enhance.fitsTag}</span>{/if}
        </span>
        <span class="ll">{S.enhance.looks[m].line}</span>
      </button>
    {/each}
  </div>

  <div class="strength" class:off={!sliderOn}>
    <div class="srow">
      <label for="enh-strength" class="cg-title">{S.enhance.strength}</label>
      <span class="sval mono" data-strength-value>{sliderOn ? S.enhance.strengthValue(sliderValue) : '–'}</span>
    </div>
    <input
      id="enh-strength"
      class="range"
      type="range"
      min="0"
      max={FULL}
      step="1"
      value={sliderValue}
      disabled={!sliderOn}
      aria-valuetext={sliderOn ? S.enhance.strengthValue(sliderValue) : S.enhance.strengthOff}
      onpointerdown={() => (dragging = true)}
      onpointercancel={() => (dragging = false)}
      oninput={onSlide}
      onchange={onLand}
    />
    <p class="plain small">{sliderOn ? S.enhance.strengthHelp : S.enhance.strengthOff}</p>
  </div>

  <figure class="view" data-enhance-preview>
    <div class="box" bind:clientWidth={boxW} bind:clientHeight={boxH}>
      <SwapImage
        url={enhancedUrl}
        version={pageKey}
        alt={enhancedOn ? S.enhance.previewAlt(lookName(shown.mode)) : S.enhance.originalAlt}
        onsize={(w, h) => (natural = [w, h])}
        onstate={(s) => {
          mainLoading = s.loading;
          mainFailed = s.failed;
        }}
      />
      <div class="orig" class:on={showOriginal} class:split style:--at="{cutAt}%">
        <SwapImage url={origUrl} version={pageKey} alt={showOriginal || split ? S.enhance.originalAlt : ''} />
      </div>
      {#if split && wantOriginal}
        <span class="divider" style:left="{cutAt}%" aria-hidden="true"></span>
      {/if}
    </div>
    <figcaption>
      <span>{caption}</span>
      {#if mainLoading}<span class="spin" aria-hidden="true"></span>{/if}
    </figcaption>
  </figure>

  <div class="cmp">
    <button
      type="button"
      class="btn btn-sm hold"
      aria-pressed={showOriginal}
      disabled={!enhancedOn}
      title={S.enhance.compareHint}
      data-enhance-hold
      onpointerdown={holdDown}
      onpointerup={holdUp}
      onpointercancel={() => (holding = false)}
      onpointerleave={() => holdUp()}
      onkeydown={holdKeyDown}
      onkeyup={holdKeyUp}
    >
      <Icon name="compare" size={16} /> {S.enhance.compareHold}
    </button>
    <label class="splitsw" class:dim={!enhancedOn}>
      <input type="checkbox" bind:checked={split} disabled={!enhancedOn} data-enhance-split />
      {S.enhance.splitView}
    </label>
  </div>
  {#if split && enhancedOn}
    <div class="splitrow">
      <label for="enh-split" class="small">{S.enhance.splitHint}</label>
      <input id="enh-split" class="range" type="range" min="0" max="100" step="1" bind:value={splitAt} aria-label={S.enhance.splitSlider} aria-valuetext={splitText} />
    </div>
  {/if}

  <div class="status" role="status" aria-live="polite" data-enhance-status>{status}</div>

  {#if held}<p class="plain small" data-enhance-held>{S.enhance.heldNote}</p>{/if}

  <details class="help" data-enhance-help>
    <summary>{S.enhance.helpTitle}</summary>
    <p>{S.enhance.helpClean}</p>
    <p>{S.enhance.helpBw}</p>
    <p>{S.enhance.helpOriginal}</p>
    <p>{S.enhance.helpPreview}</p>
  </details>
</section>

<style>
  .enh {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 16px 18px 14px;
    border-bottom: 1px solid var(--line-soft);
  }

  .sec-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
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

  .plain {
    margin: 0;
    font-size: 13px;
    color: var(--text-2);
  }

  .plain.small,
  .small {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.4;
  }

  .scope {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .cg-title {
    font-size: 13px;
    font-weight: 500;
  }

  .seg.wrap {
    flex-wrap: wrap;
    width: 100%;
  }

  .seg.wrap button {
    flex: 1 1 auto;
    justify-content: center;
    white-space: normal;
    text-align: center;
  }

  .count {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 500;
    color: var(--accent-text);
  }

  .own {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px 6px;
  }

  .looks {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .look {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    width: 100%;
    min-height: var(--ctl-h);
    padding: 8px 12px;
    text-align: left;
    background: var(--surface);
    border: 1px solid var(--line-strong);
    border-radius: 10px;
    color: var(--text);
  }

  .look:hover {
    background: var(--surface-2);
  }

  .look[aria-pressed='true'] {
    border-color: var(--accent);
    background: var(--accent-tint);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .ln {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    font-size: 14px;
    font-weight: 600;
  }

  .ll {
    font-size: 12px;
    line-height: 1.35;
    color: var(--text-2);
  }

  .tag.good {
    background: var(--good-bg);
    color: var(--good-fg);
  }

  .strength {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .strength.off {
    opacity: 0.7;
  }

  .srow {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
  }

  .sval {
    font-size: 13px;
    color: var(--text);
  }

  .range {
    width: 100%;
    min-height: 32px;
    accent-color: var(--accent);
  }

  .view {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .box {
    position: relative;
    width: 100%;
    height: clamp(220px, 36vh, 380px);
    overflow: hidden;
    border-radius: 10px;
    border: 1px solid var(--line-strong);
    background:
      repeating-conic-gradient(var(--tile-bg) 0% 25%, var(--surface-2) 0% 50%) 0 0 / 16px 16px;
  }

  .orig {
    position: absolute;
    inset: 0;
    opacity: 0;
  }

  .orig.on {
    opacity: 1;
  }

  .orig.split {
    opacity: 1;
    clip-path: inset(0 calc(100% - var(--at)) 0 0);
  }

  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: var(--accent);
    box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.7);
    pointer-events: none;
  }

  figcaption {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    color: var(--text-3);
  }

  .spin {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    border: 2px solid var(--line-strong);
    border-top-color: var(--accent);
  }

  @media (prefers-reduced-motion: no-preference) {
    .spin {
      animation: spin 0.8s linear infinite;
    }
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  .cmp {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px 14px;
  }

  .hold {
    touch-action: none;
    user-select: none;
  }

  .hold[aria-pressed='true'] {
    background: var(--accent-tint);
    border-color: var(--accent);
  }

  .splitsw {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    min-height: var(--ctl-h-sm);
    font-size: 13px;
  }

  .splitsw.dim {
    opacity: 0.5;
  }

  .splitrow {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .status {
    font-size: 12px;
    color: var(--text-2);
    min-height: 1.4em;
  }

  .help {
    font-size: 12px;
    line-height: 1.45;
    color: var(--text-2);
  }

  .help summary {
    cursor: pointer;
    font-weight: 600;
    color: var(--text);
    min-height: var(--ctl-h-sm);
    display: flex;
    align-items: center;
  }

  .help p {
    margin: 4px 0 0;
  }
</style>
