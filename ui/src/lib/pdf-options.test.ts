// SPDX-License-Identifier: MIT OR Apache-2.0
// SPDX-FileCopyrightText: 2026 Auto Crop contributors

// The options of "Save as one PDF" on the front end (ROADMAP M11.36, M11.54): page size, margin and title, the sheet that asks
// for them, what is remembered, and the contract with the shell. Drift guards read the Rust sources.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { DEFAULT_PDF_PREFS, MAX_MARGIN_MM, MAX_TITLE_CHARS, PDF_PAGE_SIZES, clampMargin, cleanTitle, parsePdfPrefs } from './pdf-options.ts';
import { S } from './strings.ts';

const read = (rel: string) => readFileSync(new URL(rel, import.meta.url), 'utf8');
const rust = (f: string) => read(`../../../${f}`);
const camel = (f: string) => f.replace(/_([a-z0-9])/g, (_, c: string) => c.toUpperCase());

test("the PDF options of the webview are the fields of the shell struct, and the values are the engine's", () => {
  const app = rust('crates/shell/src/app.rs');
  const start = app.indexOf('struct PdfOptionsIn {');
  const fields = [...app.slice(start, app.indexOf('\n}', start)).matchAll(/^ {4}([a-z_]+): /gm)].map((m) => camel(m[1]));
  const types = read('./types.ts');
  const iface = types.slice(types.indexOf('export interface PdfOptions {'), types.indexOf('/** Where a "Save as one PDF" run is'));
  const keys = [...iface.matchAll(/^ {2}([a-zA-Z]+): /gm)].map((m) => m[1]);
  assert.deepEqual([...keys].sort(), [...fields].sort());
  // The page sizes: the variants of the engine enum, lowercase (serde rename), in the order the sheet lists them.
  const run = rust('crates/engine/src/run_pdf.rs');
  const at = run.indexOf('pub enum PdfPageSize {');
  assert.match(run.slice(at - 120, at), /#\[serde\(rename_all = "lowercase"\)\]/);
  const variants = [...run.slice(at, run.indexOf('\n}', at)).matchAll(/^ {4}([A-Za-z0-9]+),/gm)].map((m) => m[1].toLowerCase());
  assert.deepEqual([...PDF_PAGE_SIZES], variants);
  assert.match(types, /export type PdfPageSize = 'fit' \| 'a4' \| 'a5' \| 'letter';/);
  // The limits.
  assert.match(run, new RegExp(`pub const MAX_MARGIN_MM: f64 = ${MAX_MARGIN_MM}\\.0;`));
  assert.match(rust('crates/codecs/src/encoders/pdf.rs'), new RegExp(`\\.take\\(${MAX_TITLE_CHARS}\\)`), 'the writer keeps this many characters of a title');
  assert.match(app, /title: options\.title\.filter\(/, 'a blank title is the file name');
});

test('the remembered page size and margin survive nonsense, and a margin is whole millimetres from 0 to 50', () => {
  assert.deepEqual(parsePdfPrefs(null), DEFAULT_PDF_PREFS);
  assert.deepEqual(parsePdfPrefs('{not json'), DEFAULT_PDF_PREFS);
  assert.deepEqual(parsePdfPrefs('[1]'), DEFAULT_PDF_PREFS);
  assert.deepEqual(parsePdfPrefs('{"pageSize":"a3","marginMm":"wide"}'), DEFAULT_PDF_PREFS);
  assert.deepEqual(parsePdfPrefs('{"pageSize":"letter","marginMm":12.6}'), { pageSize: 'letter', marginMm: 13 });
  assert.deepEqual(parsePdfPrefs('{"pageSize":"a4","marginMm":500}'), { pageSize: 'a4', marginMm: 50 });
  assert.equal(clampMargin(-3), 0);
  assert.equal(clampMargin('7'), 7);
  assert.equal(clampMargin(''), DEFAULT_PDF_PREFS.marginMm);
  assert.equal(clampMargin(NaN), DEFAULT_PDF_PREFS.marginMm);
  assert.equal(clampMargin(Infinity), DEFAULT_PDF_PREFS.marginMm);
  // Fit-to-picture is the default, as before the options existed.
  assert.equal(DEFAULT_PDF_PREFS.pageSize, 'fit');
  // A copy, never the shared object.
  assert.notEqual(parsePdfPrefs(null), DEFAULT_PDF_PREFS);
});

test('a blank title is none; control characters and length are cut', () => {
  assert.equal(cleanTitle(''), null);
  assert.equal(cleanTitle('   '), null);
  assert.equal(cleanTitle('  Receipts (May)  '), 'Receipts (May)');
  assert.equal(cleanTitle('a\u0000b\nc'), 'a b c');
  assert.equal([...(cleanTitle('x'.repeat(MAX_TITLE_CHARS + 50)) ?? '')].length, MAX_TITLE_CHARS);
  assert.equal(cleanTitle('åäö'), 'åäö');
});

test('the options sheet names every page size, labels its fields, and only a fixed page has a margin', () => {
  const sheet = read('./components/PdfSheet.svelte');
  for (const a of ['data-pdf-size', 'data-pdf-margin', 'data-pdf-title', 'data-pdf-go', 'data-pdf-sheet-cancel']) assert.ok(sheet.includes(a), a);
  for (const id of ['pdf-size', 'pdf-margin', 'pdf-doc-title']) {
    assert.match(sheet, new RegExp(`<label for="${id}">`), `${id} has a label`);
    assert.match(sheet, new RegExp(`id="${id}"`), id);
  }
  assert.match(sheet, /disabled=\{!fixed\}/, 'pages the size of their pictures have no margin');
  assert.match(sheet, /const fixed = \$derived\(pageSize !== 'fit'\)/);
  assert.match(sheet, /aria-labelledby="pdf-title"/);
  assert.match(sheet, /oncancel=\{\(e\) => \{ e\.preventDefault\(\); oncancel\(\); \}\}/, 'Escape closes the sheet through the same path as Cancel');
  assert.match(sheet, /\{#each PDF_PAGE_SIZES as s \(s\)\}/);
  assert.match(sheet, /SIZE_LABEL: Record<PdfPageSize, string>/, 'a new page size is a type error until it has a label');
  assert.match(sheet, /<form class="sheet" onsubmit=\{go\}>/, 'Enter in a field chooses');
  assert.match(sheet, /maxlength=\{MAX_TITLE_CHARS\}/);
  assert.equal(new Set([S.pdf.sizeFit, S.pdf.sizeA4, S.pdf.sizeA5, S.pdf.sizeLetter]).size, 4);
  for (const t of [S.pdf.sheetTitle, S.pdf.size, S.pdf.sizeNote, S.pdf.margin, S.pdf.marginUnit, S.pdf.marginNote, S.pdf.marginOff, S.pdf.title, S.pdf.titlePlaceholder, S.pdf.titleNote, S.pdf.choose, S.pdf.sheetCancel]) {
    assert.ok(t.length > 0);
  }
  assert.equal(S.pdf.sheetIntro(1), '1 image, a page for every crop, in the order they are shown. The originals are not touched.');
  assert.match(S.pdf.sheetIntro(3), /^3 images,/);
  assert.match(S.pdf.marginNote, new RegExp(`${MAX_MARGIN_MM} mm`));
  assert.match(S.pdf.sizeNote, /never enlarged/);
});

test('the store remembers the page size and margin (not the title) and sends all three', () => {
  const store = read('./store.svelte.ts');
  const run = store.slice(store.indexOf('async savePdf('), store.indexOf('cancelPdf()'));
  assert.match(run, /lsSet\(LS\.pdfPrefs, JSON\.stringify\(prefs\)\)/);
  assert.match(run, /marginMm: clampMargin\(choice\.marginMm\)/);
  assert.match(run, /title: cleanTitle\(choice\.title\)/);
  assert.doesNotMatch(run, /lsSet\([^)]*title/i, 'the title is per document');
  assert.match(store, /pdfPrefs = \$state<PdfPrefs>\(parsePdfPrefs\(lsGet\(LS\.pdfPrefs\)\)\)/);
});
