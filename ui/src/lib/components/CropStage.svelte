<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The crop canvas: the EXIF-oriented source proxy, the quad overlay and its handles, zoom and pan.
  // The 48 px hit areas ARE the focusable buttons (PLAN 6.10), so touch targets, focus rings and the
  // accessibility tree cannot drift; the 16 px dual-stroke dot is drawn inside. All geometry is
  // normalised (0..1 of the displayed image), so the proxy's pixel size never matters.
  import type { Snippet } from 'svelte';
  import { cloneQuad, centroid, mid, moveQuad, toPercent, zoomAbout, type HandleKind, type Quad } from '../quad.ts';
  import { S } from '../strings.ts';
  import Icon from './Icon.svelte';

  let {
    src,
    name,
    fallbackAspect,
    quad,
    showOverlay,
    onpreview,
    oncommit,
    children,
  }: {
    src: string;
    name: string;
    /** width / height of the item, used until the proxy has loaded. */
    fallbackAspect: number;
    quad: Quad | null;
    showOverlay: boolean;
    /** The live quad during a drag, or null when it ends or is cancelled. */
    onpreview: (q: Quad | null) => void;
    oncommit: (q: Quad, label: string, announce: string) => void;
    /** Extra overlays (the inset, banners) rendered inside the stage. */
    children?: Snippet;
  } = $props();

  const GUTTER = 32;
  const HIT = 48;

  let stageW = $state(0);
  let stageH = $state(0);
  let natW = $state(0);
  let natH = $state(0);
  let loadFailed = $state(false);
  let stageEl = $state<HTMLDivElement | null>(null);

  const aspect = $derived(natW > 0 && natH > 0 ? natW / natH : fallbackAspect > 0 ? fallbackAspect : 0.75);
  const bw = $derived.by(() => {
    const aw = Math.max(40, stageW - 2 * GUTTER);
    const ah = Math.max(40, stageH - 2 * GUTTER);
    return Math.min(aw, ah * aspect);
  });
  const bh = $derived(bw / aspect);

  // View: image top-left in stage px (px, py) and zoom multiplier z (1 = fit).
  let z = $state(1);
  let px = $state(0);
  let py = $state(0);
  let fitMode = $state(true);

  $effect(() => {
    // Keep the image centred while in fit mode (and when the stage is resized).
    if (fitMode) {
      z = 1;
      px = (stageW - bw) / 2;
      py = (stageH - bh) / 2;
    }
  });

  export function fit(): void {
    fitMode = true;
  }

  export function zoomBy(factor: number, cx = stageW / 2, cy = stageH / 2): void {
    const v = zoomAbout({ z, px, py }, factor, cx, cy);
    fitMode = false;
    z = v.z;
    px = v.px;
    py = v.py;
  }

  export function actualSize(): void {
    // 100% of the proxy: one image pixel per CSS pixel.
    if (natW > 0) zoomBy(natW / (bw * z));
  }

  const sx = (v: number) => px + v * bw * z;
  const sy = (v: number) => py + v * bh * z;

  // ---- display geometry --------------------------------------------------------------------
  let dragQuad = $state.raw<Quad | null>(null);
  const dq = $derived(dragQuad ?? quad);
  const screen = $derived(dq ? dq.map((p) => ({ x: sx(p.x), y: sy(p.y) })) : []);
  const polygon = $derived(screen.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' '));
  const dim = $derived(
    screen.length === 4
      ? `M${px} ${py}H${px + bw * z}V${py + bh * z}H${px}Z M${screen.map((p) => `${p.x.toFixed(1)} ${p.y.toFixed(1)}`).join('L')}Z`
      : '',
  );

  interface HandleDef {
    key: string;
    kind: HandleKind;
    idx: number;
    x: number;
    y: number;
    label: string;
    hidden: boolean;
  }

  const handles = $derived.by<HandleDef[]>(() => {
    if (!dq || screen.length !== 4) return [];
    const out: HandleDef[] = [];
    dq.forEach((p, i) => {
      out.push({
        key: `c${i}`,
        kind: 'corner',
        idx: i,
        x: screen[i].x,
        y: screen[i].y,
        label: S.editor.handleLabel(S.editor.corners[i], toPercent(p.x), toPercent(p.y)),
        hidden: false,
      });
    });
    for (let i = 0; i < 4; i++) {
      const a = dq[i];
      const b = dq[(i + 1) % 4];
      const m = mid(a, b);
      const near = Math.hypot(screen[i].x - screen[(i + 1) % 4].x, screen[i].y - screen[(i + 1) % 4].y) < 96;
      out.push({
        key: `e${i}`,
        kind: 'edge',
        idx: i,
        x: (screen[i].x + screen[(i + 1) % 4].x) / 2,
        y: (screen[i].y + screen[(i + 1) % 4].y) / 2,
        label: S.editor.handleLabel(S.editor.edges[i], toPercent(m.x), toPercent(m.y)),
        hidden: near,
      });
    }
    const c = centroid(dq);
    out.push({
      key: 'g',
      kind: 'grip',
      idx: 0,
      x: sx(c.x),
      y: sy(c.y),
      label: S.editor.handleLabel(S.editor.grip, toPercent(c.x), toPercent(c.y)),
      hidden: false,
    });
    return out;
  });

  // ---- handle drag -------------------------------------------------------------------------
  interface Drag {
    kind: HandleKind;
    idx: number;
    pointerId: number;
    pointerType: string;
    x0: number;
    y0: number;
    start: Quad;
    moved: boolean;
  }
  let drag = $state.raw<Drag | null>(null);
  let loupe = $state.raw<{ x: number; y: number; nx: number; ny: number } | null>(null);

  const labelFor = (kind: HandleKind) =>
    kind === 'corner' ? S.editor.labels.moveCorner : kind === 'edge' ? S.editor.labels.moveEdge : S.editor.labels.moveOutline;

  function handleDown(e: PointerEvent, h: HandleDef): void {
    if (!quad || e.button > 0) return;
    e.preventDefault();
    e.stopPropagation();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    el.focus({ preventScroll: true });
    drag = { kind: h.kind, idx: h.idx, pointerId: e.pointerId, pointerType: e.pointerType, x0: e.clientX, y0: e.clientY, start: cloneQuad(quad), moved: false };
  }

  function handleMove(e: PointerEvent): void {
    const d = drag;
    if (!d || d.pointerId !== e.pointerId) return;
    const dxs = e.clientX - d.x0;
    const dys = e.clientY - d.y0;
    if (!d.moved && Math.hypot(dxs, dys) < 3) return;
    if (!d.moved) drag = { ...d, moved: true };
    const next = moveQuad(d.start, d.kind, d.idx, dxs / (bw * z), dys / (bh * z));
    dragQuad = next;
    onpreview(next);
    if (d.pointerType !== 'mouse') {
      const p = d.kind === 'corner' ? next[d.idx] : d.kind === 'edge' ? mid(next[d.idx], next[(d.idx + 1) % 4]) : centroid(next);
      loupe = { x: sx(p.x), y: sy(p.y), nx: p.x, ny: p.y };
    }
  }

  function handleUp(e: PointerEvent, h: HandleDef): void {
    const d = drag;
    if (!d || d.pointerId !== e.pointerId) return;
    drag = null;
    loupe = null;
    const final = dragQuad;
    if (d.moved && final) {
      oncommit(final, labelFor(d.kind), h.label);
      // The parent now supplies the same quad as `quad`; keep the draft one frame to avoid a flicker.
      requestAnimationFrame(() => {
        dragQuad = null;
        onpreview(null);
      });
    } else {
      dragQuad = null;
      onpreview(null);
    }
  }

  function cancelDrag(): void {
    if (!drag) return;
    drag = null;
    dragQuad = null;
    loupe = null;
    onpreview(null);
  }

  function handleKey(e: KeyboardEvent, h: HandleDef): void {
    if (!quad) return;
    const step = e.shiftKey ? 10 : 1;
    const dir: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
    };
    const d = dir[e.key];
    if (!d) return;
    e.preventDefault();
    e.stopPropagation();
    // Ctrl+arrows move the whole outline from any handle (PLAN 6.4.1).
    const kind: HandleKind = e.ctrlKey || e.metaKey ? 'grip' : h.kind;
    const next = moveQuad(quad, kind, h.idx, d[0] / (bw * z), d[1] / (bh * z));
    oncommit(next, labelFor(kind), h.label);
  }

  // ---- pan and zoom ------------------------------------------------------------------------
  const pointers = new Map<number, { x: number; y: number }>();
  let pan: { x0: number; y0: number; px0: number; py0: number } | null = null;
  let pinch: { d0: number; z0: number } | null = null;

  function stagePoint(e: { clientX: number; clientY: number }): { x: number; y: number } {
    const r = stageEl!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  function stageDown(e: PointerEvent): void {
    if ((e.target as HTMLElement).closest('.handle, .zoom, [data-nostage]')) return;
    stageEl!.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 1) pan = { x0: e.clientX, y0: e.clientY, px0: px, py0: py };
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinch = { d0: Math.hypot(a.x - b.x, a.y - b.y) || 1, z0: z };
      pan = null;
    }
  }

  function stageMove(e: PointerEvent): void {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pinch && pointers.size >= 2) {
      const [a, b] = [...pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y) || 1;
      const m = stagePoint({ clientX: (a.x + b.x) / 2, clientY: (a.y + b.y) / 2 });
      zoomBy((pinch.z0 * (d / pinch.d0)) / z, m.x, m.y);
    } else if (pan) {
      fitMode = false;
      px = pan.px0 + (e.clientX - pan.x0);
      py = pan.py0 + (e.clientY - pan.y0);
    }
  }

  function stageUp(e: PointerEvent): void {
    pointers.delete(e.pointerId);
    pinch = null;
    pan = null;
    if (pointers.size === 1) {
      const [p] = [...pointers.values()];
      pan = { x0: p.x, y0: p.y, px0: px, py0: py };
    }
  }

  $effect(() => {
    // Wheel zoom needs a non-passive listener so the page does not scroll (and Ctrl+wheel does not page-zoom).
    const el = stageEl;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const p = stagePoint(e);
      zoomBy(Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)), p.x, p.y);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  });

  function onWindowKey(e: KeyboardEvent): void {
    if (e.key === 'Escape' && drag) {
      e.preventDefault();
      e.stopPropagation();
      cancelDrag();
    }
  }

  // The loupe sits up and left of the pointer, flipped near the edges (PLAN 6.2.5).
  const LOUPE = 124;
  const loupePos = $derived.by(() => {
    if (!loupe) return null;
    let x = loupe.x - 40 - LOUPE;
    let y = loupe.y - 40 - LOUPE;
    if (x < 8) x = loupe.x + 40;
    if (y < 8) y = loupe.y + 40;
    return { x, y };
  });
</script>

<svelte:window onkeydowncapture={onWindowKey} />

<div
  class="stage"
  bind:this={stageEl}
  bind:clientWidth={stageW}
  bind:clientHeight={stageH}
  onpointerdown={stageDown}
  onpointermove={stageMove}
  onpointerup={stageUp}
  onpointercancel={stageUp}
  oncontextmenu={(e) => e.preventDefault()}
  role="presentation"
>
  {#if !loadFailed}
    <img
      class="photo"
      {src}
      alt={S.editor.sourceAlt(name)}
      draggable="false"
      style:left="{px}px"
      style:top="{py}px"
      style:width="{bw * z}px"
      style:height="{bh * z}px"
      onload={(e) => {
        const img = e.currentTarget as HTMLImageElement;
        natW = img.naturalWidth;
        natH = img.naturalHeight;
      }}
      onerror={() => (loadFailed = true)}
    />
  {:else}
    <div class="failed">{S.editor.loadError}</div>
  {/if}

  {#if showOverlay && quad && screen.length === 4}
    <svg class="overlay" width={stageW} height={stageH} aria-hidden="true">
      <path d={dim} fill="#0b0d12" fill-opacity="0.5" fill-rule="evenodd" />
      <polygon points={polygon} fill="none" stroke="#ffffff" stroke-width="2.5" stroke-linejoin="round" />
      <polygon points={polygon} fill="none" stroke="#15181e" stroke-width="1" stroke-dasharray="5 5" />
    </svg>

    <div class="handles" role="group" aria-label={S.editor.cropGroup}>
      {#each handles as h (h.key)}
        {#if !h.hidden}
          <button
            type="button"
            class="handle {h.kind}"
            class:active={drag && drag.kind === h.kind && drag.idx === h.idx && drag.moved}
            data-handle
            aria-roledescription={h.kind === 'grip' ? S.editor.moveRole : S.editor.handleRole}
            aria-label={h.label}
            style:left="{h.x - HIT / 2}px"
            style:top="{h.y - HIT / 2}px"
            onpointerdown={(e) => handleDown(e, h)}
            onpointermove={handleMove}
            onpointerup={(e) => handleUp(e, h)}
            onpointercancel={cancelDrag}
            onlostpointercapture={() => {
              if (drag && !drag.moved) cancelDrag();
            }}
            onkeydown={(e) => handleKey(e, h)}
          >
            {#if h.kind === 'grip'}
              <span class="gripicon"><Icon name="move" size={20} stroke={2} /></span>
            {:else}
              <span class="dot"></span>
            {/if}
          </button>
        {/if}
      {/each}
    </div>
  {/if}

  {#if loupe && loupePos && !loadFailed}
    <div class="loupe" role="img" aria-label="4x loupe" style:left="{loupePos.x}px" style:top="{loupePos.y}px">
      <img
        {src}
        alt=""
        draggable="false"
        style:width="{bw * z * 4}px"
        style:height="{bh * z * 4}px"
        style:left="{LOUPE / 2 - 3 - loupe.nx * bw * z * 4}px"
        style:top="{LOUPE / 2 - 3 - loupe.ny * bh * z * 4}px"
      />
      <svg class="cross" viewBox="0 0 118 118" width="118" height="118" aria-hidden="true">
        <path d="M59 8V110M8 59H110" stroke="#2b4fd8" stroke-width="1.5" />
        <circle cx="59" cy="59" r="5" fill="none" stroke="#fff" stroke-width="2" />
      </svg>
    </div>
  {/if}

  <div class="zoom" role="group" aria-label="Zoom">
    <button type="button" class="zbtn" aria-label={S.editor.zoomOut} onclick={() => zoomBy(1 / 1.25)}><Icon name="minus" size={18} stroke={2} /></button>
    <button type="button" class="zbtn fit" onclick={fit}>{S.editor.fit}</button>
    <button type="button" class="zbtn" aria-label={S.editor.zoomIn} onclick={() => zoomBy(1.25)}><Icon name="plus" size={18} stroke={2} /></button>
  </div>

  {@render children?.()}
</div>

<style>
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    background: var(--canvas);
    touch-action: none;
    cursor: grab;
    user-select: none;
    -webkit-user-select: none;
  }

  .stage:active {
    cursor: grabbing;
  }

  .photo {
    position: absolute;
    display: block;
    max-width: none;
    pointer-events: none;
    box-shadow: 0 4px 24px rgba(0, 0, 0, 0.35);
  }

  .failed {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--canvas-text-2);
  }

  .overlay {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
  }

  .handles {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  .handle {
    position: absolute;
    width: 48px;
    height: 48px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: auto;
    touch-action: none;
    cursor: grab;
    background: transparent;
    border: 0;
    border-radius: 50%;
    color: #fff;
  }

  .handle.active {
    background: rgba(43, 79, 216, 0.28);
    outline: 2px dashed #9db4ff;
    outline-offset: -2px;
  }

  .handle:focus-visible {
    outline: 3px solid #ffffff;
    outline-offset: -3px;
    box-shadow: 0 0 0 5px rgba(21, 24, 30, 0.9);
  }

  .dot {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #ffffff;
    border: 2.5px solid #15181e;
    box-shadow: 0 0 0 2px #ffffff;
  }

  .edge .dot {
    width: 12px;
    height: 12px;
    border-radius: 3px;
  }

  .active .dot {
    background: #2b4fd8;
    border-color: #ffffff;
    box-shadow: 0 0 0 2px #15181e;
  }

  .grip {
    cursor: move;
  }

  .gripicon {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: flex;
    align-items: center;
    justify-content: center;
    background: rgba(20, 24, 30, 0.62);
    border: 2px solid #ffffff;
  }

  .loupe {
    position: absolute;
    width: 124px;
    height: 124px;
    border-radius: 50%;
    overflow: hidden;
    border: 3px solid #fff;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
    background: #6b5b4e;
    pointer-events: none;
  }

  .loupe img {
    position: absolute;
    max-width: none;
  }

  .cross {
    position: absolute;
    left: 0;
    top: 0;
  }

  .zoom {
    position: absolute;
    left: 14px;
    bottom: 14px;
    display: flex;
    gap: 6px;
  }

  .zbtn {
    min-width: var(--ctl-h);
    height: var(--ctl-h);
    padding: 0 10px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: rgba(38, 42, 51, 0.85);
    border: 1px solid var(--canvas-btn-line);
    border-radius: 8px;
    color: #fff;
    font-size: 13px;
    font-weight: 500;
  }

  .zbtn:hover {
    background: rgba(60, 66, 80, 0.95);
  }

  .zbtn:focus-visible {
    outline-color: #ffffff;
  }
</style>
