<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The one-time explanation the first time Curved edges is entered: what it fixes, what it does not, and that nothing
  // is written until the page is accepted. "Got it" remembers (settings.curvedIntroAck) and carries on into Curved;
  // "Not now" closes it and changes nothing, so it is asked again next time.
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    open,
    suggest = false,
    ongo,
    oncancel,
  }: {
    open: boolean;
    /** The person came through "Suggest curves": the button then says so. */
    suggest?: boolean;
    ongo: () => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  });
</script>

<dialog bind:this={dialog} aria-labelledby="fc-title" oncancel={(e) => { e.preventDefault(); oncancel(); }} data-first-curved>
  <div class="sheet">
    <div class="head">
      <span class="ico"><Icon name="curve" size={22} stroke={2} /></span>
      <h2 id="fc-title">{S.curvedIntro.title}</h2>
    </div>
    <p>{S.curvedIntro.lead}</p>
    <dl>
      <dt>{S.curvedIntro.fixesTitle}</dt>
      <dd>{S.curvedIntro.fixes}</dd>
      <dt>{S.curvedIntro.doesNotTitle}</dt>
      <dd>{S.curvedIntro.doesNot}</dd>
      <dt>{S.curvedIntro.safeTitle}</dt>
      <dd>{S.curvedIntro.safe}</dd>
    </dl>
    <div class="buttons">
      <button type="button" class="btn btn-primary btn-lg" data-curved-intro-go onclick={ongo}>{suggest ? S.curvedIntro.goSuggest : S.curvedIntro.go}</button>
      <button type="button" class="btn-link" onclick={oncancel}>{S.curvedIntro.notNow}</button>
    </div>
  </div>
</dialog>

<style>
  dialog {
    width: min(480px, calc(100vw - 32px));
    max-height: calc(100vh - 32px);
    padding: 0;
    border: 0;
    border-radius: 16px;
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-lg);
  }

  dialog::backdrop {
    background: var(--scrim);
  }

  .sheet {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 24px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .ico {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: var(--accent-tint);
    color: var(--accent-text);
    display: flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
  }

  h2 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
  }

  p {
    margin: 0;
    line-height: 1.5;
    color: var(--text-2);
  }

  dl {
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  dt {
    margin-top: 8px;
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
  }

  dd {
    margin: 0;
    line-height: 1.5;
    color: var(--text-2);
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .buttons .btn-lg {
    width: 100%;
    white-space: normal;
    text-align: center;
  }
</style>
