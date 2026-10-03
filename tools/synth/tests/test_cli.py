# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""Command line behaviour that does not need a rendered suite."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from synth import plan  # noqa: E402


def run(args, env_extra=None):
    env = {**os.environ, "PYTHONPATH": str(ROOT), **(env_extra or {})}
    return subprocess.run([sys.executable, "-m", "synth", *args], capture_output=True, text=True, env=env, cwd=ROOT)


class OcrCheckTests(unittest.TestCase):
    def test_a_missing_tesseract_is_reported_loudly_and_skips_unless_required(self):
        with tempfile.TemporaryDirectory() as empty:
            env = {"PATH": empty, "SYSTEMROOT": os.environ.get("SYSTEMROOT", "")}
            skip = run(["check-ocr"], env)
            self.assertEqual(skip.returncode, 0, skip.stderr)
            self.assertIn("SKIPPED", skip.stderr)
            self.assertIn("NOT run", skip.stderr)
            req = run(["check-ocr", "--require"], env)
            self.assertEqual(req.returncode, 3)
            self.assertIn("FAIL", req.stderr)


class PinTests(unittest.TestCase):
    def test_pins_fix_a_tag_and_unknown_pins_are_refused(self):
        _, images = plan.build("p", 3, 30, plan.pins_to_overrides(["lighting=dim", "clutter=none", "aspect=receipt"]))
        self.assertEqual({i.lighting for i in images}, {"dim"})
        self.assertEqual({i.clutter for i in images}, {"none"})
        self.assertEqual({i.scene.aspect for i in images}, {"receipt"})
        with self.assertRaises(ValueError):
            plan.pins_to_overrides(["lighting=purple"])
        with self.assertRaises(ValueError):
            plan.pins_to_overrides(["nope=1"])


class QuotaCommandTests(unittest.TestCase):
    def test_quotas_command_passes_a_full_plan_and_fails_a_thin_one(self):
        _, images = plan.build("q", 9, 600)
        with tempfile.TemporaryDirectory() as d:
            m = Path(d) / "manifest.jsonl"
            m.write_text("".join(json.dumps({"tags": i.tags()}) + "\n" for i in images), encoding="utf8")
            thin = run(["quotas", str(m)])
            self.assertEqual(thin.returncode, 1)
            self.assertIn("QUOTA", thin.stderr)
            ok = run(["quotas", str(m), "--min", "5"])
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertIn("quotas met", ok.stdout)


class LicenceCommandTests(unittest.TestCase):
    def test_the_licence_audit_passes_on_the_locked_environment_and_catches_a_banned_name(self):
        ok = run(["licences"])
        self.assertEqual(ok.returncode, 0, ok.stdout + ok.stderr)
        with tempfile.TemporaryDirectory() as d:
            lock = Path(d) / "requirements.lock"
            lock.write_text("albumentationsx==2.0.0 \\\n    --hash=sha256:" + "0" * 64 + "\n", encoding="utf8")
            bad = run(["licences", "--lock", str(lock)])
            self.assertEqual(bad.returncode, 1)
            self.assertIn("banned package", bad.stderr)


if __name__ == "__main__":
    unittest.main()
