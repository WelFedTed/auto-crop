# Contributing to Auto Crop

**Status: planning stage.** There is no source code yet and code contributions are not being accepted. Feedback on the plan is welcome through [issues](https://github.com/WelFedTed/auto-crop/issues) and [discussions](https://github.com/WelFedTed/auto-crop/discussions). Start with [PLAN.md](PLAN.md) and [ROADMAP.md](ROADMAP.md).

This file describes the rules that will apply once development starts.

## Ground rules

- **Licence:** everything is dual-licensed MIT OR Apache-2.0. Do not add AGPL, GPL or non-commercial dependencies or model weights; LGPL only as separate, dynamically linked, replaceable libraries. `cargo-deny` enforces this.
- **Clean-room:** do not copy code from GPL, AGPL or non-commercial projects. Algorithms from such work are reimplemented from the papers.
- **DCO, no CLA:** sign off every commit (`git commit -s`), certifying the [Developer Certificate of Origin](https://developercertificate.org/).
- **Conventional Commits** for messages (`feat:`, `fix:`, `docs:`, `chore:` ...).
- **Every file** carries an SPDX licence header (MIT OR Apache-2.0) and copyright line, or is covered by `REUSE.toml`; `reuse lint` must pass.
- **Privacy:** no telemetry and no network calls unless the user opts in.
- **Safe writes:** user files are only written through the safe-write path (verified temp output, backup, atomic replace). See [PLAN 2.7](docs/plan/02-architecture.md).

## Building

Not yet applicable. `cargo xtask doctor` lists what is missing from the native toolchain (CMake, NASM, Ninja, Node) and from the dev tools used by the oracles and profiling (Python 3.12, Tesseract 5, ImageMagick, unpaper, Valgrind on Linux) and prints the install command for each; `doctor --strict` also fails on missing dev tools, which is what the devcontainer runs. It never installs anything. The CI policy guards (`cargo xtask ci-guards`) are described in [docs/policy/ci-guards.md](docs/policy/ci-guards.md). Note that "no C toolchain needed" is false for this project.

## Test data and personal images (B21)

- **No real personal photos or documents** in this repository: not in commits, fixtures, pull requests, issues, screenshots, logs or CI output. That includes your own receipts, IDs, letters and phone photos, even redacted.
- Tests use **synthetic data** (generated from a seed, see `cargo xtask synth`) or **public datasets with a permissive licence** that have a row in the [provenance register](docs/provenance.md) and, for datasets and weights, in the [provenance log](docs/policy/provenance/README.md). Fetch datasets by pinned SHA-256; never commit them and never use LFS.
- **Aggregate-only metrics.** Accuracy results are published as counts, means, percentiles and per-slice numbers for slices with enough images, never per-image rows, paths, thumbnails or OCR text. A detection failure you want reported goes into an issue as a description or a synthetic reproduction, not as the image.
- The **real hand-labelled golden set stays private** on the maintainer's machine and runs only in a private, secret-gated workflow, never on fork pull requests. See the [golden-set policy](docs/testing/golden-set.md). Golden images are never used for training or tuning.
- A new tool, font, fixture, dataset or weight needs its row in [docs/provenance.md](docs/provenance.md) in the same pull request (`cargo xtask provenance` checks the native libraries, pinned Python tools and fonts).

## Ticking the ROADMAP

[ROADMAP.md](ROADMAP.md) is a living checklist. Tick a box (`- [x]`) in the same pull request or commit that implements **and verifies** the item; never tick a GATE item by inference; never renumber or delete an ID; run `cargo xtask roadmap-check` once it exists. New work is appended under the right milestone with the next free ID.

## AI assistance

This project is planned and written with AI assistance (Claude). All output is reviewed by the maintainer. No code is knowingly copied from GPL, AGPL or non-commercial projects; algorithms derived from such work are reimplemented clean-room from papers. If you use AI tools for a contribution, say so in the pull request and make sure you can explain and license every line.

## Branches

The maintainer pushes directly to `main`. External contributors use forks and pull requests.

## Conduct

Be kind and constructive: see the [Code of Conduct](CODE_OF_CONDUCT.md).
