// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { canStartHandle, isDrag, isHold, isTap, stageStart, type ArbiterState } from './gesture.ts';

const s = (p: Partial<ArbiterState> = {}): ArbiterState => ({ handleDrag: false, stagePointers: 0, tool: 'none', ...p });

test('a handle starts a drag only when nothing else owns the pointers', () => {
  assert.equal(canStartHandle(s()), true);
  assert.equal(canStartHandle(s({ handleDrag: true })), false, 'a second finger on another handle is ignored');
  assert.equal(canStartHandle(s({ stagePointers: 1 })), false, 'a finger already panning, so a handle must not grab the pinch');
  assert.equal(canStartHandle(s({ stagePointers: 2 })), false);
});

test('a pointer on the background does not pan while a handle drags', () => {
  assert.equal(stageStart(s({ handleDrag: true })), 'ignore');
  assert.equal(stageStart(s({ handleDrag: true, stagePointers: 1 })), 'ignore');
});

test('the first background pointer pans (or draws, with a drawing tool) and the second always pinches', () => {
  assert.equal(stageStart(s()), 'pan');
  assert.equal(stageStart(s({ tool: 'merge' })), 'pan');
  assert.equal(stageStart(s({ tool: 'add-tap' })), 'pan', 'a tap tool still lets the view pan; the tap decides on release');
  assert.equal(stageStart(s({ tool: 'add-draw' })), 'box');
  assert.equal(stageStart(s({ tool: 'cut' })), 'line');
  for (const tool of ['none', 'add-draw', 'cut', 'merge', 'add-tap'] as const) {
    assert.equal(stageStart(s({ stagePointers: 1, tool })), 'pinch', `${tool}: a second finger pinches and cancels any box or line`);
  }
});

test('a tap is short and barely moved; a drag or a long press is not', () => {
  assert.equal(isTap({ x: 10, y: 10, t: 0 }, { x: 13, y: 12, t: 150 }), true);
  assert.equal(isTap({ x: 10, y: 10, t: 0 }, { x: 40, y: 12, t: 150 }), false);
  assert.equal(isTap({ x: 10, y: 10, t: 0 }, { x: 10, y: 10, t: 900 }), false);
  assert.equal(isDrag({ x: 0, y: 0, t: 0 }, { x: 30, y: 0, t: 100 }), true);
  assert.equal(isDrag({ x: 0, y: 0, t: 0 }, { x: 5, y: 0, t: 100 }), false);
});

test('a hold needs time and stillness', () => {
  assert.equal(isHold({ x: 0, y: 0, t: 0 }, { x: 2, y: 3, t: 520 }), true);
  assert.equal(isHold({ x: 0, y: 0, t: 0 }, { x: 2, y: 3, t: 300 }), false);
  assert.equal(isHold({ x: 0, y: 0, t: 0 }, { x: 40, y: 0, t: 800 }), false);
});
