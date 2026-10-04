# One scan to N files: threat model and data-loss checklist

ROADMAP M10.66. Written with the group commit (M10.23, M10.24), not after it: every hazard below is
mapped to the test that fails if the hazard comes back. A hazard with no test is listed as open and
blocks the release (M10.66). The protocol itself is in `crates/engine/src/group.rs` and PLAN 2.7.

## The invariant

At every instant, for one scan, the folder holds **the scan alone, or the complete verified set**
(the scan may remain beside the set when it could not be removed). Never a partial set. The scan's
bytes are always somewhere: in place, or in a verified backup. Nothing is deleted that no journal
and no hash names.

## The protocol in one screen

backup (verified) -> journal `Writing` -> N temps, each fsynced and re-read -> keep what a re-save
replaces -> journal `Committing` (hashes, durable) -> re-stat the scan -> N no-clobber moves ->
**commit point** -> fsync folder, retire surplus old outputs -> re-check the scan, unlink it LAST ->
manifest `Saved` -> delete journal. Recovery at start: `Writing` rolls back; `Committing` rolls
forward only if all N outputs verify against the journalled hashes (a placed file, or an intact temp
whose target is free), else rolls back to the scan and its backup.

## Hazards

| # | Hazard | Mitigation | Test |
|---|---|---|---|
| H1 | A failure between two renames leaves 2 of 4 files | everything before the commit point is undone, removing only files whose bytes match the journal | `a_failure_at_every_step_leaves_the_old_state_or_the_complete_set`, `a_name_taken_between_the_plan_and_the_renames_stops_the_whole_group` |
| H2 | A crash leaves a partial set | journal plus recovery that rolls forward only when all N verify, else back | `a_crash_at_every_step_is_recovered_to_the_old_state_or_the_complete_set`, `a_crash_at_any_step_of_an_engine_save_is_recovered_at_the_next_start` |
| H3 | The process dies for real (no unwinding, no destructors) | same, driven by a child process that exits at each step | `killing_the_process_at_every_step_never_leaves_a_partial_set` |
| H4 | Recovery itself is interrupted | idempotent roll forward and roll back | `a_crash_inside_recovery_is_itself_recovered` |
| H5 | An output name collides with an existing file, a queued source, another group, or a name that differs only by case or accents | one collision key, whole-group base rename, phase-2 recheck, no-clobber move | `adversarial_groups_never_collide`, `names_of_other_queued_sources_and_groups_are_avoided`, `phase_two_notices_a_name_that_appeared_after_the_plan`, `names_of_other_open_images_are_never_planned`, `a_copy_leaves_the_scan_and_never_overwrites` |
| H6 | A foreign file appears at an output name during the commit or while crashed | the move refuses to overwrite; the foreign file is never touched or deleted | `no_output_ever_overwrites_an_existing_file_even_when_crashed_and_recovered`, `a_name_taken_between_the_plan_and_the_renames_stops_the_whole_group` |
| H7 | The scan is unlinked before the set exists | the unlink is the last step and is preceded by a re-check of the scan's hash | `the_scan_is_removed_after_the_last_output_is_in_place_never_before` |
| H8 | The scan was changed after it was read | re-stat before the renames (abort, backup kept), re-hash before the unlink (scan left alone, note `SOURCE_CHANGED`) | `a_scan_changed_before_the_renames_is_left_alone_and_nothing_is_written`, `a_scan_changed_after_the_renames_is_not_unlinked`, `a_scan_changed_after_opening_is_left_alone` |
| H9 | The scan cannot be removed (locked by another program) | the complete set stays, the scan stays, note `SAVED_SOURCE_IN_USE`; on Windows the test holds a real handle | `a_scan_that_cannot_be_removed_keeps_the_set_and_says_so` |
| H10 | A temp is corrupt or truncated | each temp is re-read, hashed and decoded before the journal says `Committing` | `a_corrupt_temp_fails_verification_and_nothing_is_placed` |
| H11 | Disk full or an encoder error on item k | all earlier temps are removed, the backup is discarded, the scan is untouched | `a_failing_encoder_for_one_item_writes_none`, the `DiskFull` runs of `a_failure_at_every_step_...` |
| H12 | A re-save replaces a derived file the user edited | only the previous output whose hash still matches is replaced; otherwise `PlanStale` at the group level, a new base name with notice `derived.user_edited` at the engine level | `a_re_save_never_replaces_a_derived_file_the_user_edited` (group and engine) |
| H13 | A re-save loses the old derived bytes | replaced and retired files are kept in `<backup>/superseded/` before anything changes; a rolled-back re-save restores them and leaves nothing behind | `a_re_save_keeps_the_replaced_and_retired_bytes_in_the_store`, the re-save scenarios of the fail and crash runs |
| H14 | Rollback or recovery puts the scan back when it should not (a re-save), or does not when it should | only a group that was going to remove the scan restores it, from the verified backup | `a_re_save_...` runs (`a re-save put the scan back`), `a_rollback_puts_the_scan_back_from_the_backup_when_it_is_gone` |
| H15 | Restore loses the scan or a derived file | the scan is restored first from a verified temp and a no-clobber move; Remove moves into the store and records each move; a changed derived file is kept | `restore_keep_returns_the_scan_and_leaves_the_derived_files`, `restore_remove_moves_unchanged_derived_files_into_the_store_and_keeps_edited_ones`, `restore_after_a_restart_works_and_is_repeatable` |
| H16 | Purge or cleanup deletes outside the store or a file nobody journalled | store ids are validated (28 hex digits), `remove_unused` touches only `BackedUp` or manifest-less entries, recovery deletes only files whose hash is journalled | `ids_that_could_escape_the_store_are_refused`, `recovery_leaves_unreadable_journals_and_unrelated_files_alone`, `purge_removes_only_expired_unpinned_saved_backups` |
| H17 | A multi-page TIFF (or any source this build cannot write) is replaced | `NOT_REPLACEABLE` (split and single-item path), notice `tiff.multi_page` or `format.write_unavailable`, file untouched; copies are PNG on opt-in | `a_multi_page_tiff_is_never_replaced_and_copies_are_png` |
| H18 | A held or doubtful scan replaces the original | all-or-nothing hold rule for Replace; acceptance covers one exact state. A copy is not held because it destroys nothing | `by_default_a_split_scan_is_held_and_nothing_is_written`, `auto_save_of_splits_is_experimental_and_needs_every_crop_good`, `an_acceptance_covers_exactly_the_state_that_was_accepted`, `a_copy_of_an_unaccepted_split_is_written_and_destroys_nothing` |
| H19 | An old manifest or edit state cannot be read | additive manifest fields, `EditState` v1 to v2 migration, newer schemas refused not truncated | `v1_documents_migrate_losslessly_to_v2`, `a_newer_schema_is_refused_and_the_input_is_never_touched` |
| H20 | An item panic takes the app down or corrupts a save | every render and encode runs under `catch_unwind` and fails that scan only | `run_isolated` in `scan.rs` (no dedicated injection test yet, see Open) |

## Open (not covered, or covered only in part)

* **Kills per OS.** The suite kills the child at every protocol step (about 26 per scenario). The
  gate asks for at least 500 kills per OS at random instants (M10.64): a randomised loop belongs in
  the nightly job and has not been written.
* **Power loss.** There is no `FaultFs` model of un-fsynced data and directory entries. The protocol
  fsyncs the temps, the backup, the journal and (Unix) the folder, but the claim is untested against
  a loss model.
* **Hard-link fallback.** Where hard links are unavailable (FAT, some network shares) the no-clobber
  move falls back to check-then-rename, which has a small race window. It is exercised by no test on
  such a file system. The journal's hash check bounds the damage; a stricter fallback needs the Win32
  `MoveFileExW` without replace, which `std` does not expose and `engine` may not call (unsafe is
  forbidden there).
* **Cross-volume.** The backup store may be on another volume (a verified copy). Temps and outputs
  always share the target folder, so moves never cross a volume. No test sets up two volumes.
* **Injected panics.** `run_isolated` wraps each render and encode; a panic test needs a hook in the
  render path.
* **Sharing violations.** The retry ladder (10 to 640 ms) is shared with the single-item swap; only
  the locked-scan case is injected.
