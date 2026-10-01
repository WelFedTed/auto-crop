# HEVC legal-read plan

Decision references: B12 (bundle libde265), B15 ($0 budget), A-5. Design: [PLAN 8.7](../plan/08-oss-release-security-risks.md). Roadmap: M0.27 (plan), M6.02 and M6.03 (sequencing). **This document is a plan, not legal advice.**

## Why

HEIC/HEIF images usually use the HEVC (H.265) codec, which is covered by patents licensed through several pools. The status of free-software decoders is unsettled and no verified free-software safe harbour is known. Official builds bundle libde265 (decision B12), so the project wants a documented, dated reading of the pool policies before it distributes an HEVC-capable build.

## Questions to answer

1. Which pools and licensors cover HEVC decoding today (Access Advance, Via LA and any successor, Avanci Video, Sisvel)? Re-check: a December 2025 report of a pool consolidation was unverified.
2. Do those pools' published policies exempt or price decode-only software that is **free of charge** and distributed over the internet? Record dated quotes and URLs.
3. Is a decoder shipped as a **separate, replaceable shared library** (LGPL libde265 loaded by libheif) treated differently from one compiled in?
4. What changes with a `no-hevc` variant that leaves HEVC to the operating-system codec (Windows WIC, macOS ImageIO)?
5. Which jurisdictions matter for distribution (where the maintainer lives, where users are, where releases are hosted)?
6. What disclosures are prudent (README and SECURITY.md wording, release notes)?

## $0 routes

- Re-read each pool's published policy and FAQ with the date and URL recorded; note anything unclear.
- Ask a free open-source legal clinic or foundation help desk if one accepts the question; record the date and any reply, or the absence of one.
- Ask the upstream libheif and libde265 maintainers how they handle distribution, as a data point only.
- Record the **gap** honestly: without a lawyer's opinion, the project documents its reading and the residual risk.

## Outputs

- A one-page, dated, non-advice summary in this folder (`hevc-read-summary.md`) with sources.
- The README and SECURITY.md paragraph (the README already has "HEIC and patents"; finalise the wording from the summary).
- A decision record if the reading changes anything: the owner decides whether to reopen B12 (A-5), for example by shipping `no-hevc` as the default with libde265 as a separate download.

## Timing

Due **before the first published HEVC-capable build if practicable, and no later than 1.0** (assumption A-5). Release tooling enforces the sequencing (roadmap M6.02, M6.03). A `no-hevc`-only official release needs the owner's recorded decision.

## Status

Not started. Update this section with dates as each step is done.
