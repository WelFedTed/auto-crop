#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Instruction-count gate (ROADMAP M1.58): compare two gungraun result sets.

Input: the JSON that `cargo bench --bench gungraun -- --output-format=json` prints to stdout, one
benchmark summary per line (a JSON array of summaries is accepted too), once for the base build
and once for the head build. Only Callgrind `Ir` (instructions executed) is compared: it is
deterministic, so the thresholds can be tight.

A benchmark is *worse* by (head - base) / base:
  * more than --fail-pct (default 5)  -> FAIL, exit status 1 (blocks the change)
  * more than --warn-pct (default 2)  -> WARN, exit status 0
  * a benchmark present in base but absent in head -> FAIL (a gate must not be dodged by deleting
    its benchmark); one only in head is reported as NEW and does not fail.

`--expect-fail` is the canary mode: exit 0 only if the gate *failed* (an injected slowdown must be
caught), exit 1 if it passed. Exit status 2 is a usage or parse error.

Pure standard library; Python 3.9 or newer.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path
from typing import Iterable

EVENT = "Ir"


class GateError(Exception):
    """The input could not be understood (not a regression)."""


def bench_key(summary: dict) -> str:
    module = summary.get("module_path") or summary.get("function_name")
    if not module:
        raise GateError("summary without module_path/function_name")
    ident = summary.get("id")
    return f"{module}::{ident}" if ident else str(module)


def _callgrind_total_ir(summary: dict) -> int:
    profiles = summary.get("profiles")
    if isinstance(profiles, dict):  # tolerate {"profiles": {"0": {...}}}
        profiles = list(profiles.values())
    if not isinstance(profiles, list):
        raise GateError(f"{bench_key(summary)}: no profiles")
    for profile in profiles:
        if str(profile.get("tool")) != "Callgrind":
            continue
        metrics = profile["data"]["total"]["metrics"]
        ir = metrics.get(EVENT)
        if ir is None:
            raise GateError(f"{bench_key(summary)}: Callgrind total has no {EVENT}")
        values = ir["values"]
        if "new" not in values:
            raise GateError(f"{bench_key(summary)}: {EVENT} has no new value")
        return int(values["new"])
    raise GateError(f"{bench_key(summary)}: no Callgrind profile")


def parse_results(text: str) -> dict[str, int]:
    """Maps benchmark key -> instructions, from JSON lines or a JSON array."""
    docs: list = []
    stripped = text.strip()
    if not stripped:
        return {}
    if stripped.startswith("["):
        docs.extend(json.loads(stripped))
    else:
        for n, line in enumerate(stripped.splitlines(), 1):
            line = line.strip()
            if not line:
                continue
            try:
                docs.append(json.loads(line))
            except json.JSONDecodeError as e:
                raise GateError(f"line {n} is not JSON: {e}") from e
    out: dict[str, int] = {}
    for doc in docs:
        key = bench_key(doc)
        if key in out:
            raise GateError(f"duplicate benchmark {key}")
        out[key] = _callgrind_total_ir(doc)
    return out


@dataclass(frozen=True)
class Row:
    key: str
    base: int | None
    head: int | None
    status: str  # ok | warn | FAIL | missing | new | faster

    @property
    def pct(self) -> Fraction | None:
        if self.base is None or self.head is None or self.base == 0:
            return None
        return Fraction(self.head - self.base, self.base) * 100


def classify(base: int, head: int, fail_pct: Fraction, warn_pct: Fraction) -> str:
    # Exact integer arithmetic: head > base * (1 + pct/100), no float rounding at the threshold.
    if base == 0:
        return "ok" if head == 0 else "FAIL"
    if head * 100 > base * (100 + fail_pct):
        return "FAIL"
    if head * 100 > base * (100 + warn_pct):
        return "warn"
    if head * 100 < base * (100 - warn_pct):
        return "faster"
    return "ok"


def compare(
    base: dict[str, int], head: dict[str, int], fail_pct: Fraction, warn_pct: Fraction
) -> list[Row]:
    rows = []
    for key in sorted(set(base) | set(head)):
        b, h = base.get(key), head.get(key)
        if b is None:
            rows.append(Row(key, None, h, "new"))
        elif h is None:
            rows.append(Row(key, b, None, "missing"))
        else:
            rows.append(Row(key, b, h, classify(b, h, fail_pct, warn_pct)))
    return rows


def failed(rows: Iterable[Row]) -> bool:
    return any(r.status in ("FAIL", "missing") for r in rows)


def render(rows: list[Row], fail_pct: Fraction, warn_pct: Fraction, title: str) -> str:
    def cell(v: int | None) -> str:
        return "-" if v is None else f"{v:,}"

    lines = [
        f"### {title}",
        "",
        f"Callgrind `{EVENT}` (instructions), head vs base: fail above +{float(fail_pct):g}%, "
        f"warn above +{float(warn_pct):g}%.",
        "",
        "| benchmark | base | head | change | status |",
        "|---|---:|---:|---:|---|",
    ]
    for r in rows:
        pct = "-" if r.pct is None else f"{float(r.pct):+.2f}%"
        lines.append(f"| `{r.key}` | {cell(r.base)} | {cell(r.head)} | {pct} | {r.status} |")
    n_fail = sum(r.status in ("FAIL", "missing") for r in rows)
    n_warn = sum(r.status == "warn" for r in rows)
    lines += ["", f"{len(rows)} benchmarks: {n_fail} failing, {n_warn} warning."]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", required=True, type=Path, help="gungraun JSON of the base build")
    ap.add_argument("--head", required=True, type=Path, help="gungraun JSON of the head build")
    ap.add_argument("--fail-pct", default="5", help="fail when worse by more than this (default 5)")
    ap.add_argument("--warn-pct", default="2", help="warn when worse by more than this (default 2)")
    ap.add_argument("--title", default="Instruction-count gate")
    ap.add_argument("--summary", type=Path, help="append the Markdown table here ($GITHUB_STEP_SUMMARY)")
    ap.add_argument("--expect-fail", action="store_true", help="canary: succeed only if the gate fails")
    args = ap.parse_args(argv)
    try:
        fail_pct, warn_pct = Fraction(args.fail_pct), Fraction(args.warn_pct)
        if not 0 <= warn_pct <= fail_pct:
            raise GateError("need 0 <= warn-pct <= fail-pct")
        base = parse_results(args.base.read_text(encoding="utf-8"))
        head = parse_results(args.head.read_text(encoding="utf-8"))
        if not base or not head:
            raise GateError("an input holds no benchmarks (did the benchmark run?)")
    except (GateError, OSError, ValueError, KeyError, json.JSONDecodeError) as e:
        print(f"perf-gate: {type(e).__name__}: {e}", file=sys.stderr)
        return 2
    rows = compare(base, head, fail_pct, warn_pct)
    report = render(rows, fail_pct, warn_pct, args.title)
    print(report)
    if args.summary:
        with args.summary.open("a", encoding="utf-8") as f:
            f.write(report)
    bad = failed(rows)
    if args.expect_fail:
        if bad:
            print("perf-gate canary: the injected slowdown was caught (the gate failed as it must).")
            return 0
        print("perf-gate canary: the gate PASSED an injected slowdown; it is broken.", file=sys.stderr)
        return 1
    if bad:
        print("perf-gate: FAILED (a benchmark is worse than the limit or went missing).", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
