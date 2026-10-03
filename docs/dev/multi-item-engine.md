# Multi-item scans: the engine surface for the UI

For the GUI session (ROADMAP M10.34 and the M10.35-M10.45 UI items). Everything below is in
`crates/engine` and is additive: no existing method, type or JSON field changed its meaning. The
Tauri commands and the Svelte side are not written; this page says what each command would call
and what comes back. Names follow `docs/plan/02-architecture.md` 2.5 (ids only, no pixels over IPC,
no paths from the webview).

## Vocabulary

| Engine | UI | Notes |
|---|---|---|
| image (`ItemView`, `id: u32`) | grid tile, the opened file | what `list_items` returns today |
| crop (`CropView`, `id: u32`) | item chip, overlay quad, output file | `EditState.items` in `core`; ids are stable for the life of the state and never reused |

`ItemView.edit` and `autoEdit` are still "the first included crop", so a UI that knows nothing about
splitting keeps working on the first crop of a split scan. A scan with N included crops is saved as N
files; with one included crop it is saved exactly as before.

## New fields on existing JSON (all `serde(default)`, camelCase)

`ItemView`:

| Field | Type | Meaning |
|---|---|---|
| `crops` | `CropView[]` | every crop in output order, included or not |
| `split` | `SplitView \| null` | policy, profile, order mode, triage, acceptance |
| `historyPosition` | number | stable undo position, for `revertCrop` to a step |

`CropView`: `id`, `order` (1-based output rank, the `{n}` of the file name; 0 while excluded),
`include`, `edit` (`Edit \| null`), `autoEdit` (the detector's proposal, null for a crop drawn by
hand), `mirror`, `origin` (`auto` / `manual` / `autoThenEdited`), `confidence`, `band`
(`good` / `check` / `failed` at the Strict cutoff; a crop the user placed or edited counts as
reviewed, `good`), `edited`, `outputName` (the planned file name), `renderKey` (use it in the crop
image URL as the cache buster instead of `gen`: editing crop 2 changes only crop 2's key).

`SplitView`: `policy` (`auto` / `always` / `never`), `profile` (`photos` / `receipts`),
`orderMode` (`reading` / `manual`), `triage` (`{kind: "approved"}` /
`{kind: "heldForReview", itemsNeedCheck: n}` / `{kind: "noItems"}`), `accepted`, `isSplit` (the scan
would be saved as several files, or was), `included`.

`SavedInfo.outputs: string[]` (the N file names of a split save; empty for one-to-one saves).
`SaveOutcome.notes: ErrorCode[]` and `SaveOutcome.notices: string[]`.
`BackupFile.kind` (`"OneToOne"` / `"OneToN"`) and `BackupFile.derived: {name, bytes, state}[]` with
`state` one of `unchanged` / `changed` / `missing` / `removed`.
`RestoreOutcome.derived` (the same list after the restore).

`Settings` gains `splitPolicy` (default `auto`), `splitProfile` (default `photos`) and
`autoSaveSplits` (default `false`). The UI must send back the whole object it received from
`get_settings` (spread it) or these reset to their defaults.

## New error codes (`ErrorCode` in `ui/src/lib/types.ts` and a message each)

`PLAN_STALE`, `GROUP_COMMIT_FAILED`, `SAVED_SOURCE_IN_USE`, `HELD_FOR_REVIEW`, `ITEM_OP`,
`NOT_REPLACEABLE` (message keys `err.plan_stale` and so on, B20). `HELD_FOR_REVIEW` is not a failure:
nothing was written and the scan waits for the user.

## Operations (each is ONE undo step; every one returns the new `ItemView`)

| Tauri command (suggested) | Engine call | Notes |
|---|---|---|
| `set_crop_edit(id, crop, edit, phase, label, gesture)` | `set_crop_edit` | `phase: "live"` records nothing; the gesture id is combined with the crop id so a drag is one step per crop |
| `add_crop(id, quad?, at?)` | `add_crop` | `quad` if the UI drew a box; else the item at `at` (the detector's `detect_at`, snapped to edges); else a box around `at`; else the frame inset 20% |
| `remove_crop(id, crop)` / `restore_crop` | same | sets `include`; nothing is deleted |
| `merge_crops(id, crops[])` | `merge_crops` | minimum-area rectangle of the union; refuses an excluded crop |
| `cut_crop(id, crop, {axis, t0, t1})` | `cut_crop` | `axis` is `vertical` or `horizontal`; `t0`, `t1` are the fractions along the two edges the cut ends on (0.5 and 0.5 is "split in halves"); each piece must cover at least 2% of the scan |
| `move_crop(id, crop, toIndex)` / `use_reading_order(id)` | same | a move switches to manual order, which re-detection keeps |
| `turn_crop`, `set_crop_angle`, `flip_crop` | same | per-crop turn, fine angle, mirror |
| `revert_crop(id, crop, {kind: "auto"} \| {kind: "step", position})` | `revert_crop` | only that crop changes |
| `redetect(id, {policy?, profile?})` | `redetect` | `policy: "never"` is "Treat as one item"; keeps crops the user placed, edited or removed |
| `accept_scan(id)` / `unaccept_scan(id)` | same | see the hold rule below |
| `undo(id)`, `redo(id)`, `reset_to_auto(id)` | unchanged | labels name the item: `Straighten (item 2)` |

A refused operation returns `Err(ITEM_OP)` (or `DEGENERATE`) and changes nothing.

## Crop images

`Engine::crop_image_bytes(id, crop, CropImage::Thumb | Result)` returns encoded JPEG bytes for the
`acimg` scheme. Suggested URL: `acimg://localhost/<token>/<id>/crop/<crop>/<thumb|result>?k=<renderKey>`.
The engine caches by (image, kind, the crop's own render hash), so the answer for a crop whose
`renderKey` did not change is the same cached entry. `ImageKind::Thumb` of a split scan is the whole
scan; `Src` is unchanged. A panic while rendering fails that call only.

## The hold rule (0.x preview, M10.29)

`save_items` of an image that would become two or more files writes NOTHING unless one of these holds:

1. the user accepted the scan: `accept_scan(id)` records the exact state (its render hash). Any later
   edit withdraws it (`split.accepted` goes false); undoing back to the accepted state restores it;
2. `Settings.autoSaveSplits` is on (Experimental) AND `split.triage` is `approved` (every included
   crop is Good at the Strict cutoff 0.95 and raised no hold code).

Otherwise the outcome is `{ok: false, error: "HELD_FOR_REVIEW", notices: ["split.held"]}` and nothing
changes on disk, for `replace` and `copy` alike. The UI's "Save" on a reviewed scan is
`accept_scan` followed by `save_items`; "Save all" on a batch sends only what the user may auto-save.

## Saving

`save_items(ids, target, runName, notify)` is unchanged. For a split it returns one `SaveOutcome` for
the scan with `saved.outputs` (names in output order). Behaviour:

* **Replace**: the scan is backed up (verified), N outputs named `{name}_{n}` (zero-padded to
  `max(2, digits(N))`) are written as one group, the scan is removed last. A taken name moves the whole
  group to `name (2)_01...`; names of other open images and of saves in flight are never planned.
* **Copy**: outputs go to `<folder>/AutoCrop/`, the scan stays, no backup. A TIFF or other source this
  build cannot write is saved as PNG copies.
* A multi-page or non-writable source is never replaced: `NOT_REPLACEABLE` with
  `notices: ["tiff.multi_page"]` (or `format.write_unavailable`); the file is untouched.
* `notes` after a successful save: `SAVED_SOURCE_IN_USE` (the set is complete, the scan could not be
  removed because another program has it open) or `SOURCE_CHANGED` (the scan was changed meanwhile and
  is left alone).
* `notices: ["derived.user_edited"]`: a file from the previous save was edited by the user, so it was
  not replaced and the new set has a new base name.
* A **re-save** after more edits renders again from the pristine backup. Unchanged files of the
  previous set are replaced, surplus ones are moved into the backup store (`superseded/`), nothing is
  deleted. N may go up, down, or back to one (then the plain name returns).
* A crash at any point is repaired at the next `Engine::new`: the folder holds the scan alone or the
  complete set, never a partial one (`docs/design/one-to-n-safety.md`).

## Restore and the Backups panel

`list_backups` rows of a split have `kind: "OneToN"` and `derived`. New calls:

* `restore_file_derived(fileId, mode, derived: "keep" | "remove", notify)` and
  `restore_run_derived(runId, derived, notify)` ("Restore all from this run").
* The old `restore_file` and `restore_run` keep the derived files (Keep).

The scan returns byte for byte. If something else now occupies its path it comes back as
`name (restored).ext` and nothing is replaced. **Remove** moves the derived files that are still
exactly as saved into `<backup>/derived-by-restore/` (reversible, never a delete); a file that changed
since it was saved, or is gone, is kept and listed as `changed` or `missing`. The dialog should
preselect Keep and list the changed files.

`Engine::processed_by(path)` is the "Already processed" guard: it looks the file's hash up in every
backup's output list, so any of the N outputs of a split is recognised (`outputIndex`,
`outputCount`).

## Detector seam

`Engine::set_item_detector(Arc<dyn ItemDetector>)`. Until `imgproc::items` is installed the app uses
`NoSplit` (every scan takes the single-item route) and `redetect` with a split policy finds nothing.
`ItemDetector::detect` returns the accepted items with their per-item `Confidence` (hold codes in
`reasons`) and `detect_at` finds the item at a tapped point.

## Not in the engine (for the UI session)

The session-level undo of a preset applied to 20 scans (`SessionHistory` in `core` is ready; the engine
returns `historyPosition` before and after each `redetect`), the overlay, chips, grid badges and
dialogs, the Tauri command wrappers and the TypeScript types.
