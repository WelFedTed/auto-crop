<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The one-time sheet before the first split that replaces a scan (PLAN 6.2.6, M10.43): "1 scan becomes 4 files".
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    open,
    count,
    first,
    onreplace,
    onkeep,
    oncancel,
  }: {
    open: boolean;
    count: number;
    /** The first planned file name, for the example. */
    first: string;
    onreplace: () => void;
    onkeep: () => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    else if (!open && dialog.open) dialog.close();
  });
</script>

<dialog bind:this={dialog} aria-labelledby="fs-title" oncancel={(e) => { e.preventDefault(); oncancel(); }} data-first-split>
  <div class="sheet">
    <div class="head">
      <span class="ico"><Icon name="shield" size={22} /></span>
      <h2 id="fs-title">{S.save.firstSplitTitle}</h2>
    </div>
    <p>{S.save.firstSplitBody(count, first)}</p>
    <div class="buttons">
      <button type="button" class="btn btn-primary btn-lg" onclick={onreplace}>{S.save.firstSplitReplace}</button>
      <button type="button" class="btn btn-lg alt" onclick={onkeep}>{S.save.firstSplitKeep}</button>
      <button type="button" class="btn-link" onclick={oncancel}>{S.firstWrite.cancel}</button>
    </div>
  </div>
</dialog>

<style>
  dialog {
    width: min(460px, calc(100vw - 32px));
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
    gap: 16px;
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

  .alt {
    font-weight: 500;
  }
</style>
