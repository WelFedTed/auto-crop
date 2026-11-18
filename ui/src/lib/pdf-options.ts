// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The choices of "Save as one PDF" that stay between runs (page size, margin) and the rules for them. The values are the
// engine's: `PdfPageSize` and `MAX_MARGIN_MM` in crates/engine/src/run_pdf.rs (pdf-run.test.ts reads that file).

import type { PdfPageSize } from './types.ts';

/** The page sizes, in the order the dialog lists them; `fit` = each page is its picture's size. */
export const PDF_PAGE_SIZES: readonly PdfPageSize[] = ['fit', 'a4', 'a5', 'letter', 'custom'];

/** The largest page margin, in millimetres. */
export const MAX_MARGIN_MM = 50;

/** The smallest and largest side of a custom sheet, in millimetres (the engine's `MIN_CUSTOM_MM` and `MAX_CUSTOM_MM`). */
export const MIN_CUSTOM_MM = 20;
export const MAX_CUSTOM_MM = 1200;

/** The longest title the dialog takes (the writer keeps 200 characters). */
export const MAX_TITLE_CHARS = 200;

/** What is remembered: the title is per document and is not. */
export interface PdfPrefs {
  pageSize: PdfPageSize;
  marginMm: number;
  customWidthMm: number;
  customHeightMm: number;
}

export const DEFAULT_PDF_PREFS: PdfPrefs = { pageSize: 'fit', marginMm: 10, customWidthMm: 100, customHeightMm: 150 };

/** A margin the engine accepts: a whole number of millimetres from 0 to the maximum (anything else is the nearest). */
export function clampMargin(v: unknown): number {
  const n = typeof v === 'number' ? v : typeof v === 'string' && v.trim() !== '' ? Number(v) : NaN;
  if (!Number.isFinite(n)) return DEFAULT_PDF_PREFS.marginMm;
  return Math.min(MAX_MARGIN_MM, Math.max(0, Math.round(n)));
}

/** One side of a custom sheet the engine accepts: whole millimetres from 20 to 1200 (anything else is the nearest; a blank or non-number is `fallback`). */
export function clampSide(v: unknown, fallback: number): number {
  const n = typeof v === 'number' ? v : typeof v === 'string' && v.trim() !== '' ? Number(v) : NaN;
  if (!Number.isFinite(n)) return fallback;
  return Math.min(MAX_CUSTOM_MM, Math.max(MIN_CUSTOM_MM, Math.round(n)));
}

/** The remembered choices, or the defaults for anything missing or wrong (storage can hold anything). */
export function parsePdfPrefs(raw: string | null): PdfPrefs {
  if (!raw) return { ...DEFAULT_PDF_PREFS };
  try {
    const v: unknown = JSON.parse(raw);
    if (typeof v !== 'object' || v === null) return { ...DEFAULT_PDF_PREFS };
    const o = v as Record<string, unknown>;
    const size = (PDF_PAGE_SIZES as readonly unknown[]).includes(o.pageSize) ? (o.pageSize as PdfPageSize) : DEFAULT_PDF_PREFS.pageSize;
    return {
      pageSize: size,
      marginMm: clampMargin(o.marginMm),
      customWidthMm: clampSide(o.customWidthMm, DEFAULT_PDF_PREFS.customWidthMm),
      customHeightMm: clampSide(o.customHeightMm, DEFAULT_PDF_PREFS.customHeightMm),
    };
  } catch {
    return { ...DEFAULT_PDF_PREFS };
  }
}

/** The title as sent: trimmed, cut to the limit, and `null` when blank (the file's name is then the title). */
export function cleanTitle(raw: string): string | null {
  const t = [...raw.replace(/[\u0000-\u001f\u007f]/g, ' ').trim()].slice(0, MAX_TITLE_CHARS).join('').trim();
  return t === '' ? null : t;
}
