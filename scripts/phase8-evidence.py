#!/usr/bin/env python3
"""Phase 8 runtime evidence: every Phase 8 scenario run against the ORIGINAL
code (`model-check --verify-source --allow-execution --source-repeat N`),
each report saved to OUT_DIR/<scenario>.json, and a Markdown summary.

Usage: scripts/phase8-evidence.py OUT_DIR [N]

Exits 1 when any scenario was not run on the original code N times, or a run
was not followed or did not match the model: a blocked run is not evidence.
"""
import glob
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release", "midwrite")


def main():
    out = sys.argv[1]
    n = sys.argv[2] if len(sys.argv) > 2 else "5"
    os.makedirs(out, exist_ok=True)
    incomplete = []
    rows = ["| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(os.path.join(ROOT, "fixtures/phase8/scenarios/*.json"))):
        name = os.path.basename(path)[:-5]
        t0 = time.perf_counter()
        p = subprocess.run([BIN, "model-check", path, "--verify-source", "--allow-execution",
                            "--source-repeat", n, "--format", "json"], capture_output=True, text=True)
        elapsed = time.perf_counter() - t0
        open(os.path.join(out, name + ".json"), "w").write(p.stdout)
        r = json.loads(p.stdout)
        sv = r.get("sourceVerification", {})
        runs = sv.get("runs", [])
        followed = sum(1 for x in runs if x["coordination"] == "followed")
        matched = sum(1 for x in runs if x.get("reproduced") or x.get("recoveryReproduced"))
        inv = sorted({x["invariant"] for x in runs})
        rec = sv.get("recovery") or {}
        crashes = rec.get("crashes", [])
        restarts = rec.get("restarts", [])
        effects = rec.get("effects", {})
        if not (len(runs) == int(n) and followed == len(runs) and matched == len(runs)):
            incomplete.append(f"{name}: {sv.get('status', 'not performed')}, {followed}/{len(runs)} followed, "
                              f"{matched}/{len(runs)} matched; {'; '.join(sv.get('problems', []))[:300]}")
        rows.append("| {} | {} | {} | {}/{} / {}/{} | {} | {} | {} | {} | {} | {:.1f} |".format(
            name, r["exploration"]["verdict"], r["evidence"]["level"], followed, len(runs), matched, len(runs),
            ", ".join(inv),
            ", ".join(c["status"] for c in crashes) or "—",
            ", ".join(f"#{x['attempt']} {x['status']}" for x in restarts) or "—",
            ("yes" if effects.get("agree") else "NO") if effects else "—",
            (sv.get("postgresVersion") or "").replace("postgres (PostgreSQL) ", ""),
            elapsed))
        print(rows[-1], flush=True)
    open(os.path.join(out, "SUMMARY.md"), "w").write(
        f"# Phase 8 original-code evidence ({n} runs per scenario)\n\n" + "\n".join(rows) + "\n")
    if incomplete:
        print("INCOMPLETE:\n" + "\n".join(incomplete), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
