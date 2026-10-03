# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Unit tests of the instruction-count gate logic (ROADMAP M1.58).

Run: `python3 -m unittest discover -s tools/perf_gate`.

The synthetic summaries follow the gungraun 0.20 summary schema (version 7): one JSON object per
benchmark with `module_path`, `id` and `profiles[].data.total.metrics.Ir.values.new`. If a file
`fixtures/recorded_base.jsonl` exists (a real CI run's output, committed after the first
workflow run) the same checks also run on it, with a synthetic +10% head derived from it.
"""

from __future__ import annotations

import copy
import json
import tempfile
import unittest
from fractions import Fraction
from pathlib import Path

import compare

HERE = Path(__file__).parent

BASE_IR = {
    "gungraun::kernels::decode::jpeg_256x192": 41_230_117,
    "gungraun::kernels::resize::area_512x384_to_128x96": 9_804_512,
    "gungraun::kernels::warp::plain_512x384_to_256x352": 88_441_903,
    "gungraun::kernels::sauvola_w31::sauvola_w31_256x192": 12_345_678,
    "gungraun::kernels::canary::histogram_x1000": 5_000_000,
}


def summary(key: str, ir: int) -> dict:
    module, _, ident = key.rpartition("::")
    # In gungraun the id is the `#[bench::id]` name, the module path ends with the function name.
    function = module.rpartition("::")[2]
    return {
        "version": "7",
        "kind": "LibraryBenchmark",
        "module_path": f"gungraun::kernels::{function}",
        "id": ident,
        "function_name": function,
        "profiles": [
            {
                "tool": "Callgrind",
                "data": {
                    "parts": [],
                    "total": {
                        "metrics": {
                            "Ir": {"values": {"new": ir}},
                            "L1hits": {"values": {"new": ir // 2}},
                        },
                        "regressions": [],
                    },
                },
            }
        ],
    }


def jsonl(irs: dict[str, int]) -> str:
    return "".join(json.dumps(summary(k, v)) + "\n" for k, v in irs.items())


def scaled(irs: dict[str, int], factor: Fraction, only: str | None = None) -> dict[str, int]:
    return {
        k: int(v * factor) if only is None or k == only else v for k, v in irs.items()
    }


class Run:
    """Writes two result files and runs the CLI on them."""

    def __init__(self, base_text: str, head_text: str, *extra: str):
        self.dir = tempfile.TemporaryDirectory()
        d = Path(self.dir.name)
        (d / "base.jsonl").write_text(base_text, encoding="utf-8")
        (d / "head.jsonl").write_text(head_text, encoding="utf-8")
        self.summary = d / "summary.md"
        self.code = compare.main(
            ["--base", str(d / "base.jsonl"), "--head", str(d / "head.jsonl"),
             "--summary", str(self.summary), *extra]
        )

    def __del__(self):
        self.dir.cleanup()


CANARY = "gungraun::kernels::canary::histogram_x1000"


class Classify(unittest.TestCase):
    F, W = Fraction(5), Fraction(2)

    def status(self, base: int, head: int) -> str:
        return compare.classify(base, head, self.F, self.W)

    def test_boundaries_are_exact(self):
        self.assertEqual(self.status(1000, 1000), "ok")
        self.assertEqual(self.status(1000, 1020), "ok")  # exactly +2% does not warn
        self.assertEqual(self.status(1000, 1021), "warn")
        self.assertEqual(self.status(1000, 1050), "warn")  # exactly +5% does not fail
        self.assertEqual(self.status(1000, 1051), "FAIL")
        self.assertEqual(self.status(1000, 979), "faster")
        self.assertEqual(self.status(1000, 980), "ok")

    def test_large_counts_do_not_lose_precision(self):
        base = 10**15
        self.assertEqual(self.status(base, base * 105 // 100), "warn")
        self.assertEqual(self.status(base, base * 105 // 100 + 1), "FAIL")

    def test_zero_base(self):
        self.assertEqual(self.status(0, 0), "ok")
        self.assertEqual(self.status(0, 1), "FAIL")


class Gate(unittest.TestCase):
    def test_noop_passes(self):
        r = Run(jsonl(BASE_IR), jsonl(BASE_IR))
        self.assertEqual(r.code, 0)
        self.assertIn("0 failing, 0 warning", r.summary.read_text())

    def test_injected_10_percent_regression_fails(self):
        r = Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(11, 10), only=CANARY)))
        self.assertEqual(r.code, 1)
        text = r.summary.read_text()
        self.assertIn("+10.00%", text)
        self.assertIn("FAIL", text)

    def test_every_benchmark_10_percent_worse_fails(self):
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(11, 10)))).code, 1)

    def test_between_2_and_5_percent_warns_but_passes(self):
        r = Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(103, 100), only=CANARY)))
        self.assertEqual(r.code, 0)
        text = r.summary.read_text()
        self.assertIn("warn", text)
        self.assertIn("0 failing, 1 warning", text)

    def test_just_under_and_over_5_percent(self):
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(105, 100), only=CANARY))).code, 0)
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(1051, 1000), only=CANARY))).code, 1)

    def test_improvement_passes(self):
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(scaled(BASE_IR, Fraction(80, 100)))).code, 0)

    def test_removed_benchmark_fails(self):
        head = {k: v for k, v in BASE_IR.items() if k != CANARY}
        r = Run(jsonl(BASE_IR), jsonl(head))
        self.assertEqual(r.code, 1)
        self.assertIn("missing", r.summary.read_text())

    def test_new_benchmark_does_not_fail(self):
        head = dict(BASE_IR)
        head["gungraun::kernels::nick_w31::nick_w31_256x192"] = 7
        r = Run(jsonl(BASE_IR), jsonl(head))
        self.assertEqual(r.code, 0)
        self.assertIn("new", r.summary.read_text())

    def test_custom_limits(self):
        irs = scaled(BASE_IR, Fraction(108, 100), only=CANARY)
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(irs), "--fail-pct", "10").code, 0)
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(irs), "--fail-pct", "7.5").code, 1)

    def test_expect_fail_canary_mode(self):
        bad = jsonl(scaled(BASE_IR, Fraction(11, 10), only=CANARY))
        self.assertEqual(Run(jsonl(BASE_IR), bad, "--expect-fail").code, 0)
        # A canary that the gate lets through is itself a failure.
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(BASE_IR), "--expect-fail").code, 1)

    def test_json_array_input(self):
        arr = json.dumps([summary(k, v) for k, v in BASE_IR.items()])
        self.assertEqual(Run(arr, jsonl(BASE_IR)).code, 0)


class BadInput(unittest.TestCase):
    def test_empty_and_garbage_are_usage_errors(self):
        self.assertEqual(Run("", jsonl(BASE_IR)).code, 2)
        self.assertEqual(Run(jsonl(BASE_IR), "not json\n").code, 2)

    def test_missing_callgrind_profile(self):
        s = summary(CANARY, 5)
        s["profiles"][0]["tool"] = "DHAT"
        self.assertEqual(Run(jsonl(BASE_IR), json.dumps(s) + "\n").code, 2)

    def test_duplicate_benchmark(self):
        one = json.dumps(summary(CANARY, 5)) + "\n"
        self.assertEqual(Run(jsonl(BASE_IR), one + one).code, 2)

    def test_bad_limits(self):
        self.assertEqual(Run(jsonl(BASE_IR), jsonl(BASE_IR), "--fail-pct", "1", "--warn-pct", "2").code, 2)


class Recorded(unittest.TestCase):
    """The same logic on a real gungraun run, once one has been committed."""

    path = HERE / "fixtures" / "recorded_base.jsonl"

    @unittest.skipUnless(path.exists(), "no recorded fixture committed yet")
    def test_recorded_noop_and_injected_regression(self):
        text = self.path.read_text(encoding="utf-8")
        base = compare.parse_results(text)
        self.assertGreater(len(base), 3)
        self.assertEqual(Run(text, text).code, 0)
        # +10% on every instruction count of one benchmark: the gate must block it.
        victim = sorted(base)[0]
        out = []
        for line in text.splitlines():
            doc = json.loads(line)
            if compare.bench_key(doc) == victim:
                doc = copy.deepcopy(doc)
                for p in doc["profiles"]:
                    if p["tool"] == "Callgrind":
                        v = p["data"]["total"]["metrics"]["Ir"]["values"]
                        v["new"] = int(v["new"]) * 11 // 10
            out.append(json.dumps(doc))
        r = Run(text, "\n".join(out) + "\n")
        self.assertEqual(r.code, 1)
        self.assertIn(victim, r.summary.read_text())


if __name__ == "__main__":
    unittest.main()
