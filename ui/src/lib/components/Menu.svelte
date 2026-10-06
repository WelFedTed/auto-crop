<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // A small popup menu (role="menu"): opened by press-and-hold, the context key, Shift+F10 or a button. Arrow
  // keys move, Home and End jump, Enter or Space runs, Escape closes and hands focus back, a click outside
  // closes. Items 44 px high so a finger can hit them. Disabled items stay visible (with the reason) and are
  // skipped by the arrow keys, so the person sees what exists and why it is off.
  import { onMount, tick } from 'svelte';
  import type { MenuEntry } from '../stage-types.ts';

  let {
    entries,
    at,
    label,
    onclose,
  }: {
    entries: MenuEntry[];
    /** Where it opens, in viewport pixels (the corner nearest the opener). */
    at: { x: number; y: number };
    label: string;
    /** `restoreFocus` is false when an entry ran something that moves focus itself. */
    onclose: (restoreFocus: boolean) => void;
  } = $props();

  let el = $state<HTMLDivElement | null>(null);
  let pos = $state({ x: 0, y: 0 });
  let active = $state(0);
  const enabled = $derived(entries.map((e, i) => (e.disabled ? -1 : i)).filter((i) => i >= 0));

  onMount(() => {
    pos = { x: at.x, y: at.y };
    void tick().then(() => {
      if (!el) return;
      const r = el.getBoundingClientRect();
      pos = {
        x: Math.max(8, Math.min(at.x, window.innerWidth - r.width - 8)),
        y: Math.max(8, Math.min(at.y, window.innerHeight - r.height - 8)),
      };
      active = enabled[0] ?? 0;
      focusActive();
    });
    const outside = (e: PointerEvent) => {
      if (el && !el.contains(e.target as Node)) onclose(false);
    };
    window.addEventListener('pointerdown', outside, true);
    return () => window.removeEventListener('pointerdown', outside, true);
  });

  function focusActive(): void {
    el?.querySelector<HTMLElement>(`[data-i="${active}"]`)?.focus({ preventScroll: true });
  }

  function move(delta: number): void {
    if (enabled.length === 0) return;
    const at0 = enabled.indexOf(active);
    active = enabled[(at0 + delta + enabled.length) % enabled.length];
    focusActive();
  }

  function onKey(e: KeyboardEvent): void {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      move(1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      move(-1);
    } else if (e.key === 'Home') {
      e.preventDefault();
      active = enabled[0] ?? 0;
      focusActive();
    } else if (e.key === 'End') {
      e.preventDefault();
      active = enabled[enabled.length - 1] ?? 0;
      focusActive();
    } else if (e.key === 'Escape' || e.key === 'Tab') {
      e.preventDefault();
      e.stopPropagation();
      onclose(true);
    }
  }

  function run(entry: MenuEntry): void {
    if (entry.disabled) return;
    onclose(false);
    entry.run();
  }
</script>

<div
  bind:this={el}
  class="menu"
  role="menu"
  aria-label={label}
  tabindex="-1"
  style:left="{pos.x}px"
  style:top="{pos.y}px"
  onkeydown={onKey}
  data-menu
>
  {#each entries as e, i (e.key)}
    <button
      type="button"
      role="menuitem"
      class="item"
      class:danger={e.danger}
      data-i={i}
      data-key={e.key}
      tabindex={i === active ? 0 : -1}
      aria-disabled={e.disabled ? 'true' : undefined}
      aria-describedby={e.disabled || e.hint ? `mh-${e.key}` : undefined}
      onclick={() => run(e)}
      onfocus={() => (active = i)}
    >
      <span class="l">{e.label}</span>
      {#if e.disabled || e.hint}
        <span class="h" id="mh-{e.key}">{e.disabled || e.hint}</span>
      {/if}
    </button>
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 60;
    min-width: 220px;
    max-width: min(320px, calc(100vw - 16px));
    padding: 6px;
    display: flex;
    flex-direction: column;
    gap: 2px;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--line-strong);
    border-radius: 12px;
    box-shadow: var(--shadow-lg);
  }

  .item {
    min-height: 44px;
    padding: 6px 12px;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    justify-content: center;
    gap: 1px;
    background: transparent;
    border: 0;
    border-radius: 8px;
    text-align: left;
    font-size: 14px;
    font-weight: 500;
    color: var(--text);
  }

  .item:hover:not([aria-disabled='true']),
  .item:focus-visible {
    background: var(--accent-tint);
  }

  .item:focus-visible {
    outline: 2px solid var(--focus);
    outline-offset: -2px;
  }

  .item[aria-disabled='true'] {
    color: var(--text-3);
    cursor: default;
  }

  .item.danger:not([aria-disabled='true']) .l {
    color: var(--fail-fg);
  }

  .h {
    font-size: 12px;
    font-weight: 400;
    color: var(--text-3);
    line-height: 1.3;
  }
</style>
