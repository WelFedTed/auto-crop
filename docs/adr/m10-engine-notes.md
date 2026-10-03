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
| M10.29 | `Approved(Auto)` only if every item is Good at Strict | Same predicate (`core::scan_triage`). The save path adds the owner's 0.x rule: a split is written only when the user accepted that exact state (`accept_scan`), or when `Settings.autoSaveSplits` (Experimental, off by default) is on and triage is `Approved`. Held means nothing is written, for Copy as well as Replace | "Held for review by default" |
| M10.30 | `lossless_plan` returns `NotApplicable(MultiItem)` | The engine has no lossless path yet (M2.24), so there is nothing to disable: every split item is rendered and re-encoded once | |
| M10.31 | never replace a multi-page TIFF | `NOT_REPLACEABLE` with notice `tiff.multi_page` (or `format.write_unavailable`) for a split; the single-item path of the same build returns `UNSUPPORTED_OUTPUT` for a source with no writer. Copies are PNG (JPEG for HEIC) | The two codes should be unified when the UI has copy for `NOT_REPLACEABLE` |
| M10.28 | `render_item`, tile keys | `Engine::crop_image_bytes`; the cache key is the image id, the kind and the crop's render hash mixed with its id. No interactive and batch pools exist in the engine yet, so "parallel per item" is not implemented; the render is deterministic on 1 and 8 threads (test) | |
| M10.47 | `process --split` flags | Not wired: the CLI has no single-item `process` command yet (only `dev-pipeline`), so there is nothing to extend | |
| M10.63 | `MemoryBudget` weight | Not wired: the engine does not use `MemoryBudget` for single saves either. A split decodes the scan once and renders one item at a time, so the peak is the decoded scan plus one item and one encoded file | |
| M10.32, M10.33 | per-item enhancement | Not implemented: the engine has no enhancer. `Item.enhance_override` is part of the model and of the per-crop render key | |

## Decisions the owner must make

1. **Default profile.** `splitProfile` defaults to Photos (placed orientation kept, no orientation net).
   M10.12 pairs Receipts with "Documents and receipts". Which is the app default?
2. **Copies of a split.** They are held like replacements (the output is all or nothing). Say if a copy
   may be written without acceptance, since it destroys nothing.
3. **Edited derived file on re-save.** The whole new set gets a new base name and the old set stays.
   The alternative is to replace the unchanged ones and suffix only the edited one's successor.
4. **Older builds and v2 states.** A manifest whose edit state is v2 is not readable by a 0.0.1 build.
   Fine before a release; say if downgrade safety matters.
5. **Error codes.** `PLAN_STALE`, `GROUP_COMMIT_FAILED`, `SAVED_SOURCE_IN_USE`, `HELD_FOR_REVIEW`,
   `ITEM_OP`, `NOT_REPLACEABLE` need copy in the UI; the engine already emits them.
