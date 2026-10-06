<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // Restore dialog for a split scan (PLAN 6.2.7, M10.44): "Restore scan.jpg. 4 files were made from it.
  // ( ) Keep them  ( ) Remove them". Keep is preselected. Files that were edited since they were saved are listed
  // and are kept either way (the engine moves only files that are still exactly as saved, and never deletes).
  import { formatBytes } from '../format.ts';
  import { S } from '../strings.ts';
  import type { DerivedAction, DerivedFile } from '../types.ts';
  import Icon from './Icon.svelte';

  let {
    open,
    title,
    files,
    busy = false,
    onrestore,
    oncancel,
  }: {
    open: boolean;
    title: string;
    /** Every file made from the scan(s) being restored. */
    files: DerivedFile[];
    busy?: boolean;
    onrestore: (action: DerivedAction) => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let action = $state<DerivedAction>('keep');

  const changed = $derived(files.filter((f) => f.state === 'changed'));
  const missing = $derived(files.filter((f) => f.state === 'missing'));

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      action = 'keep'; // Keep is always preselected
      dialog.showModal();
    } else if (!open && dialog.open) dialog.close();
  });

  const stateNote = (f: DerivedFile) => S.backups.derivedState[f.state] ?? '';
</script>

<dialog bind:this={dialog} aria-labelledby="rd-title" oncancel={(e) => { e.preventDefault(); oncancel(); }} data-restore-dialog>
  <div class="sheet">
    <div class="head">
      <span class="ico"><Icon name="backups" size={22} /></span>
      <h2 id="rd-title">{title}</h2>
    </div>
    <p>{S.restoreDialog.body(files.length)}</p>
    <ul class="files" aria-label={S.backups.derivedHeading(files.length)}>
      {#each files as f (f.name)}
        <li class={f.state}>
          <span class="n mono">{f.name}</span>
          <span class="s">{formatBytes(f.bytes)} · {stateNote(f)}</span>
        </li>
      {/each}
    </ul>
    <fieldset>
      <legend class="sr-only">{S.restoreDialog.legend}</legend>
      <label class="opt" class:on={action === 'keep'}>
        <input type="radio" name="derived" value="keep" checked={action === 'keep'} onchange={() => (action = 'keep')} />
        <span><b>{S.restoreDialog.keep}</b><span class="d">{S.restoreDialog.keepNote}</span></span>
      </label>
      <label class="opt" class:on={action === 'remove'}>
        <input type="radio" name="derived" value="remove" checked={action === 'remove'} onchange={() => (action = 'remove')} />
        <span><b>{S.restoreDialog.remove}</b><span class="d">{S.restoreDialog.removeNote}</span></span>
      </label>
    </fieldset>
    {#if changed.length > 0}
      <div class="warn" role="note" data-changed-note>
        <b>{S.restoreDialog.changedHeading}</b>
        {changed.map((f) => f.name).join(', ')}
      </div>
    {/if}
    {#if missing.length > 0}
      <div class="sub">{missing.map((f) => f.name).join(', ')}: {S.restoreDialog.missingFileNote}</div>
    {/if}
    <div class="buttons">
      <button type="button" class="btn btn-primary btn-lg" disabled={busy} onclick={() => onrestore(action)}>{S.restoreDialog.restore}</button>
      <button type="button" class="btn-link" onclick={oncancel}>{S.restoreDialog.cancel}</button>
    </div>
  </div>
</dialog>

<style>
  dialog {
    width: min(520px, calc(100vw - 32px));
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
    padding: 22px;
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
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    color: var(--text-2);
  }

  .files {
    list-style: none;
    margin: 0;
    padding: 0;
    max-height: 160px;
    overflow: auto;
    border: 1px solid var(--line-soft);
    border-radius: 10px;
  }

  .files li {
    display: flex;
    justify-content: space-between;
    gap: 12px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line-soft);
    font-size: 12px;
  }

  .files li:last-child {
    border-bottom: 0;
  }

  .files li.changed {
    background: var(--check-bg);
    color: var(--check-text-strong);
  }

  .files li.missing,
  .files li.removed {
    color: var(--text-3);
  }

  .s {
    flex-shrink: 0;
  }

  fieldset {
    margin: 0;
    padding: 0;
    border: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .opt {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--line-strong);
    border-radius: 10px;
    cursor: pointer;
  }

  .opt.on {
    border-color: var(--accent);
    background: var(--accent-tint);
  }

  .opt input {
    margin-top: 2px;
    flex-shrink: 0;
  }

  .opt .d {
    display: block;
    margin-top: 2px;
    font-size: 12px;
    color: var(--text-2);
  }

  .warn {
    padding: 8px 10px;
    background: var(--check-bg);
    border: 1px solid var(--check-line);
    border-radius: 8px;
    color: var(--check-text-strong);
    font-size: 12px;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }

  .sub {
    font-size: 12px;
    color: var(--text-3);
    overflow-wrap: anywhere;
  }

  .buttons {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .buttons .btn-lg {
    width: 100%;
  }
</style>
