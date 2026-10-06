// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The undo history of one image as the engine keeps it, for the browser mock: a list of states with a cursor,
// where an edit made under the SAME gesture id as the newest entry replaces that entry instead of adding another
// (a drag or a burst of arrow-key nudges is ONE undo step). Pure, so it runs under `node --test`.

export interface Entry<S> {
  state: S;
  label: string;
  /** `<gesture id>:<crop id>` of the edit that made this entry, if it had one. */
  gesture?: string;
}

/**
 * Records `entry` after the cursor. The redo branch is dropped. When `entry` carries a gesture id equal to that of
 * the entry at the cursor (and that entry is not the base state), the two are merged: the new state replaces the
 * old one and the undo step keeps the first label.
 */
export function pushEntry<S>(hist: Entry<S>[], cursor: number, entry: Entry<S>): { hist: Entry<S>[]; cursor: number } {
  const kept = hist.slice(0, cursor + 1);
  const top = kept[kept.length - 1];
  if (entry.gesture !== undefined && top && kept.length > 1 && top.gesture === entry.gesture) {
    kept[kept.length - 1] = { state: entry.state, label: top.label, gesture: entry.gesture };
  } else {
    kept.push(entry);
  }
  return { hist: kept, cursor: kept.length - 1 };
}

/** The key an edit's gesture id and crop id make: a drag on crop 2 never merges with one on crop 3. */
export function gestureKey(gesture: number | null | undefined, crop: number): string | undefined {
  return gesture === null || gesture === undefined ? undefined : `${gesture}:${crop}`;
}
