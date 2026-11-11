#!/usr/bin/env python3
"""Model-checker regression: `midwrite model-check` of an earlier binary and
of this build must give identical reports (JSON and text) on every model
scenario fixture, in the default search (sleep sets).

Usage: scripts/model-identity.py OLD_BINARY [OUT_FILE] [--ignore FIELD ...]
           [--json-only] [--pattern GLOB ...] [-- EXTRA_MODEL_CHECK_ARGS]

--ignore FIELD  a dotted JSON path removed from both reports before
                comparing, for a field a change is meant to alter (for
                example `exploration.metrics.visitedBytes` when the
                visited-state storage changes, or `tool.version`).
--json-only     compare the JSON reports only (use it with `--ignore
                tool.version` when the text output names the version).
--pattern GLOB  scenario files to compare (relative to the repository;
                repeatable). Default: fixtures/concurrency/scenarios,
                fixtures/concurrency/corpus, fixtures/scenarios/*/ and the
                model-only scenarios of fixtures/model (in-flight, replicas,
                tags). An earlier binary that cannot read a newer scenario
                shows up as DIFFERENT: restrict the patterns to what it
                supports.
Arguments after `--` are passed to both binaries (for example `--dpor`).

Exit status 1 on any difference.
"""
import argparse
import glob
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "midwrite")
PATTERNS = [
    "fixtures/concurrency/scenarios/*.json",
    "fixtures/concurrency/corpus/*.json",
    "fixtures/scenarios/server-actions/*.json",
    "fixtures/scenarios/crash-retry/*.json",
    "fixtures/scenarios/in-flight/*.json",
    "fixtures/scenarios/connection-loss/*.json",
    "fixtures/model/in-flight/*.json",
    "fixtures/model/replicas/*.json",
    "fixtures/model/tags/*.json",
]


def run(binary, scenario, *extra):
    p = subprocess.run([binary, "model-check", scenario, "--no-timing", *extra], capture_output=True, text=True)
    return p.returncode, p.stdout


def strip(report, ignore):
    for path in ignore:
        *parents, last = path.split(".")
        node = report
        for key in parents:
            node = node.get(key) if isinstance(node, dict) else None
            if node is None:
                break
        if isinstance(node, dict):
            node.pop(last, None)
    return report


def main():
    argv = sys.argv[1:]
    extra = []
    if "--" in argv:
        i = argv.index("--")
        argv, extra = argv[:i], argv[i + 1:]
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("old")
    ap.add_argument("out", nargs="?")
    ap.add_argument("--ignore", action="append", default=[])
    ap.add_argument("--json-only", action="store_true")
    ap.add_argument("--pattern", action="append")
    a = ap.parse_args(argv)
    out = open(a.out, "w") if a.out else sys.stdout
    different = 0
    total = 0
    for pattern in a.pattern or PATTERNS:
        for path in sorted(glob.glob(os.path.join(ROOT, pattern))):
            name = os.path.relpath(path, ROOT)
            total += 1
            (c1, j1), (c2, j2) = run(a.old, path, "--format", "json", *extra), run(NEW, path, "--format", "json", *extra)
            same = c1 == c2
            try:
                same = same and strip(json.loads(j1), a.ignore) == strip(json.loads(j2), a.ignore)
            except Exception:
                same = same and j1 == j2
            if a.json_only:
                text_same = True
            else:
                (t1, x1), (t2, x2) = run(a.old, path, *extra), run(NEW, path, *extra)
                text_same = t1 == t2 and x1 == x2
            verdict = "identical" if same and text_same else "DIFFERENT"
            different += verdict != "identical"
            try:
                v = json.loads(j2)["exploration"]
                detail = f"{v['verdict']}, {v['metrics']['states']} states, {v['metrics']['transitions']} transitions"
            except Exception:
                detail = "(not JSON)"
            text = "not compared" if a.json_only else ("=" if text_same else "≠")
            print(f"{name}: {verdict} (json {'=' if same else '≠'}, text {text}; {detail})", file=out, flush=True)
    print(f"\n{total - different} of {total} scenarios identical", file=out)
    sys.exit(1 if different else 0)


if __name__ == "__main__":
    main()
