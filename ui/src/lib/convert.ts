// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The pure rules of the Convert screen: option defaults and their storage, whether the run button is on and why
// not, when a progress announcement is due, and how a conversion shows up in Backups. No DOM, no Tauri.

import type { BackupFile, ConvertFileResult, ConvertOptions, ConvertPreview, ConvertQuality, ConvertTarget, QualityPreset } from './types.ts';

export const DEFAULT_OPTIONS: ConvertOptions = {
  target: 'jpg',
  colour: 'srgb',
  quality: { kind: 'preset', preset: 'balanced' },
  stripLocation: false,
  placement: 'replace',
  reencode: false,
};

/** The quality a "Custom" choice starts from. */
export const CUSTOM_QUALITY_START = 85;

export const PRESETS: readonly QualityPreset[] = ['small', 'balanced', 'best'];

/** The targets, in the order the screen lists them. */
export const TARGETS: readonly ConvertTarget[] = ['jpg', 'png', 'tiff', 'webp', 'pdf', 'avif'];

/** The targets this build can write: AVIF only when the shell says it has the writer. */
export function availableTargets(avif: boolean): ConvertTarget[] {
  return TARGETS.filter((t) => t !== 'avif' || avif);
}

/** Whether the quality setting does anything for this target: the JPG, the JPG inside a PDF page, and AVIF (its own scale). */
export function usesQuality(target: ConvertTarget): boolean {
  return target === 'jpg' || target === 'pdf' || target === 'avif';
}

export function clampQuality(n: number): number {
  if (!Number.isFinite(n)) return CUSTOM_QUALITY_START;
  return Math.min(100, Math.max(1, Math.round(n)));
}

function oneOf<T extends string>(v: unknown, allowed: readonly T[], fallback: T): T {
  return typeof v === 'string' && (allowed as readonly string[]).includes(v) ? (v as T) : fallback;
}

function qualityOf(v: unknown): ConvertQuality {
  const q = v as { kind?: unknown; preset?: unknown; value?: unknown } | null;
  if (q && q.kind === 'fixed' && typeof q.value === 'number') return { kind: 'fixed', value: clampQuality(q.value) };
  if (q && q.kind === 'preset') return { kind: 'preset', preset: oneOf(q.preset, PRESETS, 'balanced') };
  return { ...DEFAULT_OPTIONS.quality } as ConvertQuality;
}

/** Options from whatever was stored: every field is checked, so a stale or hand-edited value falls back to its default. */
export function sanitiseOptions(raw: unknown): ConvertOptions {
  const o = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>;
  return {
    target: oneOf(o.target, TARGETS, DEFAULT_OPTIONS.target),
    colour: oneOf(o.colour, ['srgb', 'preserve'] as const, DEFAULT_OPTIONS.colour),
    quality: qualityOf(o.quality),
    stripLocation: o.stripLocation === true,
    placement: oneOf(o.placement, ['replace', 'keep', 'folder'] as const, DEFAULT_OPTIONS.placement),
    reencode: o.reencode === true,
  };
}

export function parseOptions(stored: string | null): ConvertOptions {
  if (!stored) return sanitiseOptions(null);
  try {
    return sanitiseOptions(JSON.parse(stored));
  } catch {
    return sanitiseOptions(null);
  }
}

export function serialiseOptions(o: ConvertOptions): string {
  return JSON.stringify(sanitiseOptions(o));
}

export type WhyOff = 'files' | 'checking' | 'nothing' | 'folder' | null;

/** Why the Convert button is off, or null when it can run. Order: no files, still checking, nothing to do, no folder. */
export function whyOff(total: number, preview: ConvertPreview | null, options: ConvertOptions, checking: boolean): WhyOff {
  if (total === 0) return 'files';
  if (!preview || checking) return 'checking';
  if (preview.willConvert === 0) return 'nothing';
  if (options.placement === 'folder' && !preview.folder) return 'folder';
  return null;
}

/** The number of the 10% step a progress is in: 0 to 10. */
export function stepOf(done: number, total: number): number {
  return total <= 0 ? 10 : Math.min(10, Math.floor((done / total) * 10));
}

/** Whether a screen-reader announcement is due when a run goes from `before` to `done` files of `total`: at each new 10% step and at the end. */
export function announceDue(before: number, done: number, total: number): boolean {
  if (done >= total) return before < total;
  return stepOf(done, total) > stepOf(before, total);
}

/** The Backups file id of a result's backup. The engine names the backup of a replaced original; its only file is `0`. */
export function restoreFileId(backupId: string): string {
  return backupId.includes('/') ? backupId : `${backupId}/0`;
}

function stem(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(0, dot) : name;
}

/**
 * A conversion in Backups: a backup of one original with exactly one derived file, which carries the original's name
 * (or `name (2)` when that was taken). A split scan has two or more derived files named `name_01`, `name_02`...
 */
export function isConversionBackup(f: Pick<BackupFile, 'kind' | 'name' | 'derived'>): boolean {
  if (f.kind !== 'OneToN' || f.derived.length !== 1) return false;
  const a = stem(f.name);
  const b = stem(f.derived[0].name).replace(/ \(\d+\)$/, '');
  return a === b;
}

export interface ResultCounts {
  converted: number;
  skipped: number;
  failed: number;
  bytesIn: number;
  bytesOut: number;
}

/** Counts of results (the sizes are those of the converted files only). */
export function countResults(results: readonly ConvertFileResult[]): ResultCounts {
  const c: ResultCounts = { converted: 0, skipped: 0, failed: 0, bytesIn: 0, bytesOut: 0 };
  for (const r of results) {
    c[r.status]++;
    if (r.status === 'converted') {
      c.bytesIn += r.bytesIn;
      c.bytesOut += r.bytesOut;
    }
  }
  return c;
}

/** The notice codes worth a chip: those the status line does not already say. */
export function chipNotices(r: Pick<ConvertFileResult, 'status' | 'code' | 'notices'>): string[] {
  const implied = new Set<string>();
  if (r.status === 'skipped' && r.code === 'SAME_FORMAT') implied.add('convert.same_format');
  if (r.status === 'skipped' && r.code === 'HEIF_UNAVAILABLE') implied.add('heif.unavailable');
  return [...new Set(r.notices)].filter((n) => !implied.has(n));
}
