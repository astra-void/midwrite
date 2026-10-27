#!/usr/bin/env python3
"""Schema plus semantic validation of model-check reports, Phase 10 rules
included, with negative tests.

    scripts/phase10-report-check.py REPORT...

Everything of scripts/phase9-report-check.py, plus (schema 5):

  wire log     `ClientSevered` is a known event, in schema 5 reports only
  losses       a loss that agrees has the model's fate, a severed client, a
               worker alive afterwards and a driver error; the wire facts fit
               the fate (forwarded xor withheld; a server answer exactly for
               answered-and-withheld); a held operation's session is open, the
               others' ended; the application did not keep using the broken
               connection's backend; transitions and actors exist
  evidence     original-code-connection-loss-reproduced needs at least one
               loss, every loss agreeing, and a counterexample
  reduction    `source-dpor` needs its mode and its reduction metrics and is
               never used with an `always` invariant (it is refused then)

Each applicable corruption must be rejected; exit status 1 otherwise.
"""
import importlib.util
import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("p9", ROOT / "scripts/phase9-report-check.py")
p9 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p9)

LOSS_LEVEL = "original-code-connection-loss-reproduced"
p9.FAILURE_LEVELS.add(LOSS_LEVEL)
p9.WIRE_EVENTS.add("ClientSevered")

phase9_semantic = p9.semantic_errors
phase9_corruptions = p9.corruptions


def losses(r):
    return ((r.get("sourceVerification") or {}).get("recovery") or {}).get("losses", [])


def semantic_errors(r):
    e = phase9_semantic(r)
    sv = r.get("sourceVerification") or {}
    if r["schemaVersion"] < 5 and any("ClientSevered" in line for line in sv.get("wireLog", [])):
        e.append("wire log: ClientSevered in a schema 4 report")
    steps = len(r["counterexample"]["steps"]) if r.get("counterexample") else None
    names = {a["name"] for a in r["actors"]}
    for l in losses(r):
        where = f"loss of {l['operation']!r} at transition {l['transition']}"
        if l["transition"] < 1 or (steps is not None and l["transition"] > steps):
            e.append(f"{where}: transition does not exist")
        if l["actor"] not in names:
            e.append(f"{where}: {l['actor']!r} is not a declared actor")
        if l["forwardedToServer"] == l["withheldFromServer"] and l["status"] != "not-observed":
            e.append(f"{where}: forwarded and withheld must be exclusive")
        fate = l.get("observedFate") or l["modelFate"]
        if l["status"] == "agree":
            if l.get("observedFate") != l["modelFate"]:
                e.append(f"{where}: agrees but fate {l.get('observedFate')} != model {l['modelFate']}")
            if not l["clientSevered"]:
                e.append(f"{where}: agrees but the client connection was not severed")
            if not l["workerAliveAfter"]:
                e.append(f"{where}: agrees but the worker did not survive (that is a crash, not a loss)")
            if not l.get("driverError"):
                e.append(f"{where}: agrees but no driver error was observed")
            if fate == "held-for-later-delivery" and not l["sessionOpen"]:
                e.append(f"{where}: held operation without an open session")
            if fate != "held-for-later-delivery" and not l["sessionEnded"]:
                e.append(f"{where}: {fate} but the backend session did not end")
        if fate == "answered-and-withheld" and l["status"] != "not-observed" and not (l["forwardedToServer"] and l.get("serverAnswer")):
            e.append(f"{where}: answered-and-withheld without a forwarded operation and a server answer")
        if fate != "answered-and-withheld" and (l["forwardedToServer"] or l.get("serverAnswer")):
            e.append(f"{where}: {fate} but the server received or answered the operation")
        if l.get("backendPid") is not None and l["backendPid"] in l.get("laterBackendPids", []):
            e.append(f"{where}: the application kept using the broken connection's backend")
    b = r.get("bounds") or {}
    if b.get("reduction") == "source-dpor":
        if not b.get("sourceDporMode"):
            e.append("source-dpor without its mode")
        if "reductionMetrics" not in r["exploration"]:
            e.append("source-dpor without reduction metrics")
        if any(i["when"] == "always" for i in r["invariants"]):
            e.append("source-dpor ran with an `always` invariant (it must be refused)")
    elif b.get("sourceDporMode") or "reductionMetrics" in r["exploration"]:
        e.append("source-dpor fields without `reduction: source-dpor`")
    if r["evidence"]["level"] == LOSS_LEVEL:
        ls = losses(r)
        if not ls or any(l["status"] != "agree" for l in ls):
            e.append(f"evidence {LOSS_LEVEL} without agreeing connection losses")
    return e


def corruptions(r):
    out = phase9_corruptions(r)
    ls = losses(r)
    if (r.get("bounds") or {}).get("reduction") == "source-dpor":
        out += [
            ("source-dpor with an always invariant", lambda b: b["invariants"][0].__setitem__("when", "always")),
            ("source-dpor without its metrics", lambda b: b["exploration"].pop("reductionMetrics")),
            ("source-dpor without its mode", lambda b: b["bounds"].pop("sourceDporMode")),
            ("source-dpor mode not in the enum", lambda b: b["bounds"].__setitem__("sourceDporMode", "optimal")),
            ("reduction metrics under another reduction", lambda b: b["bounds"].__setitem__("reduction", "dpor")),
        ]
    if r["schemaVersion"] >= 5 and r["exploration"]["verdict"] == "COUNTEREXAMPLE_FOUND" and not ls:
        out.append(("connection-loss evidence without a loss",
                    lambda b: (b["evidence"].__setitem__("level", LOSS_LEVEL))))
    if ls:
        def lo(b):
            return b["sourceVerification"]["recovery"]["losses"][0]
        fate = ls[0].get("observedFate") or ls[0]["modelFate"]
        swapped = "not-reached" if fate == "answered-and-withheld" else "answered-and-withheld"
        out += [
            ("invalid loss fate enum", lambda b: lo(b).__setitem__("modelFate", "maybe")),
            ("loss in an older schema", lambda b: b.__setitem__("schemaVersion", 4)),
            ("forwarded and withheld both", lambda b: (lo(b).__setitem__("forwardedToServer", True), lo(b).__setitem__("withheldFromServer", True))),
            ("loss at an impossible transition", lambda b: lo(b).__setitem__("transition", 0)),
            ("loss of an unknown actor", lambda b: lo(b).__setitem__("actor", "Nobody")),
        ]
        if ls[0]["status"] == "agree":
            out += [
                ("agreeing loss with a dead worker", lambda b: lo(b).__setitem__("workerAliveAfter", False)),
                ("agreeing loss without a driver error", lambda b: lo(b).pop("driverError", None)),
                ("agreeing loss with another fate", lambda b: lo(b).__setitem__("observedFate", swapped)),
                ("agreeing loss without severing the client", lambda b: lo(b).__setitem__("clientSevered", False)),
            ]
        if ls[0].get("backendPid") is not None:
            out.append(("the broken backend reused", lambda b: lo(b).setdefault("laterBackendPids", []).append(lo(b)["backendPid"])))
    return out


p9.semantic_errors = semantic_errors
p9.corruptions = corruptions

if __name__ == "__main__":
    sys.exit(p9.main())
