<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // The result inset of a CURVED page: the engine's flattened picture, nothing computed here. While a handle is
  // dragged `url` is the engine's low-resolution live preview; when the drag settles it becomes the committed
  // crop image (same pixels, full preview resolution). The inset keeps showing the previous picture until the new
  // one has loaded, so it never flashes empty. `straight` shows the straight crop for comparison instead.
  import { S } from '../strings.ts';

  let {
    url,
    straight = false,
    live,
    name,
  }: {
    /** The picture to show: a live preview, the committed crop image or the straight-crop preview. */
    url: string;
    /** The picture is the straight crop (the same corners, edges straight), not the flattened page. */
    straight?: boolean;
    /** A drag is in progress: the picture is the engine's live preview. */
    live: boolean;
    name: string;
  } = $props();

  /** The URL that has loaded and is on show. */
  let shown = $state('');
  let failed = $state(false);

  $effect(() => {
    // A new target starts loading in the hidden loader below; the visible image only swaps when it is ready.
    void url;
    failed = false;
  });

  const loading = $derived(!!url && shown !== url && !failed);
</script>

<figure class="inset" data-flat-inset data-straight={straight ? '' : undefined} aria-label={straight ? S.curved.straightAlt : S.curved.flattenedAlt}>
  <div class="canvas" class:loading>
    {#if url}
      <!-- the loader: the browser caches it, and a failed or cancelled request leaves the previous picture up -->
      <img class="loader" src={url} alt="" draggable="false" onload={() => (shown = url)} onerror={() => (failed = true)} />
    {/if}
    {#if shown}
      <img class="full" src={shown} alt={straight ? S.curved.straightAlt : `${S.curved.flattenedAlt}: ${name}`} draggable="false" />
    {/if}
  </div>
  <figcaption>
    <span class="t">{straight ? S.curved.straightCrop : S.curved.flattened}</span>
    <span class="s">{loading ? S.curved.drawing : live ? S.curved.previewWord : S.curved.renderedWord}</span>
  </figcaption>
</figure>

<style>
  .inset {
    position: absolute;
    right: 16px;
    bottom: 16px;
    margin: 0;
    padding: 0;
    width: 176px;
    border-radius: 10px;
    overflow: hidden;
    background: var(--surface);
    border: 1px solid #3a3f4b;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.4);
    pointer-events: none;
  }

  @media (max-width: 600px) {
    .inset {
      right: 8px;
      bottom: 8px;
      width: 132px;
    }
  }

  .canvas {
    position: relative;
    height: 212px;
    background: #ffffff;
    overflow: hidden;
  }

  @media (max-width: 600px) {
    .canvas {
      height: 160px;
    }
  }

  .canvas.loading::after {
    content: '';
    position: absolute;
    left: 0;
    right: 0;
    top: 0;
    height: 3px;
    background: linear-gradient(90deg, transparent, #2b4fd8, transparent);
    background-size: 60% 100%;
    background-repeat: no-repeat;
    animation: sweep 0.9s linear infinite;
  }

  @media (prefers-reduced-motion: reduce) {
    .canvas.loading::after {
      animation: none;
      background: #2b4fd8;
      opacity: 0.6;
    }
  }

  @keyframes sweep {
    from {
      background-position: -60% 0;
    }
    to {
      background-position: 160% 0;
    }
  }

  .loader {
    position: absolute;
    width: 1px;
    height: 1px;
    opacity: 0;
  }

  .full {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: #ffffff;
  }

  figcaption {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    padding: 5px 8px;
    font-size: 11px;
    color: var(--text-2);
    background: var(--surface);
  }

  .t {
    font-weight: 600;
    color: var(--text);
  }
</style>
