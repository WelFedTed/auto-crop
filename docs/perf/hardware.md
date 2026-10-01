# Test hardware and tiers

Roadmap: M0.47. Decisions: C1 (hardware floor), A-9 (hardware and testers). All performance numbers are PROVISIONAL until measured on named machines with the protocol in [spikes.md](../adr/spikes.md).

## Tiers

| Tier | Definition | Used for |
|---|---|---|
| **Tier-M** | 6 cores / 12 threads, AVX2 laptop, or Apple M1; 16 GB RAM; NVMe | Primary speed budgets (PLAN 7.1) |
| **Tier-L** | The C1 floor: 4 cores, 8 GB RAM, AVX2 (or a base Apple silicon model) | Budgets are 2x those of Tier-M; must work, not be fast |

## Machines (owner's, as of 2026-10-01)

| Machine | Role | Specs | Touch |
|---|---|---|---|
| Dev desktop "LUNCHBOX" | Day-to-day development, Windows measurements | Windows 11 Pro 10.0.26200, Intel Core i7-8700K (6C/12T, AVX2), 31.9 GB RAM, NVIDIA GeForce RTX 3060, NVMe SSD | No |
| Windows touchscreen device | Touch and pen gates (primary target) | **To record** (model, CPU, RAM, display scaling) | Yes |
| Apple silicon Mac | macOS preview, HEIC corpus, ImageIO checks | **To record** (chip, RAM, macOS version) | Trackpad |
| Intel MacBook | Intel Mac results (best-effort until 1.0) | **To record** (model, macOS version) | Trackpad |
| Linux machine | WebKitGTK spike (mouse, trackpad, keyboard only) | **To record** (distro, GPU, Wayland or X11) | **No touchscreen** |
| Flatbed scanner | Real flatbed-scan test images | **To record** (model, dpi) | n/a |

The dev desktop meets the Tier-M compute definition but is a desktop part, so its numbers are labelled "desktop, indicative" and are not a laptop result. Gates that need a Mac, a Windows touch device or a flatbed scanner stay open until measured on that hardware (A-9). Linux touch cells stay `UNMEASURED` because the Linux machine has no touchscreen.

## Rules

- Record CPU model, RAM, OS build, GPU, power plan (AC), toolchain and library versions with every measurement.
- A number from `macos-latest` or another shared CI runner is **relative only** (noisy), never an absolute budget result.
- The Apple silicon Mac belongs to the owner; schedule macOS spikes when it is available and record the date here.
