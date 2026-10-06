<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The source view: the EXIF-oriented source proxy, one SVG quad per crop with an order chip, the selected
  // crop's handles, zoom and pan, and the item tools that draw on the picture (tap to add, draw a box, cut line).
  // The 48 px hit areas ARE the focusable buttons (PLAN 6.10), so touch targets, focus rings and the
  // accessibility tree cannot drift; the 16 px dual-stroke dot is drawn inside. All geometry is
  // normalised (0..1 of the displayed image), so the proxy's pixel size never matters.
  //
  // Pointers: gesture.ts decides who owns a pointer. A handle drag owns its pointer and ignores everything
  // else; the first finger on the background pans; a second finger pinches and cancels a box or a line; a tap
  // that barely moved selects the item under it (the selected item wins overlaps).
  import { untrack, type Snippet } from 'svelte';
  import { boxQuad, centre as polyCentre, hitCrop } from '../geometry.ts';
  import { canStartHandle, isDrag, isTap, stageStart, type DownInfo, type StageTool } from '../gesture.ts';
  import { cloneQuad, centroid, mid, moveQuad, toPercent, zoomAbout, type HandleKind, type Quad } from '../quad.ts';
  import type { CutPreview, StageCrop } from '../stage-types.ts';
  import { S } from '../strings.ts';
  import type { Band, Pt } from '../types.ts';
  import Icon from './Icon.svelte';

  let {
    src,
    name,
    fallbackAspect,
    crops,
    selectedId,
    tool = 'none',
    mergeIds = [],
    mergePreview = null,
    cutPreview = null,
    showOverlay,
    bandWord,
    onselect,
    onmenu,
    onrestore,
    onaddat,
    onaddbox,
    oncutdrag,
    onpreview,
    oncommit,
    children,
  }: {
    src: string;
    name: string;
    /** width / height of the item, used until the proxy has loaded. */
    fallbackAspect: number;
    crops: StageCrop[];
    selectedId: number | null;
    tool?: StageTool;
    /** Crops picked for a merge. */
    mergeIds?: number[];
    mergePreview?: Quad | null;
    cutPreview?: CutPreview | null;
    showOverlay: boolean;
    bandWord: (b: Band | null) => string;
    /** A tap or an order chip picked this crop (`fromStage` true for a tap on the picture). */
    onselect: (id: number, fromStage: boolean) => void;
    /** A long press or the context key on a chip: open the item menu next to it. */
    onmenu: (id: number, at: { x: number; y: number }) => void;
    /** "Add as item" on a removed candidate. */
    onrestore: (id: number) => void;
    onaddat: (p: Pt) => void;
    onaddbox: (q: Quad) => void;
    /** A line was dragged across the picture (source space). */
    oncutdrag: (a: Pt, b: Pt) => void;
    /** The live quad of the selected crop during a drag, or null when it ends or is cancelled. */
    onpreview: (q: Quad | null) => void;
    oncommit: (q: Quad, label: string, announce: string, source: 'drag' | 'key') => void;
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

  // When the stage changes size (a tool panel opens under it, the window resizes) keep what is at the centre of
  // the view at the centre, instead of letting the picture slide away from where the person was looking.
  let lastW = 0;
  let lastH = 0;
  $effect(() => {
    const w = stageW;
    const h = stageH;
    untrack(() => {
      if (lastW > 0 && lastH > 0 && !fitMode) {
        px += (w - lastW) / 2;
        py += (h - lastH) / 2;
      }
    });
    lastW = w;
    lastH = h;
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

  /** Zooms so `q` fills the view (the chip bar does this when an item is picked). Never zooms out below Fit. */
  export function zoomToQuad(q: readonly Pt[]): void {
    if (stageW <= 0 || stageH <= 0) return;
    const xs = q.map((p) => p.x);
    const ys = q.map((p) => p.y);
    const [x0, x1, y0, y1] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
    const m = 56;
    const zx = (stageW - 2 * m) / Math.max(1e-3, (x1 - x0) * bw);
    const zy = (stageH - 2 * m) / Math.max(1e-3, (y1 - y0) * bh);
    const nz = Math.min(8, Math.max(1, Math.min(zx, zy)));
    fitMode = false;
    z = nz;
    px = stageW / 2 - ((x0 + x1) / 2) * bw * nz;
    py = stageH / 2 - ((y0 + y1) / 2) * bh * nz;
  }

  const sx = (v: number) => px + v * bw * z;
  const sy = (v: number) => py + v * bh * z;
  const toScreen = (q: readonly Pt[]) => q.map((p) => ({ x: sx(p.x), y: sy(p.y) }));
  const polyPts = (q: readonly Pt[]) => toScreen(q).map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' ');

  // ---- display geometry --------------------------------------------------------------------
  let dragQuad = $state.raw<Quad | null>(null);
  const selected = $derived(crops.find((c) => c.id === selectedId && c.include) ?? null);
  const quad = $derived<Quad | null>(selected?.quad ?? null);
  const dq = $derived(dragQuad ?? quad);
  const screen = $derived(dq ? dq.map((p) => ({ x: sx(p.x), y: sy(p.y) })) : []);
  const shown = $derived(crops.map((c) => (c.id === selectedId && dragQuad ? { ...c, quad: dragQuad } : c)));
  const includedShown = $derived(shown.filter((c) => c.include));
  const ghosts = $derived(shown.filter((c) => !c.include));
  /** One crop only: dim everything outside it (the single-item look). With several, dimming would hide the others. */
  const dimOutside = $derived(includedShown.length === 1 && !!selected && tool === 'none');
  const dim = $derived(
    dimOutside && screen.length === 4
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
    if (!dq || screen.length !== 4 || tool !== 'none') return [];
    const who = selected && includedShown.length > 1 ? `${S.items.itemN(selected.order)}: ` : '';
    const out: HandleDef[] = [];
    dq.forEach((p, i) => {
      out.push({
        key: `c${i}`,
        kind: 'corner',
        idx: i,
        x: screen[i].x,
        y: screen[i].y,
        label: who + S.editor.handleLabel(S.editor.corners[i], toPercent(p.x), toPercent(p.y)),
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
        label: who + S.editor.handleLabel(S.editor.edges[i], toPercent(m.x), toPercent(m.y)),
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
      label: who + S.editor.handleLabel(S.editor.grip, toPercent(c.x), toPercent(c.y)),
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

  /** Pointer capture can refuse (an unknown pointer id, a pointer that already ended): the gesture still works without it. */
  function capture(el: HTMLElement, pointerId: number): void {
    try {
      el.setPointerCapture(pointerId);
    } catch {
      // not capturable
    }
  }

  function handleDown(e: PointerEvent, h: HandleDef): void {
    if (!quad || e.button > 0) return;
    e.preventDefault();
    e.stopPropagation();
    // A finger that lands on a handle while the picture is panning or pinching is part of that gesture.
    if (!canStartHandle({ handleDrag: !!drag, stagePointers: pointers.size, tool })) return;
    const el = e.currentTarget as HTMLElement;
    capture(el, e.pointerId);
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
      oncommit(final, labelFor(d.kind), h.label, 'drag');
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
    oncommit(next, labelFor(kind), h.label, 'key');
  }

  // ---- pan, pinch, tap, box and line -----------------------------------------------------------
  const pointers = new Map<number, { x: number; y: number }>();
  const downs = new Map<number, DownInfo>();
  let pan: { x0: number; y0: number; px0: number; py0: number } | null = null;
  let pinch: { d0: number; z0: number } | null = null;
  let box = $state.raw<{ a: Pt; b: Pt } | null>(null);
  let line = $state.raw<{ a: Pt; b: Pt } | null>(null);

  function stagePoint(e: { clientX: number; clientY: number }): { x: number; y: number } {
    const r = stageEl!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  /** A pointer position in source space (0..1 of the image), clamped to the picture. */
  function imgPoint(e: { clientX: number; clientY: number }): Pt {
    const p = stagePoint(e);
    return { x: Math.min(1, Math.max(0, (p.x - px) / (bw * z))), y: Math.min(1, Math.max(0, (p.y - py) / (bh * z))) };
  }

  function stageDown(e: PointerEvent): void {
    if ((e.target as HTMLElement).closest('.handle, .zoom, [data-nostage]')) return;
    const start = stageStart({ handleDrag: !!drag, stagePointers: pointers.size, tool });
    if (start === 'ignore') return;
    capture(stageEl!, e.pointerId);
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    downs.set(e.pointerId, { x: e.clientX, y: e.clientY, t: performance.now() });
    if (start === 'pinch') {
      // The second finger always pinches and abandons a box or a line in progress.
      box = null;
      line = null;
      const [a, b] = [...pointers.values()];
      pinch = { d0: Math.hypot(a.x - b.x, a.y - b.y) || 1, z0: z };
      pan = null;
    } else if (start === 'box') {
      box = { a: imgPoint(e), b: imgPoint(e) };
    } else if (start === 'line') {
      line = { a: imgPoint(e), b: imgPoint(e) };
    } else {
      pan = { x0: e.clientX, y0: e.clientY, px0: px, py0: py };
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
    } else if (box) {
      box = { a: box.a, b: imgPoint(e) };
    } else if (line) {
      line = { a: line.a, b: imgPoint(e) };
    } else if (pan) {
      fitMode = false;
      px = pan.px0 + (e.clientX - pan.x0);
      py = pan.py0 + (e.clientY - pan.y0);
    }
  }

  function stageUp(e: PointerEvent): void {
    const down = downs.get(e.pointerId);
    const wasPinching = !!pinch;
    pointers.delete(e.pointerId);
    downs.delete(e.pointerId);
    pinch = null;
    pan = null;
    if (e.type === 'pointercancel') {
      box = null;
      line = null;
    } else if (down && !wasPinching) {
      const up: DownInfo = { x: e.clientX, y: e.clientY, t: performance.now() };
      if (box) {
        const q = isDrag(down, up) ? boxQuad(box.a, box.b) : null;
        box = null;
        if (q) onaddbox(q);
      } else if (line) {
        const l = line;
        line = null;
        if (isDrag(down, up, 24)) oncutdrag(l.a, l.b);
      } else if (isTap(down, up)) {
        tap(e);
      }
    }
    if (pointers.size === 1) {
      const [p] = [...pointers.values()];
      pan = { x0: p.x, y0: p.y, px0: px, py0: py };
    }
  }

  function tap(e: PointerEvent): void {
    const p = imgPoint(e);
    if (tool === 'add-tap') {
      onaddat(p);
      return;
    }
    if (tool === 'cut' || tool === 'add-draw') return;
    const id = hitCrop(p, includedShown.map((c) => ({ id: c.id, quad: c.quad, include: true })), selectedId, false, natW || 1, natH || 1);
    if (id !== null) onselect(id, true);
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
    if (e.key === 'Escape' && (drag || box || line)) {
      e.preventDefault();
      e.stopPropagation();
      cancelDrag();
      box = null;
      line = null;
    }
  }

  // ---- chips -----------------------------------------------------------------------------------
  let holdTimer: ReturnType<typeof setTimeout> | undefined;
  let held = false;

  function chipDown(e: PointerEvent, id: number): void {
    held = false;
    clearTimeout(holdTimer);
    const x = e.clientX;
    const y = e.clientY;
    holdTimer = setTimeout(() => {
      held = true;
      onmenu(id, { x, y });
    }, 500);
  }

  function chipEnd(): void {
    clearTimeout(holdTimer);
  }

  function chipClick(id: number): void {
    if (held) {
      held = false;
      return;
    }
    onselect(id, false);
  }

  function chipKey(e: KeyboardEvent, id: number): void {
    if (e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10')) {
      e.preventDefault();
      const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
      onmenu(id, { x: r.left, y: r.bottom });
    }
  }

  const chipPos = (c: StageCrop) => {
    const s = toScreen(c.quad);
    // the top-most corner pair's left point, nudged inside the quad
    const tl = s.reduce((a, p) => (p.y + p.x * 0.01 < a.y + a.x * 0.01 ? p : a), s[0]);
    const cc = polyCentre(s);
    return { x: tl.x + (cc.x - tl.x) * 0.24, y: tl.y + (cc.y - tl.y) * 0.24 };
  };

  const inMerge = (id: number) => mergeIds.includes(id);

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

  const toolHint = $derived(
    tool === 'add-tap' ? S.items.addTapHint : tool === 'add-draw' ? S.items.addDrawHint : tool === 'cut' ? S.items.cutHint : tool === 'merge' ? S.items.mergeHint(mergeIds.length) : '',
  );
</script>

<svelte:window onkeydowncapture={onWindowKey} />

<div
  class="stage"
  class:tool={tool !== 'none'}
  class:crosshair={tool === 'add-tap' || tool === 'add-draw' || tool === 'cut'}
  bind:this={stageEl}
  bind:clientWidth={stageW}
  bind:clientHeight={stageH}
  onpointerdown={stageDown}
  onpointermove={stageMove}
  onpointerup={stageUp}
  onpointercancel={stageUp}
  oncontextmenu={(e) => e.preventDefault()}
  role="presentation"
  data-stage
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

  {#if showOverlay}
    <svg class="overlay" width={stageW} height={stageH} aria-hidden="true" data-overlay>
      {#if dim}
        <path d={dim} fill="#0b0d12" fill-opacity="0.5" fill-rule="evenodd" />
      {/if}
      {#each ghosts as g (g.id)}
        <g class="ghost" data-ghost={g.id}>
          <polygon points={polyPts(g.quad)} fill="#ffffff" fill-opacity="0.06" stroke="#ffffff" stroke-opacity="0.75" stroke-width="2" stroke-dasharray="3 6" stroke-linejoin="round" />
        </g>
      {/each}
      {#each includedShown as c (c.id)}
        {#if c.id !== selectedId}
          <g data-crop={c.id}>
            <polygon
              points={polyPts(c.quad)}
              fill={inMerge(c.id) ? '#3e63e6' : 'none'}
              fill-opacity={inMerge(c.id) ? 0.28 : 0}
              stroke="#ffffff"
              stroke-width="2.5"
              stroke-linejoin="round"
            />
            <polygon points={polyPts(c.quad)} fill="none" stroke="#15181e" stroke-width="1" stroke-dasharray="5 5" />
          </g>
        {/if}
      {/each}
      {#if selected}
        <g data-crop={selected.id} data-selected>
          <polygon
            points={polyPts(shown.find((c) => c.id === selected.id)!.quad)}
            fill={inMerge(selected.id) ? '#3e63e6' : 'none'}
            fill-opacity={inMerge(selected.id) ? 0.28 : 0}
            stroke="#ffffff"
            stroke-width="3.5"
            stroke-linejoin="round"
          />
          <polygon points={polyPts(shown.find((c) => c.id === selected.id)!.quad)} fill="none" stroke="#2b4fd8" stroke-width="1.5" stroke-dasharray="6 5" />
        </g>
      {/if}
      {#if mergePreview}
        <polygon points={polyPts(mergePreview)} fill="#7c9bff" fill-opacity="0.18" stroke="#ffffff" stroke-width="3" stroke-dasharray="9 5" stroke-linejoin="round" data-merge-preview />
      {/if}
      {#if cutPreview}
        {#if cutPreview.pieces}
          <polygon points={polyPts(cutPreview.pieces[0])} fill="#3e63e6" fill-opacity="0.3" stroke="#ffffff" stroke-width="2.5" stroke-linejoin="round" data-cut-piece="0" />
          <polygon points={polyPts(cutPreview.pieces[1])} fill="#f5a524" fill-opacity="0.3" stroke="#ffffff" stroke-width="2.5" stroke-linejoin="round" data-cut-piece="1" />
        {/if}
        {@const l = toScreen(cutPreview.line)}
        <line x1={l[0].x} y1={l[0].y} x2={l[1].x} y2={l[1].y} stroke="#ffffff" stroke-width="4" stroke-linecap="round" />
        <line x1={l[0].x} y1={l[0].y} x2={l[1].x} y2={l[1].y} stroke={cutPreview.pieces ? '#15181e' : '#ff6b5e'} stroke-width="1.5" stroke-dasharray="6 5" stroke-linecap="round" />
      {/if}
      {#if box}
        {@const a = { x: sx(box.a.x), y: sy(box.a.y) }}
        {@const b = { x: sx(box.b.x), y: sy(box.b.y) }}
        <rect x={Math.min(a.x, b.x)} y={Math.min(a.y, b.y)} width={Math.abs(a.x - b.x)} height={Math.abs(a.y - b.y)} fill="#7c9bff" fill-opacity="0.2" stroke="#ffffff" stroke-width="2.5" stroke-dasharray="7 5" data-box />
      {/if}
      {#if line}
        {@const a = { x: sx(line.a.x), y: sy(line.a.y) }}
        {@const b = { x: sx(line.b.x), y: sy(line.b.y) }}
        <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#ffffff" stroke-width="4" stroke-linecap="round" />
        <line x1={a.x} y1={a.y} x2={b.x} y2={b.y} stroke="#15181e" stroke-width="1.5" stroke-dasharray="6 5" stroke-linecap="round" />
      {/if}
    </svg>

    <div class="chips" role="group" aria-label={S.items.overlayLabel}>
      {#each includedShown as c (c.id)}
        {@const p = chipPos(c)}
        <button
          type="button"
          class="chip {c.band ?? 'check'}"
          class:selected={c.id === selectedId}
          class:merge={inMerge(c.id)}
          data-nostage
          data-chip={c.id}
          aria-label={`${S.items.itemN(c.order)}, ${bandWord(c.band)}${c.id === selectedId ? ', selected' : ''}`}
          aria-pressed={c.id === selectedId}
          style:left="{p.x - HIT / 2}px"
          style:top="{p.y - HIT / 2}px"
          onpointerdown={(e) => chipDown(e, c.id)}
          onpointerup={chipEnd}
          onpointercancel={chipEnd}
          onpointerleave={chipEnd}
          onclick={() => chipClick(c.id)}
          onkeydown={(e) => chipKey(e, c.id)}
          oncontextmenu={(e) => {
            e.preventDefault();
            onmenu(c.id, { x: e.clientX, y: e.clientY });
          }}
        >
          <span class="chipface">
            <Icon name={c.band === 'good' ? 'good' : c.band === 'failed' ? 'failed' : 'check'} size={12} stroke={2.4} />
            <span class="n">{c.order}</span>
          </span>
        </button>
      {/each}
      {#each ghosts as g (g.id)}
        {@const gp = chipPos(g)}
        {@const n = crops.findIndex((x) => x.id === g.id) + 1}
        <button
          type="button"
          class="ghostbtn"
          data-nostage
          data-restore={g.id}
          aria-label={S.items.addAsItemFor(n)}
          style:left="{gp.x}px"
          style:top="{gp.y}px"
          onclick={() => onrestore(g.id)}
        >
          <Icon name="plus" size={14} stroke={2.4} />
          {S.items.addAsItem}
        </button>
      {/each}
    </div>

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

  {#if toolHint}
    <div class="hint" role="status" data-nostage>{toolHint}</div>
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

  .stage.crosshair {
    cursor: crosshair;
  }

  /* On a phone the tools and chip bar sit under the picture: it keeps a usable height of its own. */
  @media (max-width: 900px) {
    .stage {
      flex: 0 0 auto;
      min-height: 56vh;
    }
  }

  .stage:active {
    cursor: grabbing;
  }

  .stage.crosshair:active {
    cursor: crosshair;
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

  .handles,
  .chips {
    position: absolute;
    inset: 0;
    pointer-events: none;
  }

  /* The order chip: a 28 px face inside a 48 px hit area, number plus band icon, never colour alone. */
  .chip {
    position: absolute;
    width: 48px;
    height: 48px;
    padding: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    pointer-events: auto;
    touch-action: none;
    background: transparent;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
  }

  .chipface {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    min-width: 40px;
    height: 28px;
    padding: 0 8px 0 6px;
    justify-content: center;
    border-radius: 14px;
    border: 2px solid #ffffff;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
    font-size: 13px;
    font-weight: 700;
  }

  .chip.good .chipface {
    background: #dff5e8;
    color: #0b5b33;
  }

  .chip.check .chipface {
    background: #fdecc4;
    color: #6b3800;
  }

  .chip.failed .chipface {
    background: #fcdedb;
    color: #a3201a;
  }

  .chip.selected .chipface {
    box-shadow:
      0 0 0 3px #2b4fd8,
      0 1px 4px rgba(0, 0, 0, 0.5);
  }

  .chip.merge .chipface {
    box-shadow:
      0 0 0 3px #7c9bff,
      0 1px 4px rgba(0, 0, 0, 0.5);
  }

  .chip:focus-visible {
    outline: 3px solid #ffffff;
    outline-offset: -3px;
    box-shadow: 0 0 0 5px rgba(21, 24, 30, 0.9);
  }

  .ghostbtn {
    position: absolute;
    transform: translate(-50%, -50%);
    min-height: 44px;
    padding: 0 12px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    pointer-events: auto;
    touch-action: none;
    background: rgba(255, 255, 255, 0.92);
    color: #15181e;
    border: 2px dashed #5b6372;
    border-radius: 10px;
    font-size: 13px;
    font-weight: 600;
    white-space: nowrap;
  }

  .ghostbtn:hover {
    background: #ffffff;
  }

  .ghostbtn:focus-visible {
    outline: 3px solid #ffffff;
    outline-offset: 2px;
    box-shadow: 0 0 0 5px rgba(21, 24, 30, 0.9);
  }

  .hint {
    position: absolute;
    left: 50%;
    top: 12px;
    transform: translateX(-50%);
    max-width: calc(100% - 32px);
    padding: 8px 14px;
    border-radius: 10px;
    background: rgba(21, 24, 30, 0.9);
    border: 1px solid rgba(255, 255, 255, 0.35);
    color: #ffffff;
    font-size: 13px;
    text-align: center;
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

  @media (forced-colors: active) {
    .chipface {
      border-color: CanvasText;
      background: Canvas;
      color: CanvasText;
    }
  }
</style>
