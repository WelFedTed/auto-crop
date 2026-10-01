<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { dropFilesIntoMock, isMock } from './lib/backend.ts';
  import Toasts from './lib/components/Toasts.svelte';
  import { router } from './lib/router.svelte.ts';
  import { store } from './lib/store.svelte.ts';
  import { S } from './lib/strings.ts';
  import Backups from './routes/Backups.svelte';
  import Editor from './routes/Editor.svelte';
  import Grid from './routes/Grid.svelte';
  import Home from './routes/Home.svelte';
  import Settings from './routes/Settings.svelte';

  onMount(() => {
    const stopRouter = router.start();
    void store.init();

    // Density follows the last pointer (PLAN 6.5): touch makes controls 48 px.
    const root = document.documentElement;
    root.dataset.input = matchMedia('(any-pointer: coarse)').matches && !matchMedia('(any-pointer: fine)').matches ? 'touch' : 'mouse';
    const onPointer = (e: PointerEvent) => {
      const next = e.pointerType === 'touch' ? 'touch' : 'mouse';
      if (root.dataset.input !== next) root.dataset.input = next;
    };
    window.addEventListener('pointerdown', onPointer, { capture: true, passive: true });

    // A drop outside the zone must never navigate the webview to the file. In the shell Rust handles
    // drops; in a plain browser the mock takes them so the flow can be tried.
    const stop = (e: DragEvent) => e.preventDefault();
    const onDrop = (e: DragEvent) => {
      e.preventDefault();
      const files = Array.from(e.dataTransfer?.files ?? []);
      if (files.length === 0 || !isMock) return;
      const p = dropFilesIntoMock(files);
      if (p) void p.then((s) => store.handleAdded(s));
    };
    window.addEventListener('dragover', stop);
    window.addEventListener('drop', onDrop);

    return () => {
      stopRouter();
      window.removeEventListener('pointerdown', onPointer, { capture: true });
      window.removeEventListener('dragover', stop);
      window.removeEventListener('drop', onDrop);
    };
  });

  // Move focus to the page heading on every route change (WCAG 2.4.3). The editor does its own.
  let routeKey = $derived(router.route.name === 'item' ? 'item' : router.route.name);
  let first = true;
  $effect(() => {
    void routeKey;
    if (!store.ready) return;
    if (first) {
      first = false;
      return;
    }
    void tick().then(() => {
      document.querySelector<HTMLElement>('[data-route-heading]')?.focus({ preventScroll: true });
    });
  });
</script>

<button type="button" class="skip" onclick={() => document.querySelector<HTMLElement>('[data-route-heading]')?.focus()}>
  {S.a11y.skipToContent}
</button>

<div id="main" class="shell">
  {#if store.fatal}
    <div class="boot" role="alert">
      <h1>{S.appName}</h1>
      <p>{S.toasts.backendError}</p>
      <p class="mono">{store.fatal}</p>
    </div>
  {:else if !store.ready}
    <div class="boot" role="status">
      <h1>{S.appName}</h1>
      <p>Loading…</p>
    </div>
  {:else}
    {@const route = router.route}
    {#if route.name === 'home'}
      <Home />
    {:else if route.name === 'grid'}
      <Grid />
    {:else if route.name === 'item'}
      {#key route.id}
        <Editor id={route.id} />
      {/key}
    {:else if route.name === 'backups'}
      <Backups />
    {:else if route.name === 'settings'}
      <Settings />
    {/if}
  {/if}
</div>

<Toasts />

<style>
  .shell {
    height: 100%;
    min-height: 0;
  }

  .boot {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-2);
  }

  .boot h1 {
    margin: 0;
    color: var(--text);
    font-size: 22px;
  }

  .skip {
    position: absolute;
    border: 0;
    left: 8px;
    top: -48px;
    z-index: 100;
    padding: 10px 14px;
    border-radius: 8px;
    background: var(--accent);
    color: var(--on-accent);
    font-weight: 600;
    text-decoration: none;
  }

  .skip:focus {
    top: 8px;
  }
</style>
