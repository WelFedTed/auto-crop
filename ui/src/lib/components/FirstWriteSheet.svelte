<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { api } from '../backend.ts';
  import { formatBytes } from '../format.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    open,
    count,
    copy,
    days,
    onchoose,
    oncancel,
  }: {
    open: boolean;
    count: number;
    /** The mode that is on right now (Settings.saveAsCopy). */
    copy: boolean;
    days: number | null;
    /** `copy` is the mode the person picked; the caller stores it and acknowledges the sheet. */
    onchoose: (copy: boolean) => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let free = $state<string | null>(null);

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      dialog.showModal();
      free = null;
      api
        .listBackups()
        .then((v) => {
          free = v.freeBytes === null ? null : formatBytes(v.freeBytes);
        })
        .catch(() => {});
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });
</script>

<dialog bind:this={dialog} aria-labelledby="fw-title" oncancel={(e) => { e.preventDefault(); oncancel(); }}>
  <div class="sheet">
    <div class="head">
      <span class="ico"><Icon name="shield" size={22} /></span>
      <h2 id="fw-title">{S.firstWrite.title}</h2>
    </div>
    <p>{copy ? S.firstWrite.bodyCopy(count) : S.firstWrite.bodyReplace(count)}</p>
    <dl class="facts">
      <div><dt>{S.firstWrite.mode}</dt><dd>{copy ? S.firstWrite.modeCopy : S.firstWrite.modeReplace}</dd></div>
      {#if !copy}
        <div><dt>{S.firstWrite.keptFor}</dt><dd>{S.firstWrite.keptForValue(days)}</dd></div>
        {#if free}<div><dt>{S.firstWrite.free}</dt><dd>{S.firstWrite.freeValue(free)}</dd></div>{/if}
      {/if}
    </dl>
    <div class="buttons">
      <button type="button" class="btn btn-primary btn-lg" onclick={() => onchoose(copy)}>
        {copy ? S.firstWrite.saveCopies : S.firstWrite.replace}
      </button>
      <button type="button" class="btn btn-lg alt" onclick={() => onchoose(!copy)}>
        {copy ? S.firstWrite.replaceInstead : S.firstWrite.copies}
      </button>
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

  .facts {
    margin: 0;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--surface-2);
    border: 1px solid var(--line-soft);
    border-radius: 10px;
    font-size: 13px;
  }

  .facts div {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  dt {
    color: var(--text-3);
  }

  dd {
    margin: 0;
    font-weight: 500;
    text-align: right;
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 4px;
  }

  .buttons .btn-lg {
    width: 100%;
  }

  .alt {
    font-weight: 500;
  }
</style>
