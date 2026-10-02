# M1 core model: where the code differs from the roadmap text

Not an ADR (no spike, no GO/NO-GO): a record of the places where the M1.01-M1.11 implementation
keeps a decision from the early engine slice, or reads the item text differently, so the owner can
veto any of them. The early slice's behaviour was kept whenever the two disagreed.

| Item | Roadmap text | What the code does | Why |
|---|---|---|---|
| M1.03 | `QuadWarp` holds four **f32** corners | corners stay **f64** (`Pt { x: f64, y: f64 }`), as in the early slice | The acceptance test is a 1e-4 px round trip at 12k px. An f32 near 1.0 has an ulp of 6e-8, which is 7e-4 px at 12,000 px, so f32 corners cannot meet it. f64 passes with room to spare (`pixel_round_trip_is_exact_to_1e_4_px_at_12k_for_every_orientation`). |
| M1.03 | corners "may exceed" 0..1 | the model allows it; `QuadWarp::sanitised()` still clamps | The engine's `Edit::to_state` clamps webview input on purpose (test `webview_numbers_are_clamped_not_trusted`); the model itself no longer forbids a crop that extends past the frame. |
| M1.03 | `GridWarp` "opaque serde payload" | a concrete struct (outline, cols, rows, nodes, model, turns, mirror) | `core` may not depend on `serde_json`, so there is no `Value` to hold an opaque payload. M12.27 owns the final shape; fields are `serde(default)` so it can grow additively. |
| M1.04 | `EditState {orientation, items, margin, enhance}` | same, plus the existing `version` field name (PLAN calls it `schema`) | The field was already called `version` on disk. |
| M1.05 | `migrate(serde_json::Value)` in the "Core" group | lives in `auto_crop_engine::migrate` | M1.01 limits `core` to `serde` and `thiserror`; the guard `cargo xtask check-deps` enforces it. `core` keeps `EDIT_STATE_VERSION` and a `deny_unknown_fields` `EditState`, so a document of another shape cannot be half-read. |
| M1.05 | fixtures `fixtures/editstate/v1_*.json` | `crates/engine/fixtures/editstate/` | Next to the code that reads them. |
| M1.05 | (not in the text) | the pre-release 0.0.1 shape `{"version":1,"geometry":{...}}` is migrated by shape | The early slice wrote that shape under the same version number 1 into every backup manifest. `Manifest.edit` now loads through `migrate`, so existing backups still open. Fixtures `v1_early_slice_*`. |
| M1.02 | `SourceId` "inside `SourceRef`" | the id type, the lazy slot and `SourceRef` are in `core`; the streamed blake3 (`hash_file`) is in `engine::source` | Hashing reads a file; `core` does no I/O. |
| M1.07 | `commit()` "replaces the top entry on a matching GestureId" | `commit(label, state)` is unchanged (no gesture); `commit_gesture(label, state, Option<GestureId>)` is the new one. `History::for_edit` compares by `render_hash`, `History::new` still by `==` | Keeps every existing caller working. The engine now builds its histories with `for_edit`. A coalesced commit that returns to the previous state removes the step. |
| M1.08 | `Raster`/`RasterView` in `core` | new types in `auto_crop_core::ports`; `auto_crop_imgproc::Raster` (8-bit RGB) is unchanged | Another agent owns `imgproc`; the two are unified when `imgproc` adopts the port in M1.22/M1.24. |
| M1.09 | `ErrKind` with codes per PLAN 2.10 | all early-slice codes kept with identical `SCREAMING_SNAKE_CASE` JSON; the PLAN 2.10 codes and `Degenerate`, `SchemaTooNew`, `UnsupportedFeature`, `Cancelled`, `DeadlineExceeded` are added | `ui/src/lib/types.ts` (`ErrorCode`) and `strings.ts` do not know the new codes yet; the engine does not emit any of them today, so the UI is unaffected until it does. `CodecError::Encode` still maps to `Internal`, as before. |
| M1.10 | `acquire(bytes, &CancelToken)` | same; adds `acquire_pixels`, `job_weight`, `cap_for`, `system_cap` (uses `sysinfo`, MIT) | Nothing in the engine calls it yet (M2.03 wires it). |
| M1.11 | "daily `tracing-appender` file" | `tracing_appender::non_blocking` over our own `DailyFileWriter` | `RollingFileAppender` cannot take a fake clock, and "rotation under a fake clock" is a required test. The writer also enforces the 7-day and 20 MB limits (PROVISIONAL) and scrubs absolute paths from every line below debug level. |
| M1.11 | paths redacted above debug | `LogPath` (always redacted) for info and up, `DebugPath::new` (raw only when a debug event is enabled), plus the writer's scrub | A subscriber cannot be asked its level while it formats an event, so `DebugPath` decides when it is built. |
| M1.11 | (not wired) | `engine::logging::init` exists; neither the CLI nor the shell calls it yet | Those crates are outside this change. The engine's decode path already emits the redacted `info` event. |

Windows note found while testing M1.11: `DirEntry::metadata().len()` of a file that is open for
writing is stale until the handle closes, so the log pruner measures files with `fs::metadata(path)`.
