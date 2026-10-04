# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Fingerprint a generated synthetic suite and compare two (ROADMAP M1.78).

`suite_fingerprint.py hash SUITE_DIR OUT.json` writes the SHA-256 of manifest.jsonl and of every
image under SUITE_DIR/images. `suite_fingerprint.py compare A.json B.json` prints how many files are
byte-identical. The synthetic generator promises identical bytes for one seed only inside one
pinned environment on one operating system (docs/testing/eval-harness.md); this measures how far
that holds across operating systems. Standard library only.
"""
from __future__ import annotations

import hashlib
import json
import os
import sys


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def fingerprint(suite: str) -> dict:
    images = {}
    img_dir = os.path.join(suite, "images")
    for name in sorted(os.listdir(img_dir)):
        images[name] = sha256(os.path.join(img_dir, name))
    return {"manifest": sha256(os.path.join(suite, "manifest.jsonl")), "images": images}


def compare(a: dict, b: dict) -> str:
    names = sorted(set(a["images"]) | set(b["images"]))
    same = sum(1 for n in names if a["images"].get(n) == b["images"].get(n))
    return (
        f"manifest {'identical' if a['manifest'] == b['manifest'] else 'DIFFERENT'}; "
        f"{same} of {len(names)} image files byte-identical"
    )


def main(argv: list[str]) -> int:
    if len(argv) == 4 and argv[1] == "hash":
        with open(argv[3], "w", encoding="utf-8") as f:
            json.dump(fingerprint(argv[2]), f, sort_keys=True)
        return 0
    if len(argv) == 4 and argv[1] == "compare":
        docs = []
        for p in argv[2:]:
            with open(p, encoding="utf-8") as f:
                docs.append(json.load(f))
        print(compare(docs[0], docs[1]))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
