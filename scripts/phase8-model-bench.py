#!/usr/bin/env python3
"""Phase 8 model-checker measurements (Phase 7 binary vs this build).

1. Phase 6 fixture scenarios: verdict, states, transitions must be equal;
   median exploration wall time of N runs.
2. Exhaustive exploration of the extracted team-app action with a
   hypothetical UNIQUE(teamId, email), 2-6 actors, reduction on and off:
   states, transitions, exploration time, peak RSS.
3. Crash/retry dimensions on the extracted idempotent checkout: maxCrashes
   and maxRetries 0-2, 1-3 clients.
4. Verification cost: model only, --validate-postgres, --verify-source with
   node-postgres and postgres-js, with and without a crash.

Usage: scripts/phase8-model-bench.py PHASE7_BINARY OUT.md [N]
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


def mc(binary, scenario, *extra):
    flag = "-l" if platform.system() == "Darwin" else "-v"
    t0 = time.perf_counter()
    p = subprocess.run(["/usr/bin/time", flag, binary, "model-check", scenario, "--format", "json", *extra],
                       capture_output=True, text=True)
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


def main():
    old, out = sys.argv[1], sys.argv[2]
    n = int(sys.argv[3]) if len(sys.argv) > 3 else 5
    lines = ["# Phase 8 model-checker measurements", "",
             f"{platform.platform()}; median of {n} runs unless noted; `wall ms` is the exploration time the report states.", ""]
    lines += ["## 1. Phase 6 fixture scenarios", "",
              "| scenario | verdict | states | transitions | Phase 7 wall ms | Phase 8 wall ms |", "| --- | --- | --- | --- | --- | --- |"]
    for s in sorted(glob.glob(os.path.join(ROOT, "fixtures/concurrency/scenarios/*.json")) + glob.glob(os.path.join(ROOT, "fixtures/concurrency/corpus/*.json"))):
        a = [mc(old, s) for _ in range(n)]
        b = [mc(NEW, s) for _ in range(n)]
        da, db = a[0][0], b[0][0]
        for k in ("verdict",):
            assert da["exploration"][k] == db["exploration"][k], s
        for k in ("states", "transitions"):
            assert da["exploration"]["metrics"][k] == db["exploration"]["metrics"][k], s
        lines.append(f"| {os.path.basename(s)} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | "
                     f"{statistics.median(x[0]['timing']['wallMs'] for x in a)} | {statistics.median(x[0]['timing']['wallMs'] for x in b)} |")

    lines += ["", "## 2. Exhaustive exploration, extracted team-app action, hypothetical UNIQUE(teamId, email)", "",
              "| actors | POR | states | transitions | Phase 7 wall ms | Phase 8 wall ms | Phase 7 peak RSS MiB | Phase 8 peak RSS MiB |",
              "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    base = json.load(open(os.path.join(ROOT, "fixtures/phase7/scenarios/team-app-invite-double-submit.json")))
    for k in [2, 3, 4, 5, 6]:
        for por in [True, False]:
            if k == 6 and not por:
                continue
            doc = json.loads(json.dumps(base))
            doc.pop("sourceVerification", None)
            doc["actors"] = [dict(base["actors"][0], name=f"Submit{i+1}") for i in range(k)]
            doc["bounds"] = {"maxSteps": 120, "maxActors": 6, "maxStates": 5000000}
            doc["options"] = {"por": por}
            doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/phase7/team-app")
            for t in doc["database"]["tables"]:
                if t["name"] == "invitations":
                    t["unique"] = [["teamId", "email"]]
            path = temp_scenario(doc)
            reps = 1 if k >= 6 else (3 if k == 5 else n)
            a = [mc(old, path) for _ in range(reps)]
            b = [mc(NEW, path) for _ in range(reps)]
            os.unlink(path)
            da, db = a[0][0], b[0][0]
            assert da["exploration"]["metrics"]["states"] == db["exploration"]["metrics"]["states"]
            assert da["exploration"]["metrics"]["transitions"] == db["exploration"]["metrics"]["transitions"]
            lines.append(f"| {k} | {'on' if por else 'off'} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | "
                         f"{statistics.median(x[0]['timing']['wallMs'] for x in a)} | {statistics.median(x[0]['timing']['wallMs'] for x in b)} | "
                         f"{statistics.median(x[2] for x in a):.1f} | {statistics.median(x[2] for x in b):.1f} |")
            print(lines[-1], flush=True)

    lines += ["", "## 3. Crash and retry dimensions (extracted idempotent checkout, exhaustive)", "",
              "| clients | maxCrashes | maxRetries | verdict | states | transitions | Phase 8 wall ms |", "| --- | --- | --- | --- | --- | --- | --- |"]
    cbase = json.load(open(os.path.join(ROOT, "fixtures/phase8/scenarios/checkout-idempotent-crash-after-charge.json")))
    for clients in [1, 2, 3]:
        for crashes, retries in [(0, 0), (1, 1), (2, 2)]:
            doc = json.loads(json.dumps(cbase))
            doc.pop("sourceVerification", None)
            doc["actors"] = [dict(cbase["actors"][0], name=f"Client{i+1}",
                                  arguments=[{"requestId": f"req-{i+1}", "orderId": 1}]) for i in range(clients)]
            doc["bounds"] = {"maxSteps": 80, "maxCrashes": crashes, "maxRetries": retries, "maxStates": 5000000}
            doc["extraction"]["root"] = os.path.join(ROOT, "fixtures/phase8/shop-app")
            path = temp_scenario(doc)
            b = [mc(NEW, path) for _ in range(3)]
            os.unlink(path)
            db = b[0][0]
            lines.append(f"| {clients} | {crashes} | {retries} | {db['exploration']['verdict']} | {db['exploration']['metrics']['states']} | {db['exploration']['metrics']['transitions']} | {statistics.median(x[0]['timing']['wallMs'] for x in b)} |")
            print(lines[-1], flush=True)

    lines += ["", "## 4. Verification cost (process wall time, ms; median of 3)", "",
              "| scenario | model only | --validate-postgres | --verify-source (1 run) |", "| --- | --- | --- | --- |"]
    for name in ["fixtures/phase7/scenarios/team-app-invite-double-submit.json",
                 "fixtures/phase8/scenarios/pgjs-invite-double-submit.json",
                 "fixtures/phase8/scenarios/checkout-crash-after-charge.json",
                 "fixtures/phase8/scenarios/pgjs-checkout-crash-after-charge.json",
                 "fixtures/phase8/scenarios/accept-crash-while-waiting.json"]:
        s = os.path.join(ROOT, name)
        row = [statistics.median(mc(NEW, s)[1] for _ in range(3)),
               statistics.median(mc(NEW, s, "--validate-postgres")[1] for _ in range(3)),
               statistics.median(mc(NEW, s, "--verify-source", "--allow-execution")[1] for _ in range(3))]
        lines.append(f"| {os.path.basename(name)} | " + " | ".join(f"{x:.0f}" for x in row) + " |")
        print(lines[-1], flush=True)
    open(out, "w").write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
