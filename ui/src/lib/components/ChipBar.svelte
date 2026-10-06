<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The item chip bar (M10.36): one chip per item in output order with its number, a band icon plus word
  // (Good, Check or Failed: never colour alone), the first reason and the file name it will be saved as.
  //   * Tap or Enter selects (the source view zooms to it). Arrow keys move focus (roving tabindex).
  //   * The menu opens on press-and-hold, the context key, Shift+F10 or the "..." button.
  //   * Reorder: drag the grip, or Alt+Arrow on a chip, or the Move earlier / Move later buttons that the
  //     editor shows for the selected item (WCAG 2.5.7: a drag is never the only way).
  // Removed items sit in their own group with a Restore button.
  import { dropSlot, includedCrops, removedCrops } from '../items.ts';
  import { S } from '../strings.ts';
  import type { Band, CropView, ItemView } from '../types.ts';
  import Icon from './Icon.svelte';

  let {
    item,
    selectedId,
    mergeIds = [],
    mergeMode = false,
    bandWord,
    reasonOf,
    onselect,
    onmenu,
    onreorder,
    onmove,
    onrestore,
  }: {
    item: ItemView;
    selectedId: number | null;
    mergeIds?: number[];
    mergeMode?: boolean;
    bandWord: (b: Band | null) => string;
    reasonOf: (c: CropView) => string | null;
    onselect: (id: number) => void;
    onmenu: (id: number, at: { x: number; y: number }) => void;
    /** `slot` is the place among the OTHER included items the dragged one lands in. */
    onreorder: (id: number, slot: number) => void;
    onmove: (id: number, dir: -1 | 1) => void;
    onrestore: (id: number) => void;
  } = $props();

  const included = $derived(includedCrops(item));
  const removed = $derived(removedCrops(item));
  let listEl = $state<HTMLDivElement | null>(null);
  let focusId = $state<number | null>(null);
  /** The chip that holds tabindex 0: the focused one, else the selected one, else the first. */
  const tabId = $derived(included.some((c) => c.id === focusId) ? focusId : included.some((c) => c.id === selectedId) ? selectedId : (included[0]?.id ?? null));

  // ---- press and hold for the menu ----------------------------------------------------------------
  let holdTimer: ReturnType<typeof setTimeout> | undefined;
  let holdOrigin: { x: number; y: number } | null = null;
  let heldOpen = false;

  function holdStart(e: PointerEvent, id: number): void {
    heldOpen = false;
    clearTimeout(holdTimer);
    holdOrigin = { x: e.clientX, y: e.clientY };
    const at = { x: e.clientX, y: e.clientY };
    holdTimer = setTimeout(() => {
      heldOpen = true;
      onmenu(id, at);
    }, 500);
  }

  function holdMove(e: PointerEvent): void {
    if (holdOrigin && Math.hypot(e.clientX - holdOrigin.x, e.clientY - holdOrigin.y) > 10) clearTimeout(holdTimer);
  }

  function holdEnd(): void {
    clearTimeout(holdTimer);
  }

  function pick(id: number): void {
    if (heldOpen) {
      heldOpen = false;
      return;
    }
    onselect(id);
  }

  // ---- keyboard -------------------------------------------------------------------------------------
  function focusChip(id: number): void {
    focusId = id;
    listEl?.querySelector<HTMLElement>(`[data-main="${id}"]`)?.focus({ preventScroll: false });
  }

  function keyOn(e: KeyboardEvent, c: CropView): void {
    const at = included.findIndex((x) => x.id === c.id);
    if (e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
      e.preventDefault();
      onmove(c.id, e.key === 'ArrowLeft' ? -1 : 1);
      return;
    }
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown') {
      e.preventDefault();
      if (included[at + 1]) focusChip(included[at + 1].id);
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (included[at - 1]) focusChip(included[at - 1].id);
    } else if (e.key === 'Home') {
      e.preventDefault();
      focusChip(included[0].id);
    } else if (e.key === 'End') {
      e.preventDefault();
      focusChip(included[included.length - 1].id);
    } else if (e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) {
      e.preventDefault();
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      onmenu(c.id, { x: r.left, y: r.bottom });
    }
  }

  // ---- drag to reorder -----------------------------------------------------------------------------
  interface Dragging {
    id: number;
    pointerId: number;
    x0: number;
    dx: number;
    slot: number;
    /** Left edge of the insertion bar, relative to the list. */
    barX: number;
  }
  let dragging = $state.raw<Dragging | null>(null);

  function centresOthers(id: number): { centres: number[]; rects: DOMRect[] } {
    const nodes = [...(listEl?.querySelectorAll<HTMLElement>('[data-chip-id]') ?? [])].filter((n) => Number(n.dataset.chipId) !== id && n.dataset.removed !== '1');
    const rects = nodes.map((n) => n.getBoundingClientRect());
    return { centres: rects.map((r) => r.left + r.width / 2), rects };
  }

  function gripDown(e: PointerEvent, c: CropView): void {
    if (e.button > 0) return;
    e.preventDefault();
    try {
      (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    } catch {
      // not capturable; the drag still follows the pointer events it gets
    }
    dragging = { id: c.id, pointerId: e.pointerId, x0: e.clientX, dx: 0, slot: c.order - 1, barX: 0 };
  }

  function gripMove(e: PointerEvent): void {
    const d = dragging;
    if (!d || d.pointerId !== e.pointerId) return;
    const { centres, rects } = centresOthers(d.id);
    const slot = dropSlot(centres, e.clientX);
    const base = listEl?.getBoundingClientRect().left ?? 0;
    const barX = rects.length === 0 ? 0 : slot === 0 ? rects[0].left - base - 4 : rects[Math.min(slot, rects.length) - 1].right - base + 3;
    dragging = { ...d, dx: e.clientX - d.x0, slot, barX };
  }

  function gripUp(e: PointerEvent): void {
    const d = dragging;
    if (!d || d.pointerId !== e.pointerId) return;
    dragging = null;
    const here = included.findIndex((c) => c.id === d.id);
    if (e.type !== 'pointercancel' && Math.abs(d.dx) > 6 && d.slot !== here) onreorder(d.id, d.slot);
  }

  function onWindowKey(e: KeyboardEvent): void {
    if (e.key === 'Escape' && dragging) {
      e.preventDefault();
      e.stopPropagation();
      dragging = null;
    }
  }

  const word = (c: CropView) => bandWord(c.band);
  const iconOf = (b: Band | null) => (b === 'good' ? 'good' : b === 'failed' ? 'failed' : 'check');
</script>

<svelte:window onkeydowncapture={onWindowKey} />

<div class="bar" bind:this={listEl} role="toolbar" aria-label={S.items.barLabel} aria-orientation="horizontal" data-chipbar>
  <div class="list">
    {#each included as c (c.id)}
      <div
        class="chipitem {c.band ?? 'check'}"
        class:selected={c.id === selectedId}
        class:picked={mergeMode && mergeIds.includes(c.id)}
        class:dragging={dragging?.id === c.id}
        style:transform={dragging?.id === c.id ? `translateX(${dragging.dx}px)` : undefined}
        data-chip-id={c.id}
      >
        <button
          type="button"
          class="grip"
          tabindex="-1"
          aria-label={S.items.gripFor(c.order)}
          title={S.items.grip}
          onpointerdown={(e) => gripDown(e, c)}
          onpointermove={gripMove}
          onpointerup={gripUp}
          onpointercancel={gripUp}
        >
          <svg width="12" height="18" viewBox="0 0 12 18" aria-hidden="true"><circle cx="3" cy="3" r="1.6" /><circle cx="9" cy="3" r="1.6" /><circle cx="3" cy="9" r="1.6" /><circle cx="9" cy="9" r="1.6" /><circle cx="3" cy="15" r="1.6" /><circle cx="9" cy="15" r="1.6" /></svg>
        </button>
        <button
          type="button"
          class="main"
          data-main={c.id}
          tabindex={c.id === tabId ? 0 : -1}
          aria-current={c.id === selectedId ? 'true' : undefined}
          aria-pressed={mergeMode ? mergeIds.includes(c.id) : undefined}
          aria-label={[S.items.itemN(c.order), word(c), reasonOf(c), c.outputName].filter(Boolean).join(', ')}
          onclick={() => pick(c.id)}
          onfocus={() => (focusId = c.id)}
          onkeydown={(e) => keyOn(e, c)}
          onpointerdown={(e) => holdStart(e, c.id)}
          onpointermove={holdMove}
          onpointerup={holdEnd}
          onpointercancel={holdEnd}
          onpointerleave={holdEnd}
          oncontextmenu={(e) => {
            e.preventDefault();
            onmenu(c.id, { x: e.clientX, y: e.clientY });
          }}
        >
          <span class="num" aria-hidden="true">{c.order}</span>
          <span class="txt" aria-hidden="true">
            <span class="band"><Icon name={iconOf(c.band)} size={14} stroke={2.2} /> {word(c)}</span>
            {#if reasonOf(c)}<span class="reason">{reasonOf(c)}</span>{/if}
            {#if c.outputName}<span class="file mono">{c.outputName}</span>{/if}
          </span>
        </button>
        <button type="button" class="more" tabindex="-1" aria-haspopup="menu" aria-label={S.items.menuFor(c.order)} title={S.items.menuButton} onclick={(e) => {
            const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
            onmenu(c.id, { x: r.left, y: r.bottom });
          }}>
          <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden="true"><circle cx="3" cy="8" r="1.5" /><circle cx="8" cy="8" r="1.5" /><circle cx="13" cy="8" r="1.5" /></svg>
        </button>
      </div>
    {/each}
    {#if dragging}
      <span class="dropbar" style:left="{dragging.barX}px" aria-hidden="true"></span>
    {/if}
  </div>
  {#if removed.length > 0}
    <div class="removed" role="group" aria-label={S.items.removedHeading(removed.length)}>
      <span class="rhead">{S.items.removedHeading(removed.length)}</span>
      {#each removed as c, i (c.id)}
        <div class="chipitem gone" data-chip-id={c.id} data-removed="1">
          <button type="button" class="main restore" data-restore-chip={c.id} aria-label={S.items.addAsItemFor(i + 1)} onclick={() => onrestore(c.id)}
            oncontextmenu={(e) => {
              e.preventDefault();
              onmenu(c.id, { x: e.clientX, y: e.clientY });
            }}>
            <span class="num ghostnum" aria-hidden="true"><Icon name="plus" size={14} stroke={2.4} /></span>
            <span class="txt" aria-hidden="true">
              <span class="band">{S.items.ghostLabel(i + 1)}</span>
              <span class="reason">{S.items.addAsItem}</span>
            </span>
          </button>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .bar {
    display: flex;
    align-items: stretch;
    gap: 12px;
    padding: 8px 12px 10px;
    overflow-x: auto;
    overscroll-behavior-x: contain;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
    touch-action: pan-x;
  }

  .list {
    position: relative;
    display: flex;
    gap: 8px;
  }

  .removed {
    display: flex;
    align-items: stretch;
    gap: 8px;
    padding-left: 12px;
    border-left: 1px solid var(--canvas-line);
  }

  .rhead {
    align-self: center;
    font-size: 12px;
    font-weight: 600;
    color: var(--canvas-text-2);
    white-space: nowrap;
  }

  .chipitem {
    position: relative;
    flex: 0 0 auto;
    display: flex;
    align-items: stretch;
    min-height: 56px;
    background: var(--surface);
    color: var(--text);
    border: 2px solid var(--line-strong);
    border-radius: 12px;
    transition: transform 0.08s ease;
  }

  .chipitem.selected {
    border-color: var(--accent);
    box-shadow: 0 0 0 2px var(--accent);
  }

  .chipitem.picked {
    background: var(--accent-tint);
  }

  .chipitem.dragging {
    z-index: 3;
    opacity: 0.85;
    box-shadow: var(--shadow-lg);
    transition: none;
  }

  .chipitem.gone {
    border-style: dashed;
    background: var(--surface-2);
  }

  .grip,
  .more {
    flex: 0 0 auto;
    width: 28px;
    min-width: var(--ctl-h-sm);
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 0;
    color: var(--text-3);
    fill: currentColor;
  }

  .grip {
    cursor: grab;
    touch-action: none;
    border-radius: 10px 0 0 10px;
  }

  .grip:active {
    cursor: grabbing;
  }

  .more {
    border-radius: 0 10px 10px 0;
  }

  .grip:hover,
  .more:hover {
    background: var(--seg-bg);
    color: var(--text);
  }

  .main {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 150px;
    max-width: 260px;
    padding: 6px 8px;
    background: transparent;
    border: 0;
    text-align: left;
    color: inherit;
    touch-action: pan-x;
  }

  .main:focus-visible {
    outline: 3px solid var(--focus);
    outline-offset: -2px;
    border-radius: 8px;
  }

  .num {
    flex: 0 0 auto;
    width: 30px;
    height: 30px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    font-weight: 700;
    font-size: 14px;
  }

  .good .num {
    background: var(--good-bg);
    color: var(--good-fg);
  }

  .check .num {
    background: var(--check-bg);
    color: var(--check-fg);
  }

  .failed .num {
    background: var(--fail-bg);
    color: var(--fail-fg);
  }

  .ghostnum {
    background: var(--seg-bg);
    color: var(--text-2);
  }

  .txt {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }

  .band {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 13px;
    font-weight: 600;
  }

  .good .band {
    color: var(--good-fg);
  }

  .check .band {
    color: var(--check-fg);
  }

  .failed .band {
    color: var(--fail-fg);
  }

  .gone .band {
    color: var(--text-2);
  }

  .reason {
    max-width: 190px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    color: var(--text-2);
  }

  .file {
    font-size: 12px;
    color: var(--text-3);
  }

  .dropbar {
    position: absolute;
    top: -2px;
    bottom: -2px;
    width: 4px;
    border-radius: 2px;
    background: var(--accent);
    box-shadow: 0 0 0 2px var(--surface);
    pointer-events: none;
  }

  @media (prefers-reduced-motion: reduce) {
    .chipitem {
      transition: none;
    }
  }

  @media (forced-colors: active) {
    .chipitem {
      border-color: CanvasText;
    }

    .chipitem.selected {
      border-color: Highlight;
      outline: 2px solid Highlight;
    }
  }
</style>
