<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The item tools under the source view (M10.36 to M10.39): Add item (tap, draw a box, or a box to adjust),
  // Merge and Cut with their confirm panels, and the buttons that do for the SELECTED item what a drag or a
  // long press does (Move earlier, Move later, Remove, Reset). Every pointer gesture has a button here
  // (WCAG 2.5.7), and every button works from the keyboard.
  import type { Cut } from '../types.ts';
  import { cutFromSliders, slidersFromCut, type CutProblem } from '../geometry.ts';
  import type { StageTool } from '../gesture.ts';
  import type { MenuEntry } from '../stage-types.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';
  import Menu from './Menu.svelte';

  let {
    tool,
    count,
    selectedOrder,
    canMoveEarlier,
    canMoveLater,
    canRemove,
    canCut,
    cutNote = '',
    canMerge,
    manualOrder,
    mergeCount,
    cut,
    cutProblem,
    busy = false,
    onadd,
    onmerge,
    onmergeconfirm,
    oncut,
    oncutchange,
    oncutconfirm,
    oncancel,
    onmove,
    onremove,
    onreading,
  }: {
    tool: StageTool;
    count: number;
    /** 1-based output rank of the selected item, or null. */
    selectedOrder: number | null;
    canMoveEarlier: boolean;
    canMoveLater: boolean;
    canRemove: boolean;
    canCut: boolean;
    /** Why Cut is off, when it is (a curved page cannot be cut). */
    cutNote?: string;
    canMerge: boolean;
    manualOrder: boolean;
    mergeCount: number;
    cut: Cut | null;
    cutProblem: CutProblem | null;
    busy?: boolean;
    onadd: (kind: 'tap' | 'draw' | 'inset') => void;
    onmerge: () => void;
    onmergeconfirm: () => void;
    oncut: () => void;
    oncutchange: (c: Cut) => void;
    oncutconfirm: () => void;
    oncancel: () => void;
    onmove: (dir: -1 | 1) => void;
    onremove: () => void;
    onreading: () => void;
  } = $props();

  let addMenu = $state<{ x: number; y: number } | null>(null);
  let addBtn = $state<HTMLButtonElement | null>(null);

  const addEntries = $derived<MenuEntry[]>([
    { key: 'tap', label: S.items.addTap, hint: S.items.addTapHint.replace(' Esc cancels.', ''), run: () => onadd('tap') },
    { key: 'draw', label: S.items.addDraw, hint: S.items.addDrawHint.replace(' Esc cancels.', ''), run: () => onadd('draw') },
    { key: 'inset', label: S.items.addInset, run: () => onadd('inset') },
  ]);

  const sliders = $derived(cut ? slidersFromCut(cut) : { pos: 0.5, tilt: 0 });

  function setAxis(axis: Cut['axis']): void {
    oncutchange(cutFromSliders(axis, sliders.pos, sliders.tilt));
  }

  function setPos(v: number): void {
    if (cut) oncutchange(cutFromSliders(cut.axis, v, sliders.tilt));
  }

  function setTilt(v: number): void {
    if (cut) oncutchange(cutFromSliders(cut.axis, sliders.pos, v));
  }

  function openAdd(): void {
    const r = addBtn?.getBoundingClientRect();
    addMenu = { x: r?.left ?? 0, y: (r?.top ?? 0) - 150 };
  }
</script>

<div class="dock" role="toolbar" aria-label={S.items.toolsLabel} data-dock>
  {#if tool === 'none'}
    <span class="count">{S.items.count(count)}</span>
    <button bind:this={addBtn} type="button" class="cbtn" aria-haspopup="menu" aria-expanded={addMenu !== null} aria-keyshortcuts="I" title={`${S.items.addItem} (I)`} disabled={busy} onclick={openAdd}>
      <Icon name="plus" size={16} stroke={2} /> {S.items.addItem} <Icon name="down" size={14} />
    </button>
    <button type="button" class="cbtn" aria-keyshortcuts="M" title={`${S.items.merge} (M)`} disabled={busy || !canMerge} onclick={onmerge}>{S.items.merge}</button>
    <button type="button" class="cbtn" aria-keyshortcuts="K" title={cutNote || `${S.items.cut} (K)`} disabled={busy || !canCut} onclick={oncut}>{S.items.cut}</button>
    <span class="sep" aria-hidden="true"></span>
    <span class="sel">{selectedOrder ? S.items.itemN(selectedOrder) : S.items.selectedNone}</span>
    <button type="button" class="cbtn" aria-label={S.items.moveEarlier} title={S.items.moveEarlier} disabled={busy || !canMoveEarlier} onclick={() => onmove(-1)}><Icon name="back" size={16} stroke={2} /> {S.items.earlier}</button>
    <button type="button" class="cbtn" aria-label={S.items.moveLater} title={S.items.moveLater} disabled={busy || !canMoveLater} onclick={() => onmove(1)}>{S.items.later} <Icon name="next" size={16} stroke={2} /></button>
    <button type="button" class="cbtn" disabled={busy || !canRemove} title={S.items.removeHelp} onclick={onremove}><Icon name="trash" size={16} /> {S.items.remove}</button>
    {#if manualOrder}
      <button type="button" class="cbtn" disabled={busy} onclick={onreading}>{S.items.readingOrder}</button>
    {/if}
  {:else if tool === 'merge'}
    <span class="msg" role="status">{S.items.mergeHint(mergeCount)}</span>
    <button type="button" class="cbtn primary" disabled={busy || mergeCount < 2} onclick={onmergeconfirm}>{S.items.mergeConfirm(mergeCount)}</button>
    <button type="button" class="cbtn" onclick={oncancel}>{S.items.cancel}</button>
  {:else if tool === 'cut' && cut}
    <span class="cuttitle">{S.items.cutTitle(selectedOrder ?? 0)}</span>
    <div class="seg" role="group" aria-label={S.items.cutAxisLabel}>
      <button type="button" aria-pressed={cut.axis === 'vertical'} onclick={() => setAxis('vertical')}>{S.items.cutVertical}</button>
      <button type="button" aria-pressed={cut.axis === 'horizontal'} onclick={() => setAxis('horizontal')}>{S.items.cutHorizontal}</button>
    </div>
    <label class="slider">
      {S.items.cutPosition}
      <input type="range" min="0.05" max="0.95" step="0.01" value={sliders.pos} oninput={(e) => setPos(Number(e.currentTarget.value))} aria-valuetext={`${Math.round(sliders.pos * 100)}%`} />
    </label>
    <label class="slider">
      {S.items.cutTilt}
      <input type="range" min="-0.4" max="0.4" step="0.01" value={sliders.tilt} oninput={(e) => setTilt(Number(e.currentTarget.value))} aria-valuetext={`${Math.round(sliders.tilt * 100)}%`} />
    </label>
    <button type="button" class="cbtn" onclick={() => oncutchange(cutFromSliders(cut.axis, 0.5, 0))}>{S.items.cutHalves}</button>
    {#if cutProblem}<span class="msg warn" role="alert">{cutProblem === 'small' ? S.items.cutTooSmall : S.items.cutBad}</span>{/if}
    <button type="button" class="cbtn primary" disabled={busy || cutProblem !== null} onclick={oncutconfirm}>{S.items.cutConfirm}</button>
    <button type="button" class="cbtn" onclick={oncancel}>{S.items.cancel}</button>
  {:else}
    <span class="msg" role="status">{tool === 'add-tap' ? S.items.addTapHint : S.items.addDrawHint}</span>
    <button type="button" class="cbtn" onclick={oncancel}>{S.items.cancel}</button>
  {/if}
</div>

{#if addMenu}
  <Menu
    entries={addEntries}
    at={addMenu}
    label={S.items.addItem}
    onclose={(restore) => {
      addMenu = null;
      if (restore) addBtn?.focus();
    }}
  />
{/if}

<style>
  .dock {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    background: var(--canvas-bar);
    border-top: 1px solid var(--canvas-line);
    color: var(--canvas-text);
  }

  .count,
  .sel {
    font-size: 13px;
    font-weight: 600;
    color: var(--canvas-text);
  }

  .sel {
    color: var(--canvas-text-2);
    font-weight: 500;
  }

  .sep {
    width: 1px;
    align-self: stretch;
    margin: 4px 4px;
    background: var(--canvas-line);
  }

  .cbtn {
    min-height: var(--ctl-h);
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    background: var(--canvas-btn);
    border: 1px solid var(--canvas-btn-line);
    border-radius: 8px;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
    white-space: nowrap;
  }

  .cbtn:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.18);
  }

  .cbtn:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .cbtn.primary {
    background: var(--accent);
    border-color: transparent;
    font-weight: 600;
  }

  .cbtn.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .cbtn:focus-visible,
  .seg button:focus-visible,
  input:focus-visible {
    outline-color: #ffffff;
  }

  .msg {
    font-size: 13px;
    color: var(--canvas-text);
    flex: 1 1 220px;
  }

  .msg.warn {
    color: #ffb4ab;
    flex: 0 1 auto;
  }

  .cuttitle {
    font-weight: 600;
    font-size: 13px;
  }

  .seg {
    background: rgba(255, 255, 255, 0.12);
  }

  .seg button {
    color: var(--canvas-text-2);
    min-height: 40px;
  }

  .seg button[aria-pressed='true'] {
    background: rgba(255, 255, 255, 0.92);
    color: #15181e;
  }

  .slider {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--canvas-text);
  }

  .slider input {
    width: 130px;
    accent-color: #9db4ff;
  }
</style>
