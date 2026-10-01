# Model weights policy

Decision references: B2 (licence), B7 (hybrid ML), A-7 (weights exceptions). Design: [PLAN 8.1.5](../plan/08-oss-release-security-risks.md). Roadmap: M0.24; enforced by `cargo xtask check-models` from M4.

## Rules

1. **Shipped weights need an OSI-approved licence** (Apache-2.0, MIT, BSD) and a training-data statement. This also satisfies signing programmes that forbid proprietary components.
2. **Banned:** non-commercial, research-only, share-alike terms that could propagate to weights, and unlicensed weights (no licence stated means all rights reserved). Examples: DocTr, DocGeoNet and DocEnTr (non-commercial), DIS5K and RMBG (non-commercial), DE-GAN (GPL-3.0).
3. **Initialisation sources count.** ImageNet-initialised backbones (MobileNetV3, LCNet), DocQuadNet-derived weights and UVDoc weights are **blocked** until the owner grants a recorded exception (decision A-7, ADR, status `exception`). Otherwise the project trains from scratch or uses its own synthetic-warp weights.
4. **Every weight, dataset and asset has a provenance row** (see [provenance/](provenance/README.md)). A release build may only ship weights whose row, and whose training-data rows, are `cleared` or `exception`.
5. **No binaries in the app repo, no Git LFS.** Weights are fetched at build time by pinned SHA-256 (`models.lock`) and hash-checked at load. There is no in-app model download. User-supplied models are a post-1.0 item.
6. **The private golden set is never used for training or tuning** (B21).
7. **Training code lives in the public repo `auto-crop-models`** (code, configs, dataset fetch-by-hash, evaluation and a model card per release).

## Process

- Before using a pretrained weight or dataset, add a `pending` row with the licence URL and retrieval date.
- Audit it (licence text, training-data lineage, redistribution terms); set `cleared`, `blocked` or `banned` with the reviewer and date.
- Only the owner can set `exception`, and only with an ADR explaining the residual risk.
- Upstream questions (for example the MakeACopy weights licence, roadmap M0.26) are logged with the date, the reply, or the documented absence of a reply.
