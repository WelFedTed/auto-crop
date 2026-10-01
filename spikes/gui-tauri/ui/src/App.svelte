<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // Throwaway GUI-stack spike (ROADMAP M0.57). Measures: view transform, quad handles with 48 px
  // hit areas, Pointer Events (touch, pen, mouse), CSS matrix3d drag preview, frame statistics.
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { quadSize, quadToRect, toMatrix3d, type Pt } from './homography.ts';

  type Info = { token: string; width: number; height: number };
  type Drag = { kind: 'corner' | 'edge' | 'grip'; idx: number; start: Pt; quad: Pt[] };

  const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

  let info = $state<Info>({ token: '', width: 1600, height: 1200 });
  let imgUrl = $state('');
  let imgReady = $state(false);
  let vw = $state(800);
  let vh = $state(600);
  let viewport: HTMLDivElement | undefined = $state();
  let view = $state({ s: 0.5, tx: 0, ty: 0 });

  // Receipt corners in image pixels (matches the synthetic scene drawn by the Rust side).
  function initialQuad(): Pt[] {
    const [cx, cy, hw, hh, a] = [800, 600, 236, 476, (5 * Math.PI) / 180];
    const at = (u: number, v: number): Pt => ({
      x: cx + u * Math.cos(a) - v * Math.sin(a),
      y: cy + u * Math.sin(a) + v * Math.cos(a),
    });
    return [at(-hw, -hh), at(hw, -hh), at(hw, hh), at(-hw, hh)];
  }
  let quad = $state<Pt[]>(initialQuad());
  let drag = $state<Drag | null>(null);

  const screen = $derived(quad.map((p) => ({ x: p.x * view.s + view.tx, y: p.y * view.s + view.ty })));
  const edges = $derived([0, 1, 2, 3].map((i) => mid(screen[i], screen[(i + 1) % 4])));
  const centre = $derived({
    x: screen.reduce((s, p) => s + p.x, 0) / 4,
    y: screen.reduce((s, p) => s + p.y, 0) / 4,
  });
  const polygon = $derived(screen.map((p) => `${p.x.toFixed(1)},${p.y.toFixed(1)}`).join(' '));
  const dim = $derived(
    `M0 0H${vw}V${vh}H0Z M${screen.map((p) => `${p.x.toFixed(1)} ${p.y.toFixed(1)}`).join('L')}Z`,
  );
  const stageTransform = $derived(
    `matrix3d(${view.s},0,0,0, 0,${view.s},0,0, 0,0,1,0, ${view.tx},${view.ty},0,1)`,
  );
  const inset = $derived.by(() => {
    const size = quadSize(quad);
    const k = Math.min(220 / Math.max(size.w, 1), 300 / Math.max(size.h, 1));
    const w = Math.max(size.w * k, 1);
    const h = Math.max(size.h * k, 1);
    try {
      return { w, h, css: toMatrix3d(quadToRect(quad, w, h)) };
    } catch {
      return { w, h, css: 'none' };
    }
  });

  function mid(a: Pt, b: Pt): Pt {
    return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
  }

  // ---------- image source ----------
  function platformUrl(token: string): string {
    const win = navigator.userAgent.includes('Windows');
    return win ? `http://acimg.localhost/${token}/0/0` : `acimg://localhost/${token}/0/0`;
  }

  async function browserScene(): Promise<string> {
    const c = document.createElement('canvas');
    c.width = 1600;
    c.height = 1200;
    const g = c.getContext('2d')!;
    const grad = g.createLinearGradient(0, 0, 1600, 1200);
    grad.addColorStop(0, '#8a7968');
    grad.addColorStop(1, '#5e5045');
    g.fillStyle = grad;
    g.fillRect(0, 0, 1600, 1200);
    g.translate(800, 600);
    g.rotate((5 * Math.PI) / 180);
    g.fillStyle = '#f6f4ee';
    g.fillRect(-230, -470, 460, 940);
    g.fillStyle = '#5c6270';
    for (let i = 0; i < 38; i++) g.fillRect(-194, -430 + i * 22, 120 + ((i * 37) % 120), 7);
    const blob: Blob = await new Promise((r) => c.toBlob((b) => r(b!), 'image/png'));
    return URL.createObjectURL(blob);
  }

  function fit() {
    // Read the element directly: the bound sizes can still be 0 when the image loads first.
    const w = viewport?.clientWidth || vw || 800;
    const h = viewport?.clientHeight || vh || 600;
    const k = Math.min(w / info.width, h / info.height) * 0.92;
    view = { s: k, tx: (w - info.width * k) / 2, ty: (h - info.height * k) / 2 };
  }
  function actual() {
    zoomAbout({ x: vw / 2, y: vh / 2 }, 1 / view.s);
  }
  function zoomAbout(p: Pt, factor: number) {
    const s = Math.min(Math.max(view.s * factor, 0.05), 8);
    const f = s / view.s;
    view = { s, tx: p.x - (p.x - view.tx) * f, ty: p.y - (p.y - view.ty) * f };
  }

  onMount(() => {
    (async () => {
      if (inTauri) {
        info = await invoke<Info>('launch_info');
        imgUrl = platformUrl(info.token);
      } else {
        imgUrl = await browserScene();
      }
    })();
    const onResize = () => fit();
    window.addEventListener('resize', onResize);
    requestAnimationFrame(frame);
    const t = setInterval(publishStats, 500);
    return () => {
      window.removeEventListener('resize', onResize);
      clearInterval(t);
    };
  });

  $effect(() => {
    if (viewport) {
      const el = viewport;
      const wheel = (e: WheelEvent) => {
        e.preventDefault();
        const r = el.getBoundingClientRect();
        zoomAbout({ x: e.clientX - r.left, y: e.clientY - r.top }, Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0015)));
      };
      el.addEventListener('wheel', wheel, { passive: false });
      return () => el.removeEventListener('wheel', wheel);
    }
  });

  // ---------- pointer handling ----------
  let lastPointerType = $state('none');
  const pointers = new Map<number, Pt>();

  function local(e: PointerEvent): Pt {
    const r = viewport!.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }
  function toImage(p: Pt): Pt {
    return { x: (p.x - view.tx) / view.s, y: (p.y - view.ty) / view.s };
  }

  function bgDown(e: PointerEvent) {
    lastPointerType = e.pointerType;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, local(e));
  }
  function bgMove(e: PointerEvent) {
    const prev = pointers.get(e.pointerId);
    if (!prev) return;
    const cur = local(e);
    if (pointers.size === 1) {
      view = { ...view, tx: view.tx + cur.x - prev.x, ty: view.ty + cur.y - prev.y };
    } else if (pointers.size === 2) {
      const other = [...pointers.entries()].find(([id]) => id !== e.pointerId)![1];
      const before = Math.hypot(prev.x - other.x, prev.y - other.y);
      const after = Math.hypot(cur.x - other.x, cur.y - other.y);
      const centre = { x: (cur.x + other.x) / 2, y: (cur.y + other.y) / 2 };
      if (before > 0) zoomAbout(centre, after / before);
      view = { ...view, tx: view.tx + (cur.x - prev.x) / 2, ty: view.ty + (cur.y - prev.y) / 2 };
    }
    pointers.set(e.pointerId, cur);
  }
  function bgUp(e: PointerEvent) {
    pointers.delete(e.pointerId);
  }

  function begin(e: PointerEvent, kind: Drag['kind'], idx: number) {
    e.preventDefault();
    e.stopPropagation();
    lastPointerType = e.pointerType;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { kind, idx, start: toImage(local(e)), quad: quad.map((p) => ({ ...p })) };
  }
  function moveBy(d: Drag, dx: number, dy: number) {
    const idxs = d.kind === 'corner' ? [d.idx] : d.kind === 'edge' ? [d.idx, (d.idx + 1) % 4] : [0, 1, 2, 3];
    const w = info.width;
    const h = info.height;
    let ddx = dx;
    let ddy = dy;
    for (const i of idxs) {
      ddx = Math.min(Math.max(ddx, -d.quad[i].x), w - d.quad[i].x);
      ddy = Math.min(Math.max(ddy, -d.quad[i].y), h - d.quad[i].y);
    }
    quad = d.quad.map((p, i) => (idxs.includes(i) ? { x: p.x + ddx, y: p.y + ddy } : { ...p }));
  }
  function dragMove(e: PointerEvent) {
    if (!drag) return;
    const p = toImage(local(e));
    moveBy(drag, p.x - drag.start.x, p.y - drag.start.y);
  }
  function dragEnd() {
    drag = null;
  }
  function dragCancel() {
    if (drag) quad = drag.quad;
    drag = null;
  }
  function nudge(e: KeyboardEvent, kind: Drag['kind'], idx: number) {
    const step = (e.shiftKey ? 10 : 1) / view.s;
    const map: Record<string, [number, number]> = {
      ArrowLeft: [-step, 0],
      ArrowRight: [step, 0],
      ArrowUp: [0, -step],
      ArrowDown: [0, step],
    };
    const v = map[e.key];
    if (!v) return;
    e.preventDefault();
    moveBy({ kind, idx, start: { x: 0, y: 0 }, quad: quad.map((p) => ({ ...p })) }, v[0], v[1]);
  }

  // ---------- frame statistics ----------
  const FRAMES = 240;
  let samples: number[] = [];
  let last = 0;
  let runSamples: number[] | null = null;
  let runStart = 0;
  let runBase: Pt[] = [];
  let stats = $state({ median: 0, p95: 0, fps: 0, n: 0 });
  let result = $state('');
  let running = $state(false);

  function quantile(sorted: number[], q: number): number {
    if (!sorted.length) return 0;
    return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  }
  function summarise(xs: number[]) {
    const s = [...xs].sort((a, b) => a - b);
    const median = quantile(s, 0.5);
    return { median, p95: quantile(s, 0.95), fps: median > 0 ? 1000 / median : 0, n: xs.length };
  }
  function publishStats() {
    stats = summarise(samples);
  }
  function frame(t: number) {
    if (last) {
      const dt = t - last;
      samples.push(dt);
      if (samples.length > FRAMES) samples.shift();
      if (runSamples) runSamples.push(dt);
    }
    last = t;
    if (runSamples) {
      const e = (t - runStart) / 1000;
      if (e >= 10) {
        finishRun();
      } else {
        const a = e * Math.PI;
        const dx = Math.cos(a) * 60;
        const dy = Math.sin(a) * 60;
        quad = runBase.map((p, i) => (i === 1 ? { x: p.x + dx, y: p.y + dy } : { ...p }));
      }
    }
    requestAnimationFrame(frame);
  }
  function startRun() {
    if (running) return;
    runBase = quad.map((p) => ({ ...p }));
    runSamples = [];
    runStart = performance.now();
    running = true;
    result = '';
  }
  function finishRun() {
    const xs = runSamples ?? [];
    runSamples = null;
    running = false;
    quad = runBase;
    const s = summarise(xs);
    result = JSON.stringify(
      {
        spike: 'M0.57',
        stack: inTauri ? 'tauri' : 'browser',
        userAgent: navigator.userAgent,
        dpr: window.devicePixelRatio,
        viewport: [vw, vh],
        image: [info.width, info.height],
        samples: s.n,
        medianMs: +s.median.toFixed(2),
        p95Ms: +s.p95.toFixed(2),
        passBar: 'median <= 17 ms, p95 <= 20 ms (M0.56)',
        verdict: s.median <= 17 && s.p95 <= 20 ? 'PASS' : 'FAIL',
        note: 'scripted handle drag, mouse-class input; touch cells stay UNMEASURED until run on a device',
      },
      null,
      2,
    );
  }
  async function copyResult() {
    try {
      await navigator.clipboard.writeText(result);
    } catch {
      /* clipboard may be unavailable in some webviews; the JSON stays on screen */
    }
  }

  const corners = ['Top-left corner', 'Top-right corner', 'Bottom-right corner', 'Bottom-left corner'];
  const sides = ['Top edge', 'Right edge', 'Bottom edge', 'Left edge'];
  const pct = (p: Pt) =>
    `x ${((p.x / info.width) * 100).toFixed(1)}%, y ${((p.y / info.height) * 100).toFixed(1)}%`;
</script>

<div class="app" data-input={lastPointerType}>
  <header>
    <strong>Auto Crop · GUI spike</strong>
    <span class="tag">{inTauri ? 'Tauri' : 'browser (no Tauri)'}</span>
    <span class="grow"></span>
    <button onclick={fit}>Fit</button>
    <button onclick={actual}>100%</button>
    <button onclick={() => zoomAbout({ x: vw / 2, y: vh / 2 }, 1.25)} aria-label="Zoom in">+</button>
    <button onclick={() => zoomAbout({ x: vw / 2, y: vh / 2 }, 0.8)} aria-label="Zoom out">−</button>
    <button class="primary" onclick={startRun} disabled={running}>{running ? 'Running 10 s…' : 'Scripted drag, 10 s'}</button>
  </header>

  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="viewport"
    bind:this={viewport}
    bind:clientWidth={vw}
    bind:clientHeight={vh}
    onpointerdown={bgDown}
    onpointermove={bgMove}
    onpointerup={bgUp}
    onpointercancel={bgUp}
    ondblclick={fit}
  >
    {#if imgUrl}
      <div class="stage" style:transform={stageTransform} style:width="{info.width}px" style:height="{info.height}px">
        <img
          src={imgUrl}
          alt="Synthetic receipt on a desk"
          draggable="false"
          width={info.width}
          height={info.height}
          onload={() => {
            imgReady = true;
            fit();
          }}
        />
      </div>
    {/if}

    {#if imgReady}
      <svg class="overlay" width={vw} height={vh} aria-hidden="true">
        <path d={dim} fill="#0b0d12" opacity="0.5" fill-rule="evenodd" />
        <polygon points={polygon} fill="none" stroke="#fff" stroke-width="2.5" />
        <polygon points={polygon} fill="none" stroke="#15181e" stroke-width="1" stroke-dasharray="5 5" />
      </svg>

      <div role="group" aria-label="Crop outline">
        {#each screen as p, i (i)}
          <button
            class="handle"
            class:active={drag?.kind === 'corner' && drag.idx === i}
            aria-roledescription="crop handle"
            aria-label="{corners[i]}, {pct(quad[i])}"
            style:left="{p.x - 24}px"
            style:top="{p.y - 24}px"
            onpointerdown={(e) => begin(e, 'corner', i)}
            onpointermove={dragMove}
            onpointerup={dragEnd}
            onpointercancel={dragCancel}
            onkeydown={(e) => nudge(e, 'corner', i)}
          ><span class="dot"></span></button>
        {/each}
        {#each edges as p, i (i)}
          <button
            class="handle edge"
            aria-roledescription="crop handle"
            aria-label={sides[i]}
            style:left="{p.x - 24}px"
            style:top="{p.y - 24}px"
            onpointerdown={(e) => begin(e, 'edge', i)}
            onpointermove={dragMove}
            onpointerup={dragEnd}
            onpointercancel={dragCancel}
            onkeydown={(e) => nudge(e, 'edge', i)}
          ><span class="dot"></span></button>
        {/each}
        <button
          class="handle grip"
          aria-label="Move whole outline"
          style:left="{centre.x - 24}px"
          style:top="{centre.y - 24}px"
          onpointerdown={(e) => begin(e, 'grip', 0)}
          onpointermove={dragMove}
          onpointerup={dragEnd}
          onpointercancel={dragCancel}
          onkeydown={(e) => nudge(e, 'grip', 0)}
        ><span class="dot"></span></button>
      </div>

      <div class="inset" aria-label="Result preview (CSS matrix3d, no IPC)">
        <div class="inset-box" style:width="{inset.w}px" style:height="{inset.h}px">
          <img
            src={imgUrl}
            alt=""
            draggable="false"
            style:width="{info.width}px"
            style:height="{info.height}px"
            style:transform={inset.css}
          />
        </div>
        <span>Result</span>
      </div>
    {/if}

    <aside class="hud" aria-live="off">
      <div><b>{stats.median.toFixed(1)}</b> ms median · <b>{stats.p95.toFixed(1)}</b> ms p95 · {stats.fps.toFixed(0)} fps · n={stats.n}</div>
      <div>input: {lastPointerType} · dpr {typeof window === 'undefined' ? 1 : window.devicePixelRatio} · {vw}×{vh}</div>
      <div>zoom {(view.s * 100).toFixed(0)}% · drag {drag ? 'on' : 'off'} · image {info.width}×{info.height}</div>
    </aside>
  </div>

  {#if result}
    <section class="result">
      <pre>{result}</pre>
      <button onclick={copyResult}>Copy JSON</button>
    </section>
  {/if}
</div>

<style>
  :global(html, body) {
    margin: 0;
    height: 100%;
    overscroll-behavior: none;
    font: 14px/1.4 system-ui, 'Segoe UI', sans-serif;
    background: #2a2d34;
    color: #eceef2;
  }
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: #1d2128;
    border-bottom: 1px solid #343a46;
  }
  .grow {
    flex: 1;
  }
  .tag {
    font-size: 12px;
    padding: 2px 8px;
    border-radius: 10px;
    background: #2f3541;
  }
  button {
    min-height: 32px;
    padding: 0 12px;
    border-radius: 8px;
    border: 1px solid #4a5160;
    background: #2a2f39;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  button.primary {
    background: #3e63e6;
    border-color: #3e63e6;
    color: #fff;
    font-weight: 600;
  }
  .app[data-input='touch'] button {
    min-height: 48px;
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    touch-action: none;
    user-select: none;
    background: #3a3d44;
  }
  .stage {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    will-change: transform;
  }
  .stage img {
    display: block;
    pointer-events: none;
  }
  .overlay {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
  }
  .handle {
    position: absolute;
    width: 48px;
    height: 48px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: transparent;
    display: flex;
    align-items: center;
    justify-content: center;
    touch-action: none;
    cursor: grab;
  }
  .handle .dot {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #fff;
    border: 2.5px solid #15181e;
    box-shadow: 0 0 0 2px #fff;
  }
  .handle.edge .dot {
    width: 12px;
    height: 12px;
    border-radius: 3px;
  }
  .handle.grip {
    background: rgba(20, 24, 30, 0.55);
    border: 2px solid #fff;
  }
  .handle.grip .dot {
    display: none;
  }
  .handle.active,
  .handle:focus-visible {
    background: rgba(62, 99, 230, 0.3);
    outline: 2px dashed #9db4ff;
  }
  .inset {
    position: absolute;
    right: 16px;
    bottom: 16px;
    padding: 6px;
    border-radius: 10px;
    background: #1d2128;
    border: 1px solid #4a5160;
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    pointer-events: none;
  }
  .inset-box {
    overflow: hidden;
    position: relative;
    background: #fff;
  }
  .inset-box img {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
    max-width: none;
  }
  .hud {
    position: absolute;
    left: 12px;
    top: 12px;
    padding: 8px 10px;
    border-radius: 8px;
    background: rgba(15, 18, 22, 0.82);
    font: 12px/1.5 ui-monospace, Consolas, monospace;
    pointer-events: none;
  }
  .result {
    position: absolute;
    left: 12px;
    right: 12px;
    bottom: 12px;
    max-height: 45vh;
    overflow: auto;
    padding: 10px;
    border-radius: 10px;
    background: #14171c;
    border: 1px solid #4a5160;
    display: flex;
    gap: 10px;
    align-items: flex-start;
  }
  .result pre {
    margin: 0;
    flex: 1;
    font: 12px/1.45 ui-monospace, Consolas, monospace;
    white-space: pre-wrap;
  }
</style>
