#!/usr/bin/env python3
"""Schema plus semantic validation of model-check reports, and negative tests.

    scripts/phase9-report-check.py REPORT...

Every report must pass its JSON Schema (by `schemaVersion`, built-in
validator of scripts/validate-schemas.py) and the semantic rules below.
Then each applicable corruption is applied to a copy, one at a time, and
the copy must be REJECTED by the schema or by a semantic rule. A corruption
that is accepted is a failure. Exit status 1 on any failure.

Semantic rules (a schema cannot express them):
  verdict      COUNTEREXAMPLE_FOUND <=> a counterexample; its invariant is the
               one marked violated; no invariant is violated otherwise
  evidence     the evidence level agrees with the verdict, and the
               source-verification label agrees with the evidence level
  runs         a reproduced source verification has only followed runs
  in-flight    the fate observed equals the model's when the crash agrees;
               the client never acknowledged an in-flight operation; the
               wire facts fit the fate (forwarded xor withheld, a server
               answer exactly for answered-and-withheld) and the wire events
               are complete for it (no truncated proxy observation)
  references   every transition referenced exists in the counterexample;
               every actor or task reference names a declared actor
               (`Name` or `Name#n`, n >= 1); restarts follow a killed pid
  wire log     each line is `[ms] conn N Event...` with a known event,
               every connection (not the control channel, conn 0) starts with
               Connected, time never goes back
  metrics      a complete exploration is not truncated; terminal <= states
"""
import copy
import importlib.util
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("validate_schemas", ROOT / "scripts/validate-schemas.py")
vs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vs)

FAILURE_LEVELS = {"model-counterexample", "postgres-confirmed-schedule", "original-code-reproduced",
                  "original-code-crash-reproduced", "original-code-in-flight-failure-reproduced"}
HOLDS_LEVELS = {"original-code-recovery-reproduced", "original-code-invariant-held"}
WIRE_EVENTS = {"Connected", "BackendKey", "Message", "FaultArmed", "OperationForwarded", "OperationHeld",
               "OperationCompleted", "ResponseWithheld", "Delivered", "Discarded", "ClientClosed",
               "ServerClosed", "Unsupported"}
# The wire events an in-flight fate must show, in this order.
FATE_EVENTS = {
    "answered-and-withheld": ["OperationForwarded", "OperationCompleted", "ResponseWithheld"],
    "held-for-later-delivery": ["OperationHeld"],
    "not-reached": ["OperationHeld"],
}
WIRE_LINE = re.compile(r"^\[(\d+) ms\] conn (\d+) ([A-Za-z]+)(?::|$)")


def schema_errors(report):
    path = vs.SCHEMAS.get((report.get("kind"), report.get("schemaVersion")))
    if path is None:
        return [f"no schema for {report.get('kind')!r} version {report.get('schemaVersion')!r}"]
    schema = json.loads(path.read_text())
    return [f"{'/'.join(map(str, p))}: {m}" for p, m in vs.builtin_errors(schema, report, schema)]


def semantic_errors(r):
    e = []
    verdict = r["exploration"]["verdict"]
    ce = r.get("counterexample")
    if (verdict == "COUNTEREXAMPLE_FOUND") != (ce is not None):
        e.append(f"verdict {verdict} but counterexample {'present' if ce else 'absent'}")
    violated = [i["name"] for i in r["invariants"] if i["status"] == "violated"]
    if ce is not None and violated != [ce["invariant"]]:
        e.append(f"counterexample violates {ce['invariant']!r} but invariants marked violated: {violated}")
    if ce is None and violated:
        e.append(f"no counterexample but invariants marked violated: {violated}")
    level = r["evidence"]["level"]
    if level in FAILURE_LEVELS and verdict != "COUNTEREXAMPLE_FOUND":
        e.append(f"evidence {level} claims a failure but the verdict is {verdict}")
    if level in HOLDS_LEVELS and verdict != "NO_COUNTEREXAMPLE_WITHIN_BOUNDS":
        e.append(f"evidence {level} claims the invariant held but the verdict is {verdict}")
    x = r["exploration"]
    if x["complete"] and any(x["truncation"].values()):
        e.append("exploration complete but truncated")
    m = x["metrics"]
    if m["terminalStates"] > m["states"]:
        e.append("more terminal states than states")

    names = {a["name"] for a in r["actors"]}
    processes = {a.get("process", a["name"]) for a in r["actors"]}
    steps = len(ce["steps"]) if ce else None

    def actor_ref(where, ref):
        # A whole-process crash step names the process: `process Name`.
        known = processes if ref.startswith("process ") else names
        base, _, task = ref.removeprefix("process ").partition("#")
        if base not in known or (task and not re.fullmatch(r"[1-9][0-9]*", task)):
            e.append(f"{where}: {ref!r} is not a declared actor or task")

    def transition_ref(where, t):
        if t < 1 or (steps is not None and t > steps):
            e.append(f"{where}: transition {t} does not exist (counterexample has {steps} steps)")

    if ce:
        for s in ce["steps"]:
            actor_ref(f"counterexample step {s['index']}", s["actor"])
            for ev in s.get("events", []):
                if not 0 <= ev["actor"] < len(r["actors"]):
                    e.append(f"counterexample step {s['index']}: event actor index {ev['actor']} out of range")
    for t in (r.get("epistemic") or {}).get("twins", []):
        transition_ref("twin", t["transition"])
        actor_ref("twin", t["actor"])
        if t["actual"] == t["other"]:
            e.append(f"twin at transition {t['transition']}: the other world is the actual one")

    sv = r.get("sourceVerification")
    if sv:
        label = sv.get("evidence")
        if label and label.startswith("ORIGINAL_CODE_") and label.lower().replace("_", "-") != level:
            e.append(f"source verification says {label} but the evidence level is {level}")
        runs = sv.get("runs", [])
        if sv["status"] in ("reproduced", "recovery-reproduced", "invariant-held"):
            if not runs or any(run["coordination"] != "followed" for run in runs):
                e.append(f"status {sv['status']} with runs that were not all followed")
        for s in sv.get("statements", []):
            transition_ref("statement", s["transition"])
            actor_ref("statement", s["actor"])
        rec = sv.get("recovery") or {}
        killed = {}
        for c in rec.get("crashes", []):
            transition_ref("crash", c["transition"])
            actor_ref("crash", c["actor"])
            if "pid" in c:
                killed.setdefault(c["actor"].partition("#")[0], set()).add(c["pid"])
            f = c.get("inFlight")
            if not f:
                continue
            where = f"in-flight {f['operation']} at transition {c['transition']}"
            if c["status"] == "agree" and f.get("observedFate") != f["modelFate"]:
                e.append(f"{where}: crash agrees but fate {f.get('observedFate')} != model {f['modelFate']}")
            if f.get("clientAcknowledged"):
                e.append(f"{where}: an in-flight operation cannot have been acknowledged to the client")
            fate = f.get("observedFate") or f["modelFate"]
            if f["forwardedToServer"] == f["withheldFromServer"]:
                e.append(f"{where}: forwarded and withheld must be exclusive")
            if fate == "answered-and-withheld" and not (f["forwardedToServer"] and f.get("serverAnswer")):
                e.append(f"{where}: answered-and-withheld without a forwarded operation and a server answer")
            if fate != "answered-and-withheld" and (f["forwardedToServer"] or f.get("serverAnswer")):
                e.append(f"{where}: {fate} but the server received or answered the operation")
            kinds = [w.split(":", 1)[0] for w in f.get("wireEvents", [])]
            want = FATE_EVENTS[fate]
            it = iter(kinds)
            if not all(k in it for k in want):
                e.append(f"{where}: wire events {kinds} are incomplete for {fate} (need {want})")
        seen = {k: set(v) for k, v in killed.items()}
        for s in rec.get("restarts", []):
            transition_ref("restart", s["transition"])
            actor_ref("restart", s["actor"])
            base = s["actor"].partition("#")[0]
            if s.get("previousOutcome") == "killed" and s.get("previousPid") not in seen.get(base, set()):
                e.append(f"restart at transition {s['transition']}: previous pid {s.get('previousPid')} was never killed")
            if "pid" in s:
                seen.setdefault(base, set()).add(s["pid"])
        last, conns = -1, set()
        for line in sv.get("wireLog", []):
            w = WIRE_LINE.match(line)
            if not w or w.group(3) not in WIRE_EVENTS:
                e.append(f"wire log: malformed or unknown event {line[:60]!r}")
                continue
            ms, conn, kind = int(w.group(1)), w.group(2), w.group(3)
            if ms < last:
                e.append(f"wire log: time goes back at {line[:60]!r}")
            last = ms
            # conn 0 is the proxy's control channel (FaultArmed), not a connection.
            if conn != "0" and conn not in conns and kind != "Connected":
                e.append(f"wire log: conn {conn} starts with {kind}, not Connected")
            conns.add(conn)
    return e


def first(items, pred):
    return next((x for x in items if pred(x)), None)


def in_flight(r):
    crashes = ((r.get("sourceVerification") or {}).get("recovery") or {}).get("crashes", [])
    return first(crashes, lambda c: c.get("inFlight"))


def corruptions(r):
    """(name, mutate) pairs that apply to this report."""
    out = [
        ("missing required field", lambda b: b.pop("exploration")),
        ("numeric field as string", lambda b: b["exploration"]["metrics"].__setitem__("states", "21")),
        ("invalid verdict enum", lambda b: b["exploration"].__setitem__("verdict", "PROBABLY_FINE")),
        ("negative count", lambda b: b["exploration"]["metrics"].__setitem__("transitions", -1)),
    ]
    if r["schemaVersion"] < 4:
        out.append(("Phase 9 field in an older schema", lambda b: b.__setitem__("epistemic", {"twins": []})))
        return out
    verdict = r["exploration"]["verdict"]
    if verdict == "COUNTEREXAMPLE_FOUND":
        out += [
            ("evidence claims the invariant held", lambda b: b["evidence"].__setitem__("level", "original-code-recovery-reproduced")),
            ("verdict without its counterexample", lambda b: b.pop("counterexample")),
            ("unknown actor in a counterexample step", lambda b: b["counterexample"]["steps"][0].__setitem__("actor", "Nobody")),
        ]
    elif verdict == "NO_COUNTEREXAMPLE_WITHIN_BOUNDS":
        out.append(("evidence claims a failure", lambda b: b["evidence"].__setitem__("level", "original-code-in-flight-failure-reproduced")))
    sv = r.get("sourceVerification")
    if sv and (sv.get("evidence") or "").startswith("ORIGINAL_CODE_"):
        other = "ORIGINAL_CODE_INVARIANT_HELD" if sv["evidence"] != "ORIGINAL_CODE_INVARIANT_HELD" else "ORIGINAL_CODE_CRASH_REPRODUCED"
        out.append(("contradictory source-verification label", lambda b: b["sourceVerification"].__setitem__("evidence", other)))
    if sv and sv.get("runs") and sv["status"] in ("reproduced", "recovery-reproduced", "invariant-held"):
        out.append(("reproduced with a diverged run", lambda b: b["sourceVerification"]["runs"][-1].__setitem__("coordination", "diverged")))
    if sv and sv.get("statements"):
        out.append(("malformed task reference", lambda b: b["sourceVerification"]["statements"][0].__setitem__("actor", b["sourceVerification"]["statements"][0]["actor"].partition("#")[0] + "#0")))
        if r.get("counterexample"):
            out.append(("impossible state reference", lambda b: b["sourceVerification"]["statements"][-1].__setitem__("transition", len(b["counterexample"]["steps"]) + 5)))
    if r.get("epistemic", {}).get("twins"):
        out.append(("twin at transition 0", lambda b: b["epistemic"]["twins"][0].__setitem__("transition", 0)))
        out.append(("twin world equal to the actual one", lambda b: b["epistemic"]["twins"][0].__setitem__("other", b["epistemic"]["twins"][0]["actual"])))
    c = in_flight(r)
    if c:
        i = r["sourceVerification"]["recovery"]["crashes"].index(c)

        def fl(b):
            return b["sourceVerification"]["recovery"]["crashes"][i]["inFlight"]
        fate = c["inFlight"]["observedFate"]
        swapped = "not-reached" if fate == "answered-and-withheld" else "answered-and-withheld"
        out += [
            ("invalid fate enum", lambda b: fl(b).__setitem__("observedFate", "maybe-committed")),
            ("observed fate differs but crash agrees", lambda b: fl(b).__setitem__("observedFate", swapped)),
            ("in-flight operation acknowledged", lambda b: fl(b).__setitem__("clientAcknowledged", True)),
            ("forwarded and withheld both", lambda b: (fl(b).__setitem__("forwardedToServer", True), fl(b).__setitem__("withheldFromServer", True))),
            ("truncated proxy observation", lambda b: fl(b).__setitem__("wireEvents", fl(b)["wireEvents"][:-1])),
            ("crash at an impossible transition", lambda b: b["sourceVerification"]["recovery"]["crashes"][i].__setitem__("transition", 0)),
        ]
    if sv and sv.get("wireLog"):
        out += [
            ("invalid lifecycle event", lambda b: b["sourceVerification"]["wireLog"].append("[999999 ms] conn 1 Teleported: nowhere")),
            ("connection without Connected", lambda b: b["sourceVerification"]["wireLog"].append("[999999 ms] conn 77 ServerClosed")),
            ("wire time goes back", lambda b: b["sourceVerification"]["wireLog"].append("[0 ms] conn 1 ClientClosed")),
        ]
    rec = (sv or {}).get("recovery") or {}
    if first(rec.get("restarts", []), lambda s: s.get("previousOutcome") == "killed"):
        def bad_restart(b):
            s = first(b["sourceVerification"]["recovery"]["restarts"], lambda s: s.get("previousOutcome") == "killed")
            s["previousPid"] = 1
        out.append(("restart of a pid that was never killed", bad_restart))
    return out


def main():
    failed = False
    applied = 0
    for path in map(pathlib.Path, sys.argv[1:]):
        r = json.loads(path.read_text())
        errs = schema_errors(r)
        sem = semantic_errors(r) if not errs else []
        if errs or sem:
            failed = True
            print(f"FAIL {path.name}: valid report rejected: {(errs + sem)[:3]}")
            continue
        rejected = 0
        cases = corruptions(r)
        for name, mutate in cases:
            bad = copy.deepcopy(r)
            mutate(bad)
            why = schema_errors(bad)
            how = "schema"
            if not why:
                try:
                    why, how = semantic_errors(bad), "semantic"
                except (KeyError, TypeError) as ex:
                    why, how = [f"unreadable: {ex!r}"], "structure"
            if why:
                rejected += 1
            else:
                failed = True
                print(f"FAIL {path.name}: corruption accepted: {name}")
        applied += len(cases)
        print(f"ok   {path.name} (schema {r['schemaVersion']}): valid; {rejected}/{len(cases)} corruptions rejected")
    print(f"{applied} corruptions applied")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
