<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The "Suggest curves" result under the picture: looking, the preview with Apply and Dismiss, the plain-language
  // reading of the engine's honesty fields (low confidence, no clear curve, nearly straight), applied, or a failure.
  // The status region is always in the page (empty when idle) so a screen reader announces what appears in it. The
  // suggestion is only ever a PREVIEW here; Apply is the one button that changes the crop, and it is always there.
  import { suggestionLines, type SuggestError, type SuggestState } from '../suggest.ts';
  import { S, errorMessage } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    state: st,
    busy = false,
    onapply,
    ondismiss,
    onretry,
  }: {
    state: SuggestState;
    /** A commit is in flight: Apply waits for it. */
    busy?: boolean;
    onapply: () => void;
    ondismiss: () => void;
    onretry: () => void;
  } = $props();

  const phase = $derived(st.phase);
  const active = $derived(phase.kind === 'loading' || phase.kind === 'preview' || phase.kind === 'applied' || phase.kind === 'error');
  const lines = $derived(phase.kind === 'preview' ? suggestionLines(phase.notes) : []);
  const bend = $derived.by(() => {
    if (phase.kind !== 'preview' || phase.notes.noCurve || phase.notes.bent.length === 0) return '';
    return S.suggest.bendOf(phase.notes.bent, phase.notes.peak);
  });

  function errorLine(code: SuggestError): string {
    if (code === 'NOT_READY') return S.suggest.notReady;
    if (code === 'UNKNOWN') return S.suggest.failed;
    return `${S.suggest.failed} ${errorMessage(code)}`;
  }

  const head = $derived.by(() => {
    switch (phase.kind) {
      case 'loading':
        return S.suggest.loading;
      case 'preview':
        return phase.notes.noCurve ? S.suggest.nothing : S.suggest.ready;
      case 'applied':
        return S.suggest.applied;
      case 'error':
        return errorLine(phase.code);
      default:
        return '';
    }
  });
</script>

<div class="sugg" class:idle={!active} role="group" aria-label={S.suggest.panelLabel} data-suggest-panel data-phase={phase.kind}>
  <div class="status" role="status" aria-live="polite" data-suggest-status>
    {#if head}
      <p class="head">
        {#if phase.kind === 'loading'}<span class="spin" aria-hidden="true"></span>{/if}
        {head}
      </p>
    {/if}
    {#if phase.kind === 'preview'}
      {#if bend}<p class="bend" data-suggest-bend>{bend}</p>{/if}
      {#if lines.length > 0}
        <ul class="notes" data-suggest-notes>
          {#each lines as line (line)}
            <li><Icon name="check" size={15} stroke={2} /> <span>{line}</span></li>
          {/each}
        </ul>
      {/if}
    {/if}
  </div>

  {#if phase.kind === 'preview'}
    {#if !phase.notes.noCurve}
      <p class="guess">{S.suggest.guess}</p>
    {/if}
    <div class="row">
      <button type="button" class="act primary" data-suggest-apply disabled={busy} title={S.suggest.applyHint} onclick={onapply}>
        <Icon name="check2" size={16} stroke={2.4} /> {S.suggest.apply}
      </button>
      <button type="button" class="act" data-suggest-dismiss title={S.suggest.dismissHint} onclick={ondismiss}>{S.suggest.dismiss}</button>
      {#if !phase.notes.noCurve}
        <p class="legend">
          <span class="sw sug" aria-hidden="true"></span>{S.suggest.legendSuggested}
          <span class="sw cur" aria-hidden="true"></span>{S.suggest.legendCurrent}
        </p>
      {/if}
    </div>
  {:else if phase.kind === 'loading'}
    <div class="row">
      <button type="button" class="act" data-suggest-dismiss onclick={ondismiss}>{S.suggest.dismiss}</button>
    </div>
  {:else if phase.kind === 'error'}
    <div class="row">
      <button type="button" class="act" data-suggest-retry onclick={onretry}>{S.suggest.retry}</button>
      <button type="button" class="act" data-suggest-dismiss onclick={ondismiss}>{S.suggest.dismiss}</button>
    </div>
  {/if}
</div>

<style>
  .sugg {
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
  .sugg.idle {
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

  p,
  ul {
    margin: 0;
  }

  .head {
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .bend {
    color: var(--canvas-text-2);
  }

  .notes {
    list-style: none;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .notes li {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 6px 10px;
    background: rgba(255, 159, 28, 0.16);
    border: 1px solid rgba(255, 159, 28, 0.55);
    border-radius: 8px;
    color: var(--canvas-text);
  }

  .notes li :global(svg) {
    margin-top: 2px;
    color: #ffb84d;
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
    border-top: 3px solid;
  }

  .sw.sug {
    border-color: #ff9f1c;
  }

  .sw.cur {
    border-top-style: dashed;
    border-color: #7c9bff;
    margin-left: 8px;
  }

  .guess {
    color: var(--canvas-text-2);
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

  .spin {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    border: 2px solid rgba(255, 255, 255, 0.35);
    border-top-color: #ff9f1c;
    flex-shrink: 0;
  }

  /* Reduced motion keeps the ring still: the words say what is going on. */
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
</style>
