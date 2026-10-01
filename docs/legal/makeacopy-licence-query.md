# MakeACopy DocQuadNet-256: licence query (draft, not yet sent)

Roadmap: M0.26. Decision references: B7, A-7. Policy: [model-weights.md](../policy/model-weights.md). Provenance row: `weights:makeacopy-docquadnet-256` (status `pending`).

**Status: DRAFT. Not sent.** Posting this is a public message in the owner's name, so it waits for the owner's approval. When sent, record the date, the venue and the reply (or the absence of one after 14 days, PROVISIONAL) here and in the provenance log. No reply means dev-only use of the weights and earlier from-scratch training in M4.

## Why we are asking

Auto Crop (https://github.com/WelFedTed/auto-crop) is a free, open-source, MIT OR Apache-2.0 desktop app for cropping and straightening scans, receipts and photos. We would like to use a small corner-detection model. MakeACopy's DocQuadNet-256 looks like a good fit, but GitHub reports its licence as NOASSERTION while the project describes it as Apache-2.0, so we cannot tell whether the weights may be redistributed.

## Draft message (GitHub issue or discussion on https://github.com/egdels/makeacopy)

> **Title:** Licence and training-data statement for the DocQuadNet-256 weights?
>
> Hi, and thanks for MakeACopy. We are planning an open-source (MIT OR Apache-2.0) desktop app and are evaluating DocQuadNet-256 as a document-corner detector.
>
> Could you clarify two things?
>
> 1. **Weights licence:** under which licence are the published DocQuadNet-256 weights distributed? GitHub currently shows NOASSERTION for the repository. If they are Apache-2.0, could you add a LICENSE file (or state it in the model card) so tools can detect it?
> 2. **Training data and initialisation:** what datasets and pretrained weights were used to train it (for example whether it was initialised from other published weights, and what backgrounds and documents were used)? We want to be sure every source allows redistribution of derived weights.
>
> We will not redistribute the weights until we hear back, and we are happy to credit MakeACopy in our notices. Thank you!

## After the reply

- Record the answer, the date and the repository commit and SHA-256 of the weights in the provenance log.
- Weights stay dev-only unless the licence and the training-data lineage both clear. Shipping DocQuadNet-derived weights additionally needs an owner-granted exception (assumption A-7) because the recipe may involve UVDoc pretraining and DTD backgrounds.
