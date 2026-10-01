# Research: Open-source publishing, licensing, CI/CD, packaging and signing  (key: oss)

## Summary
The name auto-crop is free today on GitHub (WelFedTed/auto-crop returns 404), crates.io, Homebrew, Snap, AUR and Chocolatey, but it is a generic phrase, so trademark protection is weak. All six OS/arch targets have native GitHub-hosted runners, free and unlimited for public repos. The hard part is trust. Free Windows signing exists (SignPath Foundation, plus Microsoft Store MSIX re-signing). macOS notarisation needs $99/yr. Since 2026-09-01 the main Homebrew cask tap rejects apps that fail Gatekeeper. Flathub adopted an AI-disclosure policy on 2026-05-29 that matters if the app is built with Claude Code. The GUI framework matters more to licensing than any codec: Slint's free tier is GPLv3, and libheif/libde265 are LGPL-3, which is fine for any open licence. Avoid the AGPL pure-Rust HEIC crate.

## Recommendation
License the app GPL-3.0-or-later (SPDX headers, REUSE, DCO sign-off, no CLA). It fits Slint's free tier, lets you port ideas and code from Scan Tailor Advanced (GPL-3.0), and deters closed re-skins. Use MIT OR Apache-2.0 only if you want maximal reuse and the GUI is not Slint. Ship arm64-first. Ship Windows through SignPath Foundation signing plus a Microsoft Store MSIX. Ship Linux as AppImage, deb/rpm and an own Flatpak remote, then try Flathub after 1.0 with honest AI disclosure. Ship macOS as an unsigned DMG plus your own Homebrew tap until you decide to pay $99/yr. Automate with release-plz (release PR and tag), then tauri-action or cargo-packager, then SHA256SUMS and actions/attest provenance. Use the app ID io.github.welfedted.AutoCrop everywhere. Default to zero network calls. Keep test data out of Git LFS.

## Key findings
- GUI choice drives licensing: Slint's free tier is GPLv3 (your own files may stay MIT/Apache, but the shipped whole is GPL, and no AboutSlint attribution is needed). Tauri, egui and iced are MIT/Apache; Qt is LGPL and needs dynamic linking. cargo-deny should enforce the allow-list.
- HEIC: libheif and libde265 are LGPL-3 libraries. This is fine for any open licence, because full source plus rebuild instructions satisfies relinking. libheif-rs 3.0.0 (MIT) wraps libheif 1.23.x. The pure-Rust heic crate is AGPL-3.0-only OR commercial and would force AGPL. HEVC patents are a real risk: Fedora keeps HEVC out of libheif, and Windows needs the paid $0.99 HEVC extension. Prefer OS decoders (macOS ImageIO, Windows WIC) with a bundled libde265 fallback.
- Windows zero-budget path: SignPath Foundation (free, OV-level, publisher shows as SignPath Foundation, project must already be released and OSI-licensed with no commercial dual licence) plus Microsoft Store MSIX (registration is free for individuals since Sept 2025; Store re-signs MSIX free). Azure Artifact Signing costs about $9.99/mo and individuals must be in the US or Canada. EV no longer gives instant SmartScreen trust, and reputation builds with downloads for every certificate type.
- macOS: notarisation requires the $99/yr Apple Developer Program. Sequoia removed the Control-click override, so unsigned users must use System Settings > Privacy & Security > Open Anyway. Homebrew disabled Gatekeeper-failing casks in the main tap from 2026-09-01 (own taps still work), and a main-tap cask needs 75 stars (225 if self-submitted). Intel Homebrew is Tier 3 now, and the macos-15-intel runner ends Aug 2027, so go arm64-first.
- Flathub's Generative AI policy (announced 2026-05-29) requires disclosing AI-generated code or packaging. Manifests must contain no AI content, AI must not open or automate submission PRs, and reviewers may reject based on the extent of AI use. Press reports call it a ban; the docs page says disclosure. Hand-write the manifest and disclose.
- CI is free: ubuntu-24.04-arm, windows-11-arm, macos-latest (arm64) and macos-15-intel all run on public repos at no cost. Cache limit is 10 GB per repo with 7-day eviction. actions/attest@v4 gives SLSA build L2, or L3 via a reusable workflow. Immutable releases lock tags and assets.
- Avoid WiX Toolset v4 and later for MSI: it carries an Open Source Maintenance Fee for revenue-generating use, which is ambiguous if you take donations. Prefer NSIS or MSIX.
- Test data: Git LFS gives 10 GiB storage and 10 GiB/month bandwidth on the free tier, and forks count against the owner. Use a small in-repo smoke set, pinned-hash download scripts, and release assets on a separate testdata repo (one file must be smaller than the Git LFS per-file limit on your plan). Verified licences: SmartDoc 2015 CC BY 4.0, CORD CC BY 4.0, raw.pixls.us CC0. Nokia HEIF conformance files have no licence, and SROIE is unclear.

## Risks
- Flathub AI policy: if Auto Crop is substantially AI-written, submission is at reviewer discretion, the manifest must be hand-written, and PRs must be human-opened; acceptance is not guaranteed.
- SignPath chicken-and-egg: the project must already be released and actively maintained before you apply, approval time is unpublished, and expect SmartScreen warnings for the first weeks or months.
- HEVC patent exposure when bundling libde265 for HEIC decoding (Fedora excludes it; Windows charges for its codec). Behaviour varies per OS, so document it and prefer OS decoders.
- Unsigned macOS builds hit Gatekeeper friction and are excluded from the main Homebrew cask tap, which may deter non-technical users.
- Licence lock-in: GPL-3.0 excludes the Apple App Store and permissive-only reuse, and relicensing later needs every contributor. Choose before the first external PR.
- The GUI framework (decided by another workstream) can change the licence outcome, and recent Tauri/egui/Slint releases already require Rust 1.92 to 1.95. MSRV promises would be brittle.
- Name is generic ('Auto Crop' is even a Microsoft 365 command name and many Mac App Store apps). Trademark protection is weak, search discoverability is poor, and I could not query USPTO/EUIPO, so a trademark check is unverified.
- Platform drift: macos-15-intel runners end Aug 2027, and Homebrew Intel support ends Sept 2027. Also unverified due to search-budget exhaustion: Open Collective fee, Scoop Extras criteria, and GitHub attestations being free on public repos (believed true, not re-confirmed).

## Options evaluated

### GPL-3.0-or-later — recommended
Strong copyleft for the whole app, with SPDX headers and DCO.
- licence: GPL-3.0-or-later
- status: Compatible with Slint's free GPLv3 tier, LGPL-3 libheif/libde265, OpenCV (Apache-2.0), ONNX Runtime (MIT), and SignPath/Flathub/Homebrew.
- pros: Lets you port ideas/code from Scan Tailor Advanced (GPL-3.0); Works with Slint free tier; Deters closed re-skins; Accepted by SignPath, Flathub and Homebrew
- cons: Incompatible with Apple App Store distribution; Permissive-only projects cannot reuse the code; Relicensing later needs every contributor, so use DCO not CLA; Cannot include GPL-2.0-only code

### MIT OR Apache-2.0 — viable
Rust-ecosystem default dual licence, with an Apache patent grant.
- licence: MIT OR Apache-2.0
- status: Fine with Tauri, egui, iced, image, ort, tract and libheif-rs. Incompatible with Slint's free tier as a whole binary, since the shipped whole becomes GPL.
- pros: Maximum reuse and contributor comfort; Allows an App Store build later; Patent grant
- cons: Closed forks and paid re-skins are legal; Cannot borrow GPL code; Blocks the Slint-free-tier route unless the combined binary is GPL

### MPL-2.0 — viable
File-level copyleft, suitable for a reusable auto-crop-core engine crate.
- licence: MPL-2.0
- status: Used by rust-lang/mdBook and galfar/deskew. OSI-approved, so SignPath-eligible.
- pros: Middle ground: improvements to files stay open; Combines with GPL and Apache code
- cons: Less familiar to contributors and to Rust tooling; Does not solve the Slint question; File-level obligations are tricky for a solo maintainer

### AGPL-3.0 — avoid
Network-copyleft variant of GPL.
- licence: AGPL-3.0-only
- status: Only pulled in if you adopt the AGPL heic crate or dssim; otherwise unnecessary for an offline desktop app.
- pros: Closes the SaaS loophole
- cons: Irrelevant to an offline desktop app; Scares packagers and companies; No practical benefit over GPL-3.0 here

### SignPath Foundation (Windows signing) — recommended
Free OV-level Authenticode signing for qualifying OSS via GitHub Actions.
- licence: Service; project must be OSI-licensed, no commercial dual licence, no proprietary components.
- status: Active. The certificate is issued to SignPath Foundation. The project must show a Code signing policy page and already be released.
- pros: Zero cost; No hardware token; GitHub Actions trusted-build integration; Named in Microsoft's own signing-options doc
- cons: Publisher name is SignPath Foundation; Per-release approval step; Reputation accrues slowly; Approval time not published

### Azure Artifact Signing (ex Trusted Signing) — fallback
Managed Microsoft signing, Basic about $9.99/mo, 5,000 signatures per month.
- licence: Paid service
- status: GA. Organisations in the US, Canada, EU, UK, Australia, NZ, Japan, Korea, Singapore, Switzerland, Norway and Israel. Individuals US/Canada only. No instant SmartScreen trust.
- pros: Cheap; No token; Certificate carries your own name; Tauri documents it
- cons: Not zero-budget (needs an Azure subscription); Geographic limits for individuals; Identity validation takes 1 to 20 business days

### Apple Developer ID + notarisation — viable
$99/yr program that enables signing, notarisation and Homebrew main-tap casks.
- licence: Paid ($99/yr). Fee waiver only for nonprofits, schools and governments.
- status: Only route to a warning-free macOS install. Unsigned apps need Open Anyway in System Settings.
- pros: Smooth install; Eligible for homebrew/cask once notable; Enables in-app updater trust
- cons: $99 per year; No OSS waiver for individuals; Needs secrets and notarytool in CI

### Microsoft Store (MSIX) — recommended
Free Store listing; Microsoft re-signs MSIX so users see no SmartScreen warning.
- licence: Free for individual developers since Sept 2025.
- status: MSI/EXE submissions must be signed by you with a Trusted Root CA cert; only MSIX is re-signed.
- pros: Free signing and trust; Auto-updates via Store; Discoverability
- cons: MSIX packaging effort for a native Rust app (about 3 days); Certification review per release; Store-managed updates conflict with an in-app updater

## Deliverable
## Recommended licence, release and distribution plan

**1. Licence**
GPL-3.0-or-later for the whole repo, with SPDX headers, REUSE compliance and DCO sign-off (no CLA). Rules:
- No AGPL crates (`heic`, `dssim`) and no GPL-2.0-only code.
- Only OSI-licensed model weights (SignPath allows no proprietary components).
- No commercial dual licence (SignPath).
- Generate THIRD_PARTY_NOTICES with cargo-about.
- Enforce all of this with cargo-deny in CI.
- Fallback: MIT OR Apache-2.0 if the GUI is not Slint and you accept closed forks.

**2. Repo (github.com/WelFedTed/auto-crop)**
- Root: LICENSE, README (GIF, install table, Code signing policy, privacy line), CONTRIBUTING (DCO, AI-use statement), CODE_OF_CONDUCT (Contributor Covenant), SECURITY.md (private advisories), CHANGELOG (generated), deny.toml, rust-toolchain.toml.
- .github: issue forms, PR template, CODEOWNERS, FUNDING.yml, dependabot.yml (cargo and actions), workflows.
- docs/: mdBook 0.5.x, deployed to GitHub Pages.
- Ruleset on main: PR required, green CI, linear history, no force-push.
- Conventional Commits go to a release-plz release PR, which creates a tag. The tag triggers the build workflow (tauri-action, or cargo-packager for a native GUI). Note: tags made with GITHUB_TOKEN do not trigger workflows, so use a PAT or GitHub App token.
- Enable immutable releases.
- MSRV: pin current stable (1.98.1) in rust-toolchain.toml and bump quarterly.

**3. CI (free, unlimited on public repos)**
- Matrix: ubuntu-22.04 (glibc floor), ubuntu-24.04-arm, windows-2025, windows-11-arm, macos-latest (arm64), macos-15-intel. Universal macOS is lipo of two builds; retire Intel in 2027.
- Jobs: fmt, clippy -D warnings, nextest, cargo-deny, zizmor, actions pinned by SHA.
- Accuracy gate on corpus (quad IoU, angle error). It is deterministic, so it can fail the build.
- Speed: track with github-action-benchmark or Bencher. Gate only on instruction counts (gungraun on Linux), never wall-clock on shared runners.
- Release: SHA256SUMS, actions/attest@v4 provenance, CycloneDX SBOM.

**4. Packaging** (solo days)
| OS | Channels (effort) |
|---|---|
| Windows | portable zip (0.5), NSIS setup (1), MSIX to Store (3), winget via komac (0.5), Scoop (0.5); skip WiX v4+ (OSMF) |
| macOS | universal DMG (1), own Homebrew tap (0.5); main cask only if notarised and 75+ stars |
| Linux | AppImage (1), deb+rpm (1), own Flatpak remote (2), AUR -bin (0.5), Flathub (3 plus review), Snap later |

**5. Signing**
SignPath Foundation for the direct Windows download, and Store MSIX for the Store. Apply to SignPath after the first public release. macOS is unsigned until you approve $99/yr, and the README documents Open Anyway with screenshots.

**6. Updates and privacy**
No network calls by default, and models are bundled. There is no telemetry or crash upload. "Check for updates" is manual, with an opt-in weekly check against the GitHub Releases API. Any in-app updater (Tauri's works for AppImage, NSIS and macOS bundles) is compiled out of Store, Flatpak, deb/rpm and winget builds.

**7. Test corpus**
- Repo: synthetic generator with exact ground truth, plus CC0 images, at most 5 MB.
- Download: `cargo xtask fetch-corpus` with pinned SHA-256 from SmartDoc (CC BY 4.0), CORD (CC BY 4.0) and raw.pixls.us (CC0).
- Own `auto-crop-testdata` repo for release assets.
- No Git LFS.

**8. Funding and community**
GitHub Sponsors (0% fee for personal accounts), Open Collective optional. Add good-first-issue labels and a devcontainer.

**9. Name and ID**
The name is free on GitHub, crates.io (`auto-crop`), Homebrew, Snap, AUR and Chocolatey. Taken: PyPI `auto-crop` and `autocrop`, npm `autocrop`, crates.io `autocrop` (an unrelated screenshot cropper, created 2026-09-05). Use ID `io.github.welfedted.AutoCrop` for bundle ID, AppUserModelID and Flatpak; it is permanent.

**10. Phases**
- P0 (2-3d): repo, licence, CI skeleton, name and ID.
- P1 (5-7d): alpha, unsigned artifacts, attestations, portable/AppImage/DMG.
- P2 (7-10d): SignPath, winget, Scoop, tap, own Flatpak, AUR.
- P3 (10-15d): docs site, MSIX Store, Flathub with AI disclosure, updater, Sponsors.
- P4: macOS notarisation if funded, drop Intel Mac in 2027.
Total about 5 to 7 weeks part-time.

## Decision-critical claims (as researched)
- Slint can be used free under GPLv3 for open-source desktop apps; your own files may stay MIT/Apache but the combined work must be GPL; the Royalty-free tier is for proprietary apps and needs AboutSlint attribution. [https://github.com/slint-ui/slint/blob/master/LICENSE.md]
- libheif and libde265 libraries are LGPL-3.0; libheif-rs 3.0.0 is MIT and wraps libheif 1.23.x; the pure-Rust heic crate is AGPL-3.0-only OR a commercial licence. [https://github.com/strukturag/libheif]
- SignPath Foundation offers free signing only for OSI-licensed projects without commercial dual licensing or proprietary components, that are already released and actively maintained; certificate publisher is SignPath Foundation; a Code signing policy page is required. [https://signpath.org/terms]
- Azure Artifact Signing (formerly Trusted Signing) is about $9.99/month Basic; individuals only in the US and Canada; no instant SmartScreen trust; Microsoft Store re-signs MSIX for free; EV certificates no longer bypass SmartScreen. [https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options]
- Microsoft Store developer registration is free for individual developers (since Sept 2025). [https://blogs.windows.com/windowsdeveloper/2025/09/10/free-developer-registration-for-individual-developers-on-microsoft-store/]
- From 2026-09-01 Homebrew disables casks that fail Gatekeeper checks in the main homebrew/cask repo (own taps remain possible); new casks need 30 forks/30 watchers/75 stars, and self-submissions 90/90/225. [https://github.com/orgs/Homebrew/discussions/6334]
- Flathub's Generative AI policy requires disclosure of AI-generated code or packaging; manifests must contain no AI-generated content; AI must not open or automate submission PRs; reviewers may reject based on the extent of generated material. [https://docs.flathub.org/docs/for-app-authors/requirements]
- Standard GitHub-hosted runners (including ubuntu-24.04-arm, windows-11-arm, macos-15-intel) are free and unlimited on public repos; macos-15-intel is the last Intel macOS image and becomes unavailable in Aug 2027. [https://docs.github.com/en/actions/reference/runners/github-hosted-runners]

## Researcher questions for user
- Which licence stance do you want: strong copyleft (GPL-3.0-or-later), permissive (MIT OR Apache-2.0), or file-level copyleft (MPL-2.0)? — It is effectively irreversible once outside contributors join. It decides whether you can use Slint's free tier and port code from GPL projects like Scan Tailor Advanced, and whether an Apple App Store build is ever possible. (default: GPL-3.0-or-later)
- Are you willing to spend a small budget, and on what? The main candidates are $99/yr for Apple Developer ID plus notarisation, and roughly $10-15/yr for a domain. — Without the $99 the macOS build shows a Gatekeeper warning and cannot go in the main Homebrew cask tap. A domain would let you choose a reverse-DNS ID that is independent of your GitHub account. (default: Zero budget for the alpha and beta, then decide on the $99 before 1.0 (or when Sponsors covers it))
- Will the code be substantially AI-assisted (for example written with Claude Code), and are you comfortable disclosing that publicly? — Flathub now requires disclosure, forbids AI in manifests and AI-opened PRs, and reviewers may reject. That affects whether Flathub is a viable Linux channel. (default: Disclose openly and try Flathub after 1.0, with own Flatpak remote as fallback)
- Should the repo live under your personal account (WelFedTed) or a new GitHub organisation? — The GitHub owner is baked into the Flathub/Flatpak app ID and verification, and the ID is effectively permanent. An org is cleaner for multiple maintainers; GitHub redirects handle later repo moves but the ID stays. (default: Personal account now, io.github.welfedted.AutoCrop)
- How should updates work? — It sets the privacy stance, whether an updater is compiled in, and how much the release pipeline must sign (updater keys). (default: Manual button plus opt-in weekly notify-only check; package managers handle installs)

## INDEPENDENT VERIFICATION (skeptic) — overrides the researcher where they differ
- [PARTLY-TRUE] 1. Slint free under GPLv3 for OSS desktop apps; own files may stay MIT/Apache but combined work must be GPL; Royalty-free tier is for proprietary apps and needs AboutSlint attribution.
  CORRECTION: The GPLv3 half is confirmed: your files can stay MIT/Apache, the combined work is GPL. The Royalty-free half is oversimplified. LICENSE.md calls it 'proprietary', but the licence text grants use in any Desktop/Mobile/Web Application with AboutSlint or a web badge, and the Slint FAQ says it does not restrict how you license your app, explicitly including MIT open source. So an MIT OR Apache-2.0 app can use Slint free under Royalty-free. The deliverable's claim that MIT/Apache is incompatible with Slint's free tier is too strong. Caveats: Royalty-free is not OSI/copyleft, forkers must choose their own Slint licence, and it may conflict with SignPath's 'no proprietary components' rule. Embedded is excluded.
- [CONFIRMED] 2. libheif and libde265 are LGPL-3.0; libheif-rs 3.0.0 is MIT and wraps libheif 1.23.x; pure-Rust heic crate is AGPL-3.0-only OR commercial.
  CORRECTION: Both libheif and libde265 ship an LGPL v3 COPYING; their sample apps are MIT. libheif-rs 3.0.0 (2026-08-18) is MIT and depends on libheif-sys 5.3.1+1.23.1. It adds a v1_23 feature, and the minimum supported libheif is 1.17. The latest libheif release is v1.23.5 (2026-09-21). crates.io heic 0.1.6 is 'AGPL-3.0-only OR LicenseRef-Imazen-Commercial'. Gap: the heic README warns that HEVC/HEIF may be covered by third-party patents (Access Advance) and Imazen grants copyright permissions only. The plan does not address HEVC patent exposure for a bundled decoder.
- [CONFIRMED] 3. SignPath Foundation: free signing only for OSI-licensed projects without commercial dual licensing or proprietary components, already released and actively maintained; publisher is SignPath Foundation; Code signing policy page required.
  CORRECTION: Confirmed on signpath.org/terms. Requirements: an OSI licence with no commercial dual-licensing, no proprietary components, active maintenance, and already existing in releasable form with documented functionality. The certificate is issued to SignPath Foundation, and the project must publish a code signing policy on its homepage with the exact line 'Free code signing provided by SignPath.io, certificate by SignPath Foundation'. That policy must also list team roles and give a privacy statement. SignPath keeps discretion to reject on reputation and control grounds. Microsoft's Learn doc also names it.
- [CONFIRMED] 4. Azure Artifact Signing (formerly Trusted Signing) ~$9.99/month Basic; individuals US/Canada only; no instant SmartScreen trust; Store re-signs MSIX free; EV no longer bypasses SmartScreen.
  CORRECTION: All confirmed. Microsoft's signing-options doc (updated 2026-08-29) gives ~$9.99/month, individuals US/Canada only, no instant SmartScreen trust, free Store re-signing of MSIX only (MSI/EXE must be signed by you), and EV equal to OV since 2024. The Azure pricing page hides dollar figures but shows Basic at 5,000 signatures/month and Premium at 100,000. The Windows doc lists organisations only as US/CA/EU/UK. The Azure quickstart is newer and broader (adds AU, NZ, JP, KR, SG, CH, NO, IL), so the researcher's longer list is right. Extras: a paid Azure subscription is required (no free, trial or sponsored ones), individual validation needs an Individual-type billing account, and validation takes 1 to 20 business days.
- [CONFIRMED] 5. Microsoft Store developer registration is free for individual developers (since Sept 2025).
  CORRECTION: Confirmed by the Windows Developer Blog post of 2025-09-10: registration is free for individuals in nearly 200 markets, with no credit card, but with government-ID and selfie verification. Microsoft's code-signing doc also points to the free storedeveloper.microsoft.com registration.
- [PARTLY-TRUE] 6. From 2026-09-01 Homebrew disables casks failing Gatekeeper in main homebrew/cask (own taps remain); new casks need 30 forks/30 watchers/75 stars, self-submissions 90/90/225.
  CORRECTION: The date and scope are confirmed. Homebrew's discussion says it will disable Gatekeeper-failing casks only in the main cask repo, and cask audit code says homebrew/cask requires signed and notarized apps. The thresholds are misread. Homebrew's Package Acceptance Policy says at least 30 forks, 30 watchers OR 75 stars (any one). For a self-submission by the repo owner it is 90 forks, 90 watchers OR 225 stars. A repository under 30 days old is normally ineligible. Since the maintainer would self-submit, the practical bar is 225 stars (or 90 forks/watchers), not 75. Maintainers may grant exceptions.
- [PARTLY-TRUE] 7. Flathub Generative AI policy requires disclosure; manifests must contain no AI-generated content; AI must not open or automate submission PRs; reviewers may reject based on extent of generated material.
  CORRECTION: The policy content matches the live text. It also bans AI-generated commit messages, descriptions and replies on submissions, and allows rejection without further review. The date and stability are wrong. The 2026-05-29 change (PR #612) reworded a blanket LLM ban. The disclosure-based policy merged on 2026-09-04 (PR #641). On 2026-09-21 Flathub restored an explicit ban on LLM content in manifests. The rules have flipped twice in four months. A maintainer comment on PR #612 said 'vibed apps' were not being allowed. For a Claude Code-built app, expect real rejection risk and hand-write the manifest.
- [PARTLY-TRUE] 8. Standard GitHub-hosted runners (incl. ubuntu-24.04-arm, windows-11-arm, macos-15-intel) are free and unlimited on public repos; macos-15-intel is the last Intel macOS image and is unavailable from Aug 2027.
  CORRECTION: Free and unlimited for public repos is confirmed, and the arm64 Linux and Windows runners are explicitly free there. Two corrections. (1) Minutes are unlimited but concurrency is capped: on the Free plan, 20 concurrent jobs and 5 concurrent macOS jobs. (2) macos-15-intel is no longer the last Intel image. macOS 26 on Intel went GA on 2026-02-26 as macos-26-intel, and the docs list it among standard runners next to macos-15-intel. The 'last x86_64 image, until August 2027' wording comes from a Sept 2025 announcement and was not restated for macOS 26. Whether Aug 2027 still applies to macos-26-intel is unverified. macos-latest has been macOS 26 arm64 since mid-2026.

### Other errors spotted by skeptic
- Summary says Flathub 'adopted an AI-disclosure policy on 2026-05-29'. That date was the blanket-ban rewording. Disclosure policy: 2026-09-04. Manifest ban restored: 2026-09-21.
- Deliverable packaging table says 'main cask only if notarised and 75+ stars'. For a self-submission the bar is 225 stars (or 90 forks/watchers), and the repo must be over 30 days old.
- Recommendation says to use MIT OR Apache-2.0 only if the GUI is not Slint. Slint's Royalty-free licence allows MIT/Apache apps free with AboutSlint attribution, but it is non-OSI and may conflict with SignPath's 'no proprietary components' rule.
- 'GitHub Sponsors (0% fee for personal accounts)' is imprecise. Sponsorships from personal accounts carry no fee. Sponsorships from organisations cost up to 6% (3% card, 3% GitHub; invoice billing removes the card part).
- 'Skip WiX v4+ (OSMF)' overstates the fee. The Open Source Maintenance Fee applies only to organisations with more than $10,000 annual revenue. WiX v7 does enforce EULA acceptance, which is the real friction.
- raw.pixls.us is not uniformly CC0. Some older samples (from rawsamples.ch) are not, so the fetch-corpus script must filter to CC0-only samples.
- Immutable releases lock tag and assets once published. The release workflow must create a draft, upload every asset, then publish. Uploading after publish will fail (tauri-action drafts by default; release-plz and custom steps need care).
- Rust 1.98.1 is current stable today, but 1.99 is due on 2026-10-01. The plan also conflates pinning a toolchain in rust-toolchain.toml with declaring an MSRV (rust-version in Cargo.toml).
- The matrix uses ubuntu-22.04 for the glibc floor but ubuntu-24.04-arm for arm64. ubuntu-22.04-arm exists, so use it to keep the AppImage glibc floor consistent.
- Scan Tailor Advanced (GPL-3.0) was last pushed in Sept 2023 and looks unmaintained, so it is an idea source, not a live upstream. Original Scan Tailor is archived with no SPDX licence detected.
- Name availability confirmed: GitHub WelFedTed/auto-crop returns 404; crates.io auto-crop, AUR, Homebrew formula and cask, Chocolatey and Snap are free; PyPI auto-crop and autocrop are taken; npm autocrop is taken; crates.io autocrop was created 2026-09-05. The rest of the summary checked out (mdBook 0.5.4, actions/attest v4.2.2, gungraun = renamed iai-callgrind, SmartDoc and CORD CC BY 4.0).

## Sources
- https://github.com/slint-ui/slint/blob/master/LICENSE.md
- https://github.com/slint-ui/slint/blob/master/FAQ.md
- https://github.com/strukturag/libheif
- https://github.com/strukturag/libde265
- https://crates.io/crates/heic
- https://crates.io/crates/libheif-rs
- https://signpath.org/terms
- https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart
- https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options
- https://blogs.windows.com/windowsdeveloper/2025/09/10/free-developer-registration-for-individual-developers-on-microsoft-store/
- https://github.com/orgs/Homebrew/discussions/6334
- https://brew.sh/2025/11/12/homebrew-5.0.0
- https://docs.brew.sh/Package-Acceptance-Policy
- https://docs.flathub.org/docs/for-app-authors/requirements
- https://linuxiac.com/flathub-now-rejects-ai-assisted-apps-and-submissions/
- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://github.blog/changelog/2025-09-19-github-actions-macos-13-runner-image-is-closing-down/
- https://docs.github.com/en/billing/concepts/product-billing/git-lfs
- https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-large-files-on-github
- https://docs.github.com/en/actions/reference/workflows-and-actions/dependency-caching
- https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases
- https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/use-artifact-attestations
- https://docs.github.com/en/sponsors/getting-started-with-github-sponsors/about-github-sponsors
- https://v2.tauri.app/plugin/updater/
- https://v2.tauri.app/distribute/
- https://v2.tauri.app/distribute/sign/windows/
- https://github.com/wixtoolset/wix
- https://zenodo.org/records/1230218
- https://github.com/clovaai/cord
- https://raw.pixls.us/
- https://learn.microsoft.com/en-us/windows/package-manager/package/repository
- https://cybersecuritynews.com/macos-gatekeeper/
- https://github.com/ScoopInstaller/Extras
- https://docs.flathub.org/docs/for-app-authors/verification
- https://github.com/ScanTailor-Advanced/scantailor-advanced
- https://github.com/axodotdev/cargo-dist
- https://github.com/release-plz/release-plz
- https://github.com/tauri-apps/tauri-action