# 0001 - Licence, name and app ID

- **Status:** accepted
- **Date:** 2026-10-01
- **Roadmap items:** M0.01, M0.02, M0.16, M0.75
- **Decision log links:** B2, B16

## Context

The project needs a licence that fits a free, open-source desktop app with permissive-friendly dependencies, a name, and a permanent application identifier (it becomes the macOS bundle ID, Windows AppUserModelID and Flatpak ID).

## Decision

- **Licence:** MIT OR Apache-2.0 (dual), copyright holder `WelFedTed`. No AGPL, GPL or non-commercial dependencies or model weights; LGPL only as separate, dynamically linked, replaceable libraries.
- **Name:** "Auto Crop", always shown with the tagline "the offline batch fixer for scans, receipts and photos". The name is generic; availability was checked at research time (GitHub `WelFedTed/auto-crop` and crates.io `auto-crop` free; PyPI, npm and crates.io `autocrop` taken) and must be re-checked before the first crates.io release.
- **Repository:** `github.com/WelFedTed/auto-crop`, public, personal account.
- **App ID:** `io.github.welfedted.AutoCrop`, defined once in `identity.toml`.

**GO:** adopt the above.

## Consequences

`cargo-deny` and `cargo-about` enforce the dependency policy from the first workspace commit. The `auto-crop` crate name is reserved at the first release (assumption A-12); no `auto-crop-core` is published. A domain-based app ID would cost money and is not chosen (budget B15).
