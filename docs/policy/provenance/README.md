# Provenance log

`provenance.jsonl` is the **source of truth**: one JSON object per line, one record per dataset, asset or weight. CSV and Markdown views are generated with `cargo xtask provenance --render` and are never edited by hand. From milestone M1 the authoritative log moves to the `auto-crop-models` repo (M1.75); this copy holds the seed rows.

## Fields

| Field | Meaning |
|---|---|
| `id` | Stable identifier, for example `dataset:smartdoc-2015-ch1` |
| `kind` | `dataset`, `weights`, `code`, `font` or `texture` |
| `name` | Human-readable name |
| `source_url` | Where it comes from |
| `licence_url` | Page or file stating the licence terms (may equal `source_url`) |
| `spdx` | SPDX licence id, or `LicenseRef-NonCommercial`, `LicenseRef-Unlicensed`, `NOASSERTION` |
| `retrieved` | Retrieval date (YYYY-MM-DD) |
| `sha256` | Archive hash once downloaded, otherwise empty |
| `allowed_use` | For example `train+eval`, `dev comparison only`, `none` |
| `attribution` | Required attribution text, if any |
| `init_source` | For weights: what they were initialised from |
| `reviewer` | Who audited it |
| `status` | `cleared`, `pending`, `blocked`, `banned` or `exception` (owner-granted) |
| `notes` | Anything the reviewer must remember |

Seed rows record what the research found on 2026-09-30. They must be re-verified at audit time (see the [model-weights policy](../model-weights.md)). The generated views are [provenance.md](provenance.md) and [provenance.csv](provenance.csv).
