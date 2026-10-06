# Data-loss threat model of the overwrite path

ROADMAP M2.64 (first version, written with the single-file save of M2.83). PLAN 2.7 is the protocol;
this page maps every way an in-place save, a conversion or a restore could lose or corrupt an original
to the mitigation and to the test that fails if the mitigation goes away. A row with no test is listed
under **Open** and is not claimed. The 1-to-N group commit has its own table in
[design/one-to-n-safety.md](design/one-to-n-safety.md); the single-file save and the conversion run
on the same journal and the same recovery (`crates/engine/src/group.rs`).

## The invariant

At every instant the path holds **the original bytes or a fully verified output**, and the original's
bytes are **somewhere** (in place, in the verified backup, or in `superseded/`) from the moment anything
could change. Nothing is deleted that no journal and no hash names. Held, failed and never-replaceable
(D3) files are not touched at all.

## The protocol in one screen

preflight (space, file state) -> journal `Writing` -> verified backup (reflink, hardlink or re-hashed copy,
fsynced) -> fsynced temp beside the target -> re-read hash + re-decode (size, format, Orientation 1, ICC,
pixels or luma fingerprint) -> keep what a re-save replaces -> journal `Committing` (durable) -> re-stat
**and re-hash** the target -> atomic swap (`ReplaceFileW` / `rename`, retried) -> **commit point** ->
manifest `Saved`, journal removed. Recovery at start: a `Writing` journal rolls back; a `Committing` journal
rolls forward only if the verified temp (or the placed output) matches the journalled hash and the target
is still the file that was read (or missing: an interrupted swap), else it rolls back and restores the old
bytes from the backup. A target that holds somebody else's bytes is never touched.

## Threats

| # | Failure mode | Mitigation | Test |
|---|---|---|---|
| T1 | The process dies at any step (crash, `kill -9`, `TerminateProcess`, Ctrl+C that escalates) | journal + recovery (back before `Committing`, forward from then on), idempotent | `single_faults::a_crash_at_every_step_is_recovered_to_the_old_bytes_or_the_verified_output`, `::killing_the_process_at_every_step_never_loses_the_original_or_leaves_a_partial_file`, `single_save::a_crash_during_a_save_is_recovered_when_the_next_engine_starts`, `restore_matrix::a_crash_at_every_step_of_a_conversion_is_recovered` |
| T2 | Death at a random instant, inside an fs call included | the same, driven by a killed child process at random times | `kill_loop::five_hundred_random_kills_...` (every run), `::a_thousand_random_kills_...` (nightly, `--ignored`) |
| T3 | Recovery itself is interrupted | roll forward and roll back are idempotent | `single_faults::a_crash_inside_recovery_is_itself_recovered` |
| T4 | The swap half-completes (`ReplaceFileW` errors 1175 to 1177: target gone, verified temp present) | the target is re-stat'ed: missing means the temp is renamed in, present means retry; recovery does the same from the journal; if the temp is gone too, the old bytes come back from the backup | `commit::a_replace_that_stopped_part_way_with_the_target_gone_is_finished`, `::..._with_the_target_present_is_retried`, `single_faults::an_interrupted_swap_with_the_target_missing_is_finished_from_the_verified_temp`, `::a_missing_target_and_a_missing_temp_are_restored_from_the_backup` |
| T5 | An antivirus, indexer or thumbnailer holds the file (sharing violation) | retry ladder 10 to 640 ms, then a retryable `FileInUse`; nothing is changed | `commit::a_300_ms_lock_succeeds_and_a_long_one_gives_file_in_use` (Windows, real handle) |
| T6 | Disk full, at the start or in the middle | 2x headroom and a 500 MB floor checked before anything is written (`DiskFull`, nothing to undo); an ENOSPC at any step rolls everything back | `space::a_save_needs_twice_its_bytes_and_a_floor`, `single_save::a_full_disk_refuses_before_anything_is_written`, `single_faults::a_failure_at_every_step_leaves_the_old_bytes_or_the_verified_output` |
| T7 | The file is edited by another program between the read and the swap | re-stat (size, mtime) at `Committing`, **re-hash right before the swap**; a changed target is never replaced, the backup of what was read is kept | `single_faults::an_edit_between_the_read_and_the_swap_is_never_overwritten` (both detectors), `single_save::a_source_edited_after_opening_is_never_replaced_and_its_backup_is_not_made` |
| T8 | The file is edited while the process is down | recovery leaves a target that holds neither the old nor the new bytes alone; the backup stays | `single_faults::a_file_edited_while_the_process_was_down_is_left_alone_by_recovery` |
| T9 | The output is corrupt or truncated (disk fault, encoder bug) | the temp is re-read and its blake3 compared (truncation, any bit flip), then decoded and checked | `commit::a_single_flipped_bit_is_always_caught_by_the_hash`, `single_faults::a_flipped_bit_in_the_temp_is_caught_wherever_it_is`, `::a_corrupt_temp_fails_verification_and_the_file_is_untouched`, `commit::the_content_check_accepts_the_real_encode_and_refuses_a_different_picture` |
| T10 | The picture is turned twice, or not at all (EXIF Orientation) | the pixels are turned once (decode or lossless transform) and Orientation is written as 1; verification refuses an output that still asks for a turn | `lossless::all_eight_orientations_come_out_upright_with_the_tag_reset_once`, `single_save::eight_orientations_by_three_paths_all_come_out_upright`, `::the_orientation_is_applied_once_on_both_paths_and_not_again_on_reprocessing` |
| T11 | The lossless path claims more than it did | exact only: axis-aligned crop snapped outward (growth at most `max(16 px, 0.5%)`), turns with `Policy::Perfect`; anything else re-encodes | `lossless::growth_beyond_the_limit_re_encodes_instead`, `::an_edge_off_the_mcu_grid_that_a_turn_would_trim_is_refused`, `single_save::a_mcu_aligned_crop_takes_the_lossless_path_and_keeps_the_quantisation_tables` |
| T12 | Metadata leaks or is lost (GPS after `--strip-location`, ICC, thumbnail of the uncropped scan) | in-place EXIF patch: thumbnail zeroed, GPS IFD zeroed byte by byte, XMP and IPTC dropped; ICC byte-exact | `jpeg_meta::tests::strip_location_leaves_no_gps_byte_and_no_gps_entry`, `::strip_location_removes_exif_gps_xmp_iptc_and_comments_completely`, `single_save::exif_is_carried_and_strip_location_leaves_no_gps_byte`, `output::the_icc_profile_comes_back_byte_exact_in_both_formats` |
| T13 | A cloud placeholder is downloaded as a side effect, or a sync client reverts the swap | placeholders are never read (`CloudNotLocal`, unless `hydrate_cloud_files`); a save into a sync root carries the notice `sync.root` (the front end recommends a copy) | `single_save::a_cloud_placeholder_is_skipped_and_never_read` (Windows), `fsstate::sync_roots_are_recognised_by_folder_name` |
| T14 | A read-only file is "replaced" | refused as `ReadOnly` before a byte is written; a copy of it is fine | `single_save::a_read_only_file_is_refused_with_a_code_and_left_alone`, `fsstate::a_read_only_file_is_refused_before_anything_is_written` |
| T15 | A source with no writer, or more than one frame, is replaced | one gate, `fsplan::replace_refusal`, for the single save, the group and the item view; the source stays byte-identical | `fsplan::the_replaceability_gate_is_one_rule`, `formats::*`, `restore_matrix::formats_without_a_writer_stay_byte_identical_under_overwrite` |
| T16 | A backup shares data with the original and the original is written in place | the engine never writes through an existing file (a save is a new file renamed over the path); a bare `Store` copies; the engine's default hardlink is made safe by the re-hash right before the swap | `store::replacing_the_original_does_not_touch_a_linked_backup`, `restore_matrix::overwrite_then_restore_is_byte_identical_across_the_matrix` |
| T17 | Purge or clean-up deletes outside the store | ids are 28 hex digits; entries must be real directories (not links or junctions) directly inside the store; manifest names are reduced to a file name | `store::hostile_entries_never_make_a_purge_leave_the_store`, `::ids_that_could_escape_the_store_are_refused`, `::day_short_entries_survive_and_expired_ones_do_not` |
| T18 | Restore loses the original or restores a damaged one | the backup is re-hashed against the manifest, the restored temp is re-read and checked before the swap, mtime and mode are put back; a changed file is kept in the backup first; works after a restart and with every non-entry file of the store deleted | `restore_matrix::overwrite_then_restore_is_byte_identical_across_the_matrix` (24 rows), `::a_bmp_is_converted_to_png_the_source_is_backed_up_and_restore_returns_it`, `kill_loop` (restore after every kill) |
| T19 | A conversion loses the source | backup first, no-clobber output, the source unlinked last and only if it still hashes as backed up | `restore_matrix::a_bmp_is_converted_...`, `::a_taken_target_name_gets_a_number_and_nothing_is_overwritten`, `group_faults::the_scan_is_removed_after_the_last_output_is_in_place_never_before` |
| T20 | Orphan temp files accumulate | recovery removes journalled temps; `sweep_orphans` removes only files named exactly `.autocrop-<28 hex>.tmp`, older than five minutes, named by no journal, in folders being opened | `group::tests::only_our_own_old_unjournalled_temps_are_swept`, `group_faults::recovery_leaves_unreadable_journals_and_unrelated_files_alone` |
| T21 | Long, verbatim, non-ASCII or symlinked paths | `\\?\` paths for the Win32 calls, the swap resolves a symlink so the link stays valid | `single_save::a_300_character_path_round_trips`, `commit::a_300_character_path_swaps` (Windows), `::the_swap_keeps_the_new_files_mtime_and_a_symlink_valid`, `single_save::a_symlinked_file_is_replaced_at_its_target_and_the_link_stays_valid` (Unix) |

## Measured: the kill loop

Windows 11, debug build, four workers, killed child with the child still running in every run (the
instants are uniform in the first 180 ms after the child's `READY`, widened by random microsecond
sleeps at every protocol step):

| Kills | Died inside the commit protocol (journal found) | Files checked | Original intact | Verified output | Converted | Restores byte-identical | Violations |
|---|---|---|---|---|---|---|---|
| 500 (default run, about 25 s) | 288 | 930 | 276 | 569 | 85 | 654 | 0 |
| 1,000 (nightly, about 50 s) | 606 | 1,897 | 545 | 1,178 | 174 | 1,352 | 0 |

A mutated protocol (the output's manifest left unsaved) makes the same loop report 96 violations in 40
kills, so the checker does fail when it should. Linux and macOS run the same test in CI.

## Open

* **Power loss.** Data not yet on the disk (the page cache, un-fsynced directory entries) is not
  modelled (no `FaultFs`). The protocol fsyncs the backup, the temp, the journal and the manifest and, on
  Unix, the folders; the claim is argued, not tested.
* **File identity and extended attributes.** The re-stat compares size, mtime and the content hash, not
  the volume serial and file index; extended attributes and Windows alternate streams of a *new name* are
  not copied (the Windows swap keeps those of the replaced file).
* **Tampered manifest.** A restore writes to the `original_path` the manifest names after checking the
  backup's hash; a manifest edited by someone with write access to the store could point that elsewhere.
  The store is owner-only (mode 0700 on Unix, the `%LOCALAPPDATA%` ACL on Windows), manifests are not
  signed.
* **Sync clients.** A sync client may still upload the temp file or undo a swap; the notice only tells the
  front end to recommend a copy.
* **`ReplaceFileW` on exFAT, ReFS and network shares** was not compared with `rename` (the M2.28 spike):
  the fallbacks for "not supported" errors exist and are unit-tested, the file systems were not.
* **Cross-volume backups** are copies by necessity; a second volume is exercised only where the machine
  has one (`/dev/shm` on Linux CI).
* **Reflink** is attempted first on every volume but could not be exercised on NTFS; it is exercised where
  the file system clones (APFS in CI).
