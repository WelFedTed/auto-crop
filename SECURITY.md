# Security policy

Auto Crop opens untrusted image files, so security reports are welcome and taken seriously.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private reporting: [Report a vulnerability](https://github.com/WelFedTed/auto-crop/security/advisories/new).

Targets (PROVISIONAL, one part-time maintainer): acknowledgement within 7 days, a plan within 30 days.

## Supported versions

The project is in the planning stage and has no releases yet. Once releases exist, the latest release is supported.

## Scope

In scope:

- Crashes, hangs, memory exhaustion and sandbox escapes triggered by a crafted image or folder.
- Bypasses of the pixel, memory and time limits applied to decoders.
- Data-loss bugs in the overwrite/backup pipeline (a lost or corrupted original).
- Unintended network access (Auto Crop makes no network requests by default).

Out of scope:

- An attacker who already runs code as the same user, or a compromised operating system.
- Vulnerabilities in third-party libraries that are fixed upstream and merely not yet bumped (report them upstream; we track advisories for our native libraries).

## HEVC and patents

Official builds are planned to bundle libde265 for HEIC/HEIF decoding. HEVC patent exposure is a legal question, not a vulnerability; see the README. A short legal read is planned before 1.0.
