> Auto Crop design, part 8 of 8 | [PLAN.md](../../PLAN.md) | [Decision log](00-decision-log.md) | [ROADMAP.md](../../ROADMAP.md)  
> Planning draft, 2026-10-01. Numbers marked PROVISIONAL are unmeasured estimates; decisions B1-B21 and the assumptions A-1..A-12 live in the decision log and in section 1.

# 8. Open source, release engineering, security and risks

This section defines how Auto Crop is licensed, built, shipped, defended and kept alive at $0 (B15). It applies B2 (MIT OR Apache-2.0), B9 (Windows x64 first, three-OS CI from day one), B12 (bundled libde265), B17 (AI disclosure, Flathub after 1.0), B18 (offline) and B21 (private golden set). Where research disagreed with a decision, the decision won and the mitigation is stated in place. Risk-register owners are release phases; ROADMAP.md maps them to M-numbers. Unmeasured numbers are PROVISIONAL. Owner-decision assumptions (A-n) are cited where they apply; 01 §1.7 holds the full A-1 to A-12 list and 8.11 the ones this section needs decided.

## 8.1 Licensing and dependency policy

### 8.1.1 Project licence (B2)

- All first-party code, docs, translations and UI assets are `MIT OR Apache-2.0`. `LICENSE-MIT` and `LICENSE-APACHE` land in the first commit, before any external PR, because relicensing later needs every contributor. No CLA; DCO sign-off instead (C5).
- The research recommended GPL-3.0-or-later mainly for Slint's free tier and to borrow Scan Tailor Advanced code. Neither applies: Tauri is primary, and the 4lex4 Scan Tailor Advanced fork's master has had no commit since 2020 (other forks are live but GPL-3). Ideas from GPL tools (Scan Tailor, unpaper, NAPS2) are reimplemented clean-room from papers. Accepted consequence: closed forks are legal. The licence grants no name rights, so forks should rename the app and change the app ID.

### 8.1.2 Dependency policy (cargo-deny)

| Class | Licences / examples | Policy |
|---|---|---|
| Allowed | MIT, Apache-2.0 (with LLVM exception), BSD-2/3-Clause, ISC, Zlib, 0BSD, CC0-1.0, Unicode-3.0, BSL-1.0 | Automatic |
| Allowed with notice | MPL-2.0 (unmodified crates only); IJG (second licence of `jpeg-encoder`) | Listed in THIRD_PARTY_NOTICES |
| Exception only | LGPL crates, e.g. rawler/rawloader (RAW is Tier 3, outside 1.0) | Separate helper executable only; none planned |
| Denied | AGPL, GPL (any version), SSPL, BUSL, CC-BY-NC/ND, research-only or custom non-commercial terms, no-licence crates, unreviewed `LicenseRef-*` | CI fails |
| Denied by name | `heic`, `heic-decoder`, `dssim-core` (AGPL); `jpegxl-rs`, `jpegxl-sys`, `birdcage` (GPL-3.0); any x264/x265 wrapper; `opencv` in the shipping workspace | CI fails |

Current cargo-deny releases treat `[licenses]` as an allow-list (the older `deny` and `copyleft` keys were removed), so "banned" means "not listed", plus named `[bans]`. Check keys against the pinned version (PROVISIONAL sketch):

```toml
[licenses]
allow = ["MIT","Apache-2.0","Apache-2.0 WITH LLVM-exception","BSD-2-Clause","BSD-3-Clause",
         "ISC","Zlib","0BSD","CC0-1.0","Unicode-3.0","BSL-1.0","MPL-2.0","IJG"]
exceptions = []            # each entry: crate, reason, review date
[bans]
wildcards = "deny"
deny = [{ crate = "heic" }, { crate = "heic-decoder" }, { crate = "dssim-core" },
        { crate = "jpegxl-rs" }, { crate = "jpegxl-sys" }, { crate = "birdcage" }, { crate = "opencv" }]
```

- `opencv-rust` is a dev-only test oracle in `tools/oracle`, a separate workspace. CI asserts `cargo tree -i opencv` is empty for the shipping crates.
- Ported third-party source is recorded. The LSD line-segment detector may be ported only from OpenCV 4.5.4 or later, with the source file, version and licence header in the port commit and its ADR (04 §4.3.2, M1.28); everything derived from GPL, AGPL or non-commercial work is clean-room (8.1.1).
- Frontend: npm only (no other package manager), `npm ci --ignore-scripts`, a committed lockfile and an allow-list licence check (`license-checker-rseidelsohn --onlyAllow` or equivalent). OFL-1.1 is allowed for font files only. PROVISIONAL budget: at most 25 direct npm dependencies. Python tooling (`tools/synth`, the models repo) gets the same allow-list check (`pip-licenses` or equivalent), and AlbumentationsX (AGPL-3.0) is denied by name there.
- HEIC packaging traps: `libheif-sys` `embedded-libheif` (static, no plugins, four releases behind, forces x264/x265 detection) is not used, and on windows-msvc that crate always uses vcpkg, whose libheif default `hevc` feature pulls GPL x265. The HEIC spike decides whether libheif-rs can target our own build on Windows; if not, we write minimal own bindings.

### 8.1.3 Native libraries and LGPL compliance

cargo-deny cannot see C libraries, so `native-deps.toml` lists libheif, libde265, dav1d, libjpeg-turbo, libjxl, libwebp and ONNX Runtime (plus PDFium, LibRaw or libvips if any of them ever ships) and records for each the name, version, upstream URL, SHA-256 of the source tarball, SPDX licence, CMake flags, patches and advisory feed. `cargo xtask build-native [--target]` fetches only by hash. Vcpkg is not used.

| Library | Version policy | Licence | Notes |
|---|---|---|---|
| libheif | >= 1.23.5, track latest patch | LGPL-3.0 | Own decode-only CMake build, `WITH_X265=OFF`, `WITH_X264=OFF`, no encoders, security limits never disabled; loaded only by the decode worker |
| libde265 | >= 1.1.3 | LGPL-3.0 | Separate shared library, ideally a libheif plugin (the spike must confirm plugin loading on all three OSes and pin the plugin path; fallback is a directly linked shared lib) |
| dav1d | current | BSD-2-Clause | AVIF decode through libheif |
| libjpeg-turbo | >= 3.1.4 (fixes a `tj3Transform` double-free) | IJG/BSD-3/Zlib | CI asserts the linked version |
| ONNX Runtime | 1.28 via ort 2.0.0-rc.13 | MIT | Pin and checksum; the ort/rten spike may replace it. Prefer `load-dynamic` with our own pinned binaries |
| libjxl | 0.12.x | BSD-3-Clause | Encode only (B11), through our own thin FFI because `jpegxl-rs` is GPL-3.0; decode uses jxl-rs. PDFium (if PDF input ships) and CI-only Tesseract follow the same pin-and-notice rules |
| libwebp | current 1.x, track latest patch | BSD-3-Clause | Lossy encode only (B11), through our thin FFI, linked statically into the encoder crate (notices only). Decode stays in `image-webp`, because CVE-2023-4863 was a libwebp decoder bug |

LGPL procedure (an engineering reading, not legal advice):

1. libheif and libde265 are shared libraries beside the helper, never statically linked into any Auto Crop binary.
2. THIRD_PARTY_NOTICES and the About dialog carry the LGPL-3.0 and GPL-3.0 texts, and each release attaches `native-sources-<version>.tar.zst` (exact upstream tags plus any patches), so corresponding source never depends on upstream availability.
3. `docs/replacing-libheif.md` gives per-OS steps: swap the DLLs on Windows; swap the dylib and re-sign with `codesign --force --sign -` on macOS (Apple silicon requires a valid signature); extract, swap and repack an AppImage; rebuild Flatpak or use system libraries for deb/rpm. The Store MSIX package directory is signed and read-only, so there users install the same version from the direct NSIS or portable build, or rebuild from the attached native-sources archive: the direct builds are the compliance reference and the Store build is a convenience variant (M13.56).
4. `cargo xtask check-native` compares every packaged artifact's loaded libraries (`dumpbin /dependents`, `otool -L`, `ldd`) with `packaging/allowed-libs.txt` and greps for `x265_`/`x264_` symbols, catching the GPL leak the vcpkg trap would cause.

### 8.1.4 Notices, REUSE and DCO

- REUSE: every file carries `SPDX-License-Identifier: MIT OR Apache-2.0` and `SPDX-FileCopyrightText: 2026 Auto Crop contributors`; `REUSE.toml` annotates binaries, fixtures and model files; `reuse lint` runs in CI.
- cargo-about generates the Rust notices, merged with the npm and native lists into `THIRD_PARTY_NOTICES` at release time and shown offline in Help > About > Licences.
- DCO: `Signed-off-by` on every commit, checked by an in-repo `cargo xtask check-dco` (no third-party GitHub App), plus the setting requiring sign-off on web commits. Squash merges keep the PR-commit check as the record.

### 8.1.5 Model weights and data provenance

- Policy: shipped weights must carry an OSI-approved licence (Apache-2.0, MIT, BSD), which also satisfies SignPath. Non-OSI "open" terms (OpenRAIL, CC-BY-NC, research-only) are banned, and no release build may include a weight whose provenance row is not `cleared` or an owner-granted `exception` (Assumption A-7, 8.11).
- Source of truth: one JSON-lines log in the models repo (`auto-crop-models`), with a record per dataset, asset and weight: source and licence URLs, SPDX licence, retrieval date, archive SHA-256, allowed use, attribution text, initialisation source, reviewer and status. The CSV and `models/PROVENANCE.md` views are generated from the log and never edited by hand.
- Statuses: `cleared`; `pending` (not yet audited); `blocked` (until audited); `banned`; and `exception`, granted only by the owner and recorded in an ADR (A-7). `cargo xtask check-models` fails CI when `models.lock` pins a weight whose own row or training-data rows are not `cleared` or `exception`, when a SHA-256 differs, or when a manifest references an unlogged asset.
- Loading: the 1.0 loader accepts only models listed in `models.lock`, hash-checked at load and fetched at build time by pinned SHA-256, so the app repo holds no binaries or Git LFS. There is no in-app model download: the optional dewarp pack is a hash-pinned release asset installed from a local file (04 §4.8), and user-supplied models are post-1.0 (B.13).
- Bootstrap: MakeACopy DocQuadNet-256 (13.4 MB) is labelled Apache-2.0 but GitHub reports NOASSERTION. Ask upstream for an explicit weight licence and training-data statement, and record the commit, SHA-256 and reply in the log. Until confirmed in writing it is dev-only; its recipe also used UVDoc pretraining and DTD backgrounds, so shipping DocQuadNet-derived weights needs an `exception` (A-7).
- Training lives in the separate public repo `auto-crop-models` with code, configs, dataset fetch-by-hash, eval and a model card per release.

| Data or weights | Status |
|---|---|
| Own synthetic renders (CC0 or procedural backgrounds); SmartDoc 2015 and CORD (CC BY 4.0, attribution in notices) | `cleared` |
| MIDV variants | `pending`: verify licence per subset |
| UVDoc/Doc3D textures (DTD research-only, Gutenberg, DeepFloyd outputs, CVF pages), SROIE, any CC BY-SA (share-alike may propagate to weights); DocAligner weights (no licence stated), PP-LCNet_x1_0_doc_ori (training data undisclosed) and DocShadow-SD7K exports (weight licence unstated) | `blocked` until audited |
| ImageNet-initialised backbones (MobileNetV3/LCNet), DocQuadNet-derived and UVDoc weights | `blocked` until the owner grants an `exception` (A-7); otherwise from-scratch or Track B weights ship |
| DIS5K, RMBG, DocTr, DocGeoNet and DocEnTr weights (non-commercial); DE-GAN (GPL-3.0) | `banned` |
| Private golden set (B21) | Never used for training |

Not in the research files: ImageNet-pretrained backbone initialisation (MobileNetV3/LCNet) is a provenance question, since ImageNet's terms are research-oriented. The log records every initialisation source, and only the owner can waive the question, by exception (A-7). The v1.0 dewarp net (B10) needs the same audit (Track A) or training on our own synthetic warps (Track B, 04 §4.8).

### 8.1.6 Slint fallback: licence only

Relevant only if the week-1 spike triggers the fallback. Slint's Royalty-free licence lets an MIT/Apache app use it with `AboutSlint` attribution ([Slint FAQ](https://github.com/slint-ui/slint/blob/master/FAQ.md)), but it is not OSI-approved, forks must pick their own Slint licence, cargo-deny needs an explicit exception, and it may conflict with SignPath's rule. The GPLv3 tier would make the shipped whole GPL and reopen B2. Ask SignPath in writing first. Switching to Slint needs the owner's waiver of B20's RTL layout and a licence decision (Assumption A-10, 8.11).

## 8.2 Repository setup (`WelFedTed/auto-crop`)

### 8.2.1 Files

| Path | Content |
|---|---|
| `README.md` | Name plus tagline, before/after GIF and review-grid screenshot, install table with trust status and update route per channel, verify-download snippet (`sha256sum -c`, `gh attestation verify`), HEVC note, privacy line, Code signing policy, AI-assistance disclosure, ROADMAP link |
| `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSES/`, `REUSE.toml`, `deny.toml`, `native-deps.toml`, `models.lock`, `release-platforms.toml`, `rust-toolchain.toml` | Legal, pinning and the list of platforms that publish |
| `ROADMAP.md`, `AGENTS.md`, `CLAUDE.md`, `MAINTAINERS.md` | The living checklist and its ticking rule; instructions for AI coding tools; maintainer continuity and key inventory |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md` | Build steps, DCO, conventional commits, clean-room rule, AI-use policy, no private or personal images in issues; Contributor Covenant with a maintainer-chosen contact (no domain at $0) |
| `SECURITY.md` | Private vulnerability reporting, supported versions (latest release only, as libheif does), PROVISIONAL response targets (acknowledge in 7 days, plan in 30), scope (decoder crashes and limit bypasses count), HEVC/patent note (8.7) |
| `.github/` | Issue forms (bug, feature, and a "detection failure" form warning against sensitive attachments), PR template (DCO, tests, ROADMAP ID, licence check, AI disclosure), `CODEOWNERS`, `dependabot.yml`, workflows; `FUNDING.yml` once Sponsors is live |
| `docs/`, `.devcontainer/`, `xtask/`, `packaging/`, `models/`, `testdata/` | mdBook site, ADRs (`docs/adr/NNNN-slug.md`), `docs/release-runbook.md` and `docs/testing/golden-set.md`; onboarding; tooling; per-OS packaging; the generated provenance views; at most 5 MB synthetic smoke set |

Settings: private vulnerability reporting, secret scanning with push protection, Dependabot alerts, CodeQL (Rust support to verify), Discussions, topics.

### 8.2.2 Branch protection and commits

- Ruleset on `main`: PR required, required checks (Linux, Windows x64, macOS arm64), linear history, no force-push or deletion. Squash merge only, PR title as a Conventional Commit (scopes `core`, `codecs`, `imgproc`, `engine`, `worker`, `cli`, `shell`, `ui`, `xtask`, `docs`, `ci`, `deps`, `models`). Approvals stay at 0 while the maintainer is solo; CODEOWNERS routes `crates/codecs`, `crates/worker`, `crates/shell/capabilities`, `models/`, `.github/workflows`, `deny.toml`, `native-deps.toml` and `packaging/` to them, so a second maintainer's review becomes mandatory in one setting.
- Semver. 0.x releases may break CLI flags and config keys, but the first preview that can write files already holds users' originals (B3), so every release must open, migrate and restore the backup store and SQLite journal written by any earlier release, 0.x included, and refuse a downgrade with a clear message (roadmap X.25). The 1.0 contract additionally freezes CLI flags, exit codes and the config file. The `EditState`, backup-store and journal schemas change only through tested migrations; every on-disk format carries `schema_version`, and a newer schema than the app knows is refused, never rewritten.
- release-plz: one workspace version, `publish = false`, opens the release PR, updates CHANGELOG.md and creates the `vX.Y.Z` tag; `release.yml` builds the GitHub Release. Tags created with `GITHUB_TOKEN` do not trigger workflows, so use a GitHub App token (no expiry) or a fine-grained PAT with an expiry reminder.
- *Assumption A-12: 1.0 needs two RC windows of 14 days (3 days if the RC differs only by native-library bumps); 0.x previews are ordinary releases; `auto-crop` is reserved on crates.io at the first release, no `auto-crop-core`. (owner may veto)* The reservation is a minimal placeholder (`autocrop` there is an unrelated crate created 2026-09-05); every other workspace crate stays `publish = false`.

### 8.2.3 Toolchain, MSRV and updates

- `rust-toolchain.toml` pins the CI toolchain (stable 1.98.1 today; 1.99 is due 2026-10-01), bumped about quarterly. `rust-version` is separate and for an application just tracks the pin: recent Tauri, egui and Slint already need Rust 1.92 to 1.95, so a wide MSRV promise would be brittle. A real MSRV would apply only if a crate were ever published as a library (none is planned, A-12).
- Dependabot covers Cargo, npm, GitHub Actions and the devcontainer, weekly and grouped, with its cooldown option for brand-new releases (verify). No auto-merge; Tauri, ort, libheif and libde265 bumps need a human changelog read.
- Native libraries are outside Dependabot: `security.yml` polls upstream releases and published advisories for every library in `native-deps.toml` (libheif, libde265, dav1d, libjpeg-turbo, libjxl, libwebp and ONNX Runtime) and opens a `security-native` issue when a pin is behind a security release. SLA (PROVISIONAL, 03 §3.4.3): a critical or high advisory is patched and released within 7 days of the upstream fix, with a 72 h target for critical; moderate and low ones ship in the next regular release, at most 30 days. A tag-triggered rebuild-and-release path (pin bump, CI rebuild with the HEIC smoke corpus, release-plz patch release) makes this feasible for one maintainer, and missing the SLA two quarters running reopens B12 with the owner.

### 8.2.4 AI-assistance disclosure (B17)

README, CONTRIBUTING and the Flathub submission carry the same text:

> Auto Crop is developed with substantial assistance from AI coding tools (Anthropic's Claude Code). A human maintainer reviews, tests and signs off every change; the DCO sign-off is always human. No code is knowingly copied from GPL, AGPL or non-commercial projects; algorithms derived from such work are reimplemented clean-room from papers.

CONTRIBUTING rules: AI tools are allowed, but material AI-generated content is disclosed in the PR, the contributor stays responsible for licensing, and an AI tool is never `Signed-off-by` (a `Co-authored-by` trailer is the disclosure). Never paste GPL source into a prompt; clean-room algorithm work starts from a written spec derived from the paper and gets a maintainer spot-check, because AI code can echo copyleft snippets. `AGENTS.md` and CODEOWNERS mark `packaging/flathub/` human-only, because Flathub bans AI content in manifests and submissions ([requirements](https://docs.flathub.org/docs/for-app-authors/requirements)).

### 8.2.5 Name and ID

The name is free on GitHub, crates.io (`auto-crop`), Homebrew, Snap, AUR and Chocolatey, but taken on PyPI and npm, and "Auto Crop" is also a Microsoft 365 command and a Mac App Store title, so always pair it with the tagline. `io.github.welfedted.AutoCrop` is permanent (bundle ID, AppUserModelID, Flatpak). A USPTO/EUIPO check is a low-priority roadmap item.

## 8.3 CI/CD on GitHub Actions

### 8.3.1 Runner matrix

| Target | Runner | Role |
|---|---|---|
| Windows x64 | `windows-2025` | Required. Build, test, NSIS, zip, MSIX; first public release |
| Windows ARM64 | `windows-11-arm` | Nightly; best-effort until 1.0 (C1) |
| macOS arm64 | `macos-latest` (macOS 26 arm64) | Required. Build, test, DMG |
| macOS x64 | `macos-26-intel` | Nightly smoke; best-effort. Retirement date unverified (Aug 2027 was stated for `macos-15-intel`) |
| Linux x64 | `ubuntu-22.04` | Required. glibc floor; AppImage, deb, rpm |
| Linux arm64 | `ubuntu-22.04-arm` | Nightly; same glibc floor (not `ubuntu-24.04-arm`) |

Linux release artifacts build in a digest-pinned `ubuntu:22.04` container, so the glibc floor survives GitHub retiring the runner image (AppImage tooling runs extract-and-run; containers lack FUSE).

- Minutes are free on public repos, but concurrency is capped (20 jobs, 5 macOS on the Free plan; [runner docs](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)) and runners are small (Linux 4 vCPU/16 GB, macOS M1 3 vCPU/7 GB), so wall-clock benchmarks there are noise. PRs run required targets with `cancel-in-progress`; the full matrix runs nightly and on tags.
- Caches (Swatinem/rust-cache pinned by SHA, setup-node) fit the 10 GB cap and 7-day eviction; native libraries are keyed on the `native-deps.toml` hash. Release jobs use no cache, so a poisoned PR cache cannot reach a release (PROVISIONAL: 20 to 40 minutes per platform).

### 8.3.2 Workflows

| Workflow | Trigger | Content |
|---|---|---|
| `ci.yml` | PR, main | fmt, clippy `-D warnings`, nextest, `svelte-check`, vitest, eslint, i18n pseudo-locale check, `reuse lint`, cargo-deny, `check-native`, `check-models`, `check-dco`, `roadmap-check`, zizmor, docs link check; 200-image synthetic accuracy smoke (block on mean IoU -0.3 pt or failure rate +0.5 pt, PROVISIONAL); gungraun instruction-count gate above 5% on decode, resize, warp and threshold kernels (Linux only) |
| `nightly.yml` | schedule | Full accuracy suite (public and synthetic data), ARM and Intel builds. Wall-clock end-to-end runs (issue on a p50 regression above 10%) are not here: they need a stable machine, so they run as a job of the private repo's self-hosted runner (8.3.5), never on a public runner |
| `security.yml` | daily | cargo-deny advisories, `cargo audit`, `npm audit`, native-watch for every library in `native-deps.toml` (8.2.3) |
| `fuzz.yml` | PR (codec and core crates) and nightly | cargo-fuzz on our glue: EXIF/IFD rewriting, worker framing, IPC, TIFF G4 wrapper, PDF writer, ICC handling. 60 s smoke per target on PRs that touch the codec and core crates; about 1 h per target nightly (PROVISIONAL); cargo-fuzz is Unix-only, so crashers replay as ordinary tests on all three OSes. The gate before 1.0 is at least 72 h cumulative per target, nightlies counting |
| `release.yml`, `pages.yml`, `codeql.yml` | tag `v*`; main and tags; schedule | 8.3.4; one Pages site through the single `pages.yml` (metrics dashboard, `/flatpak/` remote, mdBook); CodeQL |

### 8.3.3 Build day one (B9)

The first commit has the matrix building `core` and `cli` on all three OSes, and the harness runs there before any GUI exists; the Tauri shell joins the day it exists. A red required OS blocks merge, so macOS and Linux cannot rot behind a Windows-only release. `release.yml` builds every platform on every tag but publishes only those in `release-platforms.toml`; until their previews, other artifacts are 30-day workflow artifacts for testers.

### 8.3.4 Release pipeline

1. Merge the release-plz PR; the App token creates the tag.
2. `release.yml` starts with `contents: read` and gates on the `release` environment (manual approval, a forced pause even for a solo maintainer).
3. Create a draft release. Immutable releases lock tag and assets on publish, so attach everything first ([docs](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)); enable them once the flow is proven.
4. Matrix builds (no cache): `build-native`, `package`, `check-native`, then a packaged-artifact smoke test (silent NSIS install, `auto-crop doctor --self-test`, the GUI with `--smoke-test <dir>`, convert a synthetic HEIC to JPG), which proves libheif and the libde265 plugin load in the shipped layout and records the achieved sandbox level.
5. After SignPath approval: sign in two stages (8.4.1).
6. `finalize`: `SHA256SUMS`, CycloneDX SBOM, THIRD_PARTY_NOTICES, native sources archive, the static `latest.json` and its `.sig` (direct-update variants only; the one asset every update check fetches, 8.5), and `actions/attest@v4` provenance and SBOM attestations (`id-token` and `attestations` write at job level only). Target is SLSA build L2; L3 needs a reusable-workflow layout. Builds are not bit-reproducible in v1.
7. Publish, then a verify job runs `sha256sum -c` and `gh attestation verify` on the public assets. Downstream: Homebrew tap PR, Scoop bucket, winget PR (komac), Flatpak remote update.

1.0 is cut from the last release candidate after two RC windows of 14 days (3 days if the RC differs only by native-library bumps; Assumption A-12, 8.2.2). RCs are GitHub pre-releases, so the update check never offers them (8.5).

### 8.3.5 Private golden-set workflow (B21)

The public repo holds only synthetic and permissively licensed data. `cargo xtask fetch-corpus` fetches SmartDoc and CORD (both CC BY 4.0) and CC0-only raw.pixls.us samples (some samples there are not CC0) by pinned SHA-256.

*Superseded in part by the owner decision of 2026-10-04 (one repository only): there is no private repo and no self-hosted golden runner; the golden set is evaluated locally under the gitignored `_data/`, and only aggregates are published. `docs/testing/golden-workflow.md` is current where the text below differs.* This is the one design, shared with 07 §7.5. *Assumption A-8: The golden set reaches v2 (>= 800 locked, >= 80 per gated slice, plus a ~300 dev tier) before 1.0, and its images never leave the maintainer's encrypted disk; a hosted variant needs the owner's approval. (owner may veto)*

- **Where things live.** The private repo `auto-crop-golden` holds workflows, label manifests and hashes, never images. The images stay on the maintainer's encrypted disk, outside every directory an AI coding tool can read, and a self-hosted runner attached to that repo only reads them. The private HEIC corpus (its GPS tags kept, by a recorded exception) runs only on the maintainer's Mac.
- **Build, then run only the binary.** A GitHub-hosted runner builds `auto-crop-eval` from a public SHA in a job with no golden path and no token. The self-hosted job fetches that artifact by SHA-256 and runs only that binary, in a container with networking off and the set mounted read-only. No third-party code runs beside the data: no third-party actions, build scripts or dependencies on the self-hosted runner. The public repo and public CI never see the set, and fork PRs never reach it.
- **Publishing.** A reviewed publisher step checks the evaluator's output against an allow-list schema (counts, means, percentiles, per-slice numbers, calibration bins) and pushes only that aggregate JSON to the public `metrics` branch with a fine-grained token stored in the private repo alone. Slices with n < 30 are suppressed from every public artefact, and per-image rows, paths, thumbnails and contact sheets never leave the machine. A manifest hash shows the set did not change.
- **Triggers.** `schedule` (nightly, dev tier), `workflow_dispatch` (locked set, once per release candidate, appended to an evaluation log) and `repository_dispatch` from `release.yml`. Never per push, `pull_request` or fork: per-push numbers could be differenced to reveal single images and would invite tuning against the locked set. A contributor's change reaches the set only after the maintainer has read the diff and merged it.
- **Overlap and leakage.** Overlap checks (no scene shared by training, dev and golden) run in the private repo, which holds the labels and hashes. The golden set is never used for training or tuning; if its results ever guide a change, those images are retired (07 §7.5). The written policy and the fork-PR negative test are kept in `docs/testing/golden-set.md`.
- **Stable machine.** The same self-hosted runner gives wall-clock benchmarks a quiet machine (07 §7.2); it runs nothing from the public repo's workflows.
- **Hosted variant (needs the owner's approval).** Putting the images in the private repo (Git LFS or release assets; about 2 to 4 GB at 800 images, an estimate) and running on GitHub-hosted runners would place the set on GitHub's servers and, with 2,000 free private-repo minutes per month and macOS and Windows multipliers, limit runs to a Linux nightly. It is not the plan.

## 8.4 Packaging and distribution ($0)

Channels: Windows portable zip, NSIS installer (per-user, no admin, WebView2 `downloadBootstrapper`; a direct variant with the updater and a managed one without it for winget), Store MSIX, own Scoop bucket, winget later. macOS arm64 DMG (ad-hoc signed, not notarised) and own Homebrew tap. Linux AppImage (x64, arm64 best-effort), deb and rpm from the Tauri bundler, own Flatpak remote plus a per-release `.flatpak` bundle, AUR `-bin` later, Flathub after 1.0. Every channel costs $0.

Backups must outlive the app on every channel (B3): the NSIS uninstaller asks before removing app data and defaults to keeping it, `brew uninstall --zap` never touches the backup store, the Store build keeps the store outside virtualised package data (8.4.1), and uninstall, reinstall and Restore original is tested on every channel (M13.28).

Not used, per B15: Azure Artifact Signing (about $9.99 per month), Apple Developer ID ($99 per year), a domain, and WiX MSI (its fee applies only above $10,000 revenue; the friction is EULA acceptance, and NSIS and MSIX avoid it).

### 8.4.1 Windows

- SignPath Foundation: apply once the first public release exists, since the project must already be released and actively maintained ([terms](https://signpath.org/terms)). Re-checked requirements: an OSI licence without commercial dual-licensing (MIT OR Apache-2.0 should qualify), no proprietary components (system libraries are permitted, so WebView2 should be fine, but ask), MFA for every team member on SignPath and GitHub, Author/Reviewer/Approver roles, a Code signing policy page with the line "Free code signing provided by SignPath.io, certificate by SignPath Foundation", and a privacy statement. Approval time is unpublished (plan for weeks to months) and the publisher shows as SignPath Foundation.
- Tauri's per-file `signCommand` cannot wait for remote approval, so signing takes two rounds: sign app binaries and DLLs, assemble the NSIS installer from them, then sign the installer (PROVISIONAL).
- Store MSIX: registration is free for individuals (re-checked on the [2025-09-10 Windows blog](https://blogs.windows.com/windowsdeveloper/2025/09/10/free-developer-registration-for-individual-developers-on-microsoft-store/): no card, government-ID and selfie check) and the Store re-signs MSIX; re-verify on registration day. Tauri's bundler emits NSIS and MSI, not MSIX (verify), so packaging uses MakeAppx and a hand-written full-trust manifest (about 3 days), updater compiled out. *Assumption A-6: The Store build is verified early by a hidden dry run (M6.81) and keeps backups outside virtualised package data; a Store-only `no-hevc` build or any fee needs the owner's decision (B12, B15). (owner may veto)*
- Store data safety (B3): packaged desktop apps get writes to `%LOCALAPPDATA%` virtualised into a private per-package store that is removed on uninstall, so a Store build that kept `library.db` and `backups/` there would delete the only copies of users' originals when the app is uninstalled. The Store build therefore keeps the backup store and journal in a real user-folder default chosen at first run (a per-variant `backups_dir`, M5.82), or uses the `unvirtualizedResources` capability if certification allows it. This is settled early, not at certification: M2.28 sideloads a throwaway MSIX, uninstalls it and observes where the files land, and M13.60 proves uninstall, reinstall and Restore original on the release candidate.
- Unsigned previews: README and release notes explain SmartScreen ("More info", "Run anyway"), checksums and attestations. Windows 11 Smart App Control may block unsigned apps with no override; that is not in the research, so test a clean Windows 11 with it on during the first preview. If confirmed, the Store MSIX becomes the trust route and moves earlier. Antivirus false positives are likely: file reports and keep the portable zip. On offline or locked-down Windows 10 the WebView2 runtime may be missing, so document the offline installer.

### 8.4.2 macOS

- The DMG holds an arm64 `.app` signed ad hoc (Apple silicon will not run unsigned code). Sequoia removed the Control-click override, so the README documents System Settings > Privacy & Security > Open Anyway with screenshots for macOS 12 to 26, plus an `xattr -dr com.apple.quarantine` fallback.
- Own tap repository `WelFedTed/homebrew-auto-crop`; users install with `brew install --cask welfedted/auto-crop/auto-crop`. The cask's `zap` stanza removes caches, preferences and logs only and never the backup store, so `brew uninstall --zap` cannot delete a user's backups. Since 2026-09-01 the main cask tap rejects apps that fail Gatekeeper; own taps are unaffected ([discussion](https://github.com/orgs/Homebrew/discussions/6334)). A main-tap self-submission would also need 90 forks, 90 watchers or 225 stars ([policy](https://docs.brew.sh/Package-Acceptance-Policy)), so it is not pursued. Verify unsigned casks still install from third-party taps.
- Apple silicon (macOS arm64) is first-class; ARM64 on Windows and Linux and Intel Macs are best-effort until 1.0. ONNX Runtime ships no macOS x86_64 binary, so an Intel Mac build would need rten or `load-dynamic`.

### 8.4.3 Linux

- Tauri's bundler does not produce Flatpak. `packaging/flatpak/io.github.welfedted.AutoCrop.yml` is hand-written, built with `flatpak-builder` from the release tarball and pinned native sources including libde265 (the Freedesktop runtime's libheif has none). The OSTree repo is served from the one GitHub Pages site under `/flatpak/` (M9.19 adds it to the single `pages.yml`), signed with a key held as a secret in the `release` environment (PROVISIONAL: prune to two revisions for Pages size limits; the per-release bundle is the fallback).
- Flatpak filesystem access (B3): in-place replace needs write access to the folder that holds the image, which a document-portal grant to one file does not give. The own-remote manifest therefore requests home (or Pictures) access, recorded as a decision with its trade-off (M13.67; unverified until the packaging spike); without it the app falls back to Save as copy with a notice (02 §2.13). The updater is compiled out.
- Flathub after 1.0 with a human-authored manifest and PR plus the disclosure above. The policy flipped on 2026-05-29 (blanket ban), 2026-09-04 (disclosure) and 2026-09-21 (manifest ban restored), so expect real rejection risk and keep the own remote regardless.
- Distro packagers get the `no-hevc` variant (8.7) with the updater compiled out and, post-1.0, a source tarball with vendored crates and npm packages. Signed apt and dnf repositories are a post-1.0 backlog item (B.41); until then deb and rpm are trusted through checksums and attestations.

### 8.4.4 Channel timeline

First preview (v0.1.0, Windows x64, CLI only): portable zip, unsigned, with SHA256SUMS, attestations and SBOM; SignPath applied for that week. The GUI alpha adds the NSIS installer. Store MSIX: a hidden dry run early (M6.81, Assumption A-6) and the public listing at 1.0, moved earlier only if Smart App Control blocks unsigned installs; signed builds follow whenever SignPath approves. macOS preview: DMG plus tap. Linux preview: AppImage, deb, rpm, Flatpak remote. 1.0: all three OSes solid, plus winget, Scoop, AUR. Post-1.0: Flathub, OSS-Fuzz (B.39), signed apt and dnf repositories (B.41).

## 8.5 Updates, telemetry and privacy (B18)

Nothing is fetched by default, and the update route is fixed per channel, so a tool that overwrites user files never replaces itself unasked:

| Behaviour | Channels | What the user gets |
|---|---|---|
| Direct | NSIS installer, AppImage; the DMG only if the M8.11 spike keeps install-in-place | Manual "Check for updates", an opt-in weekly notify-only check (off by default), and install only on an explicit click through tauri-plugin-updater |
| Notify-only | Portable zip (also what Scoop installs); own deb and rpm (also what the AUR `-bin` package repacks); the DMG, and so the Homebrew cask, if M8.11 drops install-in-place | The same two checks, but the banner only links to the release page; nothing is downloaded or installed. Package installs need root and their manager owns the files, yet users still learn of libheif patches. Scoop and Homebrew users upgrade with their tool |
| Compiled out | Store MSIX, Flatpak, winget-managed NSIS, distro builds and `no-hevc` builds | No updater plugin, capability, update UI or update URL in the binary; the store or package manager updates it |

Mechanics:

- Cargo features `updater` and `hevc`, plus Tauri `--config` overlays. Only capabilities listed in `tauri.conf.json` apply, so the updater capability appears only in updater variants (overlay array merging must be verified). CI asserts that compiled-out variants contain no update URL and no updater plugin in `cargo tree`, and that direct and notify-only variants carry the check with the weekly toggle off by default (M13.69 asserts it per channel).
- Both checks fetch the static `latest.json` release asset of the latest GitHub release from github.com (following its release-asset redirect): no API host, no rate limit, no token. The weekly check runs at most once per 7 days, in the background, with a 5-second timeout and silent failure; the manual check runs on click. Requests send a minimal User-Agent (version and OS) and no identifier. 0.x previews are ordinary releases (not flagged as pre-releases), so `releases/latest` serves them.
- Tauri's minisign signature is mandatory for every install. The private key sits in the gated `release` environment, so it protects against a tampered CDN but not a compromised GitHub account. Loss or leak forces a manual-reinstall release, documented in SECURITY.md.
- Crash log and Report issue: a panic hook writes `crash/<ts>-<version>.txt` in the app-data store (version, OS, backend, error codes, backtrace, recent log lines), with raw paths and file names redacted to a short hash plus extension (02 §2.10) and no image content. "Report issue" opens the GitHub issue form with a URL that carries only version, OS, webview or backend, channel and error code. Redacted diagnostics (at most 4 KB) go to the clipboard after a preview dialog shows exactly what will be copied, and the user pastes them; nothing is uploaded and nothing sensitive goes in a URL.
- Privacy statement (`docs/privacy.md`, README, SignPath policy page): processing is local; no accounts, analytics or telemetry; the only network use is the manual update check, the opt-in weekly check and links the user clicks (plus the installer's WebView2 bootstrapper download when the runtime is missing). It also discloses cached thumbnails and proxies (clearable), backups holding full originals, where the backup store lives per channel, possible requests by the OS webview itself (WebView2 in particular), and that EXIF including GPS is kept by default (C2), so the export bar shows "N files contain a location" beside the one-click "Strip location" toggle.
- CI network test: a scripted Linux session under `strace -f -e trace=connect` must show no non-loopback connection in compiled-out builds, and only github.com during an explicit check in direct and notify-only builds (PROVISIONAL).

## 8.6 Security and threat model

**Attacker model.** In scope: (A1) anyone who supplies an image, the primary threat; (A2) hostile metadata or file names that reach the webview; (A3) supply-chain attackers; (A4) another local user or process that reads or tampers with the backup store and journal (user-only ACL or 0700); (A5) a network attacker on the update channel. Out of scope: same-user malware, a compromised OS, and encrypted backups (rely on disk encryption; backups are user-only readable).

```
 untrusted files ---> [auto-crop-worker pool, sandbox level in About]  libheif, libde265, (PDFium)
   (A1)                     | shared memory, header-validated, capped
                            v
 zune-jpeg/png/tiff --> [core / engine]  EditState, journal (SQLite), backups
 turbojpeg (in-process,     ^   |  atomic writes
  caps + catch_unwind)      |   v
                  IPC (opaque IDs)   user files + backup store (user-only ACL)
                            |
                     [webview: Svelte UI]  CSP, no plugin permissions, no net, tiles via custom URI scheme
```

### 8.6.1 Threats

| Threat | Mitigation |
|---|---|
| Memory corruption in C decoders (libheif had 61 advisories in 2026, [SECURITY.md](https://github.com/strukturag/libheif/blob/master/SECURITY.md)) | Sandboxed worker (8.6.2), tracked patches (8.2.3), fuzzed glue |
| Decompression bomb, OOM (a 48 MP HEIC is about 146 MB as RGB8) | Header probe, then caps on pixels (100 MP by default, C1; 500 MP hard ceiling via Advanced), file size, frame and item counts, time and memory; libheif limits never disabled; own `image::Limits` (its default alloc limit is non-strict); byte-weighted batch admission; bomb corpus in CI |
| Panic or abort in Rust decoders | `panic=unwind`; job bodies run under `catch_unwind` on a pool with a `panic_handler` (02 §2.8), so rayon's abort-on-`spawn`-panic never triggers. A hung in-process decoder cannot be killed, so the UI marks it and abandons the thread (PROVISIONAL: `decode.isolation = all` routes those decoders through the worker if hangs recur, 03 §3.10.2) |
| Hostile metadata reaching the UI (EXIF, IPTC, file names) | Text nodes only, ESLint `svelte/no-at-html-tags` as error, no `innerHTML` or `eval`, length caps, bidi-override characters stripped from displayed names |
| Path abuse on folder drop (symlink loops, UNC, reserved names, alternate streams, placeholders) | Loop detection and a depth cap in Rust; the webview sees only opaque item IDs; placeholders skipped or hydrated after confirmation; Windows loads libraries by absolute path with `SetDefaultDllDirectories` |

Overwrite loss, webview compromise, the update channel and supply chain are covered in 8.6.3, 8.6.4, 8.5 and 8.6.5.

### 8.6.2 Decoder isolation

- `auto-crop-worker` (crate `worker`) is a separate executable that links libheif and libde265; the main process never loads them. The parent reads the file once into a shared-memory input segment and allocates the output segment from its own header probe, so the worker is given no file path or file handle, only the two memory segments (02 §2.9). The parent kills the worker on timeout (10 s plus 1 s per MP, PROVISIONAL) or cap breach.
- The pool is persistent for latency. Because overwrite is the default, a compromised worker could tamper with later outputs, so workers are recycled after any anomaly, after any decode that used over 50% of a cap, and every 32 decodes (PROVISIONAL). A crash marks the item "could not be decoded safely", leaves the original untouched (B4) and denies that content hash for the session.
- Mechanisms per OS (PROVISIONAL until the sandbox spike): Windows job object (memory limit, one process, kill-on-close) plus restricted token, or AppContainer; Linux Landlock (kernel 5.13 or later) plus a seccompiler allow-list (no sockets) and rlimits; macOS `sandbox_init` deny-default profile plus rlimits (deprecated API, still works). `birdcage` is unusable (GPL-3.0, archived 2026-07-06).
- Achieved level, shown by About and `doctor`: `appcontainer`, `job+token`, `landlock+seccomp`, `seccomp-only`, `sandbox_init` or `process-only`. Only `appcontainer`, `landlock+seccomp` and `sandbox_init` also block network and filesystem access. `job+token`, the Windows baseline until the AppContainer decision (M6.07), caps memory and processes and kills the worker on close but does not stop network use or reads of user files; `seccomp-only` (Linux without Landlock) is partial, with no filesystem ruleset; `process-only` is a separate low-privilege process with rlimits and a watchdog and nothing more. macOS and Linux run `process-only` until their sandboxes land in M8 and M9.
- If no sandbox can be established the worker runs `process-only`, and a setting can refuse HEIC decoding then.

### 8.6.3 Overwrite and backup safety (B3)

The commit protocol has one owner, 02 §2.7, with its journal states and crash-recovery rules; this section does not restate it as a second protocol. Its order, per file: plan and snapshot the source (size, mtime, file id, blake3; read-only, symlink, placeholder and sync-root checks; 2x free-space headroom); render and encode to `.autocrop-<ulid>.tmp` in the target directory and `sync_all`; verify the temp (re-read its blake3 against the encoder output, then a re-decode check); back up (reflink, else hardlink, else copy, with `manifest.json` and the `backups` row on a `synchronous=FULL` connection); journal `Committing`; compare-and-swap, then the atomic swap; fsync the parent directory on Unix; journal `Saved`. The security properties this section relies on:

- Backup and verify are unconditional for in-place writes: no `OutputSpec` value, setting or CLI flag skips them, and no backup is deleted before `Saved`.
- A source that changed between the plan and the swap (the compare-and-swap re-stat) aborts with `SourceChanged`: the backup is kept, no copy is written and nothing of the user's is deleted (the TOCTOU case).
- The swap is `rename` on Unix and `ReplaceFileW` (fallback `rename`) on Windows, with sharing-violation retries at 10, 20, 40, 80, 160, 320 and 640 ms and then a retryable `FileInUse`.
- Conversions (HEIC to JPG) and 1-to-N splits run the same protocol as a group: verify every temp, back up, no-clobber renames, then unlink the source. A group rolls forward only when every output verifies against the journalled hashes and is never left as a partial set. An unrelated file at an output name is never overwritten without a backup (`collision` is Rename, Skip or Replace, and Replace goes through backup-then-swap), and a Live Photo's paired MOV is untouched.
- Which sources may be replaced at all is decided in one place, `engine::FsPlan` (03 §3.2.3): animated, multi-page and multi-image files and formats this build cannot write back are never replaced, so the source stays byte-identical unless the user opts in per file to a new copy.
- Restore original works after saving and after closing the app. It verifies the backup hash and swaps by the same protocol; if the file no longer matches what we wrote, it offers restore anyway (the current version goes to a pre-restore backup), restore as a copy, or cancel.
- The backup store is shared by the GUI and CLI (`%LOCALAPPDATA%\AutoCrop`, `~/Library/Application Support/AutoCrop`, `$XDG_DATA_HOME/auto-crop`; the shell passes the engine's `AppPaths`) and is created user-only (ACL on Windows, 0700 on Unix). Retention defaults to 30 days (options 7, 30, 90, 365 or Never, plus a per-backup Keep pin); a purge runs at start, every 24 h and on Purge now, and deletes only files inside the store. The Backups panel warns at 10 GB used or under 5 GB free, and nothing unexpired is ever deleted silently. The Store build moves the store out of virtualised package data (8.4.1).
- Schema stability: every release opens, migrates and restores stores and journals written by earlier releases, 0.x included, and refuses a downgrade (8.2.2).
- Cloud sync: moving a source out of OneDrive, iCloud Drive or Dropbox can propagate as a deletion. Detect sync roots and placeholders, skip placeholders unless confirmed, and recommend Save as copy there. B3's default stays overwrite.
- Startup recovery replays the journal as 02 §2.7 specifies (an item that never reached `Committing` is re-queued after its temp is deleted; a `Committing` item rolls forward, is marked `Saved`, or fails with `SourceChanged` and keeps its backup) and sweeps `.autocrop-*.tmp` files. Gate: a crash-injection harness (fail points at every step, forced kills, ENOSPC) on all three OSes asserts the original bytes stay recoverable at every point and no group is left as a partial set.

### 8.6.4 Tauri hardening

- The webview holds no plugin permission: no `fs`, `shell`, `http`, `process`, `opener` or `dialog`. Pickers, drag-drop (`WindowEvent::DragDrop`), open-requests (Open with, second launches) and the save dialog all run in Rust, which registers paths and hands the UI opaque ids; commands take ids and range-checked serde types, never path strings. The output folder in `OutputSpec` `Copy{dir}` is a Rust-picked id or the default `<source folder>/AutoCrop/`. The raw-path `open_paths` command exists only in builds with the `e2e` Cargo feature (WebDriver tests), and CI fails a release build that enables it. External links go through one Rust command allowing only the project's GitHub URL prefix, which is why `opener` is not granted. Negative IPC tests (path strings, unknown ids, out-of-range values) belong to M3.64.
- One capability file (`main`), listed explicitly in `tauri.conf.json`. Release builds have devtools off and `withGlobalTauri` false; navigation and new windows are denied outside the app origin; `freezePrototype` is on. Evaluate Tauri's isolation pattern given the npm dependencies (measure IPC overhead in the spike).
- The CSP is the object below, PROVISIONAL (Tauri appends nonces and hashes for bundled assets, [docs](https://v2.tauri.app/security/csp/)). It is the single source: 02 §2.5 and the CI allow-list diff (M3.64) use it as written. Custom-protocol URLs differ per OS (`http://<scheme>.localhost` on Windows; verify), so both forms are allowed. `style-src-attr` goes if Svelte output does not need it:

```json
"csp": {
  "default-src": "'none'", "script-src": "'self'", "style-src": "'self'",
  "style-src-attr": "'unsafe-inline'",
  "img-src": "'self' blob: acimg: http://acimg.localhost",
  "connect-src": "ipc: http://ipc.localhost",
  "font-src": "'self'", "object-src": "'none'", "base-uri": "'none'",
  "form-action": "'none'", "frame-ancestors": "'none'"
}
```

- Custom URI scheme (`acimg`, name PROVISIONAL): `acimg://localhost/<launch-token>/<item-id>/<level>[/<z>/<x>/<y>]?g=<gen>` (served as `http://acimg.localhost/...` on Windows). The handler parses strictly (numeric ids, 128-bit random launch token), reads an in-memory tile store, and never maps a URL to a filesystem path. No directory listing, no CORS header, fixed `Content-Type`, `nosniff`, `Cache-Control: no-store`, 8 MB cap. Check in the spike that the webview's disk cache holds no tiles.

### 8.6.5 Supply chain

- Actions pinned by full SHA (Dependabot keeps them current), zizmor in CI, default `permissions: contents: read`, no `pull_request_target` on fork code, no secrets on fork PRs. Release secrets (updater key, App key, SignPath token, Flatpak repo key) live in the gated `release` environment. The golden-set publisher token lives only in the private repo (8.3.5), so the public repo holds no golden-set secret.
- cargo-deny advisories on every PR plus scheduled `cargo audit`; `cargo-auditable` embeds the dependency list in binaries; CycloneDX SBOM; `cargo-vet` optional post-1.0.
- Passkey or hardware-key 2FA on `WelFedTed` (SignPath also requires MFA) and offline recovery codes. `#![forbid(unsafe_code)]` in our crates that do not wrap C.

## 8.7 HEVC and patent risk (B12)

Facts: libde265 is bundled in all official builds and no legal opinion exists. The 2016 HEVC Advance exemption for software distributed to consumers may not have survived pool consolidation (trade press reports Access Advance took over Via LA's HEVC pool in December 2025; unverified), and Avanci and Sisvel exist. No verified FOSS safe harbour is known.

- README and SECURITY.md text: "HEIC images usually use the HEVC codec, which is covered by patents held through several licensing pools, and the status of free software decoders is unsettled. Official Auto Crop builds include the libde265 decoder. If you distribute Auto Crop or have concerns, use the no-hevc variant, which leaves HEVC to your operating system's codec where it has one. This is not legal advice."
- Build flag: Cargo feature `hevc` (on in every official build) and `-DAUTOCROP_HEVC=OFF` produce a `no-hevc` variant for distro packagers, or for an official fallback only if the owner records that decision (Assumption A-5). Without the libde265 plugin a HEVC-based HEIC fails with `HevcDecoderMissing`: there is no automatic fallback, and where the OS has a codec the UI and CLI point to `heic.engine = system`. AVIF still decodes; HEIF files with other codecs (JPEG, AVC, VVC, JPEG 2000) are unsupported in every build (03 §3.4.2). Never ship x265 or x264, offer no HEVC encoding (no HEIC export) and do not use vcpkg; our bundled libheif is the default HEIC backend on every OS.
- WIC and ImageIO stay optional fast paths behind the `HeicBackend` trait (`heic.engine = system`, opt-in until the parity gate passes, 03 §3.13 A3), moving the codec licence to the OS vendor. Windows then needs an HEVC extension, which bundling avoids and which we never buy (B15).
- Contingency if a claim arrives: the owner decides (A-5). The design allows the next release to make `no-hevc` the default with libde265 as a separately downloadable component.
- Legal read (Assumption A-5, 8.11): the $0 read (pool policies re-read with dates, any free-clinic reply, the gap disclosed) is due before the first published HEVC build if practicable and no later than 1.0; release tooling enforces the sequencing (M6.02, M6.03). Advisory tracking is in 8.2.3 (libde265 reportedly had 13 advisories in 2026, unverified).

## 8.8 Documentation, translations and onboarding

- **Docs.** mdBook 0.5.x on GitHub Pages: user guide, CLI reference (generated from clap), troubleshooting (SmartScreen, Gatekeeper, Linux graphics shims), architecture, security and privacy, replacing libheif, release runbook (`docs/release-runbook.md`), ADRs. The single `pages.yml` (8.3.2) publishes the book as `latest/` (newest tag) and `dev/` (main); lychee checks links.
- **Translations (B20).** The catalogue format is decided by the one-day i18n spike (02 §2.11). CI needs a pseudo-locale (expanded, accented), missing, unused and placeholder-mismatch checks (`xtask i18n-check`), and a pseudo-RTL screenshot test (a real RTL locale is a stretch goal). Start with PRs against the catalogue (same DCO and licence); move to Hosted Weblate once three or more languages are active (free-hosting terms need verifying). A language ships at 90% complete or more (PROVISIONAL) with a named maintainer; otherwise English is the fallback.
- **Onboarding.** `.devcontainer/` (Ubuntu 22.04, pinned toolchain, CMake, NASM, Ninja, WebKitGTK dev packages, Node LTS, prebuilt GHCR image) covers Linux only. On Windows and macOS, `cargo xtask doctor` checks MSVC or Xcode CLT, CMake, NASM, Ninja and Node and prints winget or brew commands. Other xtask commands: `build-native`, `check-native`, `check-models`, `check-dco`, `roadmap-check`, `fetch-corpus`, `fetch-models`, `synth`, `eval`, `calibrate`, `bench`, `licenses`, `i18n-check`, `package`. `good first issue` labels come from ROADMAP items.

## 8.9 Sustainability and funding

- Infrastructure is $0: Actions on a public repo, Pages, GHCR, and the private golden repo on the Free plan with the maintainer's own machine as its self-hosted runner (8.3.5). Scope is the main sustainability threat (R9); milestone gates are metrics, not dates.
- GitHub Sponsors is optional, and its payout setup depends on the maintainer's region (unverified). Personal-account sponsorships carry no fee; organisation ones up to 6%. `FUNDING.yml` is added once a profile exists. Receiving money does not itself break B15, but spending it on the $99 Apple fee would (open question 6). If funds appear: test hardware (a Linux touchscreen, an Intel Mac) first, then signing.
- Support: only the latest release gets security fixes, as with libheif. The release runbook (`docs/release-runbook.md`) and `MAINTAINERS.md` (accounts, keys and recovery inventory) let a second maintainer take over. Cadence: one 0.x preview per milestone, then PROVISIONAL monthly patches and quarterly minors. If abandoned, the permissive licence lets anyone fork.

## 8.10 Risk register

L = likelihood, I = impact (L/M/H). Owners are release phases: "Spike" is the week-1 work, "Harness" the benchmark and accuracy phase.

| ID | Risk | L | I | Mitigation | Owner |
|---|---|---|---|---|---|
| R1 | Linux WebKitGTK: DMABUF blank windows, pinch-zoom hijack (wry#544, tauri#13115 open), unverified touch; Slint fallback lacks OS folder drop on winit and RTL | H | H | Week-1 spike matrix (Ubuntu 24.04 and Fedora, Wayland and X11, NVIDIA and Intel, real touchscreen, 500-image folder drop); env shims; touch best-effort; UI-agnostic core; fallback must pass a folder-drop test; watch Tauri 3 and its CEF runtime as an escape hatch (the 2.x support window is unknown, roadmap X.19) | Spike; Linux preview |
| R2 | libheif and libde265 vulnerabilities (61 advisories in 2026 for libheif, a reported 13 for libde265, unverified), one largely unfunded maintainer, latest-only fixes; sandbox weaker than designed on some OS (old Landlock, AppContainer complexity, Flatpak nesting) | H | H | Sandboxed recycled workers, caps, visible sandbox level, own build patched within the 7-day SLA (8.2.3), native-watch, glue fuzzing, per-OS spike. Official builds always bundle libde265; bundled libheif is the default HEIC backend on every OS, and WIC or ImageIO stay an opt-in fast path behind the parity gate (03 §3.13 A3) | Spike (sandbox PoCs); first HEIC preview; macOS and Linux previews |
| R3 | HEVC patent claim against bundled libde265; no legal opinion | M | H | 8.7: notice, `no-hevc` variant, separable plugin, `heic.engine = system`, owner-decided contingency, $0 legal read (Assumption A-5) | Before the first published HEVC build if practicable; no later than 1.0 |
| R4 | Detection accuracy on real receipts and messy photos: no receipt benchmark, 8:1 strips on a 256x256 net, silent failures above 1% | M | H | Fused detectors, calibrated triage (B4), harness first, private golden set (B21), Strict as the default until the golden gate passes (A-3). Overwrite-by-default makes a silent failure costlier, so backup and Restore are mandatory | Harness; 1.0 gate |
| R5 | Speed budgets unmeasured and do not sum; IPC latency; HEIC lacks scaled decode | M | H | Harness before GUI, gungraun gate, proxy-first design, re-baseline after the Tier-M spike | Spike |
| R6 | Licence or provenance contamination (vcpkg x265, AGPL crates, GPL jpegxl-rs, NC weights, texture data, ImageNet backbones, AI code echoing GPL) | M | H | 8.1: cargo-deny from the first commit, `check-native`, OSI-only weights, provenance log, clean-room protocol | First public preview |
| R7 | Unsigned-build friction (SmartScreen, possibly Smart App Control, Gatekeeper, AV false positives), SignPath delay or rejection, fake copies under a generic name | H | M | SignPath applied right after 0.1; Store MSIX as trust route; documented workarounds; canonical channel list; checksums and attestations | First public preview; before 1.0 |
| R8 | Overwrite-by-default data loss (bug, crash, sync-client deletion, Store uninstall deleting virtualised backups) | M | H | The 02 §2.7 protocol (8.6.3), crash-injection gate, first-run explanation, Save as copy setting, hold-until-reviewed (B4); CLI backup, `restore` and one-time stderr notice (A-1); Store build keeps backups outside virtualised package data (8.4.1); every release opens older stores (8.2.2); uninstall, reinstall and Restore tested on every channel (M13.28) | Before any write release |
| R9 | Huge v1.0 (B10: dewarp, multi-item split, AVIF, JXL, CLI, shell integration, three OSes, ML training) for one maintainer | H | H | Gate-driven, not date-driven; per-feature exit criteria; N-quad and dense-grid-ready data model; gate-miss rule (Assumption A-4) | Continuous |
| R10 | Model supply: DocQuadNet NOASSERTION, ImageNet terms, no training repo or hardware, dewarp weight provenance | M | H | 8.1.5 audit and provenance log; own training repo; ship no uncleared weights; exceptions only by owner ADR (A-7) | Harness |
| R11 | Dependency maturity: ort is an RC; no maintained pure-Rust LSD, Sauvola, u8 Lanczos warp or G4 wrapper (a young `clahe` crate exists, unvetted); own libjxl and libwebp FFI; no macOS x86_64 ONNX Runtime binary; CMake and NASM friction | M | M | Pin versions; ort/rten spike (rten or `load-dynamic` for Intel Macs, else best-effort); backends behind traits; devcontainer and xtask | Spike |
| R12 | Store certification uncertainty (bundled decoder, full trust, helper process) and Store data loss (virtualised AppData is deleted on uninstall) | M | M | Hidden dry run early (M6.81); backup store outside virtualised package data, proven by an uninstall-reinstall-Restore test (8.4.1, M5.82, M13.60); a Store-only `no-hevc` build or any fee needs the owner's decision (Assumption A-6) | Early dry run; before the public listing |
| R13 | Flathub rejects an AI-built app (policy changed three times since May) | M | M | Own remote first; human-written manifest and PR; disclosure; submit after 1.0 | Post-1.0 |
| R14 | Supply-chain compromise (crates, npm, Actions, maintainer account) | L | H | 8.6.5 | First public preview |
| R15 | Updater key loss or compromise | L | H | Gated secret, offline backup, manual-reinstall rotation; updater only in direct builds | Updater release |
| R16 | Golden set leaks or overfits through CI | L | H | 8.3.5: images stay on the maintainer's encrypted disk, self-hosted runner attached to the private repo only, network-less container, no per-push, PR or fork triggers, allow-listed aggregates for slices with n >= 30, locked set once per RC, never trained on | Harness |
| R17 | Bus factor, account takeover or burnout | M | H | 2FA, recovery codes, release runbook (`docs/release-runbook.md`), `MAINTAINERS.md` (key and account inventory), narrow support scope, forkable licence | Continuous |
| R18 | Generic name: collision, weak trademark, poor discoverability | M | L | Tagline pairing, topics, USPTO/EUIPO check | Before 1.0 |

## 8.11 Open questions

Only genuinely open items; decisions already made are not repeated. Each item carries a proposed default so that no work waits on it, and the owner may veto any of them before the affected milestone starts (01 §1.7 holds the full A-1 to A-12 list). A-8 (golden-set hosting) and A-12 (release process) are stated in place, in 8.3.5 and 8.2.2.

1. **HEVC legal read at $0.** B12 requires a short legal read before 1.0, but B15 forbids paying. *Assumption A-5: Official builds always bundle libde265 (B12). The $0 legal read (pool policies re-read with dates, any free-clinic reply, gap disclosed) is due before the first published HEVC build if practicable, no later than 1.0; a `no-hevc`-only official release needs the owner's recorded decision. (owner may veto)*
2. **Store and the bundled decoder.** Does Store certification accept a bundled libde265, a helper process and the full-trust capability, and do backups stay safe there? *Assumption A-6: The Store build is verified early by a hidden dry run (M6.81) and keeps backups outside virtualised package data; a Store-only `no-hevc` build or any fee needs the owner's decision (B12, B15). (owner may veto)*
3. **Slint fallback thresholds and waivers.** Only if the week-1 spike triggers the fallback. *Assumption A-10: For B8, "cannot hold about 60 fps" on Linux means median frame > 22 ms after shims on the 24 MP proxy; Linux touch stays best-effort; switching to Slint needs the owner's waiver of B20 RTL layout and a licence decision. (owner may veto)* The licence choice is the non-OSI Royalty-free licence with attribution (possible SignPath conflict), reopening B2 for GPL-3.0, or rejecting the fallback (8.1.6).
4. **Gate-miss rule for v1.0 (B10).** What happens if dewarp, multi-item split, AVIF or JXL misses its exit gate at freeze. *Assumption A-4: A missed exit gate delays 1.0. Shipping the feature labelled Experimental or moving it to 1.x reopens B10 and is the owner's decision, put with measured numbers; nothing is lowered, cut or disabled silently. Proposed cut order if needed: dewarp, AVIF/JXL encoders, multi-item. (owner may veto)* Asked at the M0 wrap-up and stored as an ADR before the M10 gates (X.41).
5. **Weights exceptions.** ImageNet-pretrained backbones, the unconfirmed DocQuadNet licence and UVDoc weights. *Assumption A-7: Only OSI-licensed lineage ships. ImageNet-initialised, DocQuadNet-derived or UVDoc weights need an owner-granted exception (ADR, status `exception`); otherwise from-scratch or Track B weights ship. (owner may veto)*
6. **Funding versus B15.** If Sponsors income appears, may it pay for Apple notarisation, or does B15 stay absolute? Default until the owner decides: B15 stays absolute and any funds buy test hardware first (8.9).
