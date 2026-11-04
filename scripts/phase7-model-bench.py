#!/usr/bin/env python3
"""Phase 7 model-checker measurements.

1. Phase 6 vs Phase 7 binary on the Phase 6 fixture scenarios (median of N
   runs of the reported exploration wall time, states, transitions).
2. The extracted team-app invite scenario with 2, 3 and 4 concurrent
   submitters (joins, generated ids): states, transitions, wall time.
3. Cost of --validate-postgres (model-SQL replay) and of --verify-source
   (original code) on the two team-app scenarios (process wall time).

Usage: scripts/phase7-model-bench.py PHASE6_BINARY OUT.md [N]
"""
import glob
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "midwrite")


def mc(binary, scenario, *extra):
    t0 = time.perf_counter()
    p = subprocess.run([binary, "model-check", scenario, "--format", "json", *extra], capture_output=True, text=True)
    elapsed = (time.perf_counter() - t0) * 1000
    d = json.loads(p.stdout)
    return d, elapsed


def main():
    old, out = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    lines = ["# Phase 7 model-checker measurements", "", f"Median of {n} runs; `wallMs` is the exploration time the report states.", ""]
    lines += ["## Phase 6 vs Phase 7 binary, Phase 6 fixture scenarios", "",
              "| scenario | verdict | states | transitions | Phase 6 wall ms | Phase 7 wall ms |", "| --- | --- | --- | --- | --- | --- |"]
    for s in sorted(glob.glob(os.path.join(ROOT, "fixtures/concurrency/scenarios/*.json")) + glob.glob(os.path.join(ROOT, "fixtures/concurrency/corpus/*.json"))):
        a = [mc(old, s) for _ in range(n)]
        b = [mc(NEW, s) for _ in range(n)]
        da, db = a[0][0], b[0][0]
        assert da["exploration"]["verdict"] == db["exploration"]["verdict"], s
        assert da["exploration"]["metrics"]["states"] == db["exploration"]["metrics"]["states"], s
        wa = statistics.median(x[0]["timing"]["wallMs"] for x in a)
        wb = statistics.median(x[0]["timing"]["wallMs"] for x in b)
        lines.append(f"| {os.path.basename(s)} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | {wa} | {wb} |")
    lines += ["", "## Extracted team-app invite scenario, more submitters", "",
              "| actors | verdict | states | transitions | wall ms (exploration) |", "| --- | --- | --- | --- | --- |"]
    base = json.load(open(os.path.join(ROOT, "fixtures/phase7/scenarios/team-app-invite-double-submit.json")))
    for k in [2, 3, 4]:
        doc = json.loads(json.dumps(base))
        doc.pop("sourceVerification", None)
        doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
        doc["bounds"] = {"maxSteps": 80, "maxActors": 4, "maxStates": 2000000}
        doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/phase7/team-app")
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump(doc, f)
            path = f.name
        runs = [mc(NEW, path) for _ in range(n)]
        os.unlink(path)
        d = runs[0][0]
        lines.append(f"| {k} | {d['exploration']['verdict']} | {d['exploration']['metrics']['states']} | {d['exploration']['metrics']['transitions']} | {statistics.median(r[0]['timing']['wallMs'] for r in runs)} |")
    lines += ["", "## Runtime verification cost (process wall time, ms)", "",
              "| scenario | model only | --validate-postgres | --verify-source (1 run) | --verify-source --source-repeat 5 |", "| --- | --- | --- | --- | --- |"]
    for name in ["team-app-invite-double-submit.json", "team-app-accept-invitation-twice.json"]:
        s = os.path.join(ROOT, "fixtures/phase7/scenarios", name)
        row = [statistics.median(mc(NEW, s)[1] for _ in range(n)),
               statistics.median(mc(NEW, s, "--validate-postgres")[1] for _ in range(3)),
               statistics.median(mc(NEW, s, "--verify-source", "--allow-execution")[1] for _ in range(3)),
               mc(NEW, s, "--verify-source", "--allow-execution", "--source-repeat", "5")[1]]
        lines.append(f"| {name} | " + " | ".join(f"{x:.0f}" for x in row) + " |")
    open(out, "w").write("\n".join(lines) + "\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
