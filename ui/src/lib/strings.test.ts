// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ERRORS, FALLBACK_HOLD, HOLD, errorMessage, holdAction, holdCause, holdTitle, retentionText } from './strings.ts';
import type { ReasonCode } from './types.ts';

// Every code in the contract must have copy. The map is typed against `ReasonCode`, so adding a code to
// types.ts without adding it here fails `npm run check`.
const CONTRACT_CODES: Record<ReasonCode, true> = {
  NO_QUAD: true,
  WEAK_EDGE: true,
  PARTIAL_FRAME: true,
  ODD_ASPECT: true,
  LOW_CONTRAST_EDGE: true,
  IMPLAUSIBLE_QUAD: true,
};

test('every contract reason code has a title, cause and action', () => {
  for (const code of Object.keys(CONTRACT_CODES) as ReasonCode[]) {
    assert.ok(holdTitle({ code }).length > 0, code);
    assert.ok(holdCause({ code }).length > 10, code);
    assert.ok(holdAction({ code }).length > 3, code);
  }
});

test('PLAN 6.7 titles', () => {
  assert.equal(holdTitle({ code: 'NO_QUAD' }), "Couldn't find the edges");
  assert.equal(holdTitle({ code: 'PARTIAL_FRAME' }), 'Content may be cut off');
  assert.equal(holdTitle({ code: 'ODD_ASPECT' }), 'Unusual shape for a photo');
  assert.equal(holdTitle({ code: 'LOW_CONTRAST_EDGE' }), 'Edge hard to see');
  assert.equal(holdTitle({ code: 'BATCH_OUTLIER' as ReasonCode }), 'Differs from the rest of the batch');
});

test('WEAK_EDGE names the side', () => {
  assert.equal(holdTitle({ code: 'WEAK_EDGE', side: 'left' }), 'Edge unclear on the left side');
  assert.equal(holdTitle({ code: 'WEAK_EDGE', side: 'top' }), 'Edge unclear on the top side');
  assert.ok(!holdTitle({ code: 'WEAK_EDGE' }).includes('{side}'));
});

test('unknown codes never show a blank or a raw code', () => {
  const unknown = { code: 'FUTURE_CODE' as ReasonCode };
  assert.equal(holdTitle(unknown), 'Held for review');
  assert.ok(holdCause(unknown).length > 0);
  assert.ok(FALLBACK_HOLD.check.title.length > 0 && FALLBACK_HOLD.failed.title.length > 0);
});

test('the registry has no empty copy', () => {
  for (const [code, c] of Object.entries(HOLD)) {
    assert.ok(c.title && c.cause && c.action, code);
  }
});

test('typed error messages are complete and SOURCE_CHANGED matches PLAN 6.7', () => {
  for (const [code, text] of Object.entries(ERRORS)) assert.ok(text.length > 10, code);
  assert.equal(
    errorMessage('SOURCE_CHANGED'),
    'This file changed while Auto Crop was working, so it was left as it is. Its backup is kept.',
  );
  assert.equal(errorMessage('VERIFY_FAILED'), "Couldn't verify the new file. The original is unchanged.");
  assert.ok(errorMessage('ORIGINAL_EXPIRED').toLowerCase().includes('expired'));
  assert.ok(errorMessage('DISK_FULL').toLowerCase().includes('space'));
  assert.equal(errorMessage(null), ERRORS.INTERNAL);
});

test('retention text', () => {
  assert.equal(retentionText(30), '30 days');
  assert.equal(retentionText(null), 'until you delete them');
});
