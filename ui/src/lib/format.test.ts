// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { formatBytes, isPast } from './format.ts';

test('formatBytes', () => {
  assert.equal(formatBytes(0), '0 B');
  assert.equal(formatBytes(1536), '1.5 KB');
  assert.equal(formatBytes(96 * 1024 * 1024), '96 MB');
  assert.equal(formatBytes(212 * 1024 ** 3), '212 GB');
  assert.equal(formatBytes(-1), '0 B');
});

test('isPast', () => {
  assert.equal(isPast(null), false);
  assert.equal(isPast('2000-01-01T00:00:00Z', Date.parse('2026-01-01T00:00:00Z')), true);
  assert.equal(isPast('2030-01-01T00:00:00Z', Date.parse('2026-01-01T00:00:00Z')), false);
});
