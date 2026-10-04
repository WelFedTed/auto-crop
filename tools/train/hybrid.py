# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Offline hybrids of the learned detector and the classical one, as JSON-lines prediction files.

    python hybrid.py --manifest M --learned learned.jsonl --classical classical.jsonl --out-dir DIR [--thr 0.9]

``classical.jsonl`` comes from ``cargo run --release -p auto-crop-eval --example dump_classical``
(answer, state and the best-ranked candidates the classical detector scored). Writes one file per
variant, each scorable with ``auto-crop-eval run --predictor jsonl:FILE``:

  classical            the classical answer as is
  learned              the learned answer as is
  fallback             classical when it says good, else learned (state from the learned confidence)
  snap                 learned quad replaced by the classical candidate it agrees with (IoU >= 0.8: the
                       classical line fit then supplies the sub-pixel corners); good needs the learned
                       confidence at the threshold AND an agreeing classical candidate
  snap_solo            as snap, but a learned quad no classical candidate agrees with may still be good
                       at the threshold (the long receipts the classical detector has no candidate for)
  agree                good only if classical-good and learned agree (IoU >= 0.9); otherwise the learned
                       answer held, or the classical one when only it exists

Only ids, quads and scores are read; no image is opened.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from common import canonical_iou, is_convex  # noqa: E402


def load_jsonl(p: str) -> dict:
    out = {}
    for line in Path(p).read_text(encoding="utf8").splitlines():
        if line.strip():
            r = json.loads(line)
            out[r["id"]] = r
    return out


def iou(a, b, wh) -> float:
    if a is None or b is None:
        return 0.0
    return canonical_iou(np.asarray(a) * wh, np.asarray(b) * wh)


def build(manifest: list[dict], learned: dict, classical: dict, thr: float) -> dict[str, list[dict]]:
    outs: dict[str, list[dict]] = {k: [] for k in ("classical", "learned", "fallback", "snap", "snap_solo", "agree")}
    for m in manifest:
        i = m["id"]
        wh = np.array([m["width"], m["height"]], dtype=float)
        L = learned.get(i) or {"id": i, "quad": None, "state": "failed"}
        C = classical.get(i) or {"id": i, "quad": None, "state": "failed", "candidates": []}
        lq, lconf = L.get("quad"), float(L.get("confidence") or 0.0)
        lgood = lq is not None and lconf >= thr
        lstate = "failed" if lq is None else ("good" if lgood else "check")

        def rec(quad, state, conf, src):
            return {"id": i, "quad": quad, "confidence": conf, "state": state if quad is not None else "failed", "src": src}

        outs["classical"].append({k: C.get(k) for k in ("id", "quad", "confidence", "state")} | {"id": i})
        outs["learned"].append({"id": i, "quad": lq, "confidence": lconf, "state": lstate})
        # fallback
        if C.get("state") == "good" and C.get("quad") is not None:
            outs["fallback"].append(rec(C["quad"], "good", C.get("confidence"), "classical"))
        elif lq is not None:
            outs["fallback"].append(rec(lq, lstate, lconf, "learned"))
        else:
            outs["fallback"].append(rec(C.get("quad"), "check" if C.get("quad") else "failed", C.get("confidence"), "classical-held"))
        # snap
        best, best_iou = None, 0.0
        if lq is not None:
            cands = [c["quad"] for c in C.get("candidates", []) if c.get("quad") and c.get("forced") != "failed"]
            if C.get("quad"):
                cands.append(C["quad"])
            for cq in cands:
                v = iou(lq, cq, wh)
                if v > best_iou:
                    best, best_iou = cq, v
        if lq is not None and best is not None and best_iou >= 0.8:
            outs["snap"].append(rec(best, "good" if lgood else "check", lconf, "snapped"))
            outs["snap_solo"].append(rec(best, "good" if lgood else "check", lconf, "snapped"))
        elif lq is not None:
            outs["snap"].append(rec(lq, "check", lconf, "learned-unvalidated"))
            outs["snap_solo"].append(rec(lq, lstate, lconf, "learned-solo"))
        else:
            outs["snap"].append(rec(C.get("quad"), "check" if C.get("quad") else "failed", C.get("confidence"), "classical-held"))
            outs["snap_solo"].append(rec(C.get("quad"), "check" if C.get("quad") else "failed", C.get("confidence"), "classical-held"))
        # agree
        if lq is not None and C.get("quad") is not None:
            v = iou(lq, C["quad"], wh)
            if C.get("state") == "good" and v >= 0.9:
                outs["agree"].append(rec(C["quad"], "good", min(lconf, float(C.get("confidence") or 1.0)), "agree"))
            else:
                outs["agree"].append(rec(lq, "check", lconf, "disagree-learned"))
        elif lq is not None:
            outs["agree"].append(rec(lq, "check", lconf, "learned-only"))
        else:
            outs["agree"].append(rec(C.get("quad"), "check" if C.get("quad") else "failed", C.get("confidence"), "classical-held"))
    return outs


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--learned", required=True)
    ap.add_argument("--classical", required=True)
    ap.add_argument("--out-dir", required=True)
    ap.add_argument("--thr", type=float, default=0.9, help="learned confidence for good")
    a = ap.parse_args(argv)
    rows = [json.loads(line) for line in Path(a.manifest).read_text(encoding="utf8").splitlines() if line.strip()]
    outs = build(rows, load_jsonl(a.learned), load_jsonl(a.classical), a.thr)
    d = Path(a.out_dir)
    d.mkdir(parents=True, exist_ok=True)
    for name, recs in outs.items():
        with open(d / f"{name}.jsonl", "w", encoding="utf8", newline="\n") as f:
            for r in recs:
                f.write(json.dumps(r) + "\n")
    print("wrote", ", ".join(outs), "to", d)
    return 0


if __name__ == "__main__":
    sys.exit(main())
