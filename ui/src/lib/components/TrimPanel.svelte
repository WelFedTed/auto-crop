<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The "Trim to content" preview under the picture: the dashed outline is the picture inside the border (drawn on the
  // stage), the inset shows the trimmed page, and Apply is the one button that changes the crop (one undo step); Cancel
  // and Esc keep the outline as it is. The status region is always in the page (empty when there is nothing to say) so a
  // screen reader announces what appears in it.
  import { S, errorMessage } from '../strings.ts';
  import type { TrimState } from '../trim.ts';
  import Icon from './Icon.svelte';

  let {
    state: st,
    busy = false,
    onapply,
    oncancel,
  }: {
    state: TrimState;
    /** A commit is in flight: Apply waits for it. */
    busy?: boolean;
    onapply: () => void;
    oncancel: () => void;
  } = $props();

  const phase = $derived(st.phase);
  const active = $derived(phase.kind === 'preview' || phase.kind === 'applied' || phase.kind === 'error');
  const why = $derived(phase.kind === 'preview' ? S.trim.why[phase.proposal.reason] : '');
  const head = $derived.by(() => {
    switch (phase.kind) {
      case 'preview':
        return S.trim.ready;
      case 'applied':
        return S.trim.applied;
      case 'error':
        return phase.code === 'ITEM_OP' || phase.code === 'UNKNOWN' ? S.trim.failed : `${S.trim.failed} ${errorMessage(phase.code)}`;
      default:
        return '';
    }
  });
</script>

<div class="trim" class:idle={!active} role="group" aria-label={S.trim.panelLabel} data-trim-panel data-phase={phase.kind}>
  <div class="status" role="status" aria-live="polite" data-trim-status>
    {#if head}<p class="head">{head}</p>{/if}
    {#if why}<p class="why" data-trim-why>{why}</p>{/if}
  </div>

  {#if phase.kind === 'preview'}
    <div class="row">
      <button type="button" class="act primary" data-trim-apply disabled={busy} title={S.trim.applyHint} onclick={onapply}>
        <Icon name="check2" size={16} stroke={2.4} /> {S.trim.apply}
      </button>
      <button type="button" class="act" data-trim-cancel title={S.trim.cancelHint} onclick={oncancel}>{S.trim.cancel}</button>
      <p class="legend">
        <span class="sw prop" aria-hidden="true"></span>{S.trim.legendProposed}
        <span class="sw cur" aria-hidden="true"></span>{S.trim.legendCurrent}
      </p>
    </div>
  {/if}
</div>

<style>
  .trim {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 8px 12px;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
    border-left: 4px solid #ff9f1c;
    color: var(--canvas-text);
    font-size: 13px;
    line-height: 1.4;
  }

  /* Present for the screen reader, invisible and out of the layout when nothing is going on. */
  .trim.idle {
    position: absolute;
    width: 1px;
    height: 1px;
    padding: 0;
    margin: -1px;
    overflow: hidden;
    clip-path: inset(50%);
    border: 0;
    white-space: nowrap;
  }

  p {
    margin: 0;
  }

  .head {
    font-weight: 600;
  }

  .why {
    color: var(--canvas-text-2);
  }

  .legend {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 4px 8px;
    margin-left: auto;
    color: var(--canvas-text-2);
    font-size: 12px;
  }

  .sw {
    display: inline-block;
    width: 22px;
    height: 0;
    border-top: 3px dashed;
  }

  .sw.prop {
    border-color: #ff9f1c;
  }

  .sw.cur {
    border-top-style: solid;
    border-color: #ffffff;
    margin-left: 8px;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .act {
    min-height: 40px;
    padding: 0 16px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--canvas-btn);
    border: 1px solid var(--canvas-btn-line);
    border-radius: 8px;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
  }

  .act:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.18);
  }

  .act:focus-visible {
    outline: 2px solid #ffffff;
    outline-offset: 2px;
  }

  .act:disabled {
    opacity: 0.5;
  }

  .act.primary {
    background: #ff9f1c;
    border-color: #ff9f1c;
    color: #15181e;
    font-weight: 600;
  }

  .act.primary:hover:not(:disabled) {
    background: #ffb13f;
  }
</style>
