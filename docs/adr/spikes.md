# Spike charter and measurement protocol

Rules for every M0 (and later) spike. Merged before the first spike starts (ROADMAP M0.46).

## Charter

- **Throwaway code only.** Spike code lives under `spikes/`, never in the shipping workspace and never in default CI. It does not use `panic = "abort"`.
- **Time box.** Each spike has a time box in days (PROVISIONAL, from ROADMAP M0: GUI including Linux 5, plumbing 3, inference 3, Lanczos 3, HEIC + turbojpeg 4, sandbox 3, 8:1 strips 2). The ADR states its time box.
- **Overrun stops it.** A spike that exceeds its time box by more than 50% stops and is recorded as **inconclusive**, with what was learned and what would be needed.
- **Every spike ends in an ADR** ([template](0000-template.md)) with a results table and a **GO / NO-GO** line. An ADR without numbers, or without that line, is rejected.
- **Unrunnable cells are `UNMEASURED`**, never "pass". A cell that could not be run on real hardware says why and is re-run in the milestone that gains the hardware (assumption A-9).

## Measurement protocol

- At least **20 samples** per measurement; report **median and p95**.
- **Drop runs with more than 5% variation** (outliers from background load) and say how many were dropped.
- Laptops run on **AC power**, with a recorded power plan; record CPU model, RAM, OS build, toolchain and library versions.
- Pin library versions exactly and record SHA-256 for downloaded binaries and models.
- Report build flags (release profile, SIMD features) and thread counts (1 and the stated N).

## Results template

| Cell (device / OS / config) | Metric | N | Median | p95 | Dropped | Verdict (PASS / FAIL / UNMEASURED) | Notes |
|---|---|---|---|---|---|---|---|
