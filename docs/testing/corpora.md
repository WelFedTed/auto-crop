# Public corpora: `fetch-corpus` and the adapters

Roadmap: M1.36 (framework), M1.37 (SmartDoc 2015 Ch.1, CORD), M1.38 (MIDV-500, DIBCO, raw.pixls.us). Decisions: B2 (licences), B18 (offline by default), B21 (test data). Code: `xtask/src/corpus/`. Pins: [corpus.lock.toml](../../corpus.lock.toml). Consumers: the [accuracy harness](eval-harness.md) and, later, M4 training splits.

**Status.** The framework is built and tested against a local HTTP server and synthetic trees. **No real dataset has been downloaded and no real dataset host has been contacted.** Every adapter is **UNVERIFIED against the real dataset's layout**; every pin in the lock is the placeholder `TODO-first-fetch`. See "First fetch" below for how to turn that into real, verified data.

## Commands

```
cargo xtask fetch-corpus --list
cargo xtask fetch-corpus [--sample] [--cache DIR] [--no-ingest] [adapter options] <name>
cargo xtask fetch-corpus --record-hash [--sample] <name>
cargo xtask corpus-ingest <name> --src DIR [--out DIR] [adapter options]
```

Adapter options: `--every N` (keep every Nth frame of a clip, default 10), `--scene-by clip|document`, `--dev-percent N` (default 30), `--contact-sheet N` (default 20; 0 = none). `--sample` picks the small sample file of corpora that have one (SmartDoc: about 21 MB) and is an error for those that do not. `corpus-ingest` runs only the adapter, for data obtained by hand; it applies the same licence gate.

The network is the system `curl`, run as a separate program (xtask has no HTTP or TLS crate; see [CI policy guards](../policy/ci-guards.md)).

## The lock file

`corpus.lock.toml` has one `[[corpus]]` per dataset: `name`, `adapter`, `spdx` (licence of the **data**), `licence_url`, `attribution`, free-text `notes`, and one or more `[[corpus.file]]` with `variant` (`sample` or `full`), `url`, `size`, `sha256` and `extract` (`tar`, `zip` or `none`). Rules, all enforced in `xtask/src/corpus/lock.rs` and tested:

- **A placeholder pin is refused.** A `size` or `sha256` of `TODO-first-fetch` is legal in the file but makes `fetch-corpus` stop before any request, naming the file, the reason and the remedy. The check runs for every file of the variant before the first download starts.
- **Only cleared licences.** `CC0-1.0`, `CC-BY-4.0`, `MIT`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`. `NOASSERTION`, non-commercial and share-alike licences are refused at fetch and ingest (this is why MIDV-500, which is `pending` in the provenance log, cannot be fetched yet).
- **URLs:** `https`; `ftp` (MIDV only publishes there; the pinned hash protects the bytes, and the first record is trust-on-first-use); `http` only for a loopback host.
- **Attribution** is mandatory for every licence except CC0 and is copied into every manifest line.

## Where the data goes

`<cache>/<name>/downloads/<sha16>-<file>` for the verified archives and `<cache>/<name>/<variant>/src/` for the extracted tree, with the manifests written next to the data. The cache is, in order: `--cache DIR`, `AUTOCROP_CORPUS_CACHE`, then `%LOCALAPPDATA%\auto-crop\corpus` (Windows), `~/Library/Caches/auto-crop/corpus` (macOS) or `$XDG_CACHE_HOME/auto-crop/corpus` / `~/.cache/auto-crop/corpus` (Linux).

**Dataset bytes cannot enter the repository:**

1. xtask refuses a cache, an ingest source or an ingest output inside the checkout (`<repo>/target` is allowed for generated output only), with symlinks and `..` resolved.
2. `.gitignore` ignores `/corpus-cache/`, `/corpora/`, `/datasets/` and `/golden/`.
3. The CI guard in `cargo xtask ci-guards` (section 5 of the guards document) fails on tracked archives, parquet, video, scans or RAW files outside fixtures directories, images outside the asset directories, any tracked file over 5 MiB, and any file inside a `corpus-cache`, `corpora`, `datasets` or `golden` directory.

## What verification does

1. The entry is checked (pins, licence, URL, file name) before any request.
2. `curl --fail --proto =<scheme> --proto-redir =https --max-filesize <pinned size>` downloads to `<file>.part`.
3. Size and SHA-256 are compared with the lock. A mismatch **deletes the file and refuses** (nothing is kept, nothing is extracted).
4. A cached file is re-verified on every use; a corrupted one is discarded and fetched again (and verified again).
5. Before extraction the member list is checked for absolute paths, drive letters and `..`; an escaping member refuses the whole archive. Extraction goes into a temporary directory that replaces the old tree only on success, and the result is scanned for symbolic links.

## First fetch (a maintainer, once per corpus)

1. `cargo xtask fetch-corpus --record-hash <name>` downloads into `<cache>/_quarantine`, prints `size` and `sha256`, extracts nothing and deletes the file. This is trust on first use: compare the hash with the publisher's checksum when one exists.
2. Paste both values into `corpus.lock.toml` (and confirm the URL and file name; several are marked UNVERIFIED there), and record the retrieval in the provenance log with the archive hash.
3. `cargo xtask fetch-corpus --sample <name>` (SmartDoc), then look at `contact-sheet.html` (20 quads; open it in a browser; browsers do not render TIFF, so MIDV frames show the outline on an empty frame) and read `ingest-report.json`. A layout the adapter does not understand gives an error and **no manifest**, never an empty or wrong one.
4. Only then run the full variant.

## Outputs and the manifest

`manifest.jsonl` is the [harness manifest](eval-harness.md#manifest-manifestjsonl). Each line also carries `licence`, `attribution` and `source` (the harness ignores unknown fields), and `corpus-info.json` holds the same licence data once. Items are sorted by id, so the same input gives the same bytes. The manifest is checked with the harness's own `manifest::validate` before it is written: an invalid or empty manifest is never written.

- **Splits** are by a hash of the `scene_id` (default 30% `dev`, 70% `test`), so a scene is never in two splits. **SmartDoc: one scene per clip** (`smartdoc15-<background>-<model>`, the M1.37 rule); `--scene-by document` uses the page model across all backgrounds, which is stricter and what M4.13 wants for training splits. **MIDV-500: one scene per document class** by default, `--scene-by clip` for per-clip.
- **Quads** are normalised by the image's own header size. Frames with an EXIF orientation other than 1 are left out (counted), because ground truth in stored pixels would not match the oriented frame. A quad that is not clockwise from the top-left (or has crossing edges) is counted as `invalid-quad` and left out, never reordered, except for CORD, whose outline has no promised corner order and is sorted.
- **Tags:** `dataset`, `format`, `aspect` (`receipt` when the long side is at least 2.5 times the short one, else `document`) and per adapter `background`, `doctype`, `condition`, `cord_split`.

| Adapter | Dataset (licence) | Expects (UNVERIFIED against the real data) | Writes |
|---|---|---|---|
| `smartdoc2015-ch1` | SmartDoc 2015 Ch.1 (CC BY 4.0; cite the paper and email the organisers) | `metadata.csv` or `.csv.gz` with `bg_name, model_name, frame_index, tl_x..bl_y`; frames `<bg>/<model>/<digits>.jpg` | `manifest.jsonl`, `contact-sheet.html` |
| `cord` | CORD (CC BY 4.0) | `<split>/image/*.png`, `<split>/json/*.json` with `valid_line` words and a `roi` outline (the research notes say the ROI field needs verification) | `manifest.jsonl` (receipts with an outline), `transcripts.jsonl` for the CER checks |
| `midv-500` | MIDV-500 (**licence pending**, refused) | `<doc>/images/<cond>/<clip>/<frame>.tif`, `<doc>/ground_truth/<cond>/<clip>/<frame>.json` with `quad` | `manifest.jsonl` |
| `dibco` | DIBCO / H-DIBCO via Doxa BinBench (CC0) | image and ground-truth pairs, `*_GT` or a `gt/` directory, per year; fetch-only, no quads | `binarisation.jsonl` |
| `rawpixls-cc0` | raw.pixls.us, CC0 only | `index.jsonl` (JSON lines or one array) with `path`, `licence`, `sha256` per sample | `cc0-samples.jsonl` |

The raw.pixls.us filter fails closed: an entry survives only with a licence that is exactly CC0 (`CC0`, `CC0-1.0`, `CC0 1.0`, any case), a path that stays inside the sample directory and a pinned SHA-256. A missing, empty, combined (`CC0-1.0 OR CC-BY-SA-4.0`) or other licence (`CC-BY-SA`, `Public Domain`, non-commercial) is excluded and counted by reason in `ingest-report.json`.

## Tests (no real network, no real data)

- `xtask/tests/fetch_corpus.rs`: a loopback HTTP server serving a good archive, tampered bytes, a longer and a shorter file, a truncated transfer, an error status, a redirect to http, a hostile archive with a `../` member and a zip. Checks that bad downloads are refused and leave nothing, that the cache serves a second run without a request, that placeholder pins, uncleared licences and non-loopback http are refused with **zero requests**, that `--record-hash` keeps nothing, that the real `corpus.lock.toml` refuses every dataset, and that a cache inside the repository is refused.
- `xtask/tests/corpus_adapters.rs`: synthetic trees mimicking each layout. Checks manifests validate with the harness, clip-level scenes never straddle splits, attribution and licence are in every line, bad rows are counted, a wrong layout writes nothing, MIDV is refused while its licence is `NOASSERTION`, and the mixed-licence raw.pixls.us index passes only its CC0 entries.
- `xtask/src/corpus/**` unit tests and the guard cases in `xtask/src/ci_guards/corpus_guard.rs`.

Mutation-checked once by hand: removing the SHA-256 comparison fails two fetch tests; accepting every licence fails both raw.pixls.us tests.

## Not done

- Real data: nothing was fetched, so no adapter has seen a real file, and the "20 quads on a contact sheet" acceptance of M1.37 and the MIDV licence check of M1.38 still need a real first fetch and a human look.
- Resumable downloads (M7.52 asks for them for the 13 GB SmartDoc-QA), per-sample fetch and verification for raw.pixls.us (only its index is pinned), parquet input for CORD, SmartDoc-QA and Doxa metrics (M7.51, M7.52).
