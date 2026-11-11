"""Verify current trace embedding in verification reports, starting from a recorded Linux run."""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest

import jsonschema

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("report_schemas", ROOT / "scripts/validate-schemas.py")
report_schemas = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report_schemas)


class VerificationSchemas(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        schema = json.loads((ROOT / "docs/reference/schemas/verification-report.v2.schema.json").read_text())
        cls.validator = jsonschema.validators.validator_for(schema)(schema, registry=report_schemas.registry())
        cls.recorded = json.loads((ROOT / "fixtures/reports/replay/generated-harness.linux.json").read_text())
        binary = os.environ.get("MIDWRITE_BIN", str(ROOT / "target/release/midwrite"))
        output = subprocess.run(
            [binary, "trace", str(ROOT / "fixtures/vulnerable"),
             "--config", str(ROOT / "fixtures/midwrite.json"), "--format", "json"],
            check=True, capture_output=True, text=True, cwd=ROOT,
        )
        traces = json.loads(output.stdout)["traces"]
        trace = next(trace for trace in traces if trace.get("valueRelation") is not None)
        cls.current = copy.deepcopy(cls.recorded)
        cls.current["cases"][0]["trace"] = trace
        cls.current["cases"][0]["traceId"] = trace["id"]

    def test_recorded_verification_report_validates(self):
        self.validator.validate(self.recorded)

    def test_verification_report_accepts_a_current_generated_trace(self):
        self.validator.validate(self.current)

    def test_invalid_value_relation_is_rejected(self):
        invalid = copy.deepcopy(self.current)
        invalid["cases"][0]["trace"]["valueRelation"]["status"] = "not-a-relation"
        with self.assertRaises(jsonschema.ValidationError):
            self.validator.validate(invalid)


if __name__ == "__main__":
    unittest.main()
