# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Known-text pages and receipts (ROADMAP M1.31).

A page is drawn as an *ink mask* (255 = ink) on a blank sheet of known size in millimetres, together
with the exact text that was drawn. Colour, paper texture, ink fade and every other degradation are
applied later (`degrade.py`) from the mask, so the clean render and the transcript always agree
with what was drawn. Documents are Letter or A4 pages (prose letters, invoices with line items,
forms, two-column reports); receipts are 58 to 80 mm wide with a dot-like printer font, generated
line items with prices, totals, Code 128 and QR codes, and any length up to 11.5:1.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

import numpy as np
from PIL import Image, ImageDraw

from . import barcodes, fonts, textgen
from .plan import ASPECT_RANGE
from .textgen import Rand

PT_MM = 25.4 / 72.0
AMOUNT_RE = re.compile(r"^-?\d+\.\d{2}$")

DOC_PPM = 3.0
RECEIPT_PPM = 4.0
LETTER_MM = (215.9, 279.4)
A4_MM = (210.0, 297.0)
PAPER_RGB = {
    "white": [(250, 250, 248), (252, 252, 252), (246, 247, 249)],
    "cream": [(247, 240, 218), (243, 236, 212), (250, 243, 224)],
    "coloured": [(250, 238, 160), (248, 205, 215), (205, 226, 246), (205, 238, 208)],
}
INK_RGB = [(22, 22, 24), (30, 32, 52), (40, 36, 34), (18, 24, 20)]


@dataclass
class Page:
    """One rendered page: an ink mask plus its ground truth text."""

    ink: np.ndarray  # uint8 HxW, 255 = ink (anti-aliased for documents)
    size_mm: tuple[float, float]
    ppm: float
    kind: str  # document | receipt
    layout: str
    font_family: str
    lines: list[str]
    paper_rgb: tuple[int, int, int]
    ink_rgb: tuple[int, int, int]
    thermal: bool
    extra: dict = field(default_factory=dict)

    @property
    def size_px(self) -> tuple[int, int]:
        return self.ink.shape[1], self.ink.shape[0]

    @property
    def text(self) -> str:
        return "\n".join(self.lines)

    @property
    def amounts(self) -> list[str]:
        return [t for t in re.split(r"\s+", self.text) if AMOUNT_RE.match(t)]

    def clean_gray(self) -> np.ndarray:
        """The clean render: black ink on pure white, 8-bit grey (what OCR is scored on)."""
        return 255 - self.ink

    def clean_binary(self) -> np.ndarray:
        """The clean binary render: 0 = ink, 255 = paper."""
        return np.where(self.ink > 127, 0, 255).astype(np.uint8)


class Sheet:
    def __init__(self, size_mm: tuple[float, float], ppm: float, antialias: bool = True):
        self.size_mm = size_mm
        self.ppm = ppm
        self.w = int(round(size_mm[0] * ppm))
        self.h = int(round(size_mm[1] * ppm))
        self.img = Image.new("L", (self.w, self.h), 0)
        self.d = ImageDraw.Draw(self.img)
        self.d.fontmode = "L" if antialias else "1"
        self.lines: list[str] = []

    def px(self, mm: float) -> int:
        return int(round(mm * self.ppm))

    def font(self, family: str, pt: float, bold: bool = False):
        return fonts.load(family, max(5, round(pt * PT_MM * self.ppm)), bold)

    def text(self, x: int, y: int, s: str, font, align: str = "l", record: bool = True):
        """Draw ``s`` with its baseline at ``y`` (``align``: l, r or m)."""
        if not s:
            return
        anchor = {"l": "ls", "r": "rs", "m": "ms"}[align]
        self.d.text((x, y), s, fill=255, font=font, anchor=anchor)
        if record:
            self.lines.append(s)

    def width(self, s: str, font) -> float:
        return font.getlength(s)

    def rule(self, x0: int, x1: int, y: int, thick_mm: float = 0.25):
        t = max(1, self.px(thick_mm))
        self.d.rectangle([x0, y, x1, y + t - 1], fill=255)

    def box(self, x0: int, y0: int, x1: int, y1: int, thick_mm: float = 0.25):
        t = max(1, self.px(thick_mm))
        self.d.rectangle([x0, y0, x1, y1], outline=255, width=t)

    def paste_ink(self, arr: np.ndarray, x: int, y: int):
        im = Image.fromarray(arr)
        self.img.paste(im, (x, y), im)

    def wrap(self, s: str, font, max_px: float) -> list[str]:
        words, line, out = s.split(), "", []
        for w in words:
            trial = f"{line} {w}".strip()
            if self.width(trial, font) <= max_px or not line:
                line = trial
            else:
                out.append(line)
                line = w
        if line:
            out.append(line)
        return out

    def finish(self) -> np.ndarray:
        return np.asarray(self.img, dtype=np.uint8).copy()


def _pick_colours(g: Rand, paper: str, thermal: bool):
    pr = g.pick(PAPER_RGB[paper])
    if thermal:
        # Thermal paper is white to slightly grey and prints dark grey to black.
        ir = (35, 35, 38)
    else:
        ir = g.pick(INK_RGB)
    return pr, ir


# ---------------------------------------------------------------------------------------------
# Documents
# ---------------------------------------------------------------------------------------------


def _letter(s: Sheet, g: Rand, fam: str, margin: float):
    W, H = s.size_mm
    x0, x1 = s.px(margin), s.w - s.px(margin)
    body = s.font(fam, 11)
    bold = s.font(fam, 11, True)
    y = s.px(margin + 6)
    for line in [textgen.person(g), *textgen.address(g)]:
        s.text(x1, y, line.title(), body, "r")
        y += int(body.size * 1.35)
    y += s.px(8)
    d, _ = textgen.date_time(g)
    s.text(x0, y, d, body)
    y += s.px(10)
    for line in [textgen.person(g), *textgen.address(g)]:
        s.text(x0, y, line.title(), body)
        y += int(body.size * 1.35)
    y += s.px(8)
    s.text(x0, y, f"Subject: {g.pick(textgen.HEADINGS)} of {g.pick(textgen.OBJECTS)}", bold)
    y += int(body.size * 2.2)
    s.text(x0, y, f"Dear {g.pick(textgen.FIRST)},", body)
    y += int(body.size * 2.0)
    bottom = s.h - s.px(margin + 30)
    while y < bottom:
        for ln in s.wrap(textgen.paragraph(g, g.between(3, 5)), body, x1 - x0):
            if y >= bottom:
                break
            s.text(x0, y, ln, body)
            y += int(body.size * 1.4)
        y += int(body.size * 0.9)
    y += int(body.size * 0.8)
    s.text(x0, y, "Yours sincerely,", body)
    y += int(body.size * 3.2)
    s.text(x0, y, textgen.person(g), body)


def _invoice(s: Sheet, g: Rand, fam: str, margin: float):
    x0, x1 = s.px(margin), s.w - s.px(margin)
    body = s.font(fam, 10)
    bold = s.font(fam, 10, True)
    big = s.font(fam, 22, True)
    y = s.px(margin + 8)
    s.text(x0, y, "INVOICE", big)
    inv_no = f"INV-{textgen.digits(g, 6)}"
    d, _ = textgen.date_time(g)
    s.text(x1, y - s.px(4), f"No. {inv_no}", bold, "r")
    s.text(x1, y + s.px(2), f"Date: {d}", body, "r")
    y += s.px(14)
    shop = textgen.shop_name(g)
    for line in [shop, *textgen.address(g)]:
        s.text(x0, y, line, body)
        y += int(body.size * 1.4)
    y += s.px(8)
    s.text(x0, y, f"Bill to: {textgen.person(g)}", body)
    y += s.px(12)
    cols = [x0, int(x0 + (x1 - x0) * 0.58), int(x0 + (x1 - x0) * 0.72), int(x0 + (x1 - x0) * 0.86), x1]
    s.rule(x0, x1, y - int(body.size * 1.1), 0.4)
    s.text(cols[0], y, "Description", bold)
    s.text(cols[1], y, "Qty", bold, "r")
    s.text(cols[2] + s.px(10), y, "Unit", bold, "r")
    s.text(cols[4], y, "Amount", bold, "r")
    y += int(body.size * 0.7)
    s.rule(x0, x1, y, 0.4)
    y += int(body.size * 1.4)
    n_rows = g.between(7, 16)
    total = 0
    for desc, qty, unit, line_total in textgen.invoice_items(g, n_rows):
        s.text(cols[0], y, desc, body)
        s.text(cols[1], y, str(qty), body, "r", record=False)
        s.text(cols[2] + s.px(10), y, textgen.money(unit), body, "r", record=False)
        s.text(cols[4], y, textgen.money(line_total), body, "r", record=False)
        s.lines[-1] = f"{desc} {qty} {textgen.money(unit)} {textgen.money(line_total)}"
        total += line_total
        y += int(body.size * 1.55)
    s.rule(x0, x1, y - int(body.size * 0.9), 0.3)
    tax = round(total * 0.08)
    for label, val, font in [
        ("Subtotal", total, body),
        ("Tax 8%", tax, body),
        ("Total due", total + tax, bold),
    ]:
        y += int(body.size * 1.6)
        s.text(cols[3], y, label, font, "r", record=False)
        s.text(cols[4], y, textgen.money(val), font, "r", record=False)
        s.lines.append(f"{label} {textgen.money(val)}")
    y += s.px(10)
    for ln in s.wrap("Payment is due within thirty days of the invoice date. " + textgen.sentence(g), body, x1 - x0):
        s.text(x0, y, ln, body)
        y += int(body.size * 1.4)
    # A QR code with the invoice number and a Code 128 strip near the foot of the page.
    qr = barcodes.qr_modules(f"{shop}|{inv_no}")
    mpx = max(1, s.px(22) // (qr.shape[0] + 8))
    qimg = barcodes.draw_qr(qr, mpx)
    s.paste_ink(qimg, x0, s.h - s.px(margin) - qimg.shape[0])
    lin = barcodes.draw_linear(barcodes.code128_modules(inv_no), max(1, s.px(0.35)), s.px(14))
    s.paste_ink(lin, x1 - lin.shape[1], s.h - s.px(margin) - lin.shape[0])
    s.lines.append(inv_no)


def _form(s: Sheet, g: Rand, fam: str, margin: float):
    x0, x1 = s.px(margin), s.w - s.px(margin)
    body = s.font(fam, 10)
    big = s.font(fam, 18, True)
    y = s.px(margin + 8)
    s.text(x0, y, f"{g.pick(textgen.SHOP_A)} {g.pick(textgen.HEADINGS)} Form", big)
    y += s.px(14)
    for ln in s.wrap(textgen.paragraph(g, 2), body, x1 - x0):
        s.text(x0, y, ln, body)
        y += int(body.size * 1.4)
    y += s.px(8)
    bottom = s.h - s.px(margin + 12)
    while y < bottom:
        label = g.pick(textgen.FORM_FIELDS)
        s.text(x0, y, f"{label}:", body)
        lx = x0 + s.px(48)
        if g.chance(0.6):
            val = (
                textgen.person(g)
                if "name" in label.lower()
                else textgen.digits(g, g.between(4, 9))
            )
            s.text(lx + s.px(2), y - s.px(0.8), val, body)
        s.rule(lx, x1, y + s.px(1.2), 0.2)
        y += s.px(11)
        if g.chance(0.18):
            for k in range(3):
                bx = x0 + k * s.px(40)
                s.box(bx, y - s.px(4), bx + s.px(4), y, 0.25)
                s.text(bx + s.px(6), y, g.pick(["Yes", "No", "Maybe", "Other", "Later"]), body)
            y += s.px(11)


def _report(s: Sheet, g: Rand, fam: str, margin: float):
    x0, x1 = s.px(margin), s.w - s.px(margin)
    body = s.font(fam, 10)
    head = s.font(fam, 13, True)
    big = s.font(fam, 20, True)
    y = s.px(margin + 8)
    s.text(x0, y, f"{g.pick(textgen.SHOP_A)} {g.pick(textgen.SHOP_B)} Report", big)
    y += s.px(14)
    gap = s.px(7)
    colw = (x1 - x0 - gap) // 2
    top, bottom = y, s.h - s.px(margin + 6)
    for col in range(2):
        cx = x0 + col * (colw + gap)
        y = top
        while y < bottom:
            s.text(cx, y, g.pick(textgen.HEADINGS), head)
            y += int(body.size * 1.9)
            for ln in s.wrap(textgen.paragraph(g, g.between(3, 6)), body, colw):
                if y >= bottom:
                    break
                s.text(cx, y, ln, body)
                y += int(body.size * 1.38)
            y += int(body.size * 1.1)


DOC_LAYOUTS = {"letter": _letter, "invoice": _invoice, "form": _form, "report": _report}


def render_document(rng, paper: str, ppm: float = DOC_PPM) -> Page:
    g = Rand(rng)
    size = g.pick([LETTER_MM, A4_MM])
    fam = g.pick(["sans", "serif"])
    layout = g.pick(sorted(DOC_LAYOUTS))
    s = Sheet(size, ppm, antialias=True)
    DOC_LAYOUTS[layout](s, g, fam, g.between(16, 24))
    pr, ir = _pick_colours(g, paper, thermal=False)
    return Page(
        s.finish(), size, ppm, "document", layout, fam, s.lines, pr, ir, False,
        {"paper_size": "letter" if size == LETTER_MM else "a4"},
    )


# ---------------------------------------------------------------------------------------------
# Receipts
# ---------------------------------------------------------------------------------------------


def _col_line(left: str, right: str, chars: int) -> str:
    gap = max(1, chars - len(left) - len(right))
    return left[: max(0, chars - len(right) - 1)] + " " * gap + right


def render_receipt(rng, aspect_class: str, paper: str, ppm: float = RECEIPT_PPM) -> Page:
    g = Rand(rng)
    lo, hi = ASPECT_RANGE[aspect_class]
    aspect = lo + (hi - lo) * float(rng.random())
    width_mm = g.pick([80.0, 80.0, 80.0, 72.0, 58.0])
    height_mm = round(width_mm * aspect, 1)
    s = Sheet((width_mm, height_mm), ppm, antialias=False)
    chars = {80.0: 42, 72.0: 40, 58.0: 32}[width_mm]
    margin = 3.0
    content_w = s.w - 2 * s.px(margin)
    probe = fonts.load("mono", 40)
    em_px = (content_w / chars) / (probe.getlength("M" * 40) / 40 / 40)
    fam = "mono"
    f = fonts.load(fam, max(6, int(em_px)), False)
    fb = fonts.load(fam, max(6, int(em_px)), True)
    fbig = fonts.load(fam, max(8, int(em_px * 1.9)), True)
    lh = int(f.size * 1.32)
    x0, x1, xm = s.px(margin), s.w - s.px(margin), s.w // 2
    y = s.px(4) + fbig.size
    shop = textgen.shop_name(g)
    big_chars = int(chars / 1.9)
    for part in s.wrap(shop, fbig, content_w)[:2]:
        s.text(xm, y, part, fbig, "m")
        y += int(fbig.size * 1.25)
    for line in textgen.address(g):
        s.text(xm, y, line, f, "m")
        y += lh
    d, t = textgen.date_time(g)
    y += lh // 2
    s.text(x0, y, _col_line(f"DATE {d}", f"TIME {t}", chars), f)
    y += lh
    s.text(x0, y, _col_line(f"STORE {g.between(1, 99):03d}", f"TILL {g.between(1, 9)}", chars), f)
    y += lh
    s.text(xm, y, "-" * chars, f, "m", record=False)
    y += lh

    # Reserve room for everything from the totals down so the receipt ends where it should.
    foot_lines = 4 + 5 + 3 + len(textgen.POLICY[:2])
    qr_mm = 24 if width_mm >= 72 else 20
    tail_h = lh * foot_lines + s.px(qr_mm) + s.px(18) + s.px(10)
    items_budget_end = s.h - tail_h
    total = 0
    n = 0
    next_promo = g.between(18, 30)
    while y + lh <= items_budget_end:
        name, qty, cents = textgen.receipt_items(g, 1)[0]
        left = name if qty == 1 else f"{qty} X {name}"
        s.text(x0, y, _col_line(left, textgen.money(cents), chars), f)
        total += cents
        y += lh
        n += 1
        if aspect_class in ("long", "strip") and n == next_promo and y + 5 * lh < items_budget_end:
            next_promo += g.between(18, 30)
            s.text(xm, y, "*" * chars, f, "m", record=False)
            y += lh
            for line in s.wrap(g.pick(textgen.POLICY) + " " + textgen.sentence(g).upper(), f, content_w):
                s.text(x0, y, line, f)
                y += lh
            s.text(xm, y, "*" * chars, f, "m", record=False)
            y += lh
    # Tail: totals, payment, codes, footer.
    tax = round(total * 0.07)
    s.text(xm, y, "-" * chars, f, "m", record=False)
    y += lh
    s.text(x0, y, _col_line("SUBTOTAL", textgen.money(total), chars), f)
    y += lh
    s.text(x0, y, _col_line("TAX 7%", textgen.money(tax), chars), f)
    y += lh + lh // 3
    s.text(x0, y + fbig.size // 3, _col_line("TOTAL", textgen.money(total + tax), chars // 2 + 1), fbig)
    y += int(fbig.size * 1.5)
    s.text(x0, y, _col_line("CARD ****" + textgen.digits(g, 4), textgen.money(total + tax), chars), f)
    y += lh
    s.text(x0, y, f"AUTH {textgen.digits(g, 6)}", f)
    y += lh + lh // 2
    ean = textgen.digits(g, 12)
    lin_mods = barcodes.ean13_modules(ean)
    mpx = max(1, content_w // (len(lin_mods) + 20))
    lin = barcodes.draw_linear(lin_mods, mpx, s.px(14))
    s.paste_ink(lin, (s.w - lin.shape[1]) // 2, y)
    y += lin.shape[0] + int(f.size * 0.2)
    s.text(xm, y + lh, ean, f, "m")
    y += lh + lh // 2
    qr = barcodes.qr_modules(f"{shop}/{ean}")
    qpx = max(1, s.px(qr_mm) // (qr.shape[0] + 8))
    qimg = barcodes.draw_qr(qr, qpx)
    if y + qimg.shape[0] < s.h:
        s.paste_ink(qimg, (s.w - qimg.shape[1]) // 2, y)
        y += qimg.shape[0] + lh // 2
    for line in textgen.POLICY[:2]:
        for part in s.wrap(line, f, content_w):
            if y + lh < s.h:
                s.text(xm, y, part, f, "m")
                y += lh
    pr, ir = _pick_colours(g, paper, thermal=True)
    return Page(
        s.finish(), (width_mm, height_mm), ppm, "receipt", "receipt", fam, s.lines, pr, ir, True,
        {"aspect": round(height_mm / width_mm, 3), "items": n, "chars": chars, "total": textgen.money(total + tax)},
    )


def render(rng, aspect: str, paper: str, ppm: float | None = None) -> Page:
    """The page of a scene: a document for ``aspect == "document"``, else a receipt."""
    if aspect == "document":
        return render_document(rng, paper, DOC_PPM if ppm is None else ppm)
    return render_receipt(rng, aspect, paper, RECEIPT_PPM if ppm is None else ppm)
