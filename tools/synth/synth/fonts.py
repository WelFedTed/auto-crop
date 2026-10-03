# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Fonts, taken from the pinned Python packages so nothing is downloaded or committed.

Matplotlib ships DejaVu Sans, DejaVu Sans Mono and DejaVu Serif (Bitstream Vera licence with
public-domain additions) and the STIX fonts (SIL OFL 1.1) under ``mpl-data/fonts/ttf`` together with
their licence texts. `docs/provenance.md` records them. Fonts are loaded by absolute path and cached
per process. Layout uses FreeType's basic engine, so text metrics do not depend on whether libraqm
happens to be installed.
"""

from __future__ import annotations

import importlib.util
from functools import lru_cache
from pathlib import Path

from PIL import ImageFont

# family -> (regular, bold, licence)
FAMILIES = {
    "sans": ("DejaVuSans.ttf", "DejaVuSans-Bold.ttf", "Bitstream-Vera"),
    "serif": ("STIXGeneral.ttf", "STIXGeneralBol.ttf", "OFL-1.1"),
    "mono": ("DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf", "Bitstream-Vera"),
}


@lru_cache(maxsize=1)
def font_dir() -> Path:
    spec = importlib.util.find_spec("matplotlib")
    if spec is None or not spec.submodule_search_locations:
        raise RuntimeError("matplotlib (pinned in requirements.lock) is needed for its fonts")
    return Path(next(iter(spec.submodule_search_locations))) / "mpl-data" / "fonts" / "ttf"


def path_of(family: str, bold: bool = False) -> Path:
    regular, heavy, _ = FAMILIES[family]
    return font_dir() / (heavy if bold else regular)


@lru_cache(maxsize=256)
def load(family: str, size_px: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    return ImageFont.truetype(
        str(path_of(family, bold)), max(int(size_px), 4), layout_engine=ImageFont.Layout.BASIC
    )


def licences() -> dict[str, str]:
    """SPDX-style licence of each family, for the manifest and the provenance register."""
    return {name: spec[2] for name, spec in FAMILIES.items()}
