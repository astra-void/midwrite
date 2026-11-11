#!/usr/bin/env python3
"""Partial-order reduction benchmark: a workload matrix (independent,
mostly independent and conflicting actors, pool contention, write skew,
in-flight COMMIT, crash/retry, tasks, queue redelivery, the extracted
team-app action) under every search mode.

Modes: unreduced (`--no-por`), sleep sets (default), classic DPOR (`--dpor`),
and source-set DPOR in its three configurations. For each run: verdict, distinct states, transitions executed,
wall time (the report's own exploration time), peak RSS (`/usr/bin/time`),
time to the first counterexample, and the reduction counters. A run that hits
`--max-wall-ms` is reported as truncated, never as a result.

Then actor scaling and memory on the team-app action (3-7 actors,
`--profile-storage`).

Usage: scripts/por-bench.py OUT.md [REPEAT]
       scripts/por-bench.py --write-workloads DIR

--write-workloads writes every workload as a JSON scenario to DIR (and the
team-app action with 3 to 7 actors as team-K.json) without running anything.
"""
import json
import os
import platform
import re
import statistics
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "release", "midwrite")
MODES = [
    ("unreduced", ["--no-por"]),
    ("sleep sets", []),
    ("classic DPOR", ["--dpor"]),
    ("sdpor stateful", ["--source-dpor", "stateful"]),
    ("sdpor stateless+sleep", ["--source-dpor", "stateless-sleep"]),
    ("sdpor stateful+sleep", ["--source-dpor", "stateful-sleep"]),
]
WALL_MS = 120_000


def tmp(doc):
    f = tempfile.NamedTemporaryFile("w", suffix=".json", delete=False)
    json.dump(doc, f)
    f.close()
    return f.name


def table(name, private=False, rows=3):
    return {"name": name, "columns": [{"name": "id", "notNull": True, "type": "int"}, {"name": "v", "type": "int"}],
            "primaryKey": ["id"]}


def dsl(name, actors, tables, initial, invariants, bounds=None, extra=None):
    doc = {"schemaVersion": 1, "name": name, "database": {"isolation": "read-committed", "tables": tables,
           "initialState": initial}, "actors": actors,
           "bounds": {"maxSteps": 200, "maxActors": 8, "maxStates": 50_000_000, **(bounds or {})},
           "invariants": invariants}
    doc.update(extra or {})
    return doc


def trivial(t):
    return [{"name": "ok", "when": "final", "assert": {"ge": [{"count": {"table": t, "where": {}}}, 0]}}]


def independent(n):
    tables = [table(f"p{i}") for i in range(n)]
    init = {f"p{i}": [{"id": 1, "v": 0}] for i in range(n)}
    actors = [{"name": f"A{i}", "function": "w", "arguments": {},
               "program": f"begin\nlet r = select p{i} where @id == 1\nupdate p{i} where @id == 1 set @v = @v + 1\ncommit\n"}
              for i in range(n)]
    return dsl(f"independent-{n}", actors, tables, init, trivial("p0"))


def mostly_independent(n):
    doc = independent(n)
    doc["name"] = f"mostly-independent-{n}"
    doc["database"]["tables"].append(table("shared"))
    doc["database"]["initialState"]["shared"] = [{"id": 1, "v": 0}]
    for a in doc["actors"]:
        a["program"] += "update shared where @id == 1 set @v = @v + 1\n"
    return doc


def conflicting(n):
    actors = [{"name": f"A{i}", "function": "w", "arguments": {},
               "program": "begin\nlet r = select t where @id == 1\nupdate t where @id == 1 set @v = r.v + 1\ncommit\n"}
              for i in range(n)]
    return dsl(f"conflicting-{n}", actors, [table("t")], {"t": [{"id": 1, "v": 0}]},
               [{"name": "no-lost-update", "when": "final",
                 "assert": {"ge": [{"sum": {"table": "t", "where": {}, "column": "v"}}, n]}}])


def conflicting_atomic(n):
    actors = [{"name": f"A{i}", "function": "w", "arguments": {},
               "program": "begin\nupdate t where @id == 1 set @v = @v + 1\nlet r = select t where @id == 1\ncommit\n"}
              for i in range(n)]
    return dsl(f"conflicting-atomic-{n}", actors, [table("t")], {"t": [{"id": 1, "v": 0}]},
               [{"name": "every-increment", "when": "final",
                 "assert": {"ge": [{"sum": {"table": "t", "where": {}, "column": "v"}}, n]}}])


def pool(n, cap):
    return {
        "schemaVersion": 1, "name": f"pool-{n}-tasks-of-{cap}",
        "database": {"isolation": "read-committed", "tables": [{"name": "payments", "columns": [
            {"name": "id", "type": "int", "notNull": True, "default": {"sequence": {}}},
            {"name": "req", "type": "text"}], "primaryKey": ["id"], "unique": [["req"]]}],
            "initialState": {"payments": []}},
        "actors": [{"name": f"C{i}", "function": "pay", "process": "P",
                    "program": "begin\ninsert payments set @req = rid on conflict do nothing\ncommit\n",
                    "arguments": {"rid": f"r{i}"}} for i in range(n)],
        "pools": {"P": cap},
        "bounds": {"maxSteps": 200, "maxStates": 50_000_000},
        "invariants": [{"name": "all", "when": "final", "assert": {"le": [{"count": {"table": "payments", "where": {}}}, n]}}],
    }


def write_skew(iso):
    doctors = {"name": "doctors", "columns": [{"name": "id", "notNull": True, "type": "int"},
               {"name": "on_call", "type": "bool"}], "primaryKey": ["id"]}
    actors = [{"name": f"D{i}", "function": "w", "arguments": {"me": i},
               "program": f"begin isolation {iso}\nlet n = count doctors where @on_call == true && @id != me\nif n >= 1 {{\n  update doctors where @id == me set @on_call = false\n}}\ncommit\n"}
              for i in (1, 2, 3)]
    return dsl(f"write-skew-{iso}", actors, [doctors],
               {"doctors": [{"id": i, "on_call": True} for i in (1, 2, 3)]},
               [{"name": "someone-on-call", "when": "final",
                 "assert": {"ge": [{"count": {"table": "doctors", "where": {"on_call": True}}}, 1]}}])


def crash_retry(n):
    doc = independent(n)
    doc["name"] = f"crash-retry-{n}"
    for a in doc["actors"]:
        a["retry"] = {"trigger": "onCrash"}
    doc["bounds"].update({"maxCrashes": 1, "maxRetries": 1})
    return doc


def tasks(k):
    branches = "} {\n".join(f"  update t where @id == {i + 1} set @v = @v + 1\n" for i in range(k))
    actors = [{"name": "A", "function": "w", "arguments": {}, "program": "spawn {\n" + branches + "}\njoin\n"},
              {"name": "B", "function": "w", "arguments": {}, "program": "update t where @id == 1 set @v = @v * 2\n"}]
    return dsl(f"tasks-{k}", actors, [table("t")], {"t": [{"id": i + 1, "v": 1} for i in range(k)]}, trivial("t"))


def team(k):
    base = json.load(open(os.path.join(ROOT, "fixtures/scenarios/server-actions/team-app-invite-double-submit.json")))
    doc = json.loads(json.dumps(base))
    doc.pop("sourceVerification", None)
    doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
    doc["bounds"] = {"maxSteps": 250, "maxActors": 8, "maxStates": 50_000_000}
    doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/apps/team-app")
    for t in doc["database"]["tables"]:
        if t["name"] == "invitations":
            t["unique"] = [["teamId", "email"]]
    return doc


def fixture(path, rel_keys=("extraction",)):
    doc = json.load(open(os.path.join(ROOT, path)))
    base = os.path.dirname(os.path.join(ROOT, path))
    if "extraction" in doc and "root" in doc["extraction"]:
        doc["extraction"]["root"] = os.path.normpath(os.path.join(base, doc["extraction"]["root"]))
    doc.pop("sourceVerification", None)
    doc["bounds"]["maxStates"] = 50_000_000
    doc["invariants"] = [i for i in doc.get("invariants", []) if i.get("when") == "final"] or doc["invariants"]
    return doc


WORKLOADS = [
    ("1 completely independent actors (5)", lambda: independent(5)),
    ("2 mostly independent actors (4, one shared counter)", lambda: mostly_independent(4)),
    ("3 highly conflicting transactions (4, lost update)", lambda: conflicting(4)),
    ("3b highly conflicting, safe (4 atomic increments of one row)", lambda: conflicting_atomic(4)),
    ("4 shared pool contention (4 tasks, pool of 2)", lambda: pool(4, 2)),
    ("5 read/write skew (3 doctors, SERIALIZABLE)", lambda: write_skew("serializable")),
    ("6 in-flight COMMIT + reconciliation", lambda: fixture("fixtures/model/in-flight/ambiguous-commit-lookup-then-insert-concurrent.json")),
    ("6b in-flight COMMIT, lookup + unique key, concurrent delivery", lambda: fixture("fixtures/model/in-flight/ambiguous-commit-lookup-unique.json")),
    ("7 crash/retry (4 actors)", lambda: crash_retry(4)),
    ("8 multiple tasks per process (3 tasks + 1 actor)", lambda: tasks(3)),
    ("9 queue redelivery (crash-retry job)", lambda: fixture("fixtures/scenarios/crash-retry/job-redelivery-duplicate.json")),
    ("10a team-app, 4 actors (extracted)", lambda: team(4)),
    ("10b team-app, 5 actors (extracted)", lambda: team(5)),
]


def run(path, flags):
    timing = "-l" if platform.system() == "Darwin" else "-v"
    p = subprocess.run(["/usr/bin/time", timing, BIN, "model-check", path, "--format", "json", "--no-minimize",
                        "--max-wall-ms", str(WALL_MS), *flags], capture_output=True, text=True, timeout=WALL_MS / 1000 + 120)
    rss = None
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    if m:
        rss = int(m.group(1)) / (1024 * 1024)
    m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
    if m:
        rss = int(m.group(1)) / 1024
    d = json.loads(p.stdout)
    e = d["exploration"]
    return {
        "verdict": e["verdict"], "states": e["metrics"]["states"], "transitions": e["metrics"]["transitions"],
        "ms": (d.get("timing") or {}).get("wallMs"), "rss": rss, "truncated": e["truncation"].get("maxWallTime"),
        "first": (d.get("timing") or {}).get("firstViolationMs"),
        "refused": d["bounds"].get("partialOrderReductionDisabledBecause"),
        "rm": e.get("reductionMetrics"), "terminals": e["metrics"]["terminalStates"],
    }


def write_workloads(out):
    os.makedirs(out, exist_ok=True)
    docs = {make()["name"]: make() for _, make in WORKLOADS}
    docs.update({f"team-{k}": team(k) for k in (3, 4, 5, 6, 7)})
    for name, doc in docs.items():
        with open(os.path.join(out, f"{name}.json"), "w") as f:
            json.dump(doc, f, indent=1)
            f.write("\n")
    print("wrote", sorted(docs), "to", out)


def main():
    if len(sys.argv) < 2 or sys.argv[1] in ("-h", "--help"):
        sys.exit(__doc__)
    if sys.argv[1] == "--write-workloads":
        if len(sys.argv) != 3:
            sys.exit(__doc__)
        return write_workloads(sys.argv[2])
    out = sys.argv[1]
    repeat = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    lines = [f"# Partial-order reduction benchmark\n",
             f"{platform.system()} {platform.machine()}, release build, `--no-minimize`, wall limit {WALL_MS} ms, "
             f"median of {repeat} for runs under 10 s (one run otherwise). Wall = the report's exploration time; "
             "RSS = peak resident set of the process.\n"]
    for title, make in WORKLOADS:
        path = tmp(make())
        lines.append(f"\n## {title}\n")
        lines.append("| mode | verdict | states | transitions | wall ms | first counterexample ms | peak RSS MiB | races / backtracks / summary checks |")
        lines.append("| --- | --- | ---: | ---: | ---: | ---: | ---: | --- |")
        for name, flags in MODES:
            results = [run(path, flags)]
            if (results[0]["ms"] or 0) < 10_000:
                results += [run(path, flags) for _ in range(repeat - 1)]
            r = results[0]
            ms = statistics.median([x["ms"] or 0 for x in results])
            rss = statistics.median([x["rss"] or 0 for x in results])
            rm = r["rm"] or {}
            extra = f"{rm.get('races', '')} / {rm.get('backtrackPoints', '')} / {rm.get('summaryChecks', '')}" if rm else ""
            verdict = r["verdict"] + (" (TRUNCATED at the wall limit)" if r["truncated"] else "")
            if r["refused"]:
                verdict += f" — reduction refused: {r['refused'][:80]}"
            lines.append(f"| {name} | {verdict} | {r['states']} | {r['transitions']} | {ms:.0f} | {r['first'] if r['first'] is not None else '—'} | {rss:.1f} | {extra} |")
            print(title, name, r["verdict"], r["transitions"], ms, flush=True)
        os.unlink(path)
    # Actor scaling and memory: team-app, 3-7 actors.
    lines.append("\n## Actor scaling (team-app), memory\n")
    lines.append("| actors | mode | states | transitions | wall ms | peak RSS MiB | key table + interner + edges + nodes bytes | summaries bytes (estimate) |")
    lines.append("| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |")
    for k in (3, 4, 5, 6, 7):
        path = tmp(team(k))
        for name, flags in [("sleep sets", []), ("sdpor stateful+sleep", ["--source-dpor", "stateful-sleep"]),
                            ("sdpor stateless+sleep", ["--source-dpor", "stateless-sleep"])]:
            r = run(path, flags + ["--profile-storage"])
            p = subprocess.run([BIN, "model-check", path, "--format", "json", "--no-minimize", "--profile-storage",
                                "--max-wall-ms", str(WALL_MS), *flags], capture_output=True, text=True)
            st = json.loads(p.stdout)["exploration"].get("storage") or {}
            stored = sum(st.get(x, 0) for x in ("tableBytes", "internerBytes", "edgeBytes", "nodeBytes"))
            sb = (r["rm"] or {}).get("summaryBytes", "—")
            verdict = " (TRUNCATED)" if r["truncated"] else ""
            lines.append(f"| {k} | {name}{verdict} | {r['states']} | {r['transitions']} | {r['ms']} | {r['rss']:.1f} | {stored} | {sb} |")
            print("scaling", k, name, r["transitions"], r["ms"], flush=True)
        os.unlink(path)
    open(out, "w").write("\n".join(lines) + "\n")
    print("wrote", out)


if __name__ == "__main__":
    main()
