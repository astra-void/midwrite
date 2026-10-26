#!/usr/bin/env python3
"""Phase 9 regression: `commitrift model-check` of the Phase 8 binary and of
this build must give identical reports (JSON, text) on every model scenario of
Phases 6-8. The only field allowed to differ is
`exploration.metrics.visitedBytes`: it measures the visited-state table, which
Phase 9 replaced (interned fragment keys instead of flat encodings). Everything
else - verdict, state and transition counts, schedules, witnesses, invariants,
truncation, assumptions, diagnostics - must be equal.

Usage: scripts/phase9-model-identity.py PHASE8_BINARY [OUT_FILE]
Exit status 1 on any difference.
"""
import glob
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "commitrift")
PATTERNS = [
    "fixtures/concurrency/scenarios/*.json",
    "fixtures/concurrency/corpus/*.json",
    "fixtures/phase7/scenarios/*.json",
    "fixtures/phase8/scenarios/*.json",
]


def run(binary, scenario, *extra):
    p = subprocess.run([binary, "model-check", scenario, "--no-timing", *extra], capture_output=True, text=True)
    return p.returncode, p.stdout


def strip(report):
    report["exploration"]["metrics"].pop("visitedBytes", None)
    return report


def main():
    old = sys.argv[1]
    out = open(sys.argv[2], "w") if len(sys.argv) > 2 else sys.stdout
    different = 0
    total = 0
    for pattern in PATTERNS:
        for path in sorted(glob.glob(os.path.join(ROOT, pattern))):
            name = os.path.relpath(path, ROOT)
            total += 1
            (c1, j1), (c2, j2) = run(old, path, "--format", "json"), run(NEW, path, "--format", "json")
            same = c1 == c2
            try:
                same = same and strip(json.loads(j1)) == strip(json.loads(j2))
            except Exception:
                same = same and j1 == j2
            (t1, x1), (t2, x2) = run(old, path), run(NEW, path)
            text_same = t1 == t2 and x1 == x2
            verdict = "identical" if same and text_same else "DIFFERENT"
            different += verdict != "identical"
            try:
                v = json.loads(j2)["exploration"]
                detail = f"{v['verdict']}, {v['metrics']['states']} states, {v['metrics']['transitions']} transitions"
            except Exception:
                detail = "(not JSON)"
            print(f"{name}: {verdict} (json {'=' if same else '≠'}, text {'=' if text_same else '≠'}; {detail})", file=out, flush=True)
    print(f"\n{total - different} of {total} scenarios identical", file=out)
    sys.exit(1 if different else 0)


if __name__ == "__main__":
    main()
