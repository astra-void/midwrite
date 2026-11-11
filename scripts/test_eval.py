"""Regression tests for the held-out CI precision gate (no corpus downloads)."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("eval_harness", Path(__file__).with_name("eval.py"))
EVAL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVAL)


def project(name, which="held-out"):
    return {"name": name, "set": which, "revision": "pinned-revision"}


def findings(tp, fp, intended=0, unclear=0, unlabeled=0):
    diagnostics, labels = [], []
    classes = (["true-positive"] * tp + ["false-positive"] * fp
               + ["intended-risk"] * intended + ["unclear"] * unclear
               + [None] * unlabeled)
    for index, classification in enumerate(classes):
        fingerprint = str(index)
        diagnostics.append({"fingerprint": fingerprint, "code": "MW1001",
                            "severity": "warning", "location": {"file": "app.ts", "line": 1},
                            "function": "handler"})
        if classification is not None:
            labels.append({"findingId": fingerprint, "classification": classification})
    return ({"diagnostics": diagnostics, "summary": {"filesAnalyzed": 1, "filesFailed": 0}},
            {"findings": labels, "knownPositives": None})


class PrecisionGateTests(unittest.TestCase):
    def run_gate(self, data, frozen_problem=None, which="held-out"):
        projects = [project(name, split) for name, split in data]
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp) / "eval.json"
            args = SimpleNamespace(project=None, set=which, bin=EVAL.__file__,
                                   output=str(output), gate=True)
            with (patch.object(EVAL, "load_sets", return_value={"projects": projects}),
                  patch.object(EVAL, "head"),
                  patch.object(EVAL, "verify_frozen", return_value=frozen_problem),
                  patch.object(EVAL, "analyze", side_effect=lambda p, _: data[p["name"], p["set"]][0]),
                  patch.object(EVAL, "load_labels", side_effect=lambda p: data[p["name"], p["set"]][1]),
                  contextlib.redirect_stdout(io.StringIO()),
                  contextlib.redirect_stderr(io.StringIO())):
                status = EVAL.run(args)
            return status, json.loads(output.read_text())

    def test_exact_threshold_passes_and_includes_intended_risk(self):
        status, report = self.run_gate({("sample", "held-out"): findings(8, 1, intended=1)})
        self.assertEqual(status, 0)
        self.assertTrue(report["gate"]["passed"])
        self.assertEqual(report["sets"]["held-out"]["hazardPrecision"]["value"], 0.9)

    def test_below_threshold_fails_even_when_display_rounds_to_point_nine(self):
        status, report = self.run_gate({("sample", "held-out"): findings(179, 21)})
        self.assertEqual(status, 1)
        self.assertFalse(report["gate"]["passed"])
        self.assertEqual(report["sets"]["held-out"]["hazardPrecision"]["value"], 0.895)
        self.assertIn("< 0.9", report["gate"]["reasons"][0])

    def test_gate_uses_pooled_counts_and_excludes_dev(self):
        status, report = self.run_gate({("large", "held-out"): findings(18, 0),
                                        ("small", "held-out"): findings(0, 2),
                                        ("dev", "dev"): findings(0, 100)}, which="all")
        self.assertEqual(status, 0)
        self.assertEqual(report["sets"]["held-out"]["hazardPrecision"]["value"], 0.9)
        self.assertEqual(report["sets"]["dev"]["hazardPrecision"]["value"], 0)

    def test_unlabeled_finding_fails_despite_perfect_decided_precision(self):
        status, report = self.run_gate({("sample", "held-out"): findings(10, 0, unlabeled=1)})
        self.assertEqual(status, 1)
        self.assertIn("1 held-out finding(s) unlabeled", report["gate"]["reasons"])

    def test_frozen_input_problem_fails_despite_perfect_precision(self):
        problem = "sample: labels changed since it was frozen"
        status, report = self.run_gate({("sample", "held-out"): findings(10, 0)}, problem)
        self.assertEqual(status, 1)
        self.assertIn(problem, report["gate"]["reasons"])

    def test_no_decided_labels_fails(self):
        for data in (findings(0, 0), findings(0, 0, unclear=2)):
            with self.subTest(data=data):
                status, report = self.run_gate({("sample", "held-out"): data})
                self.assertEqual(status, 1)
                self.assertIn("no decided held-out labels", report["gate"]["reasons"])

    def test_no_held_out_projects_fails(self):
        status, report = self.run_gate({})
        self.assertEqual(status, 1)
        self.assertIsNone(report["gate"])


class FetchTests(unittest.TestCase):
    def test_fetch_set_filter_and_default(self):
        dev, held_out = project("dev", "dev"), project("sample")
        for options, expected in (([], [dev, held_out]), (["--set", "held-out"], [held_out])):
            with (self.subTest(options=options),
                  patch.object(EVAL, "load_sets", return_value={"projects": [dev, held_out]}),
                  patch.object(EVAL, "fetch") as fetch,
                  patch("sys.argv", ["eval.py", "fetch", *options])):
                self.assertEqual(EVAL.main(), 0)
                self.assertEqual([call.args[0] for call in fetch.call_args_list], expected)


if __name__ == "__main__":
    unittest.main()
