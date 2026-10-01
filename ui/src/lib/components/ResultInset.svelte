<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The result inset. While the person drags (or a commit is in flight) it shows a geometry-only preview
  // computed in the webview: a CSS matrix3d homography of the loaded `src` proxy, with no IPC (PLAN 2.5).
  // Once the backend's `result` image for the new generation has loaded it takes over (cross-fade, or
  // instant under reduced motion). On Compare it shows the untouched source instead.
  import { quadSize, quadToRect, toMatrix3d, type Pt } from '../homography.ts';
  import { S } from '../strings.ts';
  import type { Edit } from '../types.ts';

  let {
    srcUrl,
    resultUrl,
    edit,
    busy,
    compare,
    name,
  }: {
    srcUrl: string;
    resultUrl: string;
    /** The edit to preview: includes the live quad of a drag and the live angle of the ruler. */
    edit: Edit;
    /** A drag, a ruler drag or a commit is in flight: show the preview, not the backend image. */
    busy: boolean;
    compare: boolean;
    name: string;
  } = $props();

  const BOX_W = 168;
  const BOX_H = 196;

  let natW = $state(0);
  let natH = $state(0);
  let loadedResult = $state('');

  const theta = $derived(((edit.quarterTurns * 90 + edit.fineDeg) * Math.PI) / 180);

  const geom = $derived.by(() => {
    if (natW === 0 || natH === 0) return null;
    const q: Pt[] = edit.quad.map((p) => ({ x: p.x * natW, y: p.y * natH }));
    const raw = quadSize(q);
    const sw = Math.max(raw.w, 1);
    const sh = Math.max(raw.h, 1);
    const c = Math.abs(Math.cos(theta));
    const s = Math.abs(Math.sin(theta));
    const rw = sw * c + sh * s;
    const rh = sw * s + sh * c;
    const k = Math.min(BOX_W / rw, BOX_H / rh);
    const w = sw * k;
    const h = sh * k;
    let css = 'none';
    try {
      css = toMatrix3d(quadToRect(q, w, h));
    } catch {
      // degenerate quad: show nothing rather than a broken image
    }
    return { w, h, rw: rw * k, rh: rh * k, css };
  });

  const showPreview = $derived(busy || loadedResult !== resultUrl);
  const rot = $derived((edit.quarterTurns * 90 + edit.fineDeg).toFixed(2));
</script>

<figure class="inset">
  <div class="canvas" style:width="{compare ? BOX_W : (geom?.rw ?? BOX_W)}px" style:height="{compare ? Math.min(BOX_H + 24, BOX_W * (natH / Math.max(natW, 1))) || BOX_H : (geom?.rh ?? BOX_H)}px">
    <!-- hidden loader for the source proxy (the browser caches it with the stage's copy) -->
    <img class="src" src={srcUrl} alt="" draggable="false" style:display="none" onload={(e) => {
        const img = e.currentTarget as HTMLImageElement;
        natW = img.naturalWidth;
        natH = img.naturalHeight;
      }} />
    {#if compare}
      <img class="full" src={srcUrl} alt={S.editor.sourceAlt(name)} draggable="false" />
    {:else}
      <img
        class="full result"
        class:hidden={showPreview}
        src={resultUrl}
        alt={S.editor.resultAlt}
        draggable="false"
        onload={() => (loadedResult = resultUrl)}
        onerror={() => (loadedResult = '')}
      />
      {#if showPreview && geom}
        <div class="rot" style:width="{geom.w}px" style:height="{geom.h}px" style:transform="translate(-50%, -50%) rotate({rot}deg)">
          <div class="warp" style:width="{geom.w}px" style:height="{geom.h}px">
            <img
              src={srcUrl}
              alt=""
              draggable="false"
              style:width="{natW}px"
              style:height="{natH}px"
              style:transform={geom.css}
            />
          </div>
        </div>
      {/if}
    {/if}
  </div>
  <figcaption>
    <span class="t">{compare ? S.editor.original : S.editor.result}</span>
    <span class="s">{compare ? S.editor.source : showPreview ? 'Preview' : 'Rendered'}</span>
  </figcaption>
</figure>

<style>
  .inset {
    position: absolute;
    right: 16px;
    bottom: 16px;
    margin: 0;
    padding: 0;
    max-width: 196px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
    border: 1px solid #3a3f4b;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
    pointer-events: none;
  }

  .canvas {
    position: relative;
    margin: 0 auto;
    overflow: hidden;
    background: #ffffff;
  }

  .full {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #ffffff;
  }

  .full.hidden {
    opacity: 0;
  }

  .result {
    transition: opacity 0.15s ease;
  }

  .rot {
    position: absolute;
    left: 50%;
    top: 50%;
    transform-origin: 50% 50%;
  }

  .warp {
    position: relative;
    overflow: hidden;
  }

  .warp img {
    position: absolute;
    left: 0;
    top: 0;
    max-width: none;
    transform-origin: 0 0;
  }

  figcaption {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 8px;
    font-size: 11px;
    color: var(--text-2);
  }

  .t {
    font-weight: 600;
    color: var(--text);
  }

  .canvas ~ figcaption {
    background: var(--surface);
  }
</style>
