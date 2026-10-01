# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Orchestrates the inference spike: 2 nets x 3 backends x fp32/int8 x 1/4 threads.
Usage: python run.py <exe> [runs]; prints a Markdown table plus JSON lines to results.jsonl."""
import json, subprocess, sys, os
import numpy as np

exe = os.path.abspath(sys.argv[1])
runs = sys.argv[2] if len(sys.argv) > 2 else "50"
nets = {"quadnet": (256, 4, 64), "orient": (224, 4, None)}
rows = []


def peaks(out):  # (4,64,64) heatmaps -> subpixel peaks in input pixels (5x5 centroid around the max)
    res = []
    h = out.reshape(4, 64, 64)
    for c in range(4):
        y, x = np.unravel_index(np.argmax(h[c]), h[c].shape)
        y0, y1, x0, x1 = max(0, y - 2), min(64, y + 3), max(0, x - 2), min(64, x + 3)
        w = h[c, y0:y1, x0:x1]
        ys, xs = np.mgrid[y0:y1, x0:x1]
        res.append(((xs * w).sum() / w.sum() * 4, (ys * w).sum() / w.sum() * 4))
    return np.array(res)


def run(be, model, inp, threads):
    out = f"out_{be}_{os.path.basename(model)}_{threads}.f32"
    p = subprocess.run([exe, be, model, inp, out, str(threads), runs], capture_output=True, text=True)
    line = p.stdout.strip().splitlines()[-1] if p.stdout.strip() else '{"ok":false,"error":"no output: ' + p.stderr[-200:].replace('"', "'") + '"}'
    r = json.loads(line)
    r["out"] = out if r.get("ok") else None
    return r


for net in nets:
    for prec in ("fp32", "int8"):
        model = f"models/{net}{'_int8' if prec == 'int8' else ''}.onnx"
        inp = f"models/{net}_input.f32"
        ref = None
        results = {}
        for be in ("ort", "rten", "tract"):
            for th in (1, 4):
                r = run(be, model, inp, th)
                r.update(net=net, prec=prec, backend=be)
                results[(be, th)] = r
        refr = results[("ort", 1)]
        if refr["ok"]:
            ref = np.fromfile(refr["out"], dtype=np.float32)
        # fp32 reference for int8 accuracy
        ref32 = None
        if prec == "int8":
            r32 = run("ort", f"models/{net}.onnx", inp, 1)
            if r32["ok"]:
                ref32 = np.fromfile(r32["out"], dtype=np.float32)
        for (be, th), r in results.items():
            if r["ok"]:
                o = np.fromfile(r["out"], dtype=np.float32)
                r["maxdiff_vs_ort"] = float(np.max(np.abs(o - ref))) if ref is not None and o.shape == ref.shape else None
                if net == "quadnet" and ref is not None and o.shape == ref.shape:
                    r["peak_px_vs_ort"] = float(np.max(np.linalg.norm(peaks(o) - peaks(ref), axis=1)))
                if ref32 is not None and o.shape == ref32.shape:
                    r["maxdiff_vs_fp32"] = float(np.max(np.abs(o - ref32)))
                    if net == "quadnet":
                        r["peak_px_vs_fp32"] = float(np.max(np.linalg.norm(peaks(o) - peaks(ref32), axis=1)))
            rows.append(r)
            r.pop("out", None)

with open("results.jsonl", "w") as f:
    for r in rows:
        f.write(json.dumps(r) + "\n")

print("| net | prec | backend | thr | ok | load ms | median ms | p95 ms | max abs diff vs ORT | peak diff vs ORT px | note |")
print("|---|---|---|---|---|---|---|---|---|---|---|")
for r in rows:
    if r["ok"]:
        print(f"| {r['net']} | {r['prec']} | {r['backend']} | {r['threads']} | yes | {r['load_ms']} | {r['median_ms']} | {r['p95_ms']} | {r.get('maxdiff_vs_ort')} | {r.get('peak_px_vs_ort')} | vs fp32: {r.get('maxdiff_vs_fp32')} / {r.get('peak_px_vs_fp32')} px |")
    else:
        print(f"| {r['net']} | {r['prec']} | {r['backend']} | {r['threads']} | NO | | | | | | {r['error'][:140]} |")
