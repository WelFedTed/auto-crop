# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Auto Crop synthetic test-data generator (ROADMAP M1.30 to M1.35).

Renders known-text pages and receipts, photographs them with a pinhole camera, and writes images
plus a ``manifest.jsonl`` (v1, the accuracy harness format) whose ground truth is analytic. It shares
no code with the application: it never imports or runs anything from ``crates/``.
"""

GENERATOR = "synth-py/1"
MANIFEST_VERSION = 1
