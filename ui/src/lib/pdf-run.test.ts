// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// "Save as one PDF" on the front end (ROADMAP M11.37, M11.50): the bulk action in the review grid, the progress, the result
// toast, the strings and the contract with the shell. Drift guards read the Rust sources, so a notice, a command or a field
// added on that side cannot be forgotten here.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { NOTICES, S, noticeText } from './strings.ts';

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const rust = (f: string) => read(`../../../${f}`);

test('every notice of the engine PDF run has copy (read from crates/engine)', () => {
  const src = rust('crates/engine/src/run_pdf.rs');
  const codes = [...src.matchAll(/pub const NOTICE_[A-Z_]+: &str = "([a-z_.]+)";/g)].map((m) => m[1]);
  assert.deepEqual([...codes].sort(), ['pdf.dpi_assumed', 'pdf.jpeg_pass_through', 'pdf.page_scaled', 'pdf.renamed']);
  for (const c of codes) {
    assert.ok(c in NOTICES, `no copy for ${c}`);
    assert.notEqual(noticeText(c), noticeText('some.future.code'), c);
  }
  assert.match(NOTICES['pdf.renamed'], /Nothing was replaced/);
});

test('the progress stages are the ones the engine reports, and the line counts pages from one', () => {
  const src = rust('crates/engine/src/run_pdf.rs');
  const stages = [...src.matchAll(/stage: "([a-z]+)"/g)].map((m) => m[1]);
  assert.deepEqual([...new Set(stages)].sort(), ['rendering', 'saving', 'verifying']);
  assert.equal(S.pdf.progress('rendering', 0, 5), 'Writing page 1 of 5');
  assert.equal(S.pdf.progress('verifying', 4, 5), 'Checking page 5 of 5');
  assert.equal(S.pdf.progress('rendering', 0, 0), 'Writing page 1 of …');
  assert.equal(S.pdf.progress('saving', 5, 5), 'Saving the PDF…');
  assert.equal(S.pdf.saved(1, 'a.pdf'), 'Saved 1 page as a.pdf.');
  assert.equal(S.pdf.saved(3, 'Scans.pdf'), 'Saved 3 pages as Scans.pdf.');
});

test('the shell registers the three PDF commands and the IPC wrappers call them by name', () => {
  const app = rust('crates/shell/src/app.rs');
  const handler = app.slice(app.indexOf('generate_handler!'));
  for (const c of ['save_pdf', 'cancel_pdf', 'open_pdf_folder']) assert.match(handler, new RegExp(`\\b${c},`), c);
  assert.match(app, /async fn save_pdf\(/, 'async: the dialog and the run must not block the main thread');
  assert.match(app, /blocking_save_file\(\)/, 'the name is picked by the Rust-side dialog');
  assert.match(app, /"pdf-progress"/);
  const backend = read('./backend.ts');
  assert.match(backend, /invoke<PdfSaved \| null>\('save_pdf', \{ ids, options \}\)/);
  assert.match(backend, /invoke<void>\('cancel_pdf'\)/);
  assert.match(backend, /invoke<void>\('open_pdf_folder'\)/);
  assert.match(backend, /impl\.on\('pdf-progress', cb\)/);
});

test('the file name is the only thing about the path that reaches the webview', () => {
  const app = rust('crates/shell/src/app.rs');
  const saved = app.slice(app.indexOf('struct PdfSaved'), app.indexOf('/// Asks where to save'));
  const fields = [...saved.matchAll(/^ {4}(?:pub )?([a-z_]+): /gm)].map((m) => m[1]);
  assert.deepEqual(fields, ['name', 'pages', 'bytes', 'pass_through_pages', 'notices', 'renamed']);
  const types = read('./types.ts');
  const iface = types.slice(types.indexOf('export interface PdfSaved'), types.indexOf('/** The commands the shell exposes'));
  for (const f of fields) {
    const camel = f.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
    assert.match(iface, new RegExp(`\\b${camel}:`), camel);
  }
  assert.doesNotMatch(iface, /path|dir|folder/i, 'no path in the result');
});

test('the grid has the action on the selection, in grid order, with progress and a cancel', () => {
  const grid = read('../routes/Grid.svelte');
  assert.match(grid, /data-bulk-pdf/);
  assert.match(grid, /sortClassified\(selectedList, sort\)/, 'page order = the order the grid shows');
  assert.match(grid, /x\.item\.crops\.some\(\(c\) => c\.include && c\.edit\)/, 'only images with a crop to write');
  assert.match(grid, /store\.savePdf\(pdfIds\)/);
  assert.match(grid, /data-pdf-progress/);
  assert.match(grid, /role="status"/);
  assert.match(grid, /data-pdf-cancel/);
  assert.match(grid, /<progress[^>]*aria-label=\{S\.pdf\.progress/s, 'the bar has an accessible name');
  assert.match(S.pdf.hint, /originals are not touched/);
});

test('the store ends a run with a toast that can open the folder, and treats a cancel as a calm message', () => {
  const store = read('./store.svelte.ts');
  const run = store.slice(store.indexOf('async savePdf('), store.indexOf('cancelPdf()'));
  assert.match(run, /label: S\.pdf\.openFolder/);
  assert.match(run, /api\.openPdfFolder\(\)/);
  assert.match(run, /codeOf\(e\) === 'CANCELLED'/);
  assert.match(run, /this\.pdfRun = null/, 'the busy state is always cleared');
  assert.match(run, /un\(\)/, 'the listener is always removed');
});
