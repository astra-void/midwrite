#!/usr/bin/env python3
"""Phase 9 final tables.

    scripts/phase9-final-tables.py EVIDENCE_DIR OUT_DIR

1. OUT_DIR/evidence-matrix.md: one row per original-code report in
   EVIDENCE_DIR (scripts/phase9-evidence.py output): entry points, PostgreSQL
   version, in-flight fate (model = observed), verdicts, runs followed and
   matched, inconclusive runs, statements that differ or fall outside the
   model, durable rows right after each kill and at the end (read from
   PostgreSQL by the coordinator), client acknowledgment, runtime invariant.
2. OUT_DIR/reduction-modes.md: verdict, completeness, states, transitions and
   the counterexample's step kinds of every Phase 9 scenario (model only),
   the recovery-strategy model scenarios, and the Phase 8 delivery scenarios
   with one in-flight fault, under full search, sleep sets and DPOR.
"""
import glob
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(os.environ.get("CARGO_TARGET_DIR") or os.path.join(ROOT, "target"), "release", "midwrite")


def rows(tables):
    if tables is None:
        return "—"
    return ", ".join(f"{k} {len(v)}" for k, v in tables.items())


def evidence_matrix(src):
    out = ["| scenario | entry points | PostgreSQL | in-flight (model fate = observed) | model verdict | runtime evidence | runs followed / matched | inconclusive | differing SQL | rows after kill | final rows | client ack | invariant (runtime) |",
           "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for f in sorted(glob.glob(os.path.join(src, "*.json"))):
        r = json.load(open(f))
        sv = r.get("sourceVerification") or {}
        runs = sv.get("runs", [])
        followed = sum(x["coordination"] == "followed" for x in runs)
        matched = sum(bool(x.get("reproduced") or x.get("recoveryReproduced")) for x in runs)
        inconclusive = sum(x["coordination"] != "followed" for x in runs)
        differing = sum(s["status"] != "agree" for s in sv.get("statements", [])) + len(sv.get("outsideModel", []))
        crashes = (sv.get("recovery") or {}).get("crashes", [])
        flights = [c["inFlight"] for c in crashes if c.get("inFlight")]
        fate = "; ".join(f"{x['operation']}: {x['modelFate']} = {x['observedFate']}" for x in flights)
        if not fate:
            fate = f"stuck: {sv['stuck']['modelKind']}" if sv.get("stuck") else "—"
        ack = "; ".join("acknowledged" if x["clientAcknowledged"] else "not acknowledged" for x in flights) or "—"
        invariant = ", ".join(sorted({str(x.get("invariant")) for x in runs})) or "not run"
        entries = ", ".join(sorted({a["function"].split("#")[-1] for a in r["actors"]}))
        pg = (sv.get("postgresVersion") or "—").replace("postgres (PostgreSQL) ", "")
        out.append(f"| {os.path.basename(f)[:-5]} | {entries} | {pg} | {fate} | {r['exploration']['verdict']} | "
                   f"{sv.get('evidence') or sv.get('status') or r['evidence']['level']} | {followed}/{len(runs)} / {matched}/{len(runs)} | "
                   f"{inconclusive} | {differing} | {'; '.join(rows(c.get('runtimeTables')) for c in crashes) or '—'} | "
                   f"{rows(sv.get('runtimeTables'))} | {ack} | {invariant} |")
    return out


def reduction_modes():
    out = ["| scenario | extra | mode | verdict | complete | states | transitions | counterexample step kinds |",
           "| --- | --- | --- | --- | --- | --- | --- | --- |"]
    cases = [(p, []) for p in sorted(glob.glob(os.path.join(ROOT, "fixtures/phase9/model-scenarios/*.json")))]
    cases += [(os.path.join(ROOT, f"fixtures/phase8/scenarios/{n}.json"), ["--max-in-flight", "1"])
              for n in ("shipping-outbox-no-loss", "shipping-outbox-relay-duplicate", "job-claimed-effect-redelivery",
                        "checkout-idempotent-crash-after-charge", "checkout-crash-after-charge")]
    cases += [(p, []) for p in sorted(glob.glob(os.path.join(ROOT, "fixtures/phase9/scenarios/*.json")))]
    for path, extra in cases:
        for mode, flags in (("full", ["--no-por"]), ("sleep sets", []), ("DPOR", ["--dpor"])):
            p = subprocess.run([BIN, "model-check", path, "--format", "json", "--no-timing", *flags, *extra],
                               capture_output=True, text=True)
            r = json.loads(p.stdout)
            x = r["exploration"]
            ce = r.get("counterexample")
            kinds = " ".join(s["kind"] for s in ce["steps"]) if ce else "—"
            out.append(f"| {os.path.basename(path)[:-5]} | {' '.join(extra) or '—'} | {mode} | {x['verdict']} | "
                       f"{x['complete']} | {x['metrics']['states']} | {x['metrics']['transitions']} | {kinds} |")
    return out


def main():
    src, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    open(os.path.join(out, "evidence-matrix.md"), "w").write("\n".join(evidence_matrix(src)) + "\n")
    open(os.path.join(out, "reduction-modes.md"), "w").write("\n".join(reduction_modes()) + "\n")


if __name__ == "__main__":
    main()
