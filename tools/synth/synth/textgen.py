# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Generated text with a known transcript (ROADMAP M1.31).

Everything here is invented by this project from short word lists written for it: no quotation of
any book, price list, retailer or person, so there is nothing to license and nothing real to leak.
Sentences are grammatical enough for an OCR language model to behave normally, and the amounts use
two decimal places so the amount-token checks of M7 can use them. Names of shops and people are
made up and unrelated to any real business.
"""

from __future__ import annotations

SUBJECTS = [
    "the committee", "our team", "the customer", "a small workshop", "the regional office",
    "every volunteer", "the new system", "the board", "a local farmer", "the review panel",
    "the project manager", "an older neighbour", "the library", "the delivery service",
    "the maintenance crew", "each department", "the young engineer", "the harbour authority",
]
VERBS = [
    "reviewed", "approved", "delayed", "completed", "repaired", "described", "requested",
    "measured", "ordered", "compared", "published", "postponed", "inspected", "prepared",
    "collected", "organised", "replaced", "recorded", "confirmed", "balanced",
]
OBJECTS = [
    "the quarterly figures", "a list of open questions", "several broken lamps",
    "the annual schedule", "two heavy boxes of paper", "the updated safety rules",
    "an invoice for materials", "the water supply report", "a long set of instructions",
    "the winter timetable", "three samples of cloth", "the budget for next year",
    "a detailed map of the valley", "the contract for the roof", "a small garden shed",
]
ENDINGS = [
    "before the end of the month", "without any further delay", "on a quiet Tuesday morning",
    "after a long discussion", "in the presence of both parties", "as agreed last spring",
    "despite the heavy rain", "with great care", "for the third time this year",
    "at the request of the council", "during the afternoon", "using the old method",
]
HEADINGS = [
    "Summary", "Background", "Findings", "Next steps", "Schedule", "Budget notes",
    "Site report", "Minutes", "Terms", "Appendix", "Overview", "Decisions",
]
SHOP_A = ["Green", "Blue", "Corner", "Harbour", "Maple", "Old Mill", "Sunrise", "Northgate",
          "Riverside", "Golden", "Willow", "Station"]
SHOP_B = ["Market", "Pharmacy", "Bakery", "Hardware", "Grocer", "Cafe", "Books", "Deli",
          "Supplies", "Fuel", "Stationery", "Foods"]
STREETS = ["High Street", "Mill Road", "Station Approach", "Park Lane", "Church Row",
           "Orchard Way", "Bridge Street", "Canal Walk", "Linden Avenue", "Quay Road"]
TOWNS = ["Ashford", "Brookton", "Carden", "Dunmere", "Eastvale", "Fenwick", "Garsby", "Holm"]
ITEMS = [
    "WHOLE MILK 2L", "BROWN BREAD", "FREE RANGE EGGS 12", "BANANAS", "RED APPLES 1KG",
    "CHEDDAR 400G", "BASMATI RICE 1KG", "OLIVE OIL 500ML", "TOMATOES 500G", "COFFEE BEANS",
    "TEA BAGS 80", "BUTTER 250G", "PASTA 500G", "ORANGE JUICE", "DISH SOAP", "BATTERIES AA 4",
    "NOTEBOOK A5", "BLUE PEN 3PK", "PAPER TOWELS", "HAND CREAM", "PAINKILLERS 16",
    "CHICKEN BREAST", "CARROTS 1KG", "ONIONS 1KG", "YOGURT 500G", "FLOUR 1.5KG",
    "SUGAR 1KG", "LIGHT BULB E27", "MASKING TAPE", "GARDEN TWINE", "SPARKLING WATER",
    "OATMEAL 750G", "HONEY 340G", "LEMONS 4PK", "WASHING LIQUID", "TOOTHPASTE",
]
DOC_ITEMS = [
    "Consulting hours", "Printed manuals", "Replacement valve", "Site visit", "Cable, 25 m",
    "Safety training", "Filter cartridge", "Storage rack", "Courier fee", "Maintenance plan",
    "Concrete mix", "Paint, 10 L", "Fastener set", "Survey work", "Installation labour",
]
FIRST = ["Alma", "Boris", "Chen", "Dara", "Elif", "Farid", "Greta", "Hugo", "Ines", "Jonas",
         "Kavya", "Lena", "Mateo", "Nadia", "Omar", "Petra"]
LAST = ["Ashby", "Brandt", "Castell", "Dunne", "Ekman", "Fischer", "Grover", "Hale", "Ibsen",
        "Jarvis", "Keane", "Lindqvist", "Moreau", "Nash", "Okafor", "Pryce"]
FORM_FIELDS = [
    "Full name", "Date of birth", "Street address", "Town or city", "Postal code", "Telephone",
    "Reference number", "Date of visit", "Department", "Signature", "Amount paid", "Notes",
]
POLICY = [
    "Items may be returned within 30 days with this receipt.",
    "Unopened goods only. Perishable goods cannot be returned.",
    "Please keep this receipt for your records.",
    "Prices include sales tax where applicable.",
    "Ask at the desk about our loyalty scheme.",
    "Thank you for shopping with us today.",
    "Opening hours: Monday to Saturday 8 am to 8 pm.",
    "Tell us how we did and enter the monthly draw.",
]


class Rand:
    """Thin helpers over a NumPy generator so text code reads naturally."""

    def __init__(self, rng):
        self.r = rng

    def pick(self, seq):
        return seq[int(self.r.integers(len(seq)))]

    def between(self, lo: int, hi: int) -> int:
        return int(self.r.integers(lo, hi + 1))

    def chance(self, p: float) -> bool:
        return bool(self.r.random() < p)


def sentence(g: Rand) -> str:
    parts = [g.pick(SUBJECTS), g.pick(VERBS), g.pick(OBJECTS)]
    if g.chance(0.7):
        parts.append(g.pick(ENDINGS))
    s = " ".join(parts) + "."
    return s[0].upper() + s[1:]


def paragraph(g: Rand, n_sentences: int) -> str:
    return " ".join(sentence(g) for _ in range(n_sentences))


def money(cents: int) -> str:
    sign = "-" if cents < 0 else ""
    cents = abs(cents)
    return f"{sign}{cents // 100}.{cents % 100:02d}"


def price_cents(g: Rand, lo: int = 49, hi: int = 2999) -> int:
    return g.between(lo, hi)


def shop_name(g: Rand) -> str:
    return f"{g.pick(SHOP_A)} {g.pick(SHOP_B)}".upper()


def address(g: Rand) -> list[str]:
    return [
        f"{g.between(1, 240)} {g.pick(STREETS).upper()}",
        f"{g.pick(TOWNS).upper()} {g.between(10, 99)}{g.pick('ABCDEFGH')} {g.between(1, 9)}{g.pick('JKLMNPQR')}{g.pick('JKLMNPQR')}",
    ]


def person(g: Rand) -> str:
    return f"{g.pick(FIRST)} {g.pick(LAST)}"


def date_time(g: Rand) -> tuple[str, str]:
    return (
        f"{g.between(1, 28):02d}/{g.between(1, 12):02d}/{g.between(2022, 2026)}",
        f"{g.between(7, 21):02d}:{g.between(0, 59):02d}",
    )


def receipt_items(g: Rand, n: int) -> list[tuple[str, int, int]]:
    """``n`` line items as (name, quantity, line total in cents)."""
    out = []
    for _ in range(n):
        qty = 1 if g.chance(0.8) else g.between(2, 4)
        unit = price_cents(g, 39, 1899)
        out.append((g.pick(ITEMS), qty, unit * qty))
    return out


def invoice_items(g: Rand, n: int) -> list[tuple[str, int, int, int]]:
    """``n`` invoice rows as (description, quantity, unit price cents, total cents)."""
    out = []
    for _ in range(n):
        qty = g.between(1, 40)
        unit = price_cents(g, 250, 18999)
        out.append((g.pick(DOC_ITEMS), qty, unit, qty * unit))
    return out


def digits(g: Rand, n: int) -> str:
    return "".join(str(g.between(0, 9)) for _ in range(n))
