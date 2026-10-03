# Architecture decision records

Numbered `NNNN-slug.md` records using [0000-template.md](0000-template.md). Every spike ends in an ADR with measured numbers and a **GO / NO-GO** line. The spike rules are in [spikes.md](spikes.md). Decisions B1-B21 live in the [decision log](../plan/00-decision-log.md); ADRs record the engineering outcomes that follow from them.

| ADR | Title | Status |
|---|---|---|
| [0001](0001-licence-name-app-id.md) | Licence, name and app ID | accepted |
| [0002](0002-lanczos-warp.md) | Lanczos perspective warp: own strip-wise u8 kernel | accepted |
| [0003](0003-long-receipt-strips.md) | Long narrow receipts: classical baseline failure rates (M4 input) | accepted (ML half skipped) |
| [0004](0004-native-libraries.md) | Native libraries: pinned CMake builds, libde265 plugin, libjpeg-turbo pin | accepted |
| [0005](0005-heic-binding.md) | HEIC binding: own thin libheif FFI, libde265 plugin, HeicBackend | accepted |
| [0006](0006-decode-sandbox.md) | Decode sandbox: level per OS, shared memory, fallbacks | accepted |
| [0007](0007-inference-backend.md) | Inference backend: ort load-dynamic, rten fallback | accepted |
| [0008](0008-codec-kernel-choices.md) | Codec kernel choices: decoder, resizer, JPEG encoder, linear-light rule | accepted |
| [0009](0009-heif-decode-backend.md) | HEIC, HEIF and AVIF decoding: libheif backend in codecs, dav1d, header walk | accepted |
