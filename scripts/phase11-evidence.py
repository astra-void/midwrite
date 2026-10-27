#!/usr/bin/env python3
"""Phase 11 evidence: discovery on the labeled first-party application, the
held-out Phase 7/8 applications and the corpus; original-code runs of the
generated counterexamples; timings and peak RSS.

Usage:
  scripts/phase11-evidence.py [--runs N] [--repeat N] [--private DIR ...]

Writes reports/phase11/evidence/. Corpus projects: .corpora/saas-starter and
.corpora/midday (public, pinned), plus every --private DIR (a pinned archive
of a private project): only aggregate counts of private projects are
written. Application code runs ONLY for the first-party commerce application
(its generated scenarios, `--verify-source --allow-execution`); no corpus
project is executed.
"""
import argparse
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "release", "commitrift")
OUT = os.path.join(ROOT, "reports", "phase11", "evidence")
APP = os.path.join(ROOT, "fixtures", "phase11", "commerce-app")


def rel(p):
    return os.path.relpath(p, ROOT)


def timed(cmd, cwd=ROOT):
    """Run with /usr/bin/time; return (exit, seconds, peak RSS bytes, stdout)."""
    t = ["/usr/bin/time", "-l"] if sys.platform == "darwin" else ["/usr/bin/time", "-v"]
    start = time.monotonic()
    p = subprocess.run(t + cmd, cwd=cwd, capture_output=True, text=True)
    secs = time.monotonic() - start
    rss = None
    m = re.search(r"(\d+)\s+maximum resident set size", p.stderr)
    if m:
        rss = int(m.group(1))
    else:
        m = re.search(r"Maximum resident set size \(kbytes\): (\d+)", p.stderr)
        if m:
            rss = int(m.group(1)) * 1024
    return p.returncode, secs, rss, p.stdout


def discover(root, *extra, out=None):
    cmd = [BIN, "discover", root, "--format", "json", *extra]
    code, secs, rss, stdout = timed(cmd)
    report = json.loads(stdout)
    if out:
        with open(out, "w") as f:
            json.dump(report, f, indent=1, sort_keys=True)
            f.write("\n")
    return code, secs, rss, report


def mib(b):
    return "n/a" if b is None else f"{b / 1048576:.1f} MiB"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=3, help="timing repetitions")
    ap.add_argument("--repeat", type=int, default=3, help="original-code runs per scenario")
    ap.add_argument("--private", action="append", default=[], help="private project archive")
    args = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    summary = ["# Phase 11 evidence", ""]
    version = subprocess.run([BIN, "--version"], capture_output=True, text=True).stdout.strip()
    sha = subprocess.run(["shasum", "-a", "256", BIN], capture_output=True, text=True).stdout.split()[0]
    summary += [f"Binary: `{version}`, sha256 `{sha[:16]}…`.", ""]

    # 1. Labeled first-party application.
    scen = os.path.join(OUT, "scenarios", "commerce-app")
    shutil.rmtree(scen, ignore_errors=True)
    code, secs, rss, r = discover(
        APP, "--model-check", "--no-timing", "--labels", os.path.join(ROOT, "fixtures/phase11/labels.json"),
        "--emit-scenarios", scen, out=os.path.join(OUT, "commerce-app.json"),
    )
    subprocess.run([BIN, "discover", APP, "--model-check", "--no-timing", "-v", "-o", os.path.join(OUT, "commerce-app.txt")])
    subprocess.run([BIN, "discover", APP, "--model-check", "--no-timing", "--format", "html", "-o", os.path.join(OUT, "commerce-app.html")])
    e = r["evaluation"]
    summary += ["## First-party benchmark (fixtures/phase11/labels.json)", ""]
    summary += ["| entry | label | outcome | own candidates |", "| --- | --- | --- | --- |"]
    for c in e["cases"]:
        summary.append(f"| `{c['entry']}` | {c['label']} | {c['outcome']} | {', '.join(c['candidates']) or '—'} |")
    for k in ("staticHazardPrediction", "modelPrediction"):
        m = e[k]
        summary.append(f"\n{k}: TP {m['truePositives']}, FP {m['falsePositives']}, FN {m['falseNegatives']}, TN {m['trueNegatives']}, precision {m.get('precision')}, recall {m.get('recall')}, F1 {m.get('f1')}")
    summary += [f"\nclass agreement: {e['classAgreement']}; unsupported reported {e['unsupportedReported']}, missed {e['unsupportedMissed']}; candidate-to-model {e['candidateToModel']}", ""]
    summary += [f"Summary: `{json.dumps(r['summary'], sort_keys=True)}`", ""]

    # 2. Held-out labels (Phase 7/8 applications).
    held = json.load(open(os.path.join(ROOT, "fixtures/phase11/heldout/labels.json")))
    summary += ["## Held-out labels (Phase 7/8 applications)", ""]
    for name, p in held["projects"].items():
        lab = os.path.join(OUT, f"{name}-labels.json")
        with open(lab, "w") as f:
            json.dump({"cases": p["cases"]}, f)
        _, _, _, hr = discover(
            os.path.join(ROOT, p["root"]), "--contracts", os.path.join(ROOT, p["contracts"]),
            "--model-check", "--no-timing", "--labels", lab, out=os.path.join(OUT, f"heldout-{name}.json"),
        )
        os.remove(lab)
        he = hr["evaluation"]
        summary += [f"### {name}", "", "| entry | label | outcome | own candidates |", "| --- | --- | --- | --- |"]
        for c in he["cases"]:
            summary.append(f"| `{c['entry']}` | {c['label']} | {c['outcome']} | {', '.join(c['candidates']) or '—'} |")
        m = he["modelPrediction"]
        summary += [f"\nmodel prediction: TP {m['truePositives']}, FP {m['falsePositives']}, FN {m['falseNegatives']}, TN {m['trueNegatives']}, precision {m.get('precision')}, recall {m.get('recall')}", ""]

    # 3. Without contracts.
    tmp = os.path.join(OUT, "no-contracts-copy")
    shutil.rmtree(tmp, ignore_errors=True)
    shutil.copytree(APP, tmp, ignore=shutil.ignore_patterns("node_modules", "dist"))
    os.remove(os.path.join(tmp, "commitrift-contracts.json"))
    _, _, _, nr = discover(tmp, "--model-check", "--no-timing")
    shutil.rmtree(tmp)
    summary += ["## The same application without commitrift-contracts.json", "",
                f"`{json.dumps(nr['summary'], sort_keys=True)}`", ""]

    # 4. Original-code runs of the generated counterexamples (first party only).
    summary += ["## Generated counterexamples on the original code", "",
                f"Each scenario {args.repeat} times, `commitrift model-check <file> --verify-source --allow-execution --source-repeat {args.repeat}`.", "",
                "| scenario | candidate | model | evidence | runs reproduced |", "| --- | --- | --- | --- | --- |"]
    runtime = []
    for c in r["candidates"]:
        for s in c["scenarios"]:
            if s["variant"]["kind"] != "actual" or not s.get("sourceVerification") or not s.get("file"):
                continue
            m = s.get("model") or {}
            if m.get("verdict") != "COUNTEREXAMPLE_FOUND":
                continue
            path = os.path.join(scen, s["file"])
            p = subprocess.run([BIN, "model-check", path, "--verify-source", "--allow-execution", "--source-repeat", str(args.repeat), "--format", "json", "--no-timing"], capture_output=True, text=True)
            mc = json.loads(p.stdout)
            with open(os.path.join(OUT, "runtime-" + s["file"]), "w") as f:
                json.dump(mc, f, indent=1, sort_keys=True)
            sv = mc.get("sourceVerification") or {}
            runs = sv.get("runs", [])
            ok = sum(1 for x in runs if x.get("reproduced"))
            level = mc["evidence"]["level"]
            runtime.append((s["file"], level, ok, len(runs)))
            summary.append(f"| `{s['file']}` | {c['id']} ({c['class']}) | {m['verdict']} | {level} | {ok}/{len(runs)} |")
    summary.append("")

    # 5. Corpus (static discovery and bounded model checking; no execution).
    summary += ["## Corpus", "", "| project | modules | entry candidates | extracted | effectful | unknown ops | tables modeled / refused | candidates | counterexamples | wall | peak RSS |", "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    corpus = [("saas-starter", os.path.join(ROOT, ".corpora/saas-starter"), True),
              ("midday", os.path.join(ROOT, ".corpora/midday"), True)]
    corpus += [(os.path.basename(p.rstrip("/")), p, False) for p in args.private]
    breakdowns = {}
    for name, path, public in corpus:
        if not os.path.isdir(path):
            summary.append(f"| {name} | not available | | | | | | | | | |")
            continue
        code, secs, rss, cr = discover(path, "--model-check")
        i, s = cr["index"], cr["summary"]
        breakdowns[name] = s["unknownOperationBreakdown"]
        agg = {"project": name, "public": public, "index": {k: v for k, v in i.items() if k != "tablesRefused"},
               "tablesRefused": len(i["tablesRefused"]), "summary": s, "timings": cr.get("timings"),
               "wallSeconds": round(secs, 2), "peakRss": rss}
        if public:
            agg["candidates"] = [{"id": c["id"], "class": c["class"], "status": c["status"], "title": c["title"],
                                  "entries": [x["selector"] for x in c["entries"]],
                                  "origins": sorted({p["origin"] for p in c["properties"]})} for c in cr["candidates"]]
            agg["tablesRefusedReasons"] = sorted({re.sub(r"`[^`]*`", "X", t[1]) for t in i["tablesRefused"]})
        with open(os.path.join(OUT, f"corpus-{name}.json"), "w") as f:
            json.dump(agg, f, indent=1, sort_keys=True)
        summary.append(f"| {name} | {cr['project']['modules']} | {i['entryCandidates']} | {i['entriesExtracted']} | {i['effectfulEntries']} | {i['unknownOperations']} | {i['tablesModeled']} / {len(i['tablesRefused'])} | {s['candidates']} | {s['modelCounterexamples']} | {secs:.2f} s | {mib(rss)} |")
    summary += ["", "Unknown operations by category:", ""]
    cats = sorted({k for b in breakdowns.values() for k in b})
    summary.append("| category | " + " | ".join(breakdowns) + " |")
    summary.append("| --- |" + " --- |" * len(breakdowns))
    for k in cats:
        summary.append(f"| {k} | " + " | ".join(str(b.get(k, 0)) for b in breakdowns.values()) + " |")
    summary.append("")

    # 6. Timings.
    def median_run(cmd):
        xs, rs = [], []
        for _ in range(args.runs):
            _, secs, rss, _ = timed(cmd)
            xs.append(secs)
            rs.append(rss or 0)
        return statistics.median(xs), max(rs)

    summary += ["## Cost centers and timings", "", f"Median of {args.runs} runs; peak RSS is the maximum.", "",
                "| what | wall | peak RSS |", "| --- | --- | --- |"]
    rows = [
        ("commerce-app: discovery only", [BIN, "discover", APP, "--format", "json"]),
        ("commerce-app: discovery + model checking", [BIN, "discover", APP, "--format", "json", "--model-check"]),
        ("commerce-app: discovery + model checking, source-DPOR", [BIN, "discover", APP, "--format", "json", "--model-check", "--source-dpor"]),
        ("commerce-app: commitrift check (static analysis)", [BIN, "check", APP, "--format", "json"]),
    ]
    old = os.path.join(ROOT, "reports/phase11/baseline/commitrift-phase10")
    if os.path.exists(old):
        rows.append(("commerce-app: commitrift check, Phase 10 binary", [old, "check", APP, "--format", "json"]))
    midday = os.path.join(ROOT, ".corpora/midday")
    if os.path.isdir(midday):
        rows += [
            ("midday: commitrift check", [BIN, "check", midday, "--config", os.path.join(ROOT, "corpora/configs/midday.json"), "--format", "json"]),
        ]
        if os.path.exists(old):
            rows.append(("midday: commitrift check, Phase 10 binary", [old, "check", midday, "--config", os.path.join(ROOT, "corpora/configs/midday.json"), "--format", "json"]))
        rows.append(("midday: discovery only", [BIN, "discover", midday, "--format", "json"]))
    for label, cmd in rows:
        secs, rss = median_run(cmd)
        summary.append(f"| {label} | {secs:.2f} s | {mib(rss)} |")
    one = discover(APP, "--model-check")[3]["timings"]
    summary += ["", f"commerce-app phase timings (one run): `{json.dumps(one, sort_keys=True)}`", ""]
    # Cache: cold and warm.
    if os.path.isdir(midday):
        cache = os.path.join(OUT, "midday-cache.json")
        if os.path.exists(cache):
            os.remove(cache)
        _, cold, _, cr = discover(midday, "--cache", cache)
        _, warm, _, wr = discover(midday, "--cache", cache)
        os.remove(cache)
        summary += ["Extraction cache on midday (discovery only):", "",
                    f"* cold: {cold:.2f} s, {cr['index']['cacheMisses']} misses",
                    f"* warm: {warm:.2f} s, {wr['index']['cacheHits']} hits, {wr['index']['cacheMisses']} misses",
                    f"* reports equal apart from timings and cache counters: {strip(cr) == strip(wr)}", ""]
    with open(os.path.join(OUT, "SUMMARY.md"), "w") as f:
        f.write("\n".join(summary) + "\n")
    print("\n".join(summary))
    bad = [x for x in runtime if not x[1].startswith("original-code-") or x[2] != x[3]]
    return 1 if bad else 0


def strip(r):
    r = json.loads(json.dumps(r))
    r.pop("timings", None)
    r["index"].pop("cacheHits", None)
    r["index"].pop("cacheMisses", None)
    return r


if __name__ == "__main__":
    sys.exit(main())
