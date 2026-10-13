<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The gentle, non-modal suggestion: "This looks like a document: Clean makes it easier to read" with Preview, Apply and
  // Not now. It never applies anything by itself (B13): Preview shows the result without recording an edit, Apply is one
  // undo step, Not now hides it for this picture until the app closes. It does not move focus or trap it.
  import { S } from '../strings.ts';
  import type { EnhanceReason } from '../types.ts';
  import Icon from './Icon.svelte';

  let {
    receipt = false,
    reasons,
    previewing = false,
    busy = false,
    onpreview,
    onapply,
    onnotnow,
  }: {
    /** A long thin page: "looks like a receipt". */
    receipt?: boolean;
    reasons: EnhanceReason[];
    previewing?: boolean;
    busy?: boolean;
    onpreview: () => void;
    onapply: () => void;
    onnotnow: () => void;
  } = $props();

  const lines = $derived(reasons.map((r) => S.enhance.reasons[r]).filter((t): t is string => !!t).slice(0, 2));
</script>

<div class="banner" role="region" aria-label={S.enhance.bannerLabel} data-enhance-banner>
  <Icon name="sparkle" size={20} />
  <div class="text">
    <div class="title">{receipt ? S.enhance.bannerReceipt : S.enhance.bannerDoc}</div>
    <div class="why">
      {#if lines.length > 0}<span data-enhance-reasons>{lines.join(" ")}</span>{" "}{/if}
      <span class="note">{S.enhance.bannerNote}</span>
    </div>
  </div>
  <div class="actions">
    <button type="button" class="btn btn-sm" aria-pressed={previewing} data-enhance-preview-btn onclick={onpreview}>
      {previewing ? S.enhance.stopPreview : S.enhance.preview}
    </button>
    <button type="button" class="btn btn-sm btn-primary" disabled={busy} title={S.enhance.applyHint} data-enhance-apply onclick={onapply}>
      {S.enhance.apply}
    </button>
    <button type="button" class="btn btn-sm" title={S.enhance.notNowHint} data-enhance-notnow onclick={onnotnow}>{S.enhance.notNow}</button>
  </div>
</div>

<style>
  .banner {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    padding: 10px 14px;
    background: var(--info-bg);
    border: 1px solid var(--info-line);
    border-radius: 12px;
    color: var(--info-text);
    font-size: 13px;
    line-height: 1.4;
  }

  .text {
    flex: 1 1 260px;
    min-width: 0;
  }

  .title {
    font-weight: 600;
    font-size: 14px;
  }

  .why {
    margin-top: 2px;
    color: var(--info-text);
  }

  .note {
    opacity: 0.85;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .actions button[aria-pressed='true'] {
    background: var(--accent-tint);
    border-color: var(--accent);
  }
</style>
