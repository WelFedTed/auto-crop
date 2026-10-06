# The `auto-crop` command line

`auto-crop` is the headless tool of Auto Crop: it crops, straightens and perspective-corrects photos
of documents, receipts and scans from a terminal or a script, using the same engine as the app.

**Status: pre-alpha (0.x).** The detector is a classical one and its confidence is an uncalibrated
heuristic: on a small hand-labelled set of real photos many crops were not within 0.9 IoU, which is
why the default is the strictest setting and a high share of images is held for review. The
command-line tool is not in a release package yet (it is part of the v0.1.0 milestone, ROADMAP M2):
build it from source with `cargo build --release -p auto-crop-cli`; the binary is
`target/release/auto-crop`. HEIC, HEIF and AVIF open only in a build with the `heif` feature
(`auto-crop --version` says which build you have). There is no enhancement, no OCR and no network
code: the tool never connects to anything.

- [Quickstart](#quickstart)
- [What happens to my originals](#what-happens-to-my-originals)
- [Reference](#reference): [inputs](#inputs), [output modes](#output-modes), [held results](#held-results-and-presets), [multi-item scans](#scans-with-several-items), [process](#process), [analyze](#analyze), [render](#render), [restore](#restore), [backups](#backups), [doctor](#doctor-and---version)
- [Exit codes](#exit-codes)
- [The run manifest](#the-run-manifest)
- [Interrupting, progress, platforms](#interrupting-progress-and-platforms)
- [What is not there yet](#what-is-not-there-yet)
- [Design notes](#design-notes)

## Quickstart

Try it on a copy of a folder first. A dry run shows what would happen and writes nothing:

```text
auto-crop process --dry-run receipts/
```

Write the results next to the originals instead of replacing them:

```text
auto-crop process --suffix _cropped receipts/          # receipts/a_cropped.jpg, ...
auto-crop process --output cleaned/ -r receipts/       # mirrors the folder tree under cleaned/
auto-crop process --copy receipts/                     # receipts/AutoCrop/a.jpg, ...
```

With no output option, `process` replaces each original **after saving a verified backup of it**, and
`restore` puts it back byte for byte:

```text
auto-crop process receipts/*.jpg                        # (PowerShell and cmd do not expand *: the tool does)
auto-crop restore receipts/a.jpg                        # one file
auto-crop restore --run 01a1103657c06520100387c0b987    # everything one run changed (the id is in the manifest)
```

A result the detector is not sure about is **held**: nothing is written for it and the run exits 4.
`analyze` says why, without writing anything:

```text
auto-crop analyze receipts/blurry.jpg
auto-crop analyze --json --emit-edit edits/ receipts/   # also writes edits/<name>.edit.json
auto-crop render --edit edits/blurry.edit.json -o fixed.jpg receipts/blurry.jpg   # your own corner edit
```

Check the machine and the build:

```text
auto-crop --version
auto-crop doctor
```

## What happens to my originals

This is the default and the reason for the rest of this page: **`auto-crop process` with no output
option overwrites your files.** It does so in a way you can undo.

1. The image is analysed and the result is rendered and encoded in memory.
2. The result is written to a temporary file in the same folder, made durable, read back and decoded;
   its size and pixel dimensions must be what was encoded.
3. A **backup of the original** is copied into the backup store and verified against the hash taken
   when the file was read. No verified backup, no write.
4. The file is checked once more (it must still be what was read) and the temporary file replaces it
   in one atomic step. The original's modification time is kept.
5. The backup is recorded as belonging to that output, so `restore` can tell an untouched result from
   one you edited afterwards.

If anything fails before step 4 the original is untouched and the item is reported as failed. A Ctrl+C
finishes the file in progress and stops, so each file is either as it was or fully replaced with its backup
in place. After a crash (a power cut, a kill) the same holds for the file itself, and a scan that was being
split into several files is finished or undone by the next start of the tool or the app (the files are
written together or not at all); a crash can leave a stray `.autocrop-*.tmp` file in the folder, which is
safe to delete (the sweep that removes those is a separate roadmap item).

**Where the backups are, and for how long.** `%LOCALAPPDATA%\AutoCrop\backups` on Windows,
`~/Library/Application Support/AutoCrop/backups` on macOS, `$XDG_DATA_HOME/auto-crop/backups` on Linux
(or under `--home DIR` / `AUTO_CROP_HOME`). They hold **full copies of your originals**, so treat the
folder like the originals. A backup is kept 30 days by default (the app's setting, which this tool
reads); `auto-crop backups purge` deletes backups on request, and a backup past its retention is
removed at the start of a later `process` run (never by `restore`, `analyze`, `render`, `backups list`,
`backups show`, `doctor` or `--dry-run`: the backup you are about to restore is not deleted first). A real
`restore` or `process` first finishes or undoes a save that a crash interrupted.

**Undoing.** `auto-crop restore <file>` puts the original back. If you have edited the result since,
it stops (exit 3) rather than overwrite your edit: `--if-modified copy` restores beside it as
`name (restored).ext`, `--if-modified backup` restores anyway and keeps your edited file inside the
backup entry. A restore is itself reversible for the same reason.

**What is never replaced.** A file is replaced in place only when this build can write its format
back without losing content: today JPEG and PNG, single frame. A WebP, TIFF (even single page), HEIC,
HEIF or AVIF source is **skipped** (`NOT_REPLACEABLE`) and stays byte-identical; use `--output`,
`--suffix` or `--copy` to get a copy (PNG, or JPEG for HEIC). Replacing an original with a file in
another format (a conversion) is not available yet, so `--format` needs a copy mode.

**What is never written.** Results below the cut-off, failed detections and multi-item scans that were
not accepted are held: no file, no backup (see [held results](#held-results-and-presets)).

**Copies.** `--output`, `--suffix` and `--copy` never touch an original and never overwrite an existing
file (a taken name gets `(2)`, or the image is skipped with `--if-exists skip`). They need no backup.

**Dry run.** `--dry-run` decodes and analyses and prints the plan; it writes no output, temporary
file, backup, journal entry, settings or notice record, and starts no purge. (If you ask for
`--manifest FILE` that file is written: it is the answer.)

**The notice.** The first time a run is about to replace originals it prints a short note to stderr
saying so, where the backups are and how to restore. It never blocks a script and never goes to
stdout; it is recorded as shown (`cli-notice-ack` in the config folder) and not repeated. `--quiet` and
`--no-config` do not show or record it.

## Reference

Global options, accepted before or after the command: `-q, --quiet`, `-v, --verbose`, `--json`,
`--ndjson`, `--home DIR` (also the environment variable `AUTO_CROP_HOME`), `--no-config`, `-h, --help`.
`-V, --version` and `help [command]` are commands of their own. Options are `--name value` or
`--name=value`; `-x` short forms may be clustered (`-rn`) and take an attached value (`-j4`); `--` ends the
options. `stdout` carries machine output only (`--json`, `--ndjson`, the report of `analyze`, the
listings); progress, warnings and summaries go to `stderr`.

The tool reads `settings.toml` only for the backup retention (and `--no-config` ignores even that),
so a run is reproduced by its command line and a toggle in the app never changes what a script does.

### Inputs

`process` and `analyze` take any number of files, folders and patterns:

- A **folder** is searched for images; with `-r`/`--recursive` also its sub-folders, up to
  `--max-depth N` levels (default 64). `--max-files N` (default 50000) stops the search and says so.
- **Patterns** (`*`, `?`, `[abc]`, `[a-z]`, and `**` for any number of folders) are expanded by the tool,
  because `cmd.exe` and PowerShell do not. A pattern that matches nothing is a failed input
  (`NO_MATCH`). Names starting with a dot are not matched by a wildcard.
- `--include GLOB` and `--exclude GLOB` (repeatable) filter by file name.
- **Links are never followed**: symbolic links, junctions and other reparse points (OneDrive
  placeholders too) are skipped and counted, so a loop cannot hang the walk and no file is reached twice.
- A walk skips hidden and system entries, the tool's own temporary files, folders called `AutoCrop` (the
  `--copy` output) and the backup store, and counts them.
- Files in a walk that are not images this build reads (`.txt`, `.pdf`, `.gif`...) are **ignored and
  counted** (`ignored_non_image`), not reported one by one. A file *named* on the command line that is
  not a readable image type is a skipped input (`UNSUPPORTED_FORMAT`); a file that is named but missing is
  a failed one (`NOT_FOUND`). Image types are matched by extension, then the content is sniffed: a
  `.jpg` that is not an image is skipped as unsupported, a damaged one fails as `CORRUPT`.
- A file is taken once however many routes lead to it.
- Files the tool wrote in an earlier run (found by hash in the backup store) are skipped as
  `ALREADY_PROCESSED`, so running a folder twice does not crop the crops. `--reprocess` takes them again.
- Paths may be any Unicode and longer than 260 characters on Windows. A command-line argument that is not
  valid Unicode is read lossily.

### Output modes

At most one of:

| Option | What is written |
|---|---|
| (none) or `--in-place` | The original is replaced after a verified backup. Needs a writable backup folder (exit 6 before anything is touched if it is not). |
| `--output DIR` (`-o`) | `DIR/<path relative to the folder you gave>`; the tree is mirrored. Files named directly go flat into `DIR`. |
| `--suffix TEXT` | Beside the original as `<name>TEXT.<ext>`. |
| `--copy` | `<folder of the original>/AutoCrop/<name>.<ext>`. |

For the three copy modes: `--name-template T` sets the file name (`{name}`, `{n}`, `{ext}`; the extension is
added if the template has no `{ext}`), `--if-exists keep-both|skip` says what to do when a name is taken
(default `keep-both`: the stem gets `(2)`), `--format jpg|png|keep` and `--quality 1-100` set the format and the
JPEG quality (default: the source's format when this build can write it, else PNG, and quality 92).
`--quality` also sets the quality of an in-place JPEG. Replacing an original in another format is not
available (usage error). `--margin PERCENT` grows (or, negative, trims) every crop by PERCENT of its size on each
side (a scale about the crop's centre, clamped to the image); it applies in every mode.

A scan with several items is written as `name_01`, `name_02`... (zero-padded to at least two digits), in
reading order.

### Held results and presets

Only results that are good enough are written (decision B4); a held result is reported, never written,
and **no option forces it**:

- A single crop is written when its confidence is **Good** at the run's cut-off. The score is an
  *uncalibrated* heuristic from 0 to 1; a hold reason raised by the detector (partial frame, weak edge...)
  also makes it Check whatever the score. Below 0.60, or a forced failure, is Failed in every mode.
- `--triage strict|balanced|aggressive` picks the cut-off: **strict** 0.95 (the default), **balanced** 0.90
  (experimental until the accuracy gate is measured), **aggressive** 0.80. `--min-confidence SCORE`
  sets it directly (0.60 to 1.0).
- No quad found: held as `DETECTION_FAILED`. Below the cut-off: `LOW_CONFIDENCE`. The detector's own
  reasons are listed in `reasons`.
- The held share is high by design while the detector is classical and uncalibrated; `analyze` shows the
  numbers per image.

### Scans with several items

A flatbed scan holding several photos or receipts is found by the engine and becomes several files.
`--split auto|always|never` (default `auto`) says whether to look (`never` treats the scan as one item), and
`--profile photos|receipts` what the items are (default `photos`).

- **Replacing the scan** is held (`SPLIT_HELD`) unless you pass `--accept-splits` *and* every item is Good
  at the strict cut-off. Then the scan is backed up and replaced by `name_01`, `name_02`..., all written or
  none; `restore` returns the scan, and `restore --derived remove` moves the unchanged derived files into
  the backup too (changed ones are always kept).
- **Copies** (`--output`, `--suffix`, `--copy`) of a split scan are written without acceptance: they
  destroy nothing. (A held *single* image is not written in any mode.)

### `process`

```text
auto-crop process [options] <file|folder|pattern>...
```

Options: the [input](#inputs) and [output](#output-modes) options above, the detection options
(`--triage`, `--min-confidence`, `--margin`, `--split`, `--profile`, `--accept-splits`), and

| Option | Meaning |
|---|---|
| `-n`, `--dry-run` | Analyse and print the plan; write nothing. Same statuses, codes and output names as the real run. |
| `--reprocess` | Take files that are outputs of an earlier run again. |
| `-j`, `--jobs N` | Images processed at once (default: half the cores, at most 4). The result does not depend on N. |
| `--mem-limit MB` | Cap on the decoded pixels of the running jobs (default: a quarter of the RAM, at most 4 GB, at most half of the free RAM). An image larger than the cap runs alone. |
| `--manifest FILE` | Also write the [run manifest](#the-run-manifest) to FILE (atomically). |
| `--hold-exit-zero` | Exit 0 when the only problem is held items. |
| `--progress auto\|always\|never` | The progress line on stderr (default: only on a terminal). |
| `--json`, `--ndjson` | The manifest as one document, or one event per line, on stdout. |

Reports on stderr: failed items always; held and skipped items and a summary unless `--quiet`; every item with
`--verbose`.

### `analyze`

```text
auto-crop analyze [options] <file|folder|pattern>...
```

Detection only: no image is written and no store is touched. For each image: size, the crops (corner
quad as fractions of the image, EXIF-oriented, turn, fine angle), the confidence at the cut-off and what
`process` would do (`write` or `held`). Takes the input and detection options, `--jobs`, `--mem-limit`,
`--timings` (per-stage milliseconds) and `--emit-edit DIR`, which writes `DIR/<name>.edit.json`, the
edit state `render --edit` applies. `--json` prints one document (`schema: auto-crop/analysis`),
`--ndjson` one event per image. Held results are a report, not a failure: exit 0 (3 for failed inputs, 5 for no input).

### `render`

```text
auto-crop render [options] --output <file> <image>
```

Writes the crop of one image to a file you choose, without touching the source and without the backup
store. The crop is the detector's, or the edit state of `--edit FILE` (an `analyze --emit-edit` file,
possibly corner-edited by hand). For the same format and quality the bytes equal `process`'s. `render`
writes what it is asked to even when `process` would hold the result (it says so on stderr, and `--json` has
`would_hold`); it refuses only what cannot be done: no crop (exit 3), an output that exists (`--force`
replaces it), an output that is the source (exit 2). A scan with several items writes `name_01`,
`name_02`... beside the given name. The format is `--format`, else the output's extension (`.jpg`, `.png`),
else the source's. Prints the output paths on stdout (or `--json`).

### `restore`

```text
auto-crop restore [options] <file|backup-id>...
auto-crop restore [options] --run <run-id>
```

Puts originals back. A file argument finds the latest saved backup whose original path, or (for a split scan)
one of whose outputs, is that file; an id (28 hexadecimal digits) names a backup; `--run` restores every
saved backup of a run, oldest first. `--if-modified fail|backup|copy` (default `fail`) says what to do when the
file changed since it was saved; `--derived keep|remove` what to do with the files of a split scan.
`--dry-run` reports what would happen (`would_restore`, and whether the file is `as_saved`, `modified`,
`missing` or already the `original`) and changes nothing. A backup that was already restored is skipped
(`ALREADY_RESTORED`); one that does not exist is `NO_BACKUP`. Exit 3 if anything failed.

### `backups`

```text
auto-crop backups list [--json]
auto-crop backups show <id> [--json]
auto-crop backups purge (--expired | --older-than DAYS | --id ID... | --all) [--yes] [--dry-run]
```

`list` and `show` never purge and never write. `purge` is the only command that destroys stored
originals: choose exactly one selection, and confirm with `--yes` (or answer at a terminal; without a
terminal and without `--yes` it refuses, exit 2). It never deletes a backup that is still being committed, nor (except
by an explicit `--id`) one that is pinned in the app. `--dry-run` lists what would go.

### `doctor` and `--version`

`auto-crop --version` prints the version and commit, the target, the features of this build, the formats
it decodes and writes, and that no network code is linked. `auto-crop doctor` (`--json` too) reports the CPU
and the **AVX2 floor** (Auto Crop is built and tested for CPUs with AVX2; `process`, `analyze` and `render` exit 6
on one without it), RAM and the memory cap, the backup store (path, entries, bytes, interrupted saves that the next
start will finish), the settings file, the decoders, the HEIF libraries when built in, and that there is no
sandbox yet (decoders run inside the process) and no webview is needed. It never creates or writes anything.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | All written, or skipped by choice (already processed, a format that is never replaced, `--if-exists skip`). |
| 1 | An internal error: a bug. |
| 2 | A usage error: the command line was wrong. Nothing was touched. |
| 3 | Some items failed (corrupt file, missing path, disk full...); the others may have succeeded. |
| 4 | No failures, but items were held for review (or the detection failed). Nothing was written for them. `--hold-exit-zero` makes it 0. |
| 5 | No supported input was found. |
| 6 | A precondition failed before anything was written: no AVX2, an unwritable backup folder or manifest path. |
| 130 | Interrupted (Ctrl+C or a termination signal) after a clean stop. |

When several apply: 130, then 1, then 3, then 4, then 5. `analyze` never exits 4. `restore` and `backups`
use 0, 2 and 3 (and 130).

## The run manifest

`process --json` prints, and `--manifest FILE` writes, one JSON document. `--ndjson` prints the same data as a
stream of events, each one line, each with `"v": 1`:
`{"t":"start", "tool", "run_id", "items", "dry_run"}`, then one `{"t":"item", ...}` per image (in completion order;
the fields are those of an entry of `items` below), then `{"t":"end", "summary", "exit_code", "exit_name", "cancelled"}`.

The schema is [schema/run-manifest.v1.schema.json](schema/run-manifest.v1.schema.json) (JSON Schema, checked
in CI against real runs). **Version 1 only ever gains fields**: ignore the ones you do not know. A
breaking change would be version 2 with a different `schema` value or `v`.

```json
{
  "schema": "auto-crop/run-manifest", "v": 1,
  "tool": { "name": "auto-crop", "version": "0.0.2" },
  "run": { "id": "...", "started": "2026-10-06T07:55:59Z", "finished": "...", "dry_run": false,
           "cancelled": false, "exit_code": 4, "exit_name": "held", "options": { "mode": "in_place", "triage": "strict", "cutoff": 0.95, "..." : "..." } },
  "summary": { "items": 3, "saved": 1, "held": 1, "failed": 0, "skipped": 1, "files_written": 1,
               "ignored_non_image": 0, "links_skipped": 0, "hidden_skipped": 0, "filtered_out": 0, "truncated": false },
  "items": [ { "index": 0, "input": "C:\\photos\\a.jpg", "status": "saved", "code": null, "reasons": [], "detail": null,
               "confidence": { "score": 0.983, "band": "good", "reasons": [] },
               "crops": 1, "split": false, "written": true,
               "outputs": [ { "path": "C:\\photos\\a.jpg", "bytes": 101866, "width": 744, "height": 705, "format": "jpeg" } ],
               "backup_id": "01a1103659a2f0e82835be2ed711",
               "ms": { "read": 0.4, "analyse": 412.6, "write": 85.6, "total": 498.9 } } ],
  "warnings": []
}
```

- `status` is one of `saved`, `held`, `failed`, `skipped`. In a dry run the statuses and output paths are those
  of the real run and `written` is `false` (`run.dry_run` is `true`).
- `code` is **one registry code** for a held, failed or skipped item; it is never English. `reasons` are further
  codes: the detector's hold reasons (`PARTIAL_FRAME`, `WEAK_EDGE`, `NO_QUAD`...) and notices such as
  `tiff.multi_page` or `format.write_unavailable`. They never hold anything on their own. `detail` is text for
  people and may change.
- `index` is the item's position (inputs that gave nothing come first, then the files in the order found); it is
  stable whatever `--jobs` is. `input` is the path as given (or walked); `outputs[].path` is absolute. Paths are
  UTF-8 (a path that is not valid Unicode is written lossily).
- `run.id` is also the id of the backup run: `auto-crop restore --run <id>`. `backup_id` is the backup of this
  original (`auto-crop backups show <id>`); it is `null` for held, skipped and copied items.
- `confidence.score` is **uncalibrated**; `band` is `good`, `check` or `failed` *at this run's cut-off*.

**Codes.** From the engine (`ErrKind`): `CORRUPT`, `UNSUPPORTED_FORMAT`, `TOO_LARGE`, `UNREADABLE`, `READ_ONLY`,
`DISK_FULL`, `FILE_IN_USE`, `VERIFY_FAILED`, `BACKUP_FAILED`, `SOURCE_CHANGED`, `NO_CROP`, `INTERNAL_PANIC`, and the
rest of its registry. From this tool:

| Code | Status | Meaning |
|---|---|---|
| `LOW_CONFIDENCE` | held | The score or a hold reason puts the crop below the cut-off. |
| `DETECTION_FAILED` | held | No usable page was found (or the band is Failed). |
| `SPLIT_HELD` | held | A scan with several items was not accepted for replacement. |
| `ALREADY_PROCESSED` | skipped | The file is an output of an earlier run (`--reprocess` overrides). |
| `NOT_REPLACEABLE` | skipped | The source is never replaced in place (format with no writer, multi-page). |
| `UNSUPPORTED_FORMAT` | skipped | Not an image this build reads. |
| `CANCELLED` | skipped | The run was interrupted before this file. |
| `EXISTS` | skipped | `--if-exists skip` and the name was taken. |
| `LINK` | skipped | A link or junction named directly. |
| `NOT_FOUND` | failed | The named path does not exist. |
| `NO_MATCH` | failed | A pattern matched nothing. |
| `MODIFIED_SINCE_SAVE`, `NO_BACKUP`, `ALREADY_RESTORED` | restore | See [restore](#restore). |

## Interrupting, progress and platforms

**Ctrl+C** (and SIGTERM and SIGHUP on Unix) stop the run cleanly: no new image is
started, the one in progress is finished (its write is never abandoned half way) or left untouched, the report
and the manifest are written, and the exit code is 130. A second Ctrl+C only repeats the message. The files
are either as they were or fully replaced with their backup in place, so `restore --run` always works.

**Progress** is one updating line on stderr when stderr is a terminal (or `--progress always`), erased before any
other line. `--quiet` prints only failed items; `--verbose` every item. There are no colours.

**Windows**: the output is UTF-8 and file names with accents or CJK characters round-trip through `--json`; long
paths work. The executable is a console program separate from the app's. **No network**: the binary links no
HTTP, TLS or socket code (`cargo xtask ci-guards` enforces it for this crate).

**Environment**: `AUTO_CROP_HOME` (same as `--home`). For tests only: `AUTO_CROP_FORCE_NO_AVX2` simulates a CPU
without AVX2, `AUTO_CROP_TEST_DELAY_MS` and `AUTO_CROP_TEST_CANCEL_AFTER` slow down and cancel a run the way
Ctrl+C does.

## What is not there yet

From [PLAN 2.12](plan/02-architecture.md) and ROADMAP M2, not implemented in this tool yet (each is a
roadmap item, not a promise of a date): `--preset receipt|document|photo|flatbed|convert-only` and the
`convert` command, `--enhance`, `--content-tight`, `--strip-location`, `--max-pixels`, `--deterministic`,
`--require-sandbox`, `--dpi`, `--bits`, `--colour`, `--if-exists replace`, `--follow-symlinks`, `backups
reindex`, `eval`, shell completions and man pages, and the batch scheduler shared with the app
(M5.20). Replacing an original with a file of another format is not available. The engine commit protocol
is the app's (verified temp file, backup, atomic replace); a SQLite journal and the Windows
`ReplaceFileW` swap of the full design (M2.28) are separate roadmap items, and the free-space check before a
write is not done yet. Of the numbers in this page, the default job count, the memory cap and the cut-offs
are provisional.

## Design notes

- **Argument parser.** A small hand-written parser driven by one flag table per command (the same tables
  generate `--help` and are checked against this page by a test), instead of `clap`: it adds no
  dependency (`clap_derive` would add a proc-macro stack and `clap` is not otherwise in the shipped graph),
  its behaviour is fully ours to test (a fuzz-style test feeds it 20,000 arbitrary argument lists), and the CLI
  surface is small. Recorded in [ADR 0011](adr/0011-cli-argument-parser.md); revisit if completions and man
  pages (M2.67) are easier from `clap`.
- **Output modes.** [PLAN 2.12](plan/02-architecture.md) describes `--in-place`, `--output` and `--suffix`
  and says there is no `--copy`; this tool also has `--copy` (the app's "Save as copy" folder, `AutoCrop/`),
  at the owner's request.
- **Who writes what.** Replacing an original is always the engine's save path. The copy modes and `render` use a
  small writer in this crate (temp file, verify by decoding, no-clobber placement, all files of a scan or none)
  because nothing that exists is touched; it uses the engine's naming and collision plan.
- **Held singles in copy modes.** A held single image is not written in any mode (B4: "no flag forces
  them"), while the *copies* of a split scan are (the engine's owner-confirmed rule: a copy destroys nothing).
  `render` is the explicit way to write a crop the detector is unsure about.
