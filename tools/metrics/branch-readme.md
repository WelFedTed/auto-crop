<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
<!-- SPDX-FileCopyrightText: 2026 Auto Crop contributors -->

# metrics (auto-crop-metrics/1)

This orphan branch holds the nightly accuracy aggregates of [WelFedTed/auto-crop](https://github.com/WelFedTed/auto-crop). It contains no source code and is written only by the `Nightly metrics` workflow (`.github/workflows/nightly-metrics.yml`).

- `data/<suite>/<predictor>/<UTC stamp>-<commit>.json`: the output of `auto-crop-eval publish` (schema `auto-crop-metrics/1`, or `auto-crop-metrics-multi/1` for the multi-item suite): aggregates and slices with n >= 30, never a per-image row, id, path or quad (golden-set policy B21, enforced by the `PublishableMetrics` type and a leak check).
- `data/.../<UTC stamp>-<commit>.timings.json`: wall-clock totals of that evaluation on the shared CI runner (noisy).
- `index.html`: a self-contained dashboard (no library, no network request) built from `data/` by `tools/metrics/build_dashboard.py`: gate table, nightly trends, latency against failure rate, per-slice trends.

The data is synthetic. It detects change between builds and never backs a real-world accuracy claim.

Nothing serves `index.html`. To publish it, enable GitHub Pages with source "Deploy from a branch", branch `metrics`, folder `/ (root)`: one setting, the owner's decision.
