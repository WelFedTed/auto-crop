<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // Fine-rotation ruler: 0.1 degree steps, scrolls under a fixed needle (drag, wheel, keys).
  // role="slider" with a spoken value (PLAN 6.10). Home resets to the detected angle.
  import { normaliseAngle, snapAngle } from '../quad.ts';
  import { S } from '../strings.ts';

  let {
    value,
    auto,
    disabled = false,
    oncommit,
    onlive,
  }: {
    value: number;
    /** Detected angle, shown as a diamond marker; null when unknown. */
    auto: number | null;
    disabled?: boolean;
    oncommit: (deg: number) => void;
    onlive: (deg: number | null) => void;
  } = $props();

  const PX_PER_DEG = 100;

  let live = $state<number | null>(null);
  const shown = $derived(live ?? value);
  let rulerEl = $state<HTMLDivElement | null>(null);
  let width = $state(0);

  let drag: { id: number; lastX: number; raw: number; moved: boolean; captured: boolean } | null = null;

  function down(e: PointerEvent): void {
    if (disabled || e.button > 0) return;
    rulerEl!.setPointerCapture(e.pointerId);
    drag = { id: e.pointerId, lastX: e.clientX, raw: value, moved: false, captured: value === 0 };
  }

  function move(e: PointerEvent): void {
    const d = drag;
    if (!d || d.id !== e.pointerId) return;
    const r = rulerEl!.getBoundingClientRect();
    const away = Math.max(0, Math.abs(e.clientY - (r.top + r.height / 2)) - 60);
    const ratio = away > 0 ? 0.1 : 1; // the farther from the ruler, the finer (PLAN 6.2.5)
    const dx = e.clientX - d.lastX;
    d.lastX = e.clientX;
    if (dx === 0) return;
    d.moved = true;
    // dragging right scrolls the ruler right, so the value under the needle falls
    d.raw = Math.max(-45, Math.min(45, d.raw - (dx / PX_PER_DEG) * ratio));
    const s = snapAngle(d.raw, e.altKey, d.captured);
    d.captured = s.captured;
    live = normaliseAngle(s.value);
    onlive(live);
  }

  function up(e: PointerEvent): void {
    const d = drag;
    if (!d || d.id !== e.pointerId) return;
    drag = null;
    if (d.moved && live !== null) {
      const v = live;
      live = null;
      onlive(null);
      if (v !== value) oncommit(v);
    }
  }

  function cancel(): void {
    drag = null;
    live = null;
    onlive(null);
  }

  function key(e: KeyboardEvent): void {
    if (disabled) return;
    const step = e.shiftKey ? 1 : 0.1;
    let next: number | null = null;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') next = value - step;
    else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') next = value + step;
    else if (e.key === 'PageUp') next = value + 5;
    else if (e.key === 'PageDown') next = value - 5;
    else if (e.key === 'Home') next = auto ?? 0;
    else if (e.key === 'End') next = 0;
    if (next === null) return;
    e.preventDefault();
    e.stopPropagation();
    const v = normaliseAngle(next);
    if (v !== value) oncommit(v);
  }

  // Wheel: 0.1 degree per notch, committed once the wheel pauses.
  let wheelTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    const el = rulerEl;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (disabled) return;
      e.preventDefault();
      const base = live ?? value;
      live = normaliseAngle(base + (e.deltaY > 0 ? -0.1 : 0.1));
      onlive(live);
      clearTimeout(wheelTimer);
      wheelTimer = setTimeout(() => {
        const v = live;
        live = null;
        onlive(null);
        if (v !== null && v !== value) oncommit(v);
      }, 250);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  });

  const shift = $derived(width / 2 - shown * PX_PER_DEG);
  const autoLeft = $derived(width / 2 + ((auto ?? 0) - shown) * PX_PER_DEG);
</script>

<div
  class="ruler"
  class:disabled
  role="slider"
  tabindex={disabled ? -1 : 0}
  aria-label={S.editor.rulerLabel}
  aria-valuemin={-45}
  aria-valuemax={45}
  aria-valuenow={shown}
  aria-valuetext={S.editor.angleSpoken(shown)}
  aria-describedby="ruler-hint"
  aria-disabled={disabled}
  bind:this={rulerEl}
  bind:clientWidth={width}
  onpointerdown={down}
  onpointermove={move}
  onpointerup={up}
  onpointercancel={cancel}
  onkeydown={key}
>
  <div class="ticks minor" style:background-position-x="{shift}px"></div>
  <div class="ticks major" style:background-position-x="{shift}px"></div>
  <div class="needle"></div>
  <div class="label mono">{S.editor.angleLabel(shown)}</div>
  {#if auto !== null && Math.abs(autoLeft - width / 2) < width / 2}
    <div class="auto" style:left="{autoLeft}px" title={S.editor.autoMarker}></div>
  {/if}
</div>
<span id="ruler-hint" class="sr-only">{S.editor.rulerHint}</span>

<style>
  .ruler {
    position: relative;
    flex: 1;
    min-width: 120px;
    height: 56px;
    overflow: hidden;
    cursor: ew-resize;
    touch-action: none;
    border-radius: 8px;
    user-select: none;
  }

  .ruler.disabled {
    opacity: 0.45;
    cursor: default;
  }

  .ruler:focus-visible {
    outline: 3px solid #ffffff;
    outline-offset: -3px;
  }

  .ticks {
    position: absolute;
    left: 0;
    right: 0;
  }

  .minor {
    top: 24px;
    height: 14px;
    background-image: repeating-linear-gradient(90deg, #8e95a5 0 1px, transparent 1px 10px);
  }

  .major {
    top: 18px;
    height: 24px;
    background-image: repeating-linear-gradient(90deg, #c4cad6 0 1.5px, transparent 1.5px 100px);
  }

  .needle {
    position: absolute;
    left: 50%;
    top: 10px;
    width: 2px;
    height: 40px;
    margin-left: -1px;
    background: #7c9bff;
  }

  .label {
    position: absolute;
    left: 50%;
    top: 0;
    transform: translateX(-50%);
    padding: 1px 8px;
    border-radius: 6px;
    background: #2b4fd8;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
  }

  .auto {
    position: absolute;
    top: 44px;
    width: 8px;
    height: 8px;
    margin-left: -4px;
    background: #f2b544;
    transform: rotate(45deg);
  }
</style>
