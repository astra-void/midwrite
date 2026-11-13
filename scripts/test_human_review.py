"""Tests for the DEV-162 human review tooling (no corpus downloads, no network)."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("human_review", Path(__file__).with_name("human-review.py"))
HR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HR)


def item(ident, kind="finding", ai="true-positive", which="held-out", stratum="held-out-finding", tier=1):
    return {"id": ident, "kind": kind, "stratum": stratum, "tier": tier, "project": ident.split(":")[0],
            "set": which, "revision": "rev", "ai": {"classification": ai}}


def entry(ident, hazard="real", actionable="unhandled-bug", **extra):
    base = {"item": ident, "revision": "rev", "hazard": hazard, "actionable": actionable,
            "blind": True, "rationale": "read the code", "codeRefs": ["a.ts:1"]}
    base.update(extra)
    return base


def record(entries, name="A Person", **extra):
    base = {"reviewer": {"name": name, "kind": "human", "independence": "did not write Midwrite"},
            "attestation": True, "completed": "2026-11-20", "items": entries}
    base.update(extra)
    return base


class ValidateTests(unittest.TestCase):
    sample = {"items": [item("p:finding:1"), item("p:finding:2", tier=2, which="dev", stratum="dev-true-positive")]}

    def test_accepts_a_complete_human_record(self):
        self.assertEqual(HR.validate(record([entry("p:finding:1")]), self.sample, complete=True), [])

    def test_rejects_a_non_human_reviewer_and_missing_attestation(self):
        rec = record([entry("p:finding:1")], attestation=False)
        rec["reviewer"]["kind"] = "ai"
        problems = HR.validate(rec, self.sample)
        self.assertTrue(any("kind 'human'" in p for p in problems))
        self.assertTrue(any("attestation" in p for p in problems))

    def test_requires_rationale_code_refs_and_the_sampled_revision(self):
        bad = entry("p:finding:1", rationale="", codeRefs=[], revision="other")
        problems = HR.validate(record([bad]), self.sample)
        self.assertEqual(len(problems), 3, problems)

    def test_keeps_hazard_and_actionable_consistent(self):
        problems = HR.validate(record([entry("p:finding:1", hazard="not-real")]), self.sample)
        self.assertTrue(any("not-applicable" in p for p in problems))
        problems = HR.validate(record([entry("p:finding:1", actionable="not-applicable")]), self.sample)
        self.assertTrue(any("real hazard" in p for p in problems))

    def test_complete_requires_every_tier_one_item(self):
        problems = HR.validate(record([entry("p:finding:2")]), self.sample, complete=True)
        self.assertTrue(any("tier-1" in p for p in problems))

    def test_unreviewed_entries_are_allowed_until_complete(self):
        blank = {"item": "p:finding:1", "revision": "rev", "hazard": None, "actionable": None,
                 "blind": None, "rationale": "", "codeRefs": []}
        self.assertEqual(HR.validate(record([blank]), self.sample), [])


class SummaryTests(unittest.TestCase):
    def test_class_mapping_separates_hazard_from_actionability(self):
        self.assertEqual(HR.human_class(entry("x")), "true-positive")
        self.assertEqual(HR.human_class(entry("x", actionable="accepted-or-mitigated")), "intended-risk")
        self.assertEqual(HR.human_class(entry("x", hazard="not-real", actionable="not-applicable")), "false-positive")
        self.assertEqual(HR.human_class(entry("x", actionable="unclear")), "unclear")

    def run_summary(self, sample, records):
        with patch.object(HR, "sha256_file", return_value="digest"):
            return HR.summarize(sample, records)

    def test_precision_needs_every_held_out_finding(self):
        sample = {"items": [item("p:finding:1"), item("p:finding:2", ai="intended-risk")]}
        partial = self.run_summary(sample, [record([entry("p:finding:1")])])
        self.assertFalse(partial["heldOutPrecision"]["complete"])
        self.assertNotIn("humanHazardPrecision", partial["heldOutPrecision"])
        full = self.run_summary(sample, [record([
            entry("p:finding:1"),
            entry("p:finding:2", hazard="not-real", actionable="not-applicable")])])
        p = full["heldOutPrecision"]
        self.assertEqual(p["humanHazardPrecision"]["value"], 0.5)
        self.assertEqual(p["aiLabelHazardPrecision"]["value"], 1.0)
        self.assertEqual(len(full["disagreementsWithAiLabels"]), 1)

    def test_reviewer_disagreements_are_reported(self):
        sample = {"items": [item("p:finding:1")]}
        out = self.run_summary(sample, [record([entry("p:finding:1")]),
                                        record([entry("p:finding:1", actionable="accepted-or-mitigated")], name="B")])
        self.assertEqual(len(out["disagreementsBetweenReviewers"]), 1)


class AdjudicateTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        (root / "labels").mkdir()
        labels = {"labeling": {}, "corrections": [],
                  "findings": [{"findingId": "1", "classification": "true-positive"},
                               {"findingId": "2", "classification": "intended-risk"}],
                  "knownPositives": [{"id": "p-KP01"}]}
        (root / "labels/p.json").write_text(json.dumps(labels))
        (root / "sets.json").write_text(json.dumps(
            {"projects": [{"name": "p", "set": "held-out", "labels": "labels/p.json"}]}))
        self.patches = [patch.object(HR, "ROOT", root), patch.object(HR, "SETS", root / "sets.json")]
        for p in self.patches:
            p.start()
        self.sample = {"items": [item("p:finding:1"), item("p:finding:2", ai="intended-risk"),
                                 item("p:known-positive:p-KP01", kind="known-positive", ai="valid")]}

    def tearDown(self):
        for p in self.patches:
            p.stop()
        self.tmp.cleanup()

    def test_changes_only_disagreeing_labels_and_logs_them(self):
        rec = record([entry("p:finding:1"),
                      entry("p:finding:2", hazard="not-real", actionable="not-applicable"),
                      entry("p:known-positive:p-KP01", hazard="not-real", actionable="not-applicable")])
        labels, _, changed, unresolved = HR.adjudicate(self.sample, [rec], {}, ["r.json"], "2026-11-20")
        self.assertEqual(unresolved, [])
        self.assertEqual(sorted(changed), ["p:finding:2", "p:known-positive:p-KP01"])
        data = next(iter(labels.values()))
        self.assertEqual(data["findings"][0]["classification"], "true-positive")
        self.assertEqual(data["findings"][1]["classification"], "false-positive")
        self.assertEqual(data["knownPositives"], [])
        kinds = [c["type"] for c in data["corrections"]]
        self.assertEqual(kinds.count("human-adjudication"), 2)
        self.assertIn("human-confirmed", kinds)
        correction = next(c for c in data["corrections"] if c.get("findingId") == "2")
        self.assertEqual(correction["previous"]["classification"], "intended-risk")
        self.assertTrue(correction["humanReviewed"])
        self.assertEqual(data["labeling"]["humanReview"]["itemsReviewed"], 3)

    def test_reviewer_disagreement_needs_a_resolution(self):
        a = record([entry("p:finding:1")])
        b = record([entry("p:finding:1", actionable="accepted-or-mitigated")], name="B")
        _, _, changed, unresolved = HR.adjudicate(self.sample, [a, b], {}, [], "2026-11-20")
        self.assertEqual((changed, unresolved), ([], ["p:finding:1"]))
        resolution = {"p:finding:1": {"classification": "intended-risk", "by": "C", "rationale": "retried"}}
        _, _, changed, unresolved = HR.adjudicate(self.sample, [a, b], resolution, [], "2026-11-20")
        self.assertEqual((changed, unresolved), (["p:finding:1"], []))


class SampleTests(unittest.TestCase):
    def test_sample_is_deterministic_and_covers_every_held_out_label(self):
        first, second = HR.build_sample(), HR.build_sample()
        self.assertEqual(first, second)
        ids = {i["id"] for i in first["items"]}
        self.assertEqual(len(ids), len(first["items"]))
        sets = json.loads(HR.SETS.read_text())
        for project in sets["projects"]:
            if project["set"] != "held-out":
                continue
            labels = json.loads((HR.ROOT / project["labels"]).read_text())
            for f in labels["findings"]:
                self.assertIn(f"{project['name']}:finding:{f['findingId']}", ids)
            for k in labels["knownPositives"]:
                self.assertIn(f"{project['name']}:known-positive:{k['id']}", ids)

    def test_committed_sample_matches_the_rule(self):
        committed = json.loads(HR.SAMPLE.read_text())
        _, stale = HR.current_sample()
        if stale:
            self.skipTest("labels changed since the sample was drawn; redraw with `human-review.py sample`")
        self.assertEqual(committed, HR.build_sample())


if __name__ == "__main__":
    unittest.main()
