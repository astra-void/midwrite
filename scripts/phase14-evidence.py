#!/usr/bin/env python3
"""Phase 14 evidence: the vertical slices, model and original code.

Usage: scripts/phase14-evidence.py [--repeat N]

Writes reports/phase14/evidence/. Application code runs ONLY for the
first-party application fixtures/phase14/request-app (`--verify-source
--allow-execution`), on disposable PostgreSQL clusters; every other slice is
static discovery and model checking. Exits 1 unless every counterexample
expected to reproduce reproduced in every run and every safe counterpart
holds.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("COMMITRIFT_BIN", os.path.join(ROOT, "target", "release", "commitrift"))
OUT = os.path.join(ROOT, "reports", "phase14", "evidence")
APP = os.path.join(ROOT, "fixtures", "phase14", "request-app")

# (slice, entry filter, class, entry suffix, expectation): "reproduce" (model
# counterexample reproduced on the original code), "hold" (no counterexample
# within the bounds), "model" (model counterexample, not run).
REQUEST = [
    ("3 multi-row INSERT, response lost, retry", "items.ts#addOrderItems", "resubmitted-write", "#addOrderItems", "reproduce"),
    ("3 multi-row INSERT (safe)", "items.ts#addOrderItemsIdempotent", "resubmitted-write", "#addOrderItemsIdempotent", "hold"),
    ("4 retry wrapper, lost COMMIT", "payments.ts#settlePayment", "ambiguous-commit", "#settlePayment", "reproduce"),
    ("4 retry wrapper (safe)", "payments.ts#settlePaymentOnce", "ambiguous-commit", "#settlePaymentOnce", "hold"),
]


def run(args):
    p = subprocess.run([BIN, *args], capture_output=True, text=True)
    try:
        return json.loads(p.stdout)
    except json.JSONDecodeError:
        sys.exit(f"{' '.join(args)}: no JSON report\n{p.stderr}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    a = ap.parse_args()
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    os.makedirs(OUT)
    scen = os.path.join(OUT, "scenarios")
    rows, failures = [], 0
    for slice_, entry, cls, suffix, expect in REQUEST:
        os.makedirs(scen, exist_ok=True)
        report = run(["discover", APP, "--model-check", "--entry", entry, "--emit-scenarios", scen,
                      "--format", "json", "--no-timing"])
        mine = [c for c in report["candidates"] if c["class"] == cls
                and all(e["selector"].endswith(suffix) for e in c["entries"])]
        if not mine:
            failures += 1
            rows.append(f"| {slice_} | `{entry}` | no {cls} candidate | - | - | - | FAILED |")
            continue
        c = mine[0]
        actual = next(sc for sc in c["scenarios"] if sc["variant"]["name"] == "actual")
        verdict = (actual.get("model") or {}).get("verdict", "-")
        evidence, repro = "model-counterexample" if verdict == "COUNTEREXAMPLE_FOUND" else "none", "-"
        if expect == "reproduce" and verdict == "COUNTEREXAMPLE_FOUND":
            mc = run(["model-check", os.path.join(scen, actual["file"]), "--verify-source",
                      "--allow-execution", "--source-repeat", str(a.repeat), "--format", "json", "--no-timing"])
            name = c["id"] + suffix.replace("#", "-")
            with open(os.path.join(OUT, f"{name}.json"), "w") as f:
                json.dump(mc, f, indent=1, sort_keys=True)
            runs = (mc.get("sourceVerification") or {}).get("runs") or []
            n = sum(1 for r in runs if r.get("reproduced"))
            repro = f"{n}/{len(runs)}"
            evidence = mc["evidence"]["level"]
            ok = runs and n == len(runs)
        elif expect == "hold":
            ok = verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
        else:
            ok = verdict == "COUNTEREXAMPLE_FOUND"
        failures += 0 if ok else 1
        rows.append(f"| {slice_} | `{c['id']}` {suffix} | {c['status']} | {verdict} | {evidence} | {repro} | {'ok' if ok else 'FAILED'} |")
    lines = ["# Phase 14 evidence: vertical slices", "",
             f"Binary `{os.path.relpath(BIN, ROOT)}`; `--source-repeat {a.repeat}`; original code runs on disposable PostgreSQL (first-party fixture only).",
             "The harness handles errors of checked-out node-postgres clients (a stated environment assumption, `fixtures/phase14/request-app/verify/harness.ts`).",
             "", "| slice | candidate | status | model | evidence | reproduced | expectation |",
             "| --- | --- | --- | --- | --- | --- | --- |", *rows]
    with open(os.path.join(OUT, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
