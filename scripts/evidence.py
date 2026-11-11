#!/usr/bin/env python3
"""Runtime evidence: model counterexamples run against the ORIGINAL code of
the first-party fixture applications (`model-check --verify-source
--allow-execution`, disposable PostgreSQL clusters), with a Markdown summary.

Usage: scripts/evidence.py SUITE [--out DIR] [--repeat N] [--strict-source]

Suites (application code runs ONLY for first-party fixtures):
  crash-retry      every scenario of fixtures/scenarios/crash-retry: process
                   crashes and retries (runs, crash checks, restarts,
                   effects model = runtime). Default --repeat 5.
  in-flight        every scenario of fixtures/scenarios/in-flight: a crash
                   with an operation in flight (fate, what the proxy
                   observed, what PostgreSQL confirmed, session cleanup,
                   stuck states). Default --repeat 5.
  connection-loss  every scenario of fixtures/scenarios/connection-loss: a
                   connection broken under a live process, or a process
                   crash with an operation in flight (driver, fault, fate,
                   driver error, worker alive, server answer, durable rows,
                   the retry's connection). `live-blind-retry-unhandled-
                   error-event` is expected to DIVERGE. Default --repeat 5.
  commerce         discovery on fixtures/apps/commerce-app and every
                   generated counterexample on the original code.
  workspace        discovery on fixtures/apps/workspace-app, its generated
                   counterexamples on the original code and the model's
                   verdicts for their safe counterparts.
  responses        fixtures/apps/request-app (return value, last-statement
                   crash, upsert; safe counterparts must hold), the
                   foreign-key lock pairs of fixtures/apps/workspace-app,
                   and the read replica (model only:
                   fixtures/model/replicas).
  retry-wrapper    fixtures/apps/retry-wrapper-app: multi-row INSERT with a
                   lost response, a retry wrapper with a lost COMMIT, and
                   their safe counterparts.
  worlds           generated worlds under every search algorithm, on
                   PostgreSQL, and against the original code (see below).
Default --repeat 3, output reports/evidence/<suite>/.

worlds:
1. `midwrite discover --model-check --emit-scenarios` on the first-party
   applications listed in WORLDS_APPS. Every emitted scenario (one per
   explored world and variant) is model-checked with the unreduced search,
   sleep sets (default), classic DPOR and source-set DPOR; verdict and
   completeness must agree.
2. Every scenario with a counterexample is replayed on PostgreSQL in
   lockstep (`--validate-postgres`): the database outcomes must match.
3. The deciding worlds of every candidate with a model counterexample in the
   applications that ship an authorized harness (SOURCE_APPS) run against
   the original TypeScript. Safe counterparts are covered by recovery-probe
   tests (crates/midwrite-cli/tests/input_worlds.rs, loops.rs).
Non-reproduction is reported, not a failure of the suite (a counterexample
against a heuristic hypothesis, or one the harness cannot drive, is recorded
with its status); with --strict-source every candidate with status
MODEL_COUNTEREXAMPLE must reproduce. Disagreements always fail it.

Every suite exits 1 when its evidence is incomplete: a scenario expected to
reproduce did not in every run, a run was not followed or did not match the
model, or a safe counterpart did not hold. A blocked or diverged run is not
evidence. MIDWRITE_BIN selects the binary (default target/release/midwrite,
or $CARGO_TARGET_DIR/release/midwrite).
"""
import argparse
import glob
import json
import os
import shutil
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("MIDWRITE_BIN") or os.path.join(
    os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release", "midwrite")


def fx(*parts):
    return os.path.join(ROOT, "fixtures", *parts)


def run(args, strict=True):
    """Run midwrite and parse its JSON report."""
    p = subprocess.run([BIN, *args], capture_output=True, text=True)
    try:
        return json.loads(p.stdout)
    except json.JSONDecodeError:
        if strict:
            sys.exit(f"{' '.join(args)}: no JSON report\n{p.stderr}")
        return {"error": p.stderr.strip()[-400:]}


def verify_source(path, repeat, *extra):
    return run(["model-check", path, "--verify-source", "--allow-execution", "--source-repeat", str(repeat),
                "--format", "json", *extra])


def dump(path, doc):
    with open(path, "w") as f:
        json.dump(doc, f, indent=1, sort_keys=True)


def write_summary(out, lines):
    with open(os.path.join(out, "SUMMARY.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))


def fresh(out):
    if os.path.isdir(out):
        shutil.rmtree(out)
    os.makedirs(out)


def runs_ok(runs, n):
    followed = sum(1 for x in runs if x["coordination"] == "followed")
    matched = sum(1 for x in runs if x.get("reproduced") or x.get("recoveryReproduced"))
    return followed, matched, len(runs) == n and followed == len(runs) and matched == len(runs)


# --- scenario directories ---------------------------------------------------

def crash_retry(out, n):
    os.makedirs(out, exist_ok=True)
    incomplete = []
    rows = ["| scenario | model verdict | evidence | runs (followed / matched) | invariant on observed state | crash checks | restarts | effects (model = runtime) | PostgreSQL | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(fx("scenarios", "crash-retry", "*.json"))):
        name = os.path.basename(path)[:-5]
        t0 = time.perf_counter()
        r = verify_source(path, n)
        elapsed = time.perf_counter() - t0
        dump(os.path.join(out, name + ".json"), r)
        sv = r.get("sourceVerification", {})
        runs = sv.get("runs", [])
        followed, matched, ok = runs_ok(runs, n)
        inv = sorted({x["invariant"] for x in runs})
        rec = sv.get("recovery") or {}
        crashes = rec.get("crashes", [])
        restarts = rec.get("restarts", [])
        effects = rec.get("effects", {})
        if not ok:
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
    with open(os.path.join(out, "SUMMARY.md"), "w") as f:
        f.write(f"# Crash/retry original-code evidence ({n} runs per scenario)\n\n" + "\n".join(rows) + "\n")
    if incomplete:
        print("INCOMPLETE:\n" + "\n".join(incomplete), file=sys.stderr)
        return 1
    return 0


def in_flight(out, n):
    os.makedirs(out, exist_ok=True)
    incomplete = []
    rows = ["| scenario | model verdict | evidence | runs (followed / matched) | in-flight: operation → fate (model = run) | server answer observed | backend before kill | session ended (ms) | stuck / pool | tasks killed | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(fx("scenarios", "in-flight", "*.json"))):
        name = os.path.basename(path)[:-5]
        t0 = time.perf_counter()
        r = verify_source(path, n)
        elapsed = time.perf_counter() - t0
        dump(os.path.join(out, name + ".json"), r)
        sv = r.get("sourceVerification") or {}
        runs = sv.get("runs", [])
        followed, matched, ok = runs_ok(runs, n)
        nothing_to_run = (r["exploration"]["verdict"] == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS"
                          and sv.get("status") == "not-run")
        if not nothing_to_run and not ok:
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
    with open(os.path.join(out, "SUMMARY.md"), "w") as f:
        f.write(f"# In-flight original-code evidence ({n} runs per scenario)\n\n" + "\n".join(rows) + "\n")
    if incomplete:
        print("INCOMPLETE:\n" + "\n".join(incomplete), file=sys.stderr)
        return 1
    return 0


EXPECT_DIVERGE = {"live-blind-retry-unhandled-error-event"}


def connection_loss(out, n):
    os.makedirs(out, exist_ok=True)
    bad = []
    rows = ["| scenario | driver | fault | model verdict | evidence | runs followed / matched | fate (model = run) | driver error | worker alive | server answer | backend before | durable rows after the loss | retry's connection | s |",
            "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for path in sorted(glob.glob(fx("scenarios", "connection-loss", "*.json"))):
        name = os.path.basename(path)[:-5]
        t0 = time.perf_counter()
        r = verify_source(path, n)
        elapsed = time.perf_counter() - t0
        dump(os.path.join(out, name + ".json"), r)
        sv = r.get("sourceVerification") or {}
        runs = sv.get("runs", [])
        followed, matched, ok = runs_ok(runs, n)
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
        elif not ok:
            bad.append(f"{name}: {sv.get('status')}, {followed}/{len(runs)} followed, {matched} matched; {'; '.join(sv.get('problems', []))[:300]}")
    lines = [f"# Connection-loss original-code evidence ({n} runs per scenario)", "", *rows]
    if bad:
        lines += ["", "NOT EVIDENCE:", *("- " + b for b in bad)]
    write_summary(out, lines)
    return 1 if bad else 0


# --- discovery-generated scenarios ------------------------------------------

def generated(out, app, title, repeat, extra_note=""):
    """Discovery on APP, every generated counterexample on the original code."""
    fresh(out)
    scen = os.path.join(out, "scenarios")
    os.makedirs(scen)
    name = os.path.basename(app)
    report = run(["discover", app, "--model-check", "--emit-scenarios", scen, "--format", "json", "--no-timing"])
    dump(os.path.join(out, f"{name}.json"), report)
    for fmt, ext in (("text", "txt"), ("html", "html")):
        q = subprocess.run([BIN, "discover", app, "--model-check", "--format", fmt, "--no-timing"],
                           capture_output=True, text=True)
        with open(os.path.join(out, f"{name}.{ext}"), "w") as f:
            f.write(q.stdout)
    lines = [f"# {title}", "", f"Binary `{os.path.relpath(BIN, ROOT)}`; `--source-repeat {repeat}`.{extra_note}", "",
             "| candidate | class | entries | status | variant | model | runtime evidence | reproduced |",
             "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    failures = 0
    for c in report["candidates"]:
        entries = ", ".join(e["selector"].split("#")[1] for e in c["entries"])
        for sc in c.get("scenarios", []):
            model = (sc.get("model") or {}).get("verdict", "-")
            level, ok = "-", "-"
            if model == "COUNTEREXAMPLE_FOUND" and sc["variant"]["name"] == "actual" and sc.get("file") \
                    and sc.get("sourceVerification"):
                mc = verify_source(os.path.join(scen, sc["file"]), repeat, "--no-timing")
                dump(os.path.join(out, "runtime-" + sc["file"]), mc)
                runs = (mc.get("sourceVerification") or {}).get("runs", [])
                good = sum(1 for x in runs if x.get("reproduced"))
                level = mc["evidence"]["level"]
                ok = f"{good}/{len(runs)}"
                if good != repeat or not level.startswith("original-code-"):
                    failures += 1
            lines.append(f"| {c['id']} | {c['class']} | {entries} | {c['status']} | {sc['variant']['name']} | {model} | {level} | {ok} |")
    write_summary(out, lines)
    return 1 if failures else 0


def commerce(out, repeat):
    return generated(out, fx("apps", "commerce-app"), "Commerce application: generated counterexamples on the original code", repeat)


def workspace(out, repeat):
    return generated(out, fx("apps", "workspace-app"), "Workspace application: generated counterexamples on the original code", repeat)


# --- vertical slices --------------------------------------------------------

# (slice, scenario, expected: "reproduce" or "hold" or "model-only")
RESPONSES = [
    ("return value", "claim-race", "reproduce"),
    ("return value (safe)", "claim-atomic", "hold"),
    ("last-statement crash", "signup-crash", "reproduce"),
    ("last-statement crash (safe)", "signup-idempotent", "hold"),
    ("upsert", "reserve-race", "reproduce"),
    ("upsert (safe)", "reserve-if-free", "hold"),
    ("upsert (safe)", "count-views", "hold"),
]


def responses(out, repeat):
    fresh(out)
    scen = os.path.join(out, "scenarios")
    os.makedirs(scen)
    rows, failures = [], 0

    def model_check(path, verify=False):
        args = ["model-check", path, "--format", "json", "--no-timing"]
        if verify:
            args += ["--verify-source", "--allow-execution", "--source-repeat", str(repeat)]
        return run(args)

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
        # not a directory.
        dump(os.path.join(out, name.replace("/", "_") + ".json"), mc)

    for slice_, name, expect in RESPONSES:
        mc = model_check(fx("apps", "request-app", "scenarios", name + ".json"), expect == "reproduce")
        record(slice_, name, mc, expect)

    # Discovery on the workspace application: the two foreign-key pairs.
    report = run(["discover", fx("apps", "workspace-app"), "--model-check", "--emit-scenarios", scen,
                  "--format", "json", "--no-timing"])
    dump(os.path.join(out, "workspace-app-discovery.json"), report)
    for pair in (["createWorkspace", "createWorkspaceLocked"], ["createWorkspaceAction/arg[0]", "createWorkspaceLocked"]):
        c = next(c for c in report["candidates"]
                 if c["class"] == "check-then-act"
                 and sorted(e["selector"].split("#")[1] for e in c["entries"]) == sorted(pair))
        file = next(sc["file"] for sc in c["scenarios"] if sc["variant"]["name"] == "actual")
        mc = model_check(os.path.join(scen, file), True)
        record("foreign-key lock", f"{c['id']} {' × '.join(pair)}", mc, "reproduce")

    # The read replica (model only).
    mc = model_check(fx("model", "replicas", "replica-read-your-writes.json"))
    record("replica staleness (model only)", "replica-read-your-writes", mc, "model-only")

    write_summary(out, [
        "# Responses, foreign-key locks and read replicas: model and original code", "",
        f"Binary `{os.path.relpath(BIN, ROOT)}`; `--source-repeat {repeat}`; original code runs on disposable PostgreSQL (first-party fixtures only). The read-replica slice is model-only: no PostgreSQL replication is run.",
        "", "| slice | scenario | model | states | evidence | reproduced | expectation |",
        "| --- | --- | --- | ---: | --- | --- | --- |", *rows])
    return 1 if failures else 0


# (slice, entry filter, class, entry suffix, expectation): "reproduce" (model
# counterexample reproduced on the original code), "hold" (no counterexample
# within the bounds), "model" (model counterexample, not run).
RETRY_WRAPPER = [
    ("multi-row INSERT, response lost, retry", "items.ts#addOrderItems", "resubmitted-write", "#addOrderItems", "reproduce"),
    ("multi-row INSERT (safe)", "items.ts#addOrderItemsIdempotent", "resubmitted-write", "#addOrderItemsIdempotent", "hold"),
    ("retry wrapper, lost COMMIT", "payments.ts#settlePayment", "ambiguous-commit", "#settlePayment", "reproduce"),
    ("retry wrapper (safe)", "payments.ts#settlePaymentOnce", "ambiguous-commit", "#settlePaymentOnce", "hold"),
]


def retry_wrapper(out, repeat):
    app = fx("apps", "retry-wrapper-app")
    fresh(out)
    scen = os.path.join(out, "scenarios")
    rows, failures = [], 0
    for slice_, entry, cls, suffix, expect in RETRY_WRAPPER:
        os.makedirs(scen, exist_ok=True)
        report = run(["discover", app, "--model-check", "--entry", entry, "--emit-scenarios", scen,
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
            mc = verify_source(os.path.join(scen, actual["file"]), repeat, "--no-timing")
            dump(os.path.join(out, c["id"] + suffix.replace("#", "-") + ".json"), mc)
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
    write_summary(out, [
        "# Retry wrapper: model and original code", "",
        f"Binary `{os.path.relpath(BIN, ROOT)}`; `--source-repeat {repeat}`; original code runs on disposable PostgreSQL (first-party fixture only).",
        "The harness handles errors of checked-out node-postgres clients (a stated environment assumption, `fixtures/apps/retry-wrapper-app/verify/harness.ts`).",
        "", "| slice | candidate | status | model | evidence | reproduced | expectation |",
        "| --- | --- | --- | --- | --- | --- | --- |", *rows])
    return 1 if failures else 0


# --- generated worlds -------------------------------------------------------

WORLDS_APPS = [
    "fixtures/eval/p13/eval-app",
    "fixtures/eval/p14/eval-app",
    "fixtures/eval/p15/eval-app",
    "fixtures/apps/worlds-app",
    "fixtures/eval/p16/eval-app",
    "fixtures/apps/zod/app",
    "fixtures/apps/retry",
    "fixtures/apps/locks",
    "fixtures/apps/nulls",
    "fixtures/apps/genid",
]
SOURCE_APPS = ["fixtures/apps/worlds-app", "fixtures/apps/retry"]
ALGORITHMS = [
    ("unreduced", ["--no-por"]),
    ("sleep-sets", []),
    ("classic-dpor", ["--dpor"]),
    ("source-dpor", ["--source-dpor"]),
]


def worlds(out, repeat, strict_source=False):
    os.makedirs(out, exist_ok=True)
    results = {"binary": os.path.relpath(BIN, ROOT), "apps": {}, "por": [], "postgres": [], "source": []}
    source_reports = {}
    for app in WORLDS_APPS:
        scen = os.path.join(out, "scenarios", app.replace("fixtures/", "").replace("/", "-"))
        shutil.rmtree(scen, ignore_errors=True)
        os.makedirs(scen)
        rep = run(["discover", os.path.join(ROOT, app), "--model-check", "--no-timing", "--format", "json",
                   "--emit-scenarios", scen], strict=False)
        results["apps"][app] = {
            "candidates": len(rep.get("candidates", [])),
            "byStatus": rep.get("summary", {}).get("byStatus", {}),
            "budgets": rep.get("budgets", {}),
            "scenarios": scen,
        }
        if app in SOURCE_APPS:
            source_reports[app] = rep
        for name in sorted(os.listdir(scen)):
            if not name.endswith(".json") or name.endswith(".counterexample.json"):
                continue
            path = os.path.join(scen, name)
            row = {"app": app, "scenario": name, "algorithms": {}}
            for label, extra in ALGORITHMS:
                mc = run(["model-check", path, "--format", "json", "--no-timing"] + extra, strict=False)
                ex = mc.get("exploration", {})
                row["algorithms"][label] = {
                    "verdict": ex.get("verdict", mc.get("error")),
                    "complete": ex.get("complete"),
                    "states": ex.get("metrics", {}).get("states"),
                }
            base = row["algorithms"]["unreduced"]
            row["agree"] = all(
                (v["verdict"], v["complete"]) == (base["verdict"], base["complete"])
                for v in row["algorithms"].values()
            )
            results["por"].append(row)
            if base["verdict"] == "COUNTEREXAMPLE_FOUND":
                mc = run(["model-check", path, "--validate-postgres", "--format", "json", "--no-timing"], strict=False)
                rt = mc.get("runtime", {})
                results["postgres"].append({
                    "app": app, "scenario": name, "status": rt.get("status"),
                    "observations": len(rt.get("observations", [])),
                    "disagreements": sum(1 for o in rt.get("observations", []) if not o.get("agree", True)),
                    "evidence": mc.get("evidence", {}).get("level"),
                })
    # Original code: deciding worlds of the harnessed applications.
    for app, rep in source_reports.items():
        scen = results["apps"][app]["scenarios"]
        for c in rep.get("candidates", []):
            entry = c["entries"][0]["selector"].split("#")[1]
            s = next((s for s in c["scenarios"] if s.get("model", {}).get("verdict") == "COUNTEREXAMPLE_FOUND"), None)
            if not s or not s.get("file"):
                continue
            mc = run(["model-check", os.path.join(scen, s["file"]), "--verify-source", "--allow-execution",
                      "--source-repeat", str(repeat), "--format", "json", "--no-timing"], strict=False)
            sv = mc.get("sourceVerification", {})
            results["source"].append({
                "app": app, "entry": entry, "candidateStatus": c["status"], "class": c.get("class"),
                "world": (s.get("world") or {}).get("id"), "scenario": s["file"], "status": sv.get("status"),
                "evidence": mc.get("evidence", {}).get("level"),
                "runs": [{k: r.get(k) for k in ("statementsAgree", "tablesAgree", "outcomesAgree", "invariant", "reproduced")}
                         for r in sv.get("runs", [])],
            })
    for r in results["apps"].values():
        r["scenarios"] = os.path.relpath(r["scenarios"], ROOT)
    with open(os.path.join(out, "results.json"), "w") as f:
        json.dump(results, f, indent=1, sort_keys=True)
        f.write("\n")
    agree = sum(1 for r in results["por"] if r["agree"])
    lines = [
        "# Generated worlds: search algorithms, PostgreSQL and original code", "",
        f"Binary `{results['binary']}`. Generated by `scripts/evidence.py worlds`.", "",
        "## Applications", "",
        "| application | candidates | statuses | worlds built | worlds invalid | worlds duplicate | worlds skipped (budget) |",
        "| --- | ---: | --- | ---: | ---: | ---: | ---: |",
    ]
    for app, r in results["apps"].items():
        b = r["budgets"]
        lines.append(f"| `{app}` | {r['candidates']} | {r['byStatus']} | {b.get('worldsBuilt')} | {b.get('worldsInvalid')} | {b.get('worldsDuplicate')} | {b.get('worldsSkippedForBudget')} |")
    lines += ["", "## Search algorithms (every emitted scenario)", "",
              f"{agree} of {len(results['por'])} scenarios: unreduced, sleep sets, classic DPOR and source-set DPOR agree on verdict and completeness.", ""]
    for r in results["por"]:
        if not r["agree"]:
            lines.append(f"* DISAGREEMENT `{r['scenario']}`: {r['algorithms']}")
    pg_ok = sum(1 for r in results["postgres"] if r["status"] == "confirmed")
    lines += ["## PostgreSQL lockstep (scenarios with a counterexample)", "",
              f"{pg_ok} of {len(results['postgres'])} confirmed; database outcomes compared: {sum(r['observations'] for r in results['postgres'])}, disagreements: {sum(r['disagreements'] for r in results['postgres'])}.", ""]
    for r in results["postgres"]:
        if r["status"] != "confirmed":
            lines.append(f"* `{r['scenario']}`: {r['status']} ({r['evidence']})")
    lines += ["", "## Original code (" + ", ".join(f"`{x}`" for x in SOURCE_APPS) + ")", "",
              "| application | entry | class | candidate status | world | original code | evidence | runs reproduced |", "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    for r in results["source"]:
        rep = sum(1 for x in r["runs"] if x.get("reproduced"))
        lines.append(f"| `{r['app']}` | `{r['entry']}` | {r['class']} | {r['candidateStatus']} | `{r['world']}` | {r['status']} | {r['evidence']} | {rep}/{len(r['runs'])} |")
    write_summary(out, lines)
    bad = agree != len(results["por"]) or sum(r["disagreements"] for r in results["postgres"]) > 0
    if strict_source:
        bad = bad or any(r["status"] != "reproduced" for r in results["source"]
                         if r["candidateStatus"] == "MODEL_COUNTEREXAMPLE")
    return 1 if bad else 0


SUITES = {
    "crash-retry": (crash_retry, 5),
    "in-flight": (in_flight, 5),
    "connection-loss": (connection_loss, 5),
    "commerce": (commerce, 3),
    "workspace": (workspace, 3),
    "responses": (responses, 3),
    "retry-wrapper": (retry_wrapper, 3),
    "worlds": (worlds, 3),
}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("suite", choices=sorted(SUITES))
    ap.add_argument("--out", help="output directory (default reports/evidence/SUITE)")
    ap.add_argument("--repeat", type=int, help="original-code runs per scenario")
    ap.add_argument("--strict-source", action="store_true",
                    help="worlds: fail unless every MODEL_COUNTEREXAMPLE candidate reproduces")
    a = ap.parse_args()
    fn, default_repeat = SUITES[a.suite]
    out = os.path.abspath(a.out or os.path.join(ROOT, "reports", "evidence", a.suite))
    repeat = a.repeat or default_repeat
    if a.suite == "worlds":
        sys.exit(fn(out, repeat, a.strict_source))
    sys.exit(fn(out, repeat))


if __name__ == "__main__":
    main()
