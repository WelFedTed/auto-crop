<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->
<script lang="ts" module>
  type El = { t: 'path'; d: string } | { t: 'circle'; cx: number; cy: number; r: number } | { t: 'rect'; x: number; y: number; width: number; height: number; rx: number };

  const p = (d: string): El => ({ t: 'path', d });

  // Stroke icons on a 24 px grid (1.75 stroke), drawn after the design canvas.
  export const ICONS = {
    back: [p('M15 5l-7 7 7 7')],
    next: [p('M9 5l7 7-7 7')],
    down: [p('M5 9l7 7 7-7')],
    undo: [p('M9 14L4 9l5-5'), p('M4 9h10a6 6 0 010 12h-3')],
    redo: [p('M15 14l5-5-5-5'), p('M20 9H10a6 6 0 000 12h3')],
    backups: [{ t: 'rect', x: 3.5, y: 4.5, width: 17, height: 4.5, rx: 1 }, p('M5 9v9a2 2 0 002 2h10a2 2 0 002-2V9M10 13h4')],
    settings: [p('M4 7h9M17 7h3M4 17h3M11 17h9'), { t: 'circle', cx: 15, cy: 7, r: 2 }, { t: 'circle', cx: 9, cy: 17, r: 2 }],
    shield: [p('M12 3l8 3v6c0 4.5-3.2 7.7-8 9-4.8-1.3-8-4.5-8-9V6z'), p('M8.5 12l2.5 2.5 4.5-5')],
    upload: [p('M12 16V5M7.5 9.5L12 5l4.5 4.5'), p('M5 15v3a2 2 0 002 2h10a2 2 0 002-2v-3')],
    image: [{ t: 'rect', x: 3.5, y: 4.5, width: 17, height: 15, rx: 2 }, { t: 'circle', cx: 9, cy: 10, r: 1.6 }, p('M4 17l5-4.5 4 3.5 3-2.5 4.5 3.5')],
    folder: [p('M3 7a2 2 0 012-2h4l2 2.5h8a2 2 0 012 2V17a2 2 0 01-2 2H5a2 2 0 01-2-2z')],
    lock: [{ t: 'rect', x: 5, y: 10.5, width: 14, height: 10, rx: 2 }, p('M8 10.5V8a4 4 0 018 0v2.5')],
    good: [{ t: 'circle', cx: 12, cy: 12, r: 9 }, p('M8 12.5l2.7 2.7L16 9.5')],
    check: [p('M12 4L21.5 20h-19z'), p('M12 10v4.5M12 17.2v.01')],
    failed: [{ t: 'circle', cx: 12, cy: 12, r: 9 }, p('M9 9l6 6M15 9l-6 6')],
    analysing: [p('M12 4v3M12 17v3M4 12h3M17 12h3M6.3 6.3l2.1 2.1M15.6 15.6l2.1 2.1M6.3 17.7l2.1-2.1M15.6 8.4l2.1-2.1')],
    crop: [p('M6 3v14a1 1 0 001 1h14M3 6h14a1 1 0 011 1v14')],
    close: [p('M6 6l12 12M18 6L6 18')],
    compare: [{ t: 'rect', x: 3.5, y: 5, width: 17, height: 14, rx: 2 }, p('M12 5v14')],
    rotateLeft: [p('M4 12a8 8 0 108-8 8 8 0 00-5.6 2.3L4 8.5'), p('M4 4v4.5h4.5')],
    rotateRight: [p('M20 12a8 8 0 11-8-8 8 8 0 015.6 2.3L20 8.5'), p('M20 4v4.5h-4.5')],
    info: [{ t: 'circle', cx: 12, cy: 12, r: 9 }, p('M12 11v5M12 8v.01')],
    plus: [p('M12 5v14M5 12h14')],
    minus: [p('M5 12h14')],
    check2: [p('M5 12.5l4.5 4.5L19 7')],
    move: [p('M12 3v18M3 12h18M9 6l3-3 3 3M9 18l3 3 3-3M6 9l-3 3 3 3M18 9l3 3-3 3')],
    pin: [p('M9 4h6l-1 6 3 3H7l3-3z'), p('M12 13v7')],
    trash: [p('M5 7h14M10 7V5h4v2M7 7l1 12h8l1-12')],
    refresh: [p('M20 11a8 8 0 10-2.3 5.7'), p('M20 4v7h-7')],
  } as const;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 18, stroke = 1.75 }: { name: IconName; size?: number; stroke?: number } = $props();
  const els = $derived(ICONS[name] as readonly El[]);
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#each els as el, i (i)}
    {#if el.t === 'path'}
      <path d={el.d} />
    {:else if el.t === 'circle'}
      <circle cx={el.cx} cy={el.cy} r={el.r} />
    {:else}
      <rect x={el.x} y={el.y} width={el.width} height={el.height} rx={el.rx} />
    {/if}
  {/each}
</svg>

<style>
  svg {
    flex-shrink: 0;
  }
</style>
