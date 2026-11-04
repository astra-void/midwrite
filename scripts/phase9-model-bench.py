#!/usr/bin/env python3
"""Phase 9 model-checker measurements: the Phase 8 binary against this build.

1. Phase 6-8 fixture scenarios: verdicts, states and transitions equal;
   median wall time of N runs each.
2. Actor scaling, extracted team-app action with a hypothetical
   UNIQUE(teamId, email), 2-7 actors (8 for this build only): states,
   transitions, wall time, peak RSS, visited bytes, bytes per stored state,
   memory per million states, for Phase 8 and Phase 9 (sleep sets), plus
   no reduction and DPOR where they finish.
3. Reduction comparison on workloads with independent actors, conflicting
   transactions, crashes, in-flight faults, retries and pools: full search,
   sleep sets, DPOR (transitions executed, states stored, wall time, RSS).
4. In-flight dimension: the Phase 8 delivery scenarios with and without
   `--max-in-flight 1`.
5. Verification cost: model only, --validate-postgres, --verify-source with a
   Phase 8 crash checkpoint and with an in-flight fault through the proxy.

Usage: scripts/phase9-model-bench.py PHASE8_BINARY OUT.md [N]
"""
import glob
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NEW = os.path.join(ROOT, "target", "release", "midwrite")


def mc(binary, scenario, *extra, timeout=1800):
    flag = "-l" if platform.system() == "Darwin" else "-v"
    t0 = time.perf_counter()
    p = subprocess.run(["/usr/bin/time", flag, binary, "model-check", scenario, "--format", "json", *extra],
                       capture_output=True, text=True, timeout=timeout)
    elapsed = (time.perf_counter() - t0) * 1000
    rss = None
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    if m:
        rss = int(m.group(1)) / (1024 * 1024)
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
    if m:
        rss = int(m.group(1)) / 1024
    return json.loads(p.stdout), elapsed, rss


def temp_scenario(doc):
    f = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
    json.dump(doc, f)
    f.close()
    return f.name


def team(k, por=True, extra_bounds=None):
    base = json.load(open(os.path.join(ROOT, "fixtures/phase7/scenarios/team-app-invite-double-submit.json")))
    doc = json.loads(json.dumps(base))
    doc.pop("sourceVerification", None)
    doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
    doc["bounds"] = {"maxSteps": 250, "maxActors": 8, "maxStates": 50000000, **(extra_bounds or {})}
    doc["options"] = {"por": por}
    doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/phase7/team-app")
    for t in doc["database"]["tables"]:
        if t["name"] == "invitations":
            t["unique"] = [["teamId", "email"]]
    return doc


def row_metrics(d):
    e = d["exploration"]
    m = e["metrics"]
    return e["verdict"], m["states"], m["transitions"], m["visitedBytes"]


def main():
    old, out = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 3
    lines = ["# Phase 9 model-checker measurements", "",
             f"{platform.platform()}; Phase 8 binary = the saved release build of commit 4dc01af; median of {n} runs unless noted; `wall ms` is the exploration time the report states.", ""]

    lines += ["## 1. Phase 6-8 fixture scenarios (identical verdicts and counts)", "",
              "| scenario | verdict | states | transitions | Phase 8 wall ms | Phase 9 wall ms |", "| --- | --- | --- | --- | --- | --- |"]
    paths = sorted(glob.glob(os.path.join(ROOT, "fixtures/concurrency/scenarios/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/concurrency/corpus/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/phase7/scenarios/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/phase8/scenarios/*.json")))
    for s in paths:
        a = [mc(old, s) for _ in range(n)]
        b = [mc(NEW, s) for _ in range(n)]
        da, db = a[0][0], b[0][0]
        assert da["exploration"]["verdict"] == db["exploration"]["verdict"], s
        for k in ("states", "transitions"):
            assert da["exploration"]["metrics"][k] == db["exploration"]["metrics"][k], s
        wa = statistics.median(x[0]["timing"]["wallMs"] for x in a)
        wb = statistics.median(x[0]["timing"]["wallMs"] for x in b)
        lines.append(f"| {os.path.relpath(s, ROOT)} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | {wa} | {wb} |")
    print("1 done", flush=True)

    lines += ["", "## 2. Actor scaling: team-app action, hypothetical UNIQUE(teamId, email)", "",
              "| actors | binary | states | transitions | wall ms | peak RSS MiB | visited bytes | bytes / state | MiB / million states |",
              "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for k in [2, 3, 4, 5, 6, 7, 8]:
        path = temp_scenario(team(k))
        reps = 1 if k >= 7 else (3 if k >= 5 else n)
        for label, binary in (("Phase 8", old), ("Phase 9", NEW)):
            if k == 8 and binary == old:
                lines.append(f"| {k} | Phase 8 | not run | | | | | | extrapolated ≈ {5441 * 4544512 / 1048576:.0f} MiB of visited keys alone |")
                continue
            runs = [mc(binary, path) for _ in range(reps)]
            v, states, trans, vb = row_metrics(runs[0][0])
            wall = statistics.median(x[0]["timing"]["wallMs"] for x in runs)
            rss = statistics.median(x[2] for x in runs)
            lines.append(f"| {k} | {label} | {states} | {trans} | {wall} | {rss:.1f} | {vb} | {vb / states:.0f} | {rss / (states / 1e6):.0f} |")
            print(lines[-1], flush=True)
        os.unlink(path)

    lines += ["", "## 3. Full search, sleep sets, DPOR (this build)", "",
              "| workload | mode | verdict | states stored | transitions executed | wall ms | peak RSS MiB |", "| --- | --- | --- | --- | --- | --- | --- |"]
    workloads = []
    for k in (3, 4):
        workloads.append((f"team-app, {k} actors (conflicting inserts)", team(k, True)))
    # Independent actors: each actor writes its own table.
    tables = [{"name": f"t{i}", "columns": [{"name": "id", "type": "int", "notNull": True}, {"name": "v", "type": "int"}], "primaryKey": ["id"]} for i in range(4)]
    for k in (2, 3, 4):
        workloads.append((f"independent actors, {k} (disjoint tables)", {
            "schemaVersion": 1, "name": f"independent-{k}",
            "database": {"isolation": "read-committed", "tables": tables[:k], "initialState": {f"t{i}": [{"id": 1, "v": 0}] for i in range(k)}},
            "actors": [{"name": f"A{i}", "function": "work", "program": f"let x = select t{i} where @id == 1\nupdate t{i} where @id == 1 set @v = @v + 1\nupdate t{i} where @id == 1 set @v = @v + 1\n", "arguments": {}} for i in range(k)],
            "bounds": {"maxSteps": 60, "maxStates": 50000000},
            "invariants": [{"name": "ok", "when": "final", "assert": {"ge": [{"count": {"table": "t0", "where": {}}}, 0]}}]}))
    pay = {"schemaVersion": 1, "name": "payments",
           "database": {"isolation": "read-committed",
                        "tables": [{"name": "payments", "columns": [{"name": "id", "type": "int", "notNull": True, "default": {"sequence": {}}}, {"name": "req", "type": "text"}], "primaryKey": ["id"], "unique": [["req"]]}],
                        "initialState": {"payments": []}},
           "actors": [{"name": f"C{i}", "function": "pay", "program": "begin\ninsert payments set @req = rid on conflict do nothing\ncommit\n", "arguments": {"rid": f"r{i}"}, "retry": {"trigger": "onFailureOrCrash"}} for i in range(2)],
           "bounds": {"maxSteps": 40, "maxCrashes": 1, "maxRetries": 1, "maxInFlight": 1, "maxStates": 50000000},
           "invariants": [{"name": "ok", "when": "final", "assert": {"le": [{"count": {"table": "payments", "where": {}}}, 2]}}]}
    workloads.append(("2 clients, crash + retry + in-flight COMMIT (idempotent)", pay))
    pool = json.loads(json.dumps(pay))
    pool["name"] = "pool"
    pool["bounds"] = {"maxSteps": 40, "maxStates": 50000000}
    pool["pools"] = {"C0": 1}
    for a in pool["actors"]:
        a["process"] = "C0"
        a.pop("retry", None)
    workloads.append(("2 tasks of one process, pool of 1", pool))
    for label, doc in workloads:
        for mode, flags, por in (("full (no reduction)", ["--no-por"], False), ("sleep sets", [], True), ("DPOR", ["--dpor"], True)):
            d = json.loads(json.dumps(doc))
            d.setdefault("options", {})["por"] = por
            path = temp_scenario(d)
            try:
                r, wall_ms, rss = mc(NEW, path, *flags, timeout=900)
                v, states, trans, vb = row_metrics(r)
                refused = r["bounds"].get("partialOrderReductionDisabledBecause")
                lines.append(f"| {label} | {mode}{' (refused: ' + refused[:40] + ')' if mode == 'DPOR' and refused else ''} | {v} | {states} | {trans} | {r['timing']['wallMs']} | {rss:.1f} |")
            except subprocess.TimeoutExpired:
                lines.append(f"| {label} | {mode} | timed out after 900 s | | | | |")
            os.unlink(path)
            print(lines[-1], flush=True)

    lines += ["", "## 4. In-flight faults on the Phase 8 delivery scenarios (this build)", "",
              "| scenario | maxInFlight | verdict | states | transitions |", "| --- | --- | --- | --- | --- |"]
    for name in ["shipping-dual-write-crash", "shipping-outbox-no-loss", "shipping-outbox-relay-duplicate", "job-redelivery-duplicate",
                 "job-claimed-effect-redelivery", "checkout-idempotent-crash-after-charge", "checkout-crash-after-charge"]:
        s = os.path.join(ROOT, f"fixtures/phase8/scenarios/{name}.json")
        for extra in ([], ["--max-in-flight", "1"]):
            r, _, _ = mc(NEW, s, "--no-timing", *extra)
            v, states, trans, _ = row_metrics(r)
            lines.append(f"| {name} | {extra[1] if extra else 0} | {v} | {states} | {trans} |")

    lines += ["", "## 5. Verification cost (process wall time, ms; median of 3)", "",
              "| scenario | model only | --validate-postgres | --verify-source (1 run) |", "| --- | --- | --- | --- |"]
    for name in ["fixtures/phase8/scenarios/checkout-crash-after-charge.json",
                 "fixtures/phase9/scenarios/ledger-blind-commit-in-flight.json",
                 "fixtures/phase9/scenarios/ledger-reconciled-commit-in-flight.json",
                 "fixtures/phase9/scenarios/ledger-shared-process-crash.json",
                 "fixtures/phase9/scenarios/ledger-pool-starvation.json"]:
        s = os.path.join(ROOT, name)
        row = [statistics.median(mc(NEW, s)[1] for _ in range(3))]
        row.append(statistics.median(mc(NEW, s, "--validate-postgres")[1] for _ in range(3)))
        row.append(statistics.median(mc(NEW, s, "--verify-source", "--allow-execution")[1] for _ in range(3)))
        lines.append(f"| {os.path.basename(name)} | " + " | ".join(f"{x:.0f}" for x in row) + " |")
        print(lines[-1], flush=True)
    open(out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
