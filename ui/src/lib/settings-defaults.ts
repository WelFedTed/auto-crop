// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

import type { Settings } from './types.ts';

/**
 * The defaults the engine uses (crates/engine/src/settings.rs). The store starts from these until
 * `getSettings` answers, and the browser mock serves them. A test keeps the field names in step with the Rust
 * struct, so a field added there cannot be forgotten here.
 */
export function defaultSettings(): Settings {
  return {
    saveAsCopy: false,
    retentionDays: 30,
    firstWriteAck: false,
    splitPolicy: 'auto',
    splitProfile: 'photos',
    autoSaveSplits: false,
  };
}

/**
 * A settings change to send to the engine: the WHOLE object received from `getSettings` with the change laid
 * over it. Sending only the changed field would reset every field this UI does not know about.
 */
export function patchedSettings(current: Settings, patch: Partial<Settings>): Settings {
  return { ...current, ...patch };
}
