#!/usr/bin/env python3
"""Phase 10 runtime evidence: every scenario of fixtures/phase10/scenarios run
against the ORIGINAL code (`model-check --verify-source --allow-execution
--source-repeat N`), each report saved to OUT_DIR/<scenario>.json, and a
Markdown summary of what was observed: the driver, the fault (a broken
connection under a live process, or a process crash with an operation in
flight), the fate (model = run), the driver's error, whether the worker lived,
the server's answer, the backend's state, the durable rows, the retry's new
connection, and the verdicts.

Usage: scripts/phase10-evidence.py OUT_DIR [N]

Exits 1 when a scenario expected to reproduce did not in all N runs. The
`unhandled-error-event` scenario is expected to DIVERGE (the process dies).
"""
import glob
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release", "commitrift")
EXPECT_DIVERGE = {"live-blind-retry-unhandled-error-event"}


def main():
    out = sys.argv[1]
    n = sys.argv[2] if len(sys.argv) > 2 else "5"
    os.makedirs(out, exist_ok=True)
    bad = []
    rows = ["| scenario | driver | fault | model verdict | evidence | runs followed / matched | fate (model = run) | driver error | worker alive | server answer | backend before | durable rows after the loss | retry's connection | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(os.path.join(ROOT, "fixtures/phase10/scenarios/*.json"))):
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
        rec = sv.get("recovery") or {}
        losses = rec.get("losses", [])
        crashes = [c for c in rec.get("crashes", []) if c.get("inFlight")]
        driver = "postgres-js 3.4.5" if name.startswith("pgjs") else "node-postgres 8.23.1"
        if losses:
            l = losses[0]
            fault = "connection broken, process alive"
            fate = f"{l['modelFate']} = {l.get('observedFate', '')}"
            err = l.get("driverError", "—")
            alive = str(l.get("workerAliveAfter"))
            answer = (l.get("serverAnswer") or "—").replace("the server answered: ", "")
            before = l.get("backendStateBefore", "—")
            rows_after = len(next(iter((l.get("runtimeTables") or {"": []}).values()), []))
            later = ", ".join(str(x) for x in l.get("laterBackendPids", [])) or ("— (postgres-js does not expose the backend)" if name.startswith("pgjs") else "—")
            if l.get("backendPid") and l.get("laterBackendPids"):
                later = f"{l['backendPid']} → {later}"
        elif crashes:
            f = crashes[0]["inFlight"]
            fault = "process killed, operation in flight"
            fate = f"{f['modelFate']} = {f['observedFate']}"
            err, alive = "— (process killed)", "False (SIGKILL)"
            answer = (f.get("serverAnswer") or "—").replace("the server answered: ", "")
            before = f.get("backendStateBeforeKill", "—")
            rows_after = len(next(iter((crashes[0].get("runtimeTables") or {"": []}).values()), []))
            later = "new process"
        else:
            fault, fate, err, alive, answer, before, rows_after, later = "—", "—", "—", "—", "—", "—", "—", "—"
            if name in EXPECT_DIVERGE:
                fault = "connection broken; no client error handler"
                err = "; ".join(sv.get("problems", []))[:160]
        rows.append(f"| {name} | {driver} | {fault} | {r['exploration']['verdict']} | {sv.get('evidence', '—')} | "
                    f"{followed} / {matched} of {len(runs)} | {fate} | {err} | {alive} | {answer} | {before} | {rows_after} | {later} | {elapsed:.1f} |")
        if name in EXPECT_DIVERGE:
            if sv.get("status") != "diverged":
                bad.append(f"{name}: expected diverged, got {sv.get('status')}")
        elif not (len(runs) == int(n) and followed == len(runs) and matched == len(runs)):
            bad.append(f"{name}: {sv.get('status')}, {followed}/{len(runs)} followed, {matched} matched; {'; '.join(sv.get('problems', []))[:300]}")
    text = "\n".join(rows) + "\n"
    if bad:
        text += "\nNOT EVIDENCE:\n" + "\n".join("- " + b for b in bad) + "\n"
    open(os.path.join(out, "SUMMARY.md"), "w").write(text)
    print(text)
    sys.exit(1 if bad else 0)


main()
