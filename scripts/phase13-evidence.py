#!/usr/bin/env python3
"""Phase 13 evidence: the vertical slices, model and original code.

Usage: scripts/phase13-evidence.py [--repeat N]

Writes reports/phase13/evidence/. Application code runs ONLY for the
first-party applications fixtures/phase13/request-app and
fixtures/phase12/workspace-app (`--verify-source --allow-execution`), on
disposable PostgreSQL clusters. The read-replica slice is model-only: no
PostgreSQL replication is run. Exits 1 unless every counterexample with a
harness reproduced in every run and every safe counterpart holds.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
OUT = os.path.join(ROOT, "reports", "phase13", "evidence")
REQ = os.path.join(ROOT, "fixtures", "phase13", "request-app", "scenarios")
WS = os.path.join(ROOT, "fixtures", "phase12", "workspace-app")

# (slice, scenario, expected: "reproduce" or "hold" or "model-only")
REQUEST = [
    ("2 return value", "claim-race", "reproduce"),
    ("2 return value (safe)", "claim-atomic", "hold"),
    ("3 last-statement crash", "signup-crash", "reproduce"),
    ("3 last-statement crash (safe)", "signup-idempotent", "hold"),
    ("E upsert", "reserve-race", "reproduce"),
    ("E upsert (safe)", "reserve-if-free", "hold"),
    ("E upsert (safe)", "count-views", "hold"),
]


def model_check(path, repeat=None):
    args = [BIN, "model-check", path, "--format", "json", "--no-timing"]
    if repeat:
        args += ["--verify-source", "--allow-execution", "--source-repeat", str(repeat)]
    p = subprocess.run(args, capture_output=True, text=True)
    return json.loads(p.stdout)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    a = ap.parse_args()
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    os.makedirs(os.path.join(OUT, "scenarios"))
    rows, failures = [], 0

    def record(slice_, name, mc, expect):
        nonlocal failures
        verdict = mc["exploration"]["verdict"]
        sv = mc.get("sourceVerification") or {}
        runs = sv.get("runs") or []
        repro = sum(1 for r in runs if r.get("reproduced"))
        ok = (expect == "hold" and verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS") or \
             (expect == "model-only" and verdict == "COUNTEREXAMPLE_FOUND") or \
             (expect == "reproduce" and verdict == "COUNTEREXAMPLE_FOUND" and runs and repro == len(runs))
        failures += 0 if ok else 1
        rows.append(f"| {slice_} | `{name}` | {verdict} | {mc['exploration']['metrics']['states'] if 'metrics' in mc['exploration'] else '-'} | "
                    f"{mc['evidence']['level']} | {f'{repro}/{len(runs)}' if runs else '-'} | {'ok' if ok else 'FAILED'} |")
        # A selector can contain `/` (`createWorkspaceAction/arg[0]`): it is
        # not a directory (closeout fix: the first run stopped here).
        with open(os.path.join(OUT, name.replace("/", "_") + ".json"), "w") as f:
            json.dump(mc, f, indent=1, sort_keys=True)

    for slice_, name, expect in REQUEST:
        mc = model_check(os.path.join(REQ, name + ".json"), a.repeat if expect == "reproduce" else None)
        record(slice_, name, mc, expect)

    # Slice 1: discovery on the Phase 12 workspace application, the two
    # foreign-key pairs that diverged in Phase 12.
    scen = os.path.join(OUT, "scenarios")
    p = subprocess.run([BIN, "discover", WS, "--model-check", "--emit-scenarios", scen,
                        "--format", "json", "--no-timing"], capture_output=True, text=True)
    report = json.loads(p.stdout)
    with open(os.path.join(OUT, "workspace-app-discovery.json"), "w") as f:
        json.dump(report, f, indent=1, sort_keys=True)
    for pair in (["createWorkspace", "createWorkspaceLocked"], ["createWorkspaceAction/arg[0]", "createWorkspaceLocked"]):
        c = next(c for c in report["candidates"]
                 if c["class"] == "check-then-act"
                 and sorted(e["selector"].split("#")[1] for e in c["entries"]) == sorted(pair))
        file = next(sc["file"] for sc in c["scenarios"] if sc["variant"]["name"] == "actual")
        mc = model_check(os.path.join(scen, file), a.repeat)
        record("1 foreign-key lock", f"{c['id']} {' × '.join(pair)}", mc, "reproduce")

    # Slice 4: the read replica (model only).
    mc = model_check(os.path.join(ROOT, "fixtures", "phase13", "model", "replica-read-your-writes.json"))
    record("4 replica staleness (model only)", "replica-read-your-writes", mc, "model-only")

    lines = ["# Phase 13 evidence: vertical slices", "",
             f"Binary `{os.path.relpath(BIN, ROOT)}`; `--source-repeat {a.repeat}`; original code runs on disposable PostgreSQL (first-party fixtures only).",
             "", "| slice | scenario | model | states | evidence | reproduced | expectation |",
             "| --- | --- | --- | ---: | --- | --- | --- |", *rows]
    with open(os.path.join(OUT, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
