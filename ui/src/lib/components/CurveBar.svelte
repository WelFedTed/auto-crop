<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The curved-edges tools under the source view: the Straight | Curved switch of the selected crop, the point
  // actions (Add point, Remove point, Straighten edge, Reset curves: every drag has a button, WCAG 2.5.7) and the
  // Flattened | Straight crop switch of the result picture. All of them are plain buttons, so the keyboard and a
  // screen reader reach everything a pointer does. The handles themselves live on the stage (CropStage).
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    curved,
    busy = false,
    view,
    canAdd,
    canRemove,
    canStraighten,
    canReset,
    selectedLabel,
    onmode,
    onview,
    onadd,
    onremove,
    onstraighten,
    onreset,
  }: {
    curved: boolean;
    busy?: boolean;
    view: 'flattened' | 'straight';
    canAdd: boolean;
    canRemove: boolean;
    canStraighten: boolean;
    canReset: boolean;
    /** What is selected, spoken and shown: "Top edge, point 2 of 3", or an invitation to select. */
    selectedLabel: string;
    onmode: (curved: boolean) => void;
    onview: (v: 'flattened' | 'straight') => void;
    onadd: () => void;
    onremove: () => void;
    onstraighten: () => void;
    onreset: () => void;
  } = $props();
</script>

<div class="bar" role="toolbar" aria-label={S.curved.toolbarLabel} data-curve-bar>
  <div class="seg" role="group" aria-label={S.curved.modeLabel}>
    <button type="button" aria-pressed={!curved} disabled={busy} title={S.curved.straightHint} onclick={() => curved && onmode(false)}>{S.curved.straight}</button>
    <button type="button" aria-pressed={curved} disabled={busy} title={S.curved.curvedHint} onclick={() => !curved && onmode(true)}>
      <Icon name="curve" size={15} stroke={2} /> {S.curved.curved}
    </button>
  </div>

  {#if curved}
    <span class="sep" aria-hidden="true"></span>
    <button type="button" class="cbtn" disabled={busy || !canAdd} title={S.curved.addPointHint} onclick={onadd}><Icon name="plus" size={15} stroke={2} /> {S.curved.addPoint}</button>
    <button type="button" class="cbtn" disabled={busy || !canRemove} aria-label={S.curved.removePoint} title={S.curved.removePointHint} onclick={onremove}><Icon name="minus" size={15} stroke={2} /> {S.curved.removeShort}</button>
    <button type="button" class="cbtn" disabled={busy || !canStraighten} aria-label={S.curved.straightenEdge} title={S.curved.straightenEdgeHint} onclick={onstraighten}>{S.curved.straightenShort}</button>
    <button type="button" class="cbtn" disabled={busy || !canReset} aria-label={S.curved.resetCurves} title={S.curved.resetCurvesHint} onclick={onreset}>{S.curved.resetShort}</button>
    <span class="sep" aria-hidden="true"></span>
    <div class="seg" role="group" aria-label={S.curved.viewLabel}>
      <button type="button" aria-pressed={view === 'flattened'} title={S.curved.viewFlattenedHint} onclick={() => onview('flattened')}>{S.curved.flattened}</button>
      <button type="button" aria-pressed={view === 'straight'} title={S.curved.viewStraightHint} onclick={() => onview('straight')}>{S.curved.straightCrop}</button>
    </div>
    <span class="sr-only" role="status" data-curve-selected>{selectedLabel}</span>
  {:else}
    <span class="msg">{S.curved.curvedHint}</span>
  {/if}
</div>

<style>
  .bar {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
    color: var(--canvas-text);
  }

  .seg {
    background: rgba(255, 255, 255, 0.12);
  }

  .seg button {
    color: var(--canvas-text-2);
    min-height: 40px;
    gap: 6px;
  }

  .seg button[aria-pressed='true'] {
    background: rgba(255, 255, 255, 0.92);
    color: #15181e;
  }

  .seg button:focus-visible,
  .cbtn:focus-visible {
    outline-color: #ffffff;
  }

  .seg button:disabled {
    opacity: 0.6;
  }

  .sep {
    width: 1px;
    align-self: stretch;
    margin: 4px 2px;
    background: var(--canvas-line);
  }

  .cbtn {
    min-height: 40px;
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

  .cbtn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.18);
  }

  .cbtn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .msg {
    font-size: 13px;
    color: var(--canvas-text-2);
    flex: 1 1 220px;
    min-width: 0;
  }

  .msg {
    color: var(--canvas-text);
  }
</style>
