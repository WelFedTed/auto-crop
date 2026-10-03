# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""File formats, EXIF orientation and colour-space variants (ROADMAP M1.34).

A picture is rendered upright. For EXIF orientation ``o`` the *stored* pixels are the inverse
transform of the upright picture, so a reader that applies the tag recovers the upright picture
exactly and the ground-truth quad (normalised to the oriented image) stays valid. Display P3
variants carry converted pixel values plus an embedded P3 profile, so they show the same colours.
"""

from __future__ import annotations

import io

import numpy as np
from PIL import Image, TiffImagePlugin

from . import icc

FORMATS = ("jpeg", "png", "tiff", "webp")
EXT = {"jpeg": "jpg", "png": "png", "tiff": "tif", "webp": "webp"}


def display(stored: np.ndarray, o: int) -> np.ndarray:
    """What a reader that honours EXIF orientation ``o`` shows for the stored pixels."""
    if o == 1:
        return stored
    if o == 2:
        return stored[:, ::-1]
    if o == 3:
        return stored[::-1, ::-1]
    if o == 4:
        return stored[::-1]
    if o == 5:
        return stored.transpose(1, 0, 2)
    if o == 6:
        return np.rot90(stored, k=-1)  # 90 degrees clockwise
    if o == 7:
        return stored.transpose(1, 0, 2)[::-1, ::-1]
    if o == 8:
        return np.rot90(stored, k=1)  # 90 degrees counter-clockwise
    raise ValueError(f"EXIF orientation must be 1..8, got {o}")


def stored_for(upright: np.ndarray, o: int) -> np.ndarray:
    """Pixels to store so that orientation ``o`` turns them into ``upright``."""
    if o in (1, 2, 3, 4, 5, 7):  # these are their own inverses
        return np.ascontiguousarray(display(upright, o))
    if o == 6:
        return np.ascontiguousarray(np.rot90(upright, k=1))
    if o == 8:
        return np.ascontiguousarray(np.rot90(upright, k=-1))
    raise ValueError(f"EXIF orientation must be 1..8, got {o}")


def srgb_profile() -> bytes:
    return icc.srgb_profile()


def exif_bytes(o: int) -> bytes:
    ex = Image.Exif()
    ex[0x0112] = o
    return ex.tobytes()


def tiff_pad_range(data: bytes) -> tuple[int, int] | None:
    """The byte range between the end of the last strip and the IFD, if there is one."""
    if data[:4] != b"II*\x00":
        return None
    ifd = int.from_bytes(data[4:8], "little")
    n = int.from_bytes(data[ifd : ifd + 2], "little")
    offsets: list[int] = []
    counts: list[int] = []
    for k in range(n):
        e = data[ifd + 2 + 12 * k : ifd + 14 + 12 * k]
        tag, typ, cnt = (int.from_bytes(e[0:2], "little"), int.from_bytes(e[2:4], "little"), int.from_bytes(e[4:8], "little"))
        size = {3: 2, 4: 4}.get(typ)
        if tag not in (273, 279) or size is None:
            continue
        if cnt * size <= 4:
            raw = e[8 : 8 + cnt * size]
        else:
            o = int.from_bytes(e[8:12], "little")
            raw = data[o : o + cnt * size]
        vals = [int.from_bytes(raw[i : i + size], "little") for i in range(0, len(raw), size)]
        (offsets if tag == 273 else counts).extend(vals)
    if not offsets or len(offsets) != len(counts):
        return None
    end = max(o + c for o, c in zip(offsets, counts))
    return (end, ifd) if end < ifd else None


def zero_tiff_padding(data: bytes) -> bytes:
    """Zero the alignment byte libtiff leaves uninitialised between the last strip and the IFD.

    When a deflate strip has an odd length, Pillow's libtiff writes one pad byte before the IFD that
    holds whatever was in memory, so the same picture encoded twice differed in a single byte (7 of
    20 TIFFs of the smoke suite on Linux). Pixels are unaffected; zeroing it makes the bytes
    reproducible. Only little-endian files with one IFD of strips are touched (Pillow's own output).
    """
    pad = tiff_pad_range(data)
    if pad is None:
        return data
    return data[: pad[0]] + bytes(pad[1] - pad[0]) + data[pad[1] :]


def encode(
    upright: np.ndarray,
    fmt: str,
    quality: int,
    orientation: int = 1,
    colorspace: str = "srgb",
    embed_srgb: bool = False,
    lossless_webp: bool = False,
) -> bytes:
    """Encode ``upright`` (uint8 RGB, sRGB values) in ``fmt`` with the given variants."""
    pixels = icc.srgb_to_p3(upright) if colorspace == "display-p3" else upright
    stored = stored_for(pixels, orientation)
    im = Image.fromarray(stored)
    profile = None
    if colorspace == "display-p3":
        profile = icc.display_p3_profile()
    elif embed_srgb:
        profile = srgb_profile()
    buf = io.BytesIO()
    kw = {} if profile is None else {"icc_profile": profile}
    if fmt == "jpeg":
        if orientation != 1:
            kw["exif"] = exif_bytes(orientation)
        im.save(buf, "JPEG", quality=int(quality), optimize=False, progressive=False, **kw)
    elif fmt == "png":
        if orientation != 1:
            kw["exif"] = exif_bytes(orientation)
        im.save(buf, "PNG", compress_level=6, **kw)
    elif fmt == "tiff":
        ifd = TiffImagePlugin.ImageFileDirectory_v2()
        if orientation != 1:
            ifd[274] = orientation
        im.save(buf, "TIFF", compression="tiff_adobe_deflate", tiffinfo=ifd, **kw)
        return zero_tiff_padding(buf.getvalue())
    elif fmt == "webp":
        if orientation != 1:
            kw["exif"] = exif_bytes(orientation)
        if lossless_webp:
            im.save(buf, "WEBP", lossless=True, quality=100, method=4, **kw)
        else:
            im.save(buf, "WEBP", quality=int(quality), method=4, **kw)
    else:
        raise ValueError(f"unknown format {fmt}")
    return buf.getvalue()
