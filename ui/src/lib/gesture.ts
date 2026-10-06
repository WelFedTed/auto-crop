// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Who owns a pointer on the source view. A handle drag, a pan, a pinch, a drawn box, a cut line and a tap
// that selects an item all start with a pointerdown, and they must never fight (PLAN 6.4.2): a finger on a
// handle is a handle drag, a second finger anywhere starts a pinch and cancels a box or a line, a pointer that
// lands while a handle is being dragged is ignored, and a tap that barely moved selects. This is the pure
// arbiter; CropStage.svelte does the DOM work.

export type StageTool = 'none' | 'add-tap' | 'add-draw' | 'cut' | 'merge';

export interface ArbiterState {
  /** A handle drag is in progress. */
  handleDrag: boolean;
  /** Pointers currently down on the stage background (not on a handle). */
  stagePointers: number;
  tool: StageTool;
}

/** What a new pointer on the stage background starts. */
export type StageStart = 'ignore' | 'pan' | 'box' | 'line' | 'pinch' | 'tap-add';

/** A pointerdown on a handle starts a drag only when nothing else owns the pointers. */
export function canStartHandle(s: ArbiterState): boolean {
  return !s.handleDrag && s.stagePointers === 0;
}

/**
 * A pointerdown on the stage background. While a handle drags it is ignored (no pan under the finger that
 * drags). A second pointer is always a pinch, whatever the tool. The first pointer pans, unless a tool draws.
 */
export function stageStart(s: ArbiterState): StageStart {
  if (s.handleDrag) return 'ignore';
  if (s.stagePointers >= 1) return 'pinch';
  switch (s.tool) {
    case 'add-draw':
      return 'box';
    case 'cut':
      return 'line';
    default:
      return 'pan';
  }
}

export interface DownInfo {
  x: number;
  y: number;
  t: number;
}

export const TAP_SLOP = 8;
export const TAP_MAX_MS = 600;

/** A press and release that barely moved and did not linger: a tap (selects an item, or adds one). */
export function isTap(down: DownInfo, up: DownInfo, slop = TAP_SLOP, maxMs = TAP_MAX_MS): boolean {
  return Math.hypot(up.x - down.x, up.y - down.y) <= slop && up.t - down.t <= maxMs;
}

/** A drag long enough to be a box or a line and not a tap. */
export function isDrag(down: DownInfo, up: DownInfo, min = 12): boolean {
  return Math.hypot(up.x - down.x, up.y - down.y) >= min;
}

/** A press that stays put for `ms` (the chip menu's press-and-hold): true once the time has passed without moving. */
export function isHold(down: DownInfo, now: DownInfo, ms = 500, slop = 10): boolean {
  return now.t - down.t >= ms && Math.hypot(now.x - down.x, now.y - down.y) <= slop;
}
