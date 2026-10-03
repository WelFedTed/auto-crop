# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors
"""The generator shares no code with the application, and no banned package enters the lock.

ROADMAP M1.32: CI fails if `tools/synth` imports app code. The application is Rust; the only ways
a Python tool could depend on it are an import of a built extension module, a subprocess call to
its binaries or to cargo, or reading its sources. All three are checked here, and the same rule
runs as an xtask CI guard (`cargo xtask ci-guards`, rule `synth-independence`) so a change to
`tools/synth` cannot skip it.
"""

import ast
import re
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

ALLOWED_IMPORTS = {
    "__future__", "argparse", "ast", "collections", "concurrent", "difflib", "dataclasses", "functools", "hashlib", "importlib",
    "io", "json", "math", "multiprocessing", "os", "pathlib", "random", "re", "shutil", "struct",
    "subprocess", "sys", "tempfile", "time", "typing", "unittest",
    "augraphy", "barcode", "cv2", "numpy", "PIL", "segno", "skimage", "synth", "matplotlib",
}
FORBIDDEN_TEXT = re.compile(r"auto[_-]crop|crates/|cargo|imgproc|auto-crop-eval", re.I)


def python_files():
    return [p for p in ROOT.rglob("*.py") if ".venv" not in p.parts and "__pycache__" not in p.parts]


class IndependenceTests(unittest.TestCase):
    def test_only_vetted_modules_are_imported(self):
        for path in python_files():
            tree = ast.parse(path.read_text(encoding="utf8"))
            for node in ast.walk(tree):
                names = []
                if isinstance(node, ast.Import):
                    names = [a.name.split(".")[0] for a in node.names]
                elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
                    names = [node.module.split(".")[0]]
                for n in names:
                    self.assertIn(n, ALLOWED_IMPORTS, f"{path.relative_to(ROOT)} imports {n}")

    def test_no_reference_to_the_application_in_code(self):
        """Names and string literals (docstrings and comments are prose and may mention it)."""
        for path in python_files():
            if path.name == "test_independence.py":
                continue
            tree = ast.parse(path.read_text(encoding="utf8"))
            docstrings = set()
            for node in ast.walk(tree):
                if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
                    body = node.body
                    if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant):
                        docstrings.add(id(body[0].value))
            for node in ast.walk(tree):
                text = None
                if isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in docstrings:
                    text = node.value
                elif isinstance(node, ast.Name):
                    text = node.id
                elif isinstance(node, ast.Attribute):
                    text = node.attr
                if text is not None:
                    self.assertIsNone(FORBIDDEN_TEXT.search(text), f"{path.relative_to(ROOT)}:{node.lineno}: {text!r}")

    def test_augraphy_is_imported_in_one_module_only(self):
        users = []
        for path in python_files():
            if path.parent.name != "synth":
                continue
            tree = ast.parse(path.read_text(encoding="utf8"))
            for node in ast.walk(tree):
                if isinstance(node, ast.Import) and any(a.name.split(".")[0] == "augraphy" for a in node.names):
                    users.append(path.name)
                if isinstance(node, ast.ImportFrom) and node.level == 0 and (node.module or "").split(".")[0] == "augraphy":
                    users.append(path.name)
        self.assertEqual(set(users), {"degrade.py"})

    def test_albumentations_is_not_in_the_requirements_or_the_lock(self):
        for name in ("requirements.txt", "requirements.lock"):
            text = (ROOT / name).read_text(encoding="utf8").lower()
            self.assertNotIn("albumentations", text, name)

    def test_the_lock_is_hashed_and_pins_augraphy(self):
        lock = (ROOT / "requirements.lock").read_text(encoding="utf8")
        entries = re.findall(r"^([A-Za-z0-9_.\-]+)==([^\s\\]+)", lock, re.M)
        self.assertGreater(len(entries), 20)
        self.assertEqual(len(re.findall(r"--hash=sha256:[0-9a-f]{64}", lock)) >= len(entries), True)
        self.assertIn(("augraphy", "8.2.6"), entries)
        for name, line in re.findall(r"^([A-Za-z0-9_.\-]+)==[^\n]*\n((?:\s+--hash[^\n]*\n)+)", lock, re.M):
            self.assertTrue(line.strip(), name)
        self.assertNotIn("opencv-python==", lock, "the GUI wheel would overwrite opencv-python-headless")

    def test_every_top_level_requirement_is_pinned_exactly(self):
        for line in (ROOT / "requirements.txt").read_text(encoding="utf8").split():
            self.assertRegex(line, r"^[A-Za-z0-9_.\-]+==[0-9][^\s]*$")


if __name__ == "__main__":
    unittest.main()
