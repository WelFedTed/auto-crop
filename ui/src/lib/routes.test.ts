// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { parseRoute, routePath } from './routes.ts';

test('hash routes parse', () => {
  assert.deepEqual(parseRoute(''), { name: 'home' });
  assert.deepEqual(parseRoute('#/'), { name: 'home' });
  assert.deepEqual(parseRoute('#/grid'), { name: 'grid' });
  assert.deepEqual(parseRoute('#/item/42'), { name: 'item', id: 42 });
  assert.deepEqual(parseRoute('#/item/nope'), { name: 'grid' });
  assert.deepEqual(parseRoute('#/backups/'), { name: 'backups' });
  assert.deepEqual(parseRoute('#/settings'), { name: 'settings' });
  assert.deepEqual(parseRoute('#/whatever'), { name: 'home' });
});

test('paths round trip', () => {
  for (const r of [{ name: 'home' }, { name: 'grid' }, { name: 'item', id: 7 }, { name: 'backups' }, { name: 'settings' }] as const) {
    assert.deepEqual(parseRoute('#' + routePath(r)), r);
  }
});
