<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts">
  // A picture that changes address without ever going blank. Two <img> elements take turns: the new address loads in
  // the one that is hidden and is shown when it has arrived, so the old picture stays up until then. Each element keeps
  // its address while it is visible, so showing it again never fetches again (the `acimg` scheme answers with no-store).
  //
  // The engine answers a preview that a newer one has superseded with an empty 204, which an <img> reports as an error:
  // if that happens to the address that is still wanted, it is asked for again (twice at most, a moment apart).
  //
  // A preview address does not say which page it is a preview of, so `version` does (the crop's page key): an element
  // that holds the same address for another version is never taken for a match.
  import { tick, untrack } from 'svelte';

  let {
    url,
    version = '',
    alt = '',
    onstate,
    onsize,
  }: {
    /** The address wanted now; '' shows nothing. */
    url: string;
    /** Changes when the same address would show a different picture. */
    version?: string;
    alt?: string;
    /** `loading` is true while the wanted address has not arrived; `failed` when it gave up. */
    onstate?: (s: { loading: boolean; failed: boolean }) => void;
    /** The natural size of the picture on show. */
    onsize?: (w: number, h: number) => void;
  } = $props();

  const slots = $state([
    { url: '', ver: '', tries: 0 },
    { url: '', ver: '', tries: 0 },
  ]);
  let active = $state(0);
  let shownUrl = $state('');
  let shownVer = $state('');
  const els: (HTMLImageElement | null)[] = [null, null];

  function report(loading: boolean, failed: boolean): void {
    onstate?.({ loading, failed });
  }

  $effect(() => {
    const want = url;
    const ver = version;
    untrack(() => {
      if (!want) {
        shownUrl = '';
        shownVer = '';
        for (const s of slots) {
          s.url = '';
          s.ver = '';
        }
        report(false, false);
        return;
      }
      if (shownUrl === want && shownVer === ver) {
        report(false, false);
        return;
      }
      const idle = 1 - active;
      const el = els[idle];
      if (slots[idle].url === want && slots[idle].ver === ver && el && el.complete && el.naturalWidth > 0) {
        // That element already holds this picture (going back to an earlier look): show it without asking again.
        loaded(idle);
        return;
      }
      slots[idle].ver = ver;
      slots[idle].tries = 0;
      report(true, false);
      if (slots[idle].url === want) {
        // The same address for another version: the element must be emptied first or it would not fetch again.
        slots[idle].url = '';
        void tick().then(() => {
          if (url === want && version === ver && slots[idle].ver === ver) slots[idle].url = want;
        });
      } else {
        slots[idle].url = want;
      }
    });
  });

  function loaded(i: number): void {
    if (!url || slots[i].url !== url || slots[i].ver !== version) return;
    active = i;
    shownUrl = url;
    shownVer = version;
    const el = els[i];
    if (el) onsize?.(el.naturalWidth, el.naturalHeight);
    report(false, false);
  }

  async function failed(i: number): Promise<void> {
    const bad = slots[i].url;
    const ver = slots[i].ver;
    if (!bad || bad !== url || ver !== version) return; // an address nobody wants any more: ignore
    if (slots[i].tries >= 2) {
      report(false, true);
      return;
    }
    slots[i].tries++;
    await new Promise((r) => setTimeout(r, 220));
    if (slots[i].url !== bad || bad !== url || ver !== version) return;
    slots[i].url = '';
    await tick();
    if (url === bad && version === ver) slots[i].url = bad;
  }
</script>

{#each slots as slot, i (i)}
  <img
    bind:this={els[i]}
    class:on={active === i && shownUrl !== ''}
    src={slot.url || undefined}
    alt={active === i ? alt : ''}
    aria-hidden={active === i ? undefined : 'true'}
    draggable="false"
    onload={() => loaded(i)}
    onerror={() => void failed(i)}
  />
{/each}

<style>
  img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    opacity: 0;
    pointer-events: none;
  }

  img.on {
    opacity: 1;
  }
</style>
