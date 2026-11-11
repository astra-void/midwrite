#!/usr/bin/env python3
"""Model-checker measurements: this build, optionally against an earlier
binary.

Usage: scripts/model-bench.py OUT.md [--old BIN] [--runs N] [--sections S,...]

Sections (default: all):
  fixtures   every model fixture scenario (fixtures/concurrency, the
             server-actions and crash-retry scenarios): verdicts, states and
             transitions must be equal for both binaries; median exploration
             wall time of N runs each.
  actors     actor scaling on the extracted team-app action with a
             hypothetical UNIQUE(teamId, email), 2-8 actors (the old binary
             up to 7): states, transitions, wall time, peak RSS, visited
             bytes, bytes per stored state, memory per million states.
  dims       crash and retry dimensions on the extracted idempotent
             checkout (maxCrashes / maxRetries 0-2, 1-3 clients, exhaustive).
  reduction  full search, sleep sets and DPOR on workloads with independent
             actors, conflicting transactions, crashes, in-flight faults,
             retries and pools (transitions executed, states stored, wall
             time, RSS).
  in-flight  the crash-retry delivery scenarios with and without
             `--max-in-flight 1`.
  verify     verification cost: model only, --validate-postgres and
             --verify-source (one run) on node-postgres and postgres-js
             scenarios, with a crash checkpoint and with an in-flight fault
             through the proxy.

`wall ms` is the exploration time the report states; RSS is the peak
resident set of the process (`/usr/bin/time`). scripts/por-bench.py compares
the search modes on a wider workload matrix.
"""
import argparse
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
SECTIONS = ["fixtures", "actors", "dims", "reduction", "in-flight", "verify"]


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
    base = json.load(open(os.path.join(ROOT, "fixtures/scenarios/server-actions/team-app-invite-double-submit.json")))
    doc = json.loads(json.dumps(base))
    doc.pop("sourceVerification", None)
    doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
    doc["bounds"] = {"maxSteps": 250, "maxActors": 8, "maxStates": 50000000, **(extra_bounds or {})}
    doc["options"] = {"por": por}
    doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/apps/team-app")
    for t in doc["database"]["tables"]:
        if t["name"] == "invitations":
            t["unique"] = [["teamId", "email"]]
    return doc


def row_metrics(d):
    e = d["exploration"]
    m = e["metrics"]
    return e["verdict"], m["states"], m["transitions"], m["visitedBytes"]


def binaries(old):
    return ([("old", old)] if old else []) + [("new", NEW)]


def fixtures(old, n):
    lines = ["## Fixture scenarios" + (" (identical verdicts and counts)" if old else ""), "",
             "| scenario | verdict | states | transitions | " + " | ".join(f"{b} wall ms" for b, _ in binaries(old)) + " |",
             "| --- | --- | --- | --- | " + " | ".join("---" for _ in binaries(old)) + " |"]
    paths = sorted(glob.glob(os.path.join(ROOT, "fixtures/concurrency/scenarios/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/concurrency/corpus/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/scenarios/server-actions/*.json"))
                   + glob.glob(os.path.join(ROOT, "fixtures/scenarios/crash-retry/*.json")))
    for s in paths:
        results = {label: [mc(binary, s) for _ in range(n)] for label, binary in binaries(old)}
        db = results["new"][0][0]
        if old:
            da = results["old"][0][0]
            assert da["exploration"]["verdict"] == db["exploration"]["verdict"], s
            for k in ("states", "transitions"):
                assert da["exploration"]["metrics"][k] == db["exploration"]["metrics"][k], s
        walls = [statistics.median(x[0]["timing"]["wallMs"] for x in results[label]) for label, _ in binaries(old)]
        lines.append(f"| {os.path.relpath(s, ROOT)} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | "
                     f"{db['exploration']['metrics']['transitions']} | " + " | ".join(str(w) for w in walls) + " |")
    print("fixtures done", flush=True)
    return lines


def actors(old, n):
    lines = ["## Actor scaling: team-app action, hypothetical UNIQUE(teamId, email)", "",
             "| actors | binary | states | transitions | wall ms | peak RSS MiB | visited bytes | bytes / state | MiB / million states |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for k in [2, 3, 4, 5, 6, 7, 8]:
        path = temp_scenario(team(k))
        reps = 1 if k >= 7 else (3 if k >= 5 else n)
        for label, binary in binaries(old):
            if k == 8 and label == "old":
                lines.append(f"| {k} | old | not run | | | | | | |")
                continue
            runs = [mc(binary, path) for _ in range(reps)]
            v, states, trans, vb = row_metrics(runs[0][0])
            wall = statistics.median(x[0]["timing"]["wallMs"] for x in runs)
            rss = statistics.median(x[2] for x in runs)
            lines.append(f"| {k} | {label} | {states} | {trans} | {wall} | {rss:.1f} | {vb} | {vb / states:.0f} | {rss / (states / 1e6):.0f} |")
            print(lines[-1], flush=True)
        os.unlink(path)
    return lines


def dims(old, n):
    lines = ["## Crash and retry dimensions (extracted idempotent checkout, exhaustive)", "",
             "| clients | maxCrashes | maxRetries | verdict | states | transitions | wall ms |", "| --- | --- | --- | --- | --- | --- | --- |"]
    cbase = json.load(open(os.path.join(ROOT, "fixtures/scenarios/crash-retry/checkout-idempotent-crash-after-charge.json")))
    for clients in [1, 2, 3]:
        for crashes, retries in [(0, 0), (1, 1), (2, 2)]:
            doc = json.loads(json.dumps(cbase))
            doc.pop("sourceVerification", None)
            doc["actors"] = [dict(cbase["actors"][0], name=f"Client{i+1}",
                                  arguments=[{"requestId": f"req-{i+1}", "orderId": 1}]) for i in range(clients)]
            doc["bounds"] = {"maxSteps": 80, "maxCrashes": crashes, "maxRetries": retries, "maxStates": 5000000}
            doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/apps/shop-app")
            path = temp_scenario(doc)
            b = [mc(NEW, path) for _ in range(3)]
            os.unlink(path)
            db = b[0][0]
            lines.append(f"| {clients} | {crashes} | {retries} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | {statistics.median(x[0]['timing']['wallMs'] for x in b)} |")
            print(lines[-1], flush=True)
    return lines


def reduction(old, n):
    lines = ["## Full search, sleep sets, DPOR (this build)", "",
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
    return lines


def in_flight(old, n):
    lines = ["## In-flight faults on the crash-retry delivery scenarios (this build)", "",
             "| scenario | maxInFlight | verdict | states | transitions |", "| --- | --- | --- | --- | --- |"]
    for name in ["shipping-dual-write-crash", "shipping-outbox-no-loss", "shipping-outbox-relay-duplicate", "job-redelivery-duplicate",
                 "job-claimed-effect-redelivery", "checkout-idempotent-crash-after-charge", "checkout-crash-after-charge"]:
        s = os.path.join(ROOT, f"fixtures/scenarios/crash-retry/{name}.json")
        for extra in ([], ["--max-in-flight", "1"]):
            r, _, _ = mc(NEW, s, "--no-timing", *extra)
            v, states, trans, _ = row_metrics(r)
            lines.append(f"| {name} | {extra[1] if extra else 0} | {v} | {states} | {trans} |")
    return lines


def verify(old, n):
    lines = ["## Verification cost (process wall time, ms; median of 3)", "",
             "| scenario | model only | --validate-postgres | --verify-source (1 run) |", "| --- | --- | --- | --- |"]
    for name in ["fixtures/scenarios/server-actions/team-app-invite-double-submit.json",
                 "fixtures/scenarios/server-actions/team-app-accept-invitation-twice.json",
                 "fixtures/scenarios/crash-retry/pgjs-invite-double-submit.json",
                 "fixtures/scenarios/crash-retry/checkout-crash-after-charge.json",
                 "fixtures/scenarios/crash-retry/pgjs-checkout-crash-after-charge.json",
                 "fixtures/scenarios/crash-retry/accept-crash-while-waiting.json",
                 "fixtures/scenarios/in-flight/ledger-blind-commit-in-flight.json",
                 "fixtures/scenarios/in-flight/ledger-reconciled-commit-in-flight.json",
                 "fixtures/scenarios/in-flight/ledger-shared-process-crash.json",
                 "fixtures/scenarios/in-flight/ledger-pool-starvation.json"]:
        s = os.path.join(ROOT, name)
        row = [statistics.median(mc(NEW, s)[1] for _ in range(3)),
               statistics.median(mc(NEW, s, "--validate-postgres")[1] for _ in range(3)),
               statistics.median(mc(NEW, s, "--verify-source", "--allow-execution")[1] for _ in range(3))]
        lines.append(f"| {os.path.basename(name)} | " + " | ".join(f"{x:.0f}" for x in row) + " |")
        print(lines[-1], flush=True)
    return lines


RUN = {"fixtures": fixtures, "actors": actors, "dims": dims, "reduction": reduction,
       "in-flight": in_flight, "verify": verify}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("out")
    ap.add_argument("--old", help="an earlier midwrite binary (fixtures and actors sections)")
    ap.add_argument("--runs", type=int, default=3, help="repetitions (median)")
    ap.add_argument("--sections", default=",".join(SECTIONS), help="comma-separated: " + ", ".join(SECTIONS))
    a = ap.parse_args()
    sections = [s for s in a.sections.split(",") if s]
    unknown = [s for s in sections if s not in RUN]
    if unknown:
        ap.error(f"unknown sections {unknown}")
    lines = ["# Model-checker measurements", "",
             f"{platform.platform()}; " + (f"old binary = `{a.old}`; " if a.old else "") +
             f"median of {a.runs} runs unless noted; `wall ms` is the exploration time the report states.", ""]
    for s in sections:
        lines += RUN[s](a.old, a.runs) + [""]
    open(a.out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
