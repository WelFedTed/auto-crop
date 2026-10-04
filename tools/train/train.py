# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Train the corner network from scratch on synthetic data, time-boxed.

    python train.py --train target/synth/train1/manifest.jsonl [...] --val target/synth/val/manifest.jsonl \
        --minutes 45 --out target/train/run1

Mixed precision (bfloat16), AdamW, cosine learning rate over the time box, EMA of the weights.
Checkpoints are chosen on the SYNTHETIC validation set only. The owner's real photos are never
used here (B21): they are evaluated once per finished model by ``predict.py``.
"""

from __future__ import annotations

import argparse
import copy
import json
import math
import sys
import time
from pathlib import Path

import numpy as np
import torch
from torch.utils.data import DataLoader

sys.path.insert(0, str(Path(__file__).resolve().parent))
from data import TrainSet, prep  # noqa: E402
from infer import run_manifest, summarise, torch_forward  # noqa: E402
from losses import total_loss  # noqa: E402
from model import CornerNet, count_params  # noqa: E402


def _worker_init(_):
    import cv2

    cv2.setNumThreads(1)
    torch.set_num_threads(1)


class EMA:
    def __init__(self, model, decay=0.999):
        self.m = copy.deepcopy(model).eval()
        self.decay = decay
        for p in self.m.parameters():
            p.requires_grad_(False)

    @torch.no_grad()
    def update(self, model, step):
        d = min(self.decay, (1 + step) / (10 + step))
        for e, p in zip(self.m.state_dict().values(), model.state_dict().values()):
            if e.dtype.is_floating_point:
                e.mul_(d).add_(p.detach(), alpha=1 - d)
            else:
                e.copy_(p)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--train", nargs="+", required=True, help="training manifests")
    ap.add_argument("--val", required=True, help="validation manifest (synthetic, scene-disjoint)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--minutes", type=float, default=45.0)
    ap.add_argument("--batch", type=int, default=64)
    ap.add_argument("--lr", type=float, default=3e-3)
    ap.add_argument("--wd", type=float, default=1e-4)
    ap.add_argument("--workers", type=int, default=8)
    ap.add_argument("--size", type=int, default=256)
    ap.add_argument("--stride", type=int, default=4, choices=[2, 4])
    ap.add_argument("--width", type=float, default=1.0)
    ap.add_argument("--val-n", type=int, default=600)
    ap.add_argument("--val-every", type=float, default=5.0, help="minutes")
    ap.add_argument("--overfit", type=int, default=0, help="train on N images without augmentation, then report corner error")
    ap.add_argument("--max-steps", type=int, default=0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--init", help="start from the EMA weights of this checkpoint (a continued run; the schedule restarts)")
    a = ap.parse_args(argv)

    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    torch.manual_seed(a.seed)
    torch.backends.cudnn.benchmark = True
    dev = "cuda"
    model = CornerNet(width=a.width, out_stride=a.stride).to(dev).to(memory_format=torch.channels_last)
    if a.init:
        sd = torch.load(a.init, map_location="cpu", weights_only=False)["ema"]
        model.load_state_dict(sd)
    ema = EMA(model)
    n_par = count_params(model)
    print(f"params {n_par}", flush=True)

    overfit = a.overfit > 0
    ds = TrainSet(a.train, a.stride, a.size, a.seed, limit=a.overfit or None, augment=not overfit)
    dl = DataLoader(ds, batch_size=a.batch if not overfit else min(a.batch, 32), shuffle=True, num_workers=a.workers if not overfit else 2, drop_last=not overfit, persistent_workers=True, prefetch_factor=4, pin_memory=True, worker_init_fn=_worker_init)
    opt = torch.optim.AdamW(model.parameters(), lr=a.lr, weight_decay=a.wd)
    log = (out / "log.jsonl").open("a", encoding="utf8")
    t0 = time.time()
    budget = a.minutes * 60.0
    step, seen, best, last_val = 0, 0, 1e9, t0
    win_t, win_n = time.time(), 0
    ema_loss = None
    done = False
    epoch = 0
    while not done:
        ds.set_epoch(epoch)
        epoch += 1
        for x, heat, off, offm in dl:
            frac = (time.time() - t0) / budget
            if frac >= 1.0 or (a.max_steps and step >= a.max_steps):
                done = True
                break
            lr = a.lr * min(1.0, (step + 1) / 300) * (0.5 * (1 + math.cos(math.pi * min(frac, 1.0))) * 0.98 + 0.02)
            for g in opt.param_groups:
                g["lr"] = lr
            x = prep(x.to(dev, non_blocking=True)).contiguous(memory_format=torch.channels_last)
            heat, off, offm = heat.to(dev, non_blocking=True), off.to(dev, non_blocking=True), offm.to(dev, non_blocking=True)
            model.train()
            with torch.autocast(device_type="cuda", dtype=torch.bfloat16):
                o = model(x)
            loss, parts = total_loss(o, heat, off, offm)
            opt.zero_grad(set_to_none=True)
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 5.0)
            opt.step()
            ema.update(model, step)
            step += 1
            seen += x.shape[0]
            win_n += x.shape[0]
            ema_loss = float(loss) if ema_loss is None else 0.98 * ema_loss + 0.02 * float(loss)
            if step % 100 == 0:
                now = time.time()
                rec = {"step": step, "epoch": epoch, "t_min": round((now - t0) / 60, 2), "loss": round(ema_loss, 4), "lr": lr, "img_s": round(win_n / (now - win_t), 1), **{k: round(v, 4) for k, v in parts.items()}}
                print(json.dumps(rec), flush=True)
                log.write(json.dumps(rec) + "\n")
                log.flush()
                win_t, win_n = now, 0
            if not overfit and time.time() - last_val > a.val_every * 60:
                last_val = time.time()
                best = validate(ema.m, a, out, step, best, log)
    # final
    torch.save({"model": model.state_dict(), "ema": ema.m.state_dict(), "args": vars(a), "step": step}, out / "last.pt")
    if overfit:
        rows = run_manifest(torch_forward(ema.m), a.train[0], a.size, a.stride, limit=a.overfit, workers=0)
        errs = []
        for r in rows:
            if r["quad"] is None:
                errs.append(99.0)
                continue
            gt = np.asarray(r["row"]["quad"]) * [r["row"]["width"], r["row"]["height"]]
            from common import canonical_quad

            q, g = canonical_quad(r["px"]), canonical_quad(gt)
            # compare in canvas scale (256 px): scale by the letterbox factor
            sc = a.size / max(r["row"]["width"], r["row"]["height"])
            errs.append(float(np.linalg.norm((q - g), axis=1).mean() * sc))
        print("OVERFIT (no augmentation, evaluation = letterbox of the same images)", json.dumps({**summarise(rows), "mean_corner_err_px_256": float(np.mean(errs)), "median": float(np.median(errs))}))
    else:
        validate(ema.m, a, out, step, best, log, final=True)
    elapsed = time.time() - t0
    summary = {"steps": step, "samples": seen, "minutes": round(elapsed / 60, 2), "avg_img_s": round(seen / elapsed, 1), "params": n_par}
    print("DONE", json.dumps(summary))
    (out / "summary.json").write_text(json.dumps(summary), encoding="utf8")
    return 0


def validate(model, a, out: Path, step: int, best: float, log, final: bool = False) -> float:
    rows = run_manifest(torch_forward(model), a.val, a.size, a.stride, limit=a.val_n, workers=3)
    s = summarise(rows)
    s.update(step=step, val=True)
    print(json.dumps(s), flush=True)
    log.write(json.dumps(s) + "\n")
    log.flush()
    score = s["fail_rate"] - 0.1 * s["mean_iou"]
    torch.save({"ema": model.state_dict(), "args": vars(a), "step": step, "val": s}, out / "last.pt")
    if score < best:
        best = score
        torch.save({"ema": model.state_dict(), "args": vars(a), "step": step, "val": s}, out / "best.pt")
    return best


if __name__ == "__main__":
    sys.exit(main())
