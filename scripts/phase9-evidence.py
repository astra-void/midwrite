#!/usr/bin/env python3
"""Phase 9 runtime evidence: every Phase 9 scenario (fixtures/phase9/scenarios)
run against the ORIGINAL code (`model-check --verify-source --allow-execution
--source-repeat N`), each report saved to OUT_DIR/<scenario>.json, and a
Markdown summary with the in-flight facts (fate, what the proxy observed, what
PostgreSQL confirmed, session cleanup time, stuck-state observations).

Usage: scripts/phase9-evidence.py OUT_DIR [N]

Exits 1 when a scenario with something to run was not run on the original code
N times, or a run was not followed or did not match the model: a blocked or
diverged run is not evidence.
"""
import glob
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release", "commitrift")


def main():
    out = sys.argv[1]
    n = sys.argv[2] if len(sys.argv) > 2 else "5"
    os.makedirs(out, exist_ok=True)
    incomplete = []
    rows = ["| scenario | model verdict | evidence | runs (followed / matched) | in-flight: operation → fate (model = run) | server answer observed | backend before kill | session ended (ms) | stuck / pool | tasks killed | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(os.path.join(ROOT, "fixtures/phase9/scenarios/*.json"))):
        name = os.path.basename(path)[:-5]
        t0 = time.perf_counter()
        p = subprocess.run([BIN, "model-check", path, "--verify-source", "--allow-execution",
                            "--source-repeat", n, "--format", "json"], capture_output=True, text=True)
        elapsed = time.perf_counter() - t0
        open(os.path.join(out, name + ".json"), "w").write(p.stdout)
        r = json.loads(p.stdout)
        sv = r.get("sourceVerification") or {}
        runs = sv.get("runs", [])
        followed = sum(1 for x in runs if x["coordination"] == "followed")
        matched = sum(1 for x in runs if x.get("reproduced") or x.get("recoveryReproduced"))
        nothing_to_run = (r["exploration"]["verdict"] == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
                          and sv.get("status") == "not-run")
        if not nothing_to_run and not (len(runs) == int(n) and followed == len(runs) and matched == len(runs)):
            incomplete.append(f"{name}: {sv.get('status', 'not performed')}, {followed}/{len(runs)} followed, "
                              f"{matched}/{len(runs)} matched; {'; '.join(sv.get('problems', []))[:300]}")
        crashes = (sv.get("recovery") or {}).get("crashes", [])
        flights = [c["inFlight"] for c in crashes if c.get("inFlight")]
        fate = "; ".join(f"{f['operation']} → {f['modelFate']} = {f['observedFate']}" for f in flights) or "—"
        answer = "; ".join(f["serverAnswer"].replace("the server answered: ", "") for f in flights if f.get("serverAnswer")) or "—"
        state = "; ".join(f["backendStateBeforeKill"] for f in flights) or "—"
        ended = "; ".join(str(c["sessionsEndedMs"]) for c in crashes if c["sessionsEnded"]) or "—"
        stuck = sv.get("stuck")
        stuck_text = "—"
        if stuck:
            o = stuck["observed"]
            stuck_text = f"{stuck['modelKind']}: quiet {o['quietMs']} ms, pool {o.get('pool')}, lock waits {o.get('lockWaits')}"
        killed = sum(1 for a in sv.get("actors", []) if a[2] == "killed")
        rows.append("| {} | {} | {} | {}/{} / {}/{} | {} | {} | {} | {} | {} | {} | {:.1f} |".format(
            name, r["exploration"]["verdict"], (sv.get("evidence") or r["evidence"]["level"]) if not nothing_to_run else "no counterexample",
            followed, len(runs), matched, len(runs), fate, answer, state, ended, stuck_text, killed or "—", elapsed))
        print(rows[-1], flush=True)
    open(os.path.join(out, "SUMMARY.md"), "w").write(
        f"# Phase 9 original-code evidence ({n} runs per scenario)\n\n" + "\n".join(rows) + "\n")
    if incomplete:
        print("INCOMPLETE:\n" + "\n".join(incomplete), file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
