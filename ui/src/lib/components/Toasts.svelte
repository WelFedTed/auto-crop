<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { store } from '../store.svelte.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';
</script>

<!-- One polite live region for transient messages, one more for screen-reader-only announcements. -->
<div class="toasts" role="status" aria-live="polite" aria-label={S.a11y.toasts}>
  {#each store.toasts as t (t.id)}
    <div class="toast {t.kind}">
      <span class="text">{t.text}</span>
      {#if t.action}
        <button
          type="button"
          class="btn btn-sm act"
          onclick={() => {
            t.action?.run();
            store.dismissToast(t.id);
          }}>{t.action.label}</button
        >
      {/if}
      <button type="button" class="icon-btn close" aria-label={S.nav.close} onclick={() => store.dismissToast(t.id)}>
        <Icon name="close" size={16} />
      </button>
    </div>
  {/each}
</div>
<div class="sr-only" role="status" aria-live="polite">{store.announcement}</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 20px;
    transform: translateX(-50%);
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: min(560px, calc(100vw - 32px));
    pointer-events: none;
  }

  .toast {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 8px 8px 16px;
    border-radius: 12px;
    background: #262a33;
    color: #ffffff;
    box-shadow: var(--shadow-lg);
    border: 1px solid #3a3f4b;
    animation: rise 0.18s ease-out;
  }

  .toast.error {
    border-color: #ff9c94;
  }

  .toast.success {
    border-color: #7fe0a8;
  }

  .text {
    flex: 1;
    min-width: 0;
    line-height: 1.4;
  }

  .act {
    background: transparent;
    color: #bfd0ff;
    border-color: #7c9bff;
    font-weight: 600;
  }

  .act:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.08);
  }

  .close {
    color: #c4cad6;
  }

  .close:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.1);
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
