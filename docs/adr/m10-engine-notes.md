# M10 engine: where the code differs from the roadmap text

Not an ADR (no spike, no GO/NO-GO): a record of the places where the multi-item engine (M10.17 to
M10.31) reads an item differently from the text, or fills a gap the early engine slice left, so the
owner can veto any of them. The IPC surface is in `docs/dev/multi-item-engine.md`, the hazards in
`docs/design/one-to-n-safety.md`.

| Item | Roadmap or plan text | What the code does | Why |
|---|---|---|---|
| M10.17 | `Item.order`, `EditState.items` for N > 1 (`order`, `auto`) | No per-item order field: the order of `EditState.items` IS the output order, and `{n}` numbers the included items in that order. `SplitState` has `policy`, `profile`, `order_mode`, `next_id`. The auto baseline is the engine's existing `auto` state, matched by item id | A stored rank beside a vector order can disagree; there is one source of truth |
| M10.17 | pure migration | `EDIT_STATE_VERSION` is 2. `v1_to_v2` adds the `split` block (policy `never`, which is what a pre-M10 state means) and a `nextId` above every id in use. v1 fixtures migrate losslessly (test) and get new snapshots | The additive route needs a version bump because `EditState` rejects unknown fields on purpose. A v2 state in a backup manifest cannot be read by an older build (it is refused as too new, never half-read) |
| M10.17 | `Never` = M2.16 | `SplitPolicy::default()` is `Never` (what serde fills in and what `EditState::default()` holds); the app Setting `splitPolicy` defaults to `Auto` and is written into every new analysis | An old document must not start splitting; a new analysis follows the user's setting |
| M10.40 | policy change is one undo entry | `History::for_edit` treats a change of policy, profile or order mode as a step of its own even when no pixel changes | "Treat as one item" on a scan the detector already sees as one item is still an undoable choice |
| M10.19 | `revert_item(Auto \| Step(n))`, one undo for a preset on 20 scans | `Engine::revert_crop(RevertTo::Auto \| Step{position})`; `redetect_many` records one `SessionCmd`, `session_undo` and `session_redo` restore every image's history position | `core::SessionHistory` already existed; the engine had no session log |
| M10.22 | NFC plus case-fold key | The key folds case, combining marks, Latin-1 and Latin Extended-A accents, `ß`, `æ`, `œ`, trailing dots and spaces. It is wider than NFC plus case fold for those letters and does not normalise other scripts | No new dependency (M10.69); the no-clobber move is the real guard on a file system that merges more |
| M10.22 | two-phase plan, rename on collision | When any of the N names is taken the WHOLE group moves to `name (2)_01...`, never a mixed numbering. Names of other open images and of saves in flight are reserved | One scan, one base name |
| M10.23, M10.24 | journal in SQLite (PLAN 2.7) | There is no SQLite in the engine yet: the journal is one small file per group in `<store>/.groups/`, replaced atomically and fsynced. The backup manifest still carries the Saved state. `Journal.owner` (pid and start time) keeps a second process or engine from recovering a commit that is still running | The store must survive loss of anything else; the journal moves into the library DB with M2.33 |
| M10.23 | no-clobber rename | A hard link (fails if the name exists, atomically) then the temp name is dropped. Where links are unavailable the move checks the name and renames, with a small race window (listed as open in the safety checklist). `std` has no portable no-replace rename and `engine` forbids `unsafe` | |
| M10.23 | source unlinked last | After the N moves, the scan's hash is checked once more; a changed scan is left alone (note `SOURCE_CHANGED`), a locked one stays (note `SAVED_SOURCE_IN_USE`), the complete set is kept in both cases | The set is already the user's result; nothing is rolled back after the commit point |
| M10.25 | additive manifest migration | `Manifest.kind` (`OneToOne` default, `OneToN`), `OutputRec.item_id` and `index` (skipped when absent), `derived_moved`. Schema stays 1. A one-to-one manifest written now is byte-compatible with older readers apart from the edit state version | |
| M10.26 | `restore(backup, derived: Keep \| Remove)` | `restore_file_derived` and `restore_run_derived`; the old `restore_file` and `restore_run` mean Keep. Remove moves only unchanged files into `<backup>/derived-by-restore/` and records each move; an occupied scan path gives `name (restored).ext` | |
| M10.27 | re-save from the pristine backup | Replaced and retired outputs are kept in `<backup>/superseded/` first. If ANY previous output was edited by the user, no previous output is replaced or retired: the new set gets a fresh base name (notice `derived.user_edited`) and the old files stay. In Copy mode there is no store, so a re-save overwrites its own unchanged copies and leaves surplus old copies in place | A user's edit is never taken away; duplicates beat loss |
| M10.29 | `Approved(Auto)` only if every item is Good at Strict | Same predicate (`core::scan_triage`). The save path adds the owner's 0.x rule: a split REPLACES the scan only when the user accepted that exact state (`accept_scan`), or when `Settings.autoSaveSplits` (Experimental, off by default) is on and triage is `Approved`. Held means nothing is written. A COPY of a split is not held (owner confirmation 2026-10-04): it removes and overwrites nothing, so it needs no acceptance and does not accept the split either | "Held for review by default" |
| M10.30 | `lossless_plan` returns `NotApplicable(MultiItem)` | The engine has no lossless path yet (M2.24), so there is nothing to disable: every split item is rendered and re-encoded once | |
| M10.31 | never replace a multi-page TIFF | `NOT_REPLACEABLE` with notice `tiff.multi_page` (more than one page) or `format.write_unavailable` (no writer for the format), on the split path AND on the single-item path (unified 2026-10-04; the single-item path used to answer `UNSUPPORTED_OUTPUT`). `UNSUPPORTED_OUTPUT` stays for encoder and file-name-template failures, so a stored value of that name still parses. Copies are PNG (JPEG for HEIC) | One code, the reason in the notice |
| M10.28 | `render_item`, tile keys | `Engine::crop_image_bytes`; the cache key is the image id, the kind and the crop's render hash mixed with its id. No interactive and batch pools exist in the engine yet, so "parallel per item" is not implemented; the render is deterministic on 1 and 8 threads (test) | |
| M10.13, M10.14 | per-item hand-off | `ClassicalItemDetector` adapts `imgproc::items::detect_items`: only `Outcome::Many` is a split; `One`, `NoItems` and `SingleItemRoute` run the existing single-item detector, so a single document behaves exactly as before (the 12 sample images and the whole `flow` suite are unchanged) | The one-item scan via `Always` equals `Never` by construction |
| M10.47 | `process --split` flags | Not wired: the CLI has no single-item `process` command yet (only `dev-pipeline`), so there is nothing to extend | |
| M10.63 | `MemoryBudget` weight | Not wired: the engine does not use `MemoryBudget` for single saves either. A split decodes the scan once and renders one item at a time, so the peak is the decoded scan plus one item and one encoded file | |
| M10.32, M10.33 | per-item enhancement | Not implemented: the engine has no enhancer. `Item.enhance_override` is part of the model and of the per-crop render key | |

## Owner confirmations 2026-10-04

The owner answered the five open questions with "go with the recommended defaults". They are decisions now.

1. **Default profile: Auto with Photos.** `splitPolicy` defaults to `Auto`, `splitProfile` to `Photos`
   (placed orientation kept, no orientation net). Receipts ("Documents and receipts", M10.12) is a
   selectable profile, not the default. Test: `splitting_is_found_but_not_saved_unseen_by_default`.
2. **Copies of a split need no acceptance.** `save_items(.., Copy)` of a split scan is written even when
   the scan is held for review, because a copy destroys nothing: the scan stays byte-identical (content
   and modification time), no backup is made, and the outputs go to free names in `AutoCrop/` (a taken
   name moves the whole set to another base; nothing is overwritten). Replace-in-place stays held until
   the split is accepted or `autoSaveSplits` (Experimental, off) approves it; a copy does not count as
   acceptance. Tests: `a_copy_of_an_unaccepted_split_is_written_and_destroys_nothing`,
   `a_copy_of_a_split_that_needs_review_is_written_too`, `by_default_a_split_scan_is_held_and_nothing_is_written`.
3. **Edited derived file on re-save: a new base name, the old set stays.** If the user edited any earlier
   output, the whole new set gets a fresh base name (notice `derived.user_edited`) and nothing old is
   replaced or retired. This is the current behaviour, kept. Test: `a_re_save_never_replaces_a_derived_file_the_user_edited`.
4. **Downgrade safety is not needed before the first release.** An older build refuses a v2 edit state
   as too new (it never half-reads it). No migration back and no dual-write. Revisit only if a release
   ships with v1 readers in the field (the first release is the owner's call).
5. **One error code for "this source cannot be replaced": `NOT_REPLACEABLE`.** The reason is the notice
   (`tiff.multi_page`, `format.write_unavailable`). `UNSUPPORTED_OUTPUT` keeps its meaning for encoder
   and template failures and is not an alias, so nothing persisted needs a serde alias (the engine stores
   no error code except the non-fatal `notes` of a saved group). `ui/src/lib/types.ts` is untouched:
   its `ErrorCode` lists neither code, and `errorMessage` falls back to the generic message; adding
   `NOT_REPLACEABLE` (and `HELD_FOR_REVIEW`, `PLAN_STALE`, `GROUP_COMMIT_FAILED`, `SAVED_SOURCE_IN_USE`,
   `ITEM_OP`) with copy belongs to the UI session. Test: `check_not_replaceable` in `tests/formats.rs`
   (single item) and `a_multi_page_tiff_is_never_replaced_and_copies_are_png` (split).
