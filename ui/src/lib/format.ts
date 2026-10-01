// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// Small display formatters (pure).

/** 1536 -> "1.5 KB". Binary units, one decimal under 10, none above. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = bytes;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  const text = u === 0 || v >= 10 ? Math.round(v).toString() : v.toFixed(1);
  return `${text} ${units[u]}`;
}

/** RFC 3339 -> "30 Sep 2026, 14:02" in the viewer's locale, or the input when it does not parse. */
export function formatDateTime(rfc3339: string): string {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return rfc3339;
  const date = d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
  const time = d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
  return `${date}, ${time}`;
}

export function formatDate(rfc3339: string): string {
  const d = new Date(rfc3339);
  if (Number.isNaN(d.getTime())) return rfc3339;
  return d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
}

/** Whether an RFC 3339 instant is in the past. */
export function isPast(rfc3339: string | null, now = Date.now()): boolean {
  if (!rfc3339) return false;
  const t = Date.parse(rfc3339);
  return Number.isFinite(t) && t < now;
}
