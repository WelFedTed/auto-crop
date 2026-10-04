# tools/train: learned page-corner detector (feasibility study)

Roadmap M4.12 to M4.16 (training data, splits, training script, export), done as a time-boxed feasibility study: [ADR 0010](../../docs/adr/0010-learned-detector-feasibility.md) has the measurements and the recommendation. Developer tool, never shipped. Weights, checkpoints and datasets are **never committed** (model-weights policy rule 5, B21): everything lives under `target/` (ignored). It imports nothing from the application (the application never imports it either); the only coupling to the repository's Rust code is through files: JSON-lines predictions in the harness format (`docs/testing/eval-harness.md`) and manifests written by `tools/synth`.

From scratch: no ImageNet weights, no pretrained download, no torchvision (M4.05, model-weights rule 3). The owner's real photos (`_data/`, private) are never used for training, tuning or checkpoint choice (B21); `predict.py` scores them once per finished model and only aggregates are reported.

## Model

`model.py`: MobileNetV3-small-layout backbone written here (random init), a 64-channel FPN and one head at stride 4 (about 0.94 M parameters, 3.8 MB in fp32 ONNX): class-agnostic corner heatmap, page mask, page-centre heatmap, and centre-to-corner offsets (four corners, trained with a loss that does not depend on which corner is listed first). `decode.py` snaps each regressed corner to the nearest heatmap peak (3x3 weighted centroid, sub-pixel), keeps the regressed position where there is no peak (occluded or out-of-frame corner), and returns confidence features (peak sharpness, number of matched corners, centre peak, mask agreement, regression-versus-peak agreement). `predict.py --fit-conf` fits a logistic confidence on the **synthetic** validation set only.

## Environment (Windows x64, CPython 3.14, NVIDIA GPU)

```
python -m venv target/train-venv
target/train-venv/Scripts/python -m pip install --require-hashes --no-deps -r tools/train/requirements.lock
```

`requirements.lock` pins every wheel by SHA-256 (torch 2.14.1+cu130 from the official PyTorch index, the rest from PyPI). On another platform install the direct pins in `requirements.txt` (with the CUDA build of your choice). Do not use the synth environment for training: it has no torch.

## Data

The generator (`tools/synth`, its own environment: `cargo xtask synth-setup`, or `python -m venv target/synth-venv` and the hashed lock) has a **training mode**, `python -m synth --train`: failure-biased quotas (long and strip receipts, textured desks, partial frames, curl, any rotation), four extra desk textures (planks, marble, terrazzo, carpet), and a hand (thumb, grip, two thumbs) that holds the page and may hide an edge or a corner; the ground truth stays the page's true corners. Evaluation suites are unchanged byte for byte (tested) and the training seeds are refused if they equal an evaluation seed.

```
tools/train/gen_data.sh 10      # val (1,000), train1..4 (4 x 5,000), test-train-mode (1,000) -> target/synth/  (about 30 min)
tools/train/gen_eval.sh 4       # smoke, smoke-b, smoke-c, mid-d, full from the default generator (eval only; full is 5,200 images)
```

## Train, export, predict, evaluate

```
cd tools/train && ../../target/train-venv/Scripts/python -m unittest discover -s tests          # geometry and target tests
T=target/train-venv/Scripts/python
$T tools/train/train.py --train target/synth/train{1,2,3,4}/manifest.jsonl --val target/synth/val/manifest.jsonl --out target/train/run1 --minutes 60
$T tools/train/predict.py --ckpt target/train/run1/best.pt --fit-conf target/synth/val/manifest.jsonl          # writes conf.json
$T tools/train/predict.py --ckpt target/train/run1/best.pt --conf target/train/run1/conf.json --manifest target/synth/smoke/manifest.jsonl --out target/preds/smoke-learned.jsonl
target/release/auto-crop-eval run --manifest target/synth/smoke/manifest.jsonl --predictor jsonl:target/preds/smoke-learned.jsonl --out target/res/smoke-learned.json --suite smoke
$T tools/train/export_onnx.py --ckpt target/train/run1/best.pt --out target/models/corner-net.onnx
```

Hybrids (classical answer plus candidates come from `cargo run --release -p auto-crop-eval --example dump_classical`, see `classical_baseline.sh`):

```
$T tools/train/hybrid.py --manifest M --learned learned.jsonl --classical classical.jsonl --out-dir target/preds/hyb --thr 0.9
$T tools/train/slices.py learned=target/res/a.json classical=target/res/b.json --axes aspect framing background
```

`train.py --overfit 64` is the M4.14 overfit check (no augmentation, same images; reports mean corner error at 256 px). `viz_samples.py` and `contact_sheet.py` draw augmented samples and quads for a quick look.

## Limits

Synthetic training data only; the synthetic-to-real gap is the open question (ADR 0010). Hands are cartoon-like capsules (occlusion topology, not photographic skin). The confidence is fitted on synthetic validation data and is not calibrated for real photographs (M4.42 needs real labels). One page per picture, no negatives (pages absent).
