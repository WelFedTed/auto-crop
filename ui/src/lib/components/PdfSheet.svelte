<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { clampMargin, clampSide, MAX_CUSTOM_MM, MAX_MARGIN_MM, MAX_TITLE_CHARS, MIN_CUSTOM_MM, PDF_PAGE_SIZES, type PdfPrefs } from '../pdf-options.ts';
  import { S } from '../strings.ts';
  import type { PdfPageSize } from '../types.ts';

  let {
    open,
    images,
    prefs,
    onchoose,
    oncancel,
  }: {
    open: boolean;
    /** How many images go in (for the line under the title). */
    images: number;
    /** The page size and margin of the last PDF. */
    prefs: PdfPrefs;
    /** The person pressed "Choose where to save"; the Save dialog comes next. */
    onchoose: (choice: PdfPrefs & { title: string }) => void;
    oncancel: () => void;
  } = $props();

  let dialog = $state<HTMLDialogElement | null>(null);
  let pageSize = $state<PdfPageSize>('fit');
  let margin = $state('10');
  let customW = $state('100');
  let customH = $state('150');
  let title = $state('');

  const SIZE_LABEL: Record<PdfPageSize, string> = { fit: S.pdf.sizeFit, a4: S.pdf.sizeA4, a5: S.pdf.sizeA5, letter: S.pdf.sizeLetter, custom: S.pdf.sizeCustom };
  const fixed = $derived(pageSize !== 'fit');

  $effect(() => {
    if (!dialog) return;
    if (open && !dialog.open) {
      pageSize = prefs.pageSize;
      margin = String(prefs.marginMm);
      customW = String(prefs.customWidthMm);
      customH = String(prefs.customHeightMm);
      title = '';
      dialog.showModal();
    } else if (!open && dialog.open) {
      dialog.close();
    }
  });

  function go(e: Event): void {
    e.preventDefault();
    onchoose({
      pageSize,
      marginMm: clampMargin(margin),
      customWidthMm: clampSide(customW, prefs.customWidthMm),
      customHeightMm: clampSide(customH, prefs.customHeightMm),
      title,
    });
  }
</script>

<dialog bind:this={dialog} aria-labelledby="pdf-title" oncancel={(e) => { e.preventDefault(); oncancel(); }} data-pdf-sheet>
  <form class="sheet" onsubmit={go}>
    <h2 id="pdf-title">{S.pdf.sheetTitle}</h2>
    <p>{S.pdf.sheetIntro(images)}</p>

    <div class="field">
      <label for="pdf-size">{S.pdf.size}</label>
      <select id="pdf-size" class="select" bind:value={pageSize} data-pdf-size>
        {#each PDF_PAGE_SIZES as s (s)}
          <option value={s}>{SIZE_LABEL[s]}</option>
        {/each}
      </select>
      <span class="note">{S.pdf.sizeNote}</span>
    </div>

    {#if pageSize === 'custom'}
      <div class="field">
        <span class="row">
          <label for="pdf-custom-w">{S.pdf.customWidth}</label>
          <input
            id="pdf-custom-w"
            class="input num"
            type="number"
            min={MIN_CUSTOM_MM}
            max={MAX_CUSTOM_MM}
            step="1"
            inputmode="numeric"
            value={customW}
            oninput={(e) => (customW = e.currentTarget.value)}
            aria-describedby="pdf-custom-note"
            data-pdf-custom-w
          />
          <label for="pdf-custom-h">{S.pdf.customHeight}</label>
          <input
            id="pdf-custom-h"
            class="input num"
            type="number"
            min={MIN_CUSTOM_MM}
            max={MAX_CUSTOM_MM}
            step="1"
            inputmode="numeric"
            value={customH}
            oninput={(e) => (customH = e.currentTarget.value)}
            aria-describedby="pdf-custom-note"
            data-pdf-custom-h
          />
          <span class="unit">{S.pdf.marginUnit}</span>
        </span>
        <span class="note" id="pdf-custom-note">{S.pdf.customNote}</span>
      </div>
    {/if}

    <div class="field">
      <label for="pdf-margin">{S.pdf.margin}</label>
      <span class="row">
        <input
          id="pdf-margin"
          class="input num"
          type="number"
          min="0"
          max={MAX_MARGIN_MM}
          step="1"
          inputmode="numeric"
          value={margin}
          oninput={(e) => (margin = e.currentTarget.value)}
          disabled={!fixed}
          aria-describedby="pdf-margin-note"
          data-pdf-margin
        />
        <span class="unit">{S.pdf.marginUnit}</span>
      </span>
      <span class="note" id="pdf-margin-note">{fixed ? S.pdf.marginNote : S.pdf.marginOff}</span>
    </div>

    <div class="field">
      <label for="pdf-doc-title">{S.pdf.title}</label>
      <input
        id="pdf-doc-title"
        class="input"
        type="text"
        maxlength={MAX_TITLE_CHARS}
        placeholder={S.pdf.titlePlaceholder}
        bind:value={title}
        aria-describedby="pdf-title-note"
        data-pdf-title
      />
      <span class="note" id="pdf-title-note">{S.pdf.titleNote}</span>
    </div>

    <div class="buttons">
      <button type="submit" class="btn btn-primary btn-lg" data-pdf-go>{S.pdf.choose}</button>
      <button type="button" class="btn-link" onclick={oncancel} data-pdf-sheet-cancel>{S.pdf.sheetCancel}</button>
    </div>
  </form>
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

  .field {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  label {
    font-size: 13px;
    font-weight: 600;
  }

  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .num {
    width: 96px;
  }

  .unit,
  .note {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.4;
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
</style>
