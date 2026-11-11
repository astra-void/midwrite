#!/usr/bin/env python3
"""Precision/recall evaluation of `midwrite check` on real projects, split into
a dev set and a held-out set (Linear DEV-147).

The sets are declared in corpora/eval/sets.json:

* dev       rules may be tuned against these projects.
* held-out  never tuned against. Their labels, configuration and pinned
            revision are frozen in corpora/eval/FROZEN.json, and `run`
            refuses to score a held-out project whose frozen files changed.
            A held-out project used to drive a fix moves to the dev set
            (`sets.json` `history`) and a new held-out project replaces it.

Usage:
  scripts/eval.py fetch [--set dev|held-out|all] [--project NAME ...]
  scripts/eval.py run [--set dev|held-out|all] [--project NAME ...]
                      [--bin PATH] [-o OUT.json] [--gate]
  scripts/eval.py freeze --project NAME [--reason TEXT]

Labels (corpora/eval/labels/<project>.json, or a legacy list of finding
labels such as corpora/labels/midday.json) classify reported findings with
the Phase 2 classes:

  true-positive    the window exists and nothing in the code handles it
  intended-risk    the window exists but the code accepts it on purpose
  false-positive   the claimed partial state is not real
  unclear          could not be decided from the source

and list `knownPositives`: unhandled windows found by an audit of the code
that did not look at the tool's output (recall denominator).

Scores, always with counts:

  hazard precision  (TP + intended) / (TP + intended + FP); the reported
                    window exists. This is the stage-1 gate (>= 0.9 on the
                    held-out set).
  bug precision     TP / (TP + intended + FP); the window is an unhandled bug.
  recall            known positives (audit) plus TP findings that match no
                    known positive; a positive is found when a reported
                    hazard of the same file lies inside its function and is
                    not labeled false-positive. "n/a" when a project has no
                    audited known positives (TP findings alone would make
                    recall 1 by construction). The hazard must also have the
                    code of the positive's pattern (A: commit then effect,
                    MW1001; B: effect then commit, MW1002; C: effect inside a
                    transaction, MW1003; D: independent commits, MW1004;
                    E: independent effects, MW1005).

`unclear` findings are excluded from both precision denominators and are
reported. Unlabeled findings are reported; `--gate` fails while any
held-out finding is unlabeled. MW9001 (analysis limitation) is not a hazard
claim and is counted separately. Nothing in the analyzed projects is
executed: `check` is static analysis.
"""
import argparse
import datetime
import hashlib
import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SETS = os.path.join(ROOT, "corpora", "eval", "sets.json")
FROZEN = os.path.join(ROOT, "corpora", "eval", "FROZEN.json")
DEFAULT_BIN = os.environ.get("MIDWRITE_BIN", os.path.join(ROOT, "target", "release", "midwrite"))
HAZARD_CODES = {"MW1001", "MW1002", "MW1003", "MW1004", "MW1005"}
CLASSES = ("true-positive", "intended-risk", "false-positive", "unclear")
GATE = 0.9
# A known positive's pattern and the diagnostic that reports that window.
PATTERN_CODES = {"A": "MW1001", "B": "MW1002", "C": "MW1003", "D": "MW1004", "E": "MW1005"}


def canonical_code(code):
    return "MW" + code[2:] if code[:2].upper() == "CP" else code


def norm_path(path):
    path = path.replace("\\", "/")
    while path.startswith("./"):
        path = path[2:]
    return path


def sha256(path):
    with open(path, "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()


def rel(path):
    return os.path.relpath(path, ROOT)


def load_sets():
    with open(SETS) as f:
        sets = json.load(f)
    for p in sets["projects"]:
        p["checkout"] = os.path.join(ROOT, p["path"])
    return sets


def selected(sets, names, which):
    out = []
    for p in sets["projects"]:
        if names and p["name"] not in names:
            continue
        if which != "all" and p["set"] != which:
            continue
        out.append(p)
    unknown = set(names or []) - {p["name"] for p in sets["projects"]}
    if unknown:
        sys.exit(f"error: unknown project(s): {', '.join(sorted(unknown))}")
    return out


# ---------------------------------------------------------------------------
# fetch
# ---------------------------------------------------------------------------

def fetch(project):
    dest = project["checkout"]
    if not os.path.isdir(os.path.join(dest, ".git")):
        subprocess.run(["git", "init", "-q", dest], check=True)
        subprocess.run(["git", "-C", dest, "remote", "add", "origin", project["repository"]], check=True)
    print(f"fetching {project['name']} @ {project['revision'][:12]}")
    subprocess.run(["git", "-C", dest, "fetch", "-q", "--depth", "1", "origin", project["revision"]], check=True)
    subprocess.run(["git", "-C", dest, "checkout", "-q", "--detach", "FETCH_HEAD"], check=True)
    head(project)


def head(project):
    dest = project["checkout"]
    try:
        actual = subprocess.run(["git", "-C", dest, "rev-parse", "HEAD"], capture_output=True,
                                text=True, check=True).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        sys.exit(f"error: {project['name']}: no checkout at {rel(dest)}; run `scripts/eval.py fetch`")
    if actual != project["revision"]:
        sys.exit(f"error: {project['name']} is at {actual}, expected {project['revision']}")
    dirty = subprocess.run(["git", "-C", dest, "status", "--porcelain", "--untracked-files=no"],
                           capture_output=True, text=True, check=True).stdout.strip()
    if dirty:
        sys.exit(f"error: {project['name']}: the checkout has local modifications")


# ---------------------------------------------------------------------------
# freeze
# ---------------------------------------------------------------------------

def frozen_files(project):
    return {key: project[key] for key in ("config", "labels")}


def load_frozen():
    if not os.path.exists(FROZEN):
        return {"schemaVersion": 1, "kind": "midwrite.eval-frozen", "projects": {}}
    with open(FROZEN) as f:
        return json.load(f)


def freeze(project, reason):
    if project["set"] != "held-out":
        sys.exit(f"error: {project['name']} is in the {project['set']} set; only held-out projects are frozen")
    frozen = load_frozen()
    previous = frozen["projects"].get(project["name"])
    entry = {
        "revision": project["revision"],
        "files": {path: sha256(os.path.join(ROOT, path)) for path in frozen_files(project).values()},
        "frozenAtUtc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "history": (previous or {}).get("history", []),
    }
    if previous:
        if not reason:
            sys.exit(f"error: {project['name']} is already frozen; refreezing needs --reason "
                     "(a label correction, never a change made to raise a score)")
        entry["history"].append({"previous": {k: previous[k] for k in ("revision", "files", "frozenAtUtc")},
                                 "reason": reason})
    frozen["projects"][project["name"]] = entry
    with open(FROZEN, "w") as f:
        json.dump(frozen, f, indent=2)
        f.write("\n")
    print(f"froze {project['name']}: {', '.join(entry['files'])}")


def verify_frozen(project):
    entry = load_frozen()["projects"].get(project["name"])
    if entry is None:
        return f"{project['name']} is held-out but not frozen (run `scripts/eval.py freeze --project {project['name']}`)"
    if entry["revision"] != project["revision"]:
        return f"{project['name']}: revision differs from the frozen one"
    for path, digest in entry["files"].items():
        full = os.path.join(ROOT, path)
        if not os.path.exists(full) or sha256(full) != digest:
            return f"{project['name']}: {path} changed since it was frozen"
    if set(entry["files"]) != set(frozen_files(project).values()):
        return f"{project['name']}: sets.json names other files than the frozen ones"
    return None


# ---------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------

def analyze(project, binary):
    config = os.path.join(ROOT, project["config"])
    cmd = [binary, "check", ".", "--config", config, "--format", "json", "--fail-on", "never"]
    done = subprocess.run(cmd, cwd=project["checkout"], capture_output=True, text=True)
    # 3: some files could not be analyzed; the report lists them (`failures`)
    # and they are counted in the score as `filesFailed`.
    if done.returncode not in (0, 3):
        sys.exit(f"error: {project['name']}: `{' '.join(cmd)}` exited {done.returncode}:\n{done.stderr}")
    return json.loads(done.stdout)


def load_labels(project):
    path = os.path.join(ROOT, project["labels"])
    if not os.path.exists(path):
        return {"findings": [], "knownPositives": None}
    with open(path) as f:
        data = json.load(f)
    if isinstance(data, list):  # legacy corpora/labels/<project>.json
        findings = [{"findingId": x["findingId"], "classification": x["classification"]}
                    for x in data if x.get("reviewed", True)]
        return {"findings": findings, "knownPositives": None}
    return {"findings": data.get("findings", []), "knownPositives": data.get("knownPositives")}


def score_project(project, report, labels):
    by_id = {x["findingId"]: x for x in labels["findings"]}
    hazards, limitations = [], 0
    for d in report["diagnostics"]:
        code = canonical_code(d["code"])
        if code not in HAZARD_CODES:
            limitations += 1
            continue
        label = by_id.get(d["fingerprint"])
        hazards.append({
            "findingId": d["fingerprint"],
            "code": code,
            "severity": d["severity"],
            "file": norm_path(d["location"]["file"]),
            "line": d["location"]["line"],
            "function": d["function"],
            "classification": label["classification"] if label else None,
        })
    counts = {c: 0 for c in CLASSES}
    unlabeled = []
    for h in hazards:
        if h["classification"] in counts:
            counts[h["classification"]] += 1
        else:
            unlabeled.append(h)
    reported_ids = {h["findingId"] for h in hazards}
    stale = sorted(set(by_id) - reported_ids)

    recall = None
    kps = labels["knownPositives"]
    if kps is not None:
        found_kps, matched_tp = [], set()
        for kp in kps:
            file = norm_path(kp["file"])
            code = PATTERN_CODES.get(kp.get("pattern"))
            hits = [h for h in hazards
                    if h["file"] == file
                    and kp["functionStartLine"] <= h["line"] <= kp["functionEndLine"]
                    and (code is None or h["code"] == code)
                    and h["classification"] != "false-positive"]
            if hits:
                found_kps.append(kp["id"])
                matched_tp.update(h["findingId"] for h in hits if h["classification"] == "true-positive")
        extra_tp = [h for h in hazards if h["classification"] == "true-positive"
                    and h["findingId"] not in matched_tp]
        recall = {
            "found": len(found_kps) + len(extra_tp),
            "positives": len(kps) + len(extra_tp),
            "knownPositivesFound": found_kps,
            "knownPositivesMissed": [kp["id"] for kp in kps if kp["id"] not in found_kps],
            "truePositiveFindingsOutsideAudit": len(extra_tp),
        }
    return {
        "hazardFindings": len(hazards),
        "bySeverity": {s: sum(1 for h in hazards if h["severity"] == s) for s in ("error", "warning", "info")},
        "analysisLimitations": limitations,
        "filesAnalyzed": report["summary"]["filesAnalyzed"],
        "filesFailed": report["summary"]["filesFailed"],
        "labels": counts,
        "unlabeled": len(unlabeled),
        "staleLabels": len(stale),
        "recall": recall,
        "findings": hazards,
    }


def ratio(num, den):
    return {"value": (num / den) if den else None, "num": num, "den": den}


def fmt(r):
    if r is None or r["den"] == 0:
        return "n/a"
    return f"{r['value']:.2f} ({r['num']}/{r['den']})"


def aggregate(scores):
    labels = {c: sum(s["labels"][c] for s in scores) for c in CLASSES}
    decided = labels["true-positive"] + labels["intended-risk"] + labels["false-positive"]
    recalls = [s["recall"] for s in scores if s["recall"] is not None]
    return {
        "hazardFindings": sum(s["hazardFindings"] for s in scores),
        "labels": labels,
        "unlabeled": sum(s["unlabeled"] for s in scores),
        "hazardPrecision": ratio(labels["true-positive"] + labels["intended-risk"], decided),
        "bugPrecision": ratio(labels["true-positive"], decided),
        "recall": ratio(sum(r["found"] for r in recalls), sum(r["positives"] for r in recalls)) if recalls else None,
        "projectsWithoutAudit": [s["name"] for s in scores if s["recall"] is None],
    }


def run(args):
    sets = load_sets()
    projects = selected(sets, args.project, args.set)
    if not os.path.exists(args.bin):
        sys.exit(f"error: no binary at {args.bin} (cargo build --release, or --bin / MIDWRITE_BIN)")
    out = {"schemaVersion": 1, "kind": "midwrite.eval-report",
           "binary": {"path": rel(args.bin) if args.bin.startswith(ROOT) else args.bin,
                      "sha256": sha256(args.bin)},
           "runAtUtc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
           "projects": [], "sets": {}}
    problems = []
    for p in projects:
        head(p)
        if p["set"] == "held-out":
            problem = verify_frozen(p)
            if problem:
                problems.append(problem)
                print(f"warning: {problem}; scored, but not valid for the gate", file=sys.stderr)
        report = analyze(p, args.bin)
        s = score_project(p, report, load_labels(p))
        s.update(name=p["name"], set=p["set"], revision=p["revision"])
        decided = s["labels"]["true-positive"] + s["labels"]["intended-risk"] + s["labels"]["false-positive"]
        s["hazardPrecision"] = ratio(s["labels"]["true-positive"] + s["labels"]["intended-risk"], decided)
        s["bugPrecision"] = ratio(s["labels"]["true-positive"], decided)
        s["recallRatio"] = ratio(s["recall"]["found"], s["recall"]["positives"]) if s["recall"] else None
        out["projects"].append(s)
    for which in ("dev", "held-out"):
        scores = [s for s in out["projects"] if s["set"] == which]
        if scores:
            out["sets"][which] = aggregate(scores)

    print(f"{'project':<12} {'set':<9} {'hazards':>7} {'unlab.':>6} {'unclear':>7}  "
          f"{'hazard precision':<20} {'bug precision':<20} recall")
    for s in out["projects"]:
        print(f"{s['name']:<12} {s['set']:<9} {s['hazardFindings']:>7} {s['unlabeled']:>6} "
              f"{s['labels']['unclear']:>7}  {fmt(s['hazardPrecision']):<20} {fmt(s['bugPrecision']):<20} "
              f"{fmt(s['recallRatio'])}")
    print()
    for which, a in out["sets"].items():
        line = (f"{which:<9} precision {fmt(a['hazardPrecision'])}, recall {fmt(a['recall'])}"
                f"  [bug precision {fmt(a['bugPrecision'])}; {a['hazardFindings']} hazards, "
                f"{a['unlabeled']} unlabeled, {a['labels']['unclear']} unclear]")
        if a["projectsWithoutAudit"]:
            line += f"  (recall excludes {', '.join(a['projectsWithoutAudit'])}: no audited positives)"
        print(line)

    gate = None
    if "held-out" in out["sets"]:
        a = out["sets"]["held-out"]
        reasons = list(problems)
        if a["unlabeled"]:
            reasons.append(f"{a['unlabeled']} held-out finding(s) unlabeled")
        hp = a["hazardPrecision"]
        if hp["value"] is None:
            reasons.append("no decided held-out labels")
        elif hp["value"] < GATE:
            reasons.append(f"held-out precision {fmt(hp)} < {GATE}")
        gate = {"threshold": GATE, "metric": "held-out hazard precision", "passed": not reasons,
                "reasons": reasons}
        print(f"\nstage-1 gate (held-out precision >= {GATE}): "
              + ("PASS" if gate["passed"] else "FAIL: " + "; ".join(reasons)))
    out["gate"] = gate
    if args.output:
        with open(args.output, "w") as f:
            json.dump(out, f, indent=2)
            f.write("\n")
    if args.gate and not (gate and gate["passed"]):
        return 1
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    f = sub.add_parser("fetch")
    f.add_argument("--set", choices=["dev", "held-out", "all"], default="all")
    f.add_argument("--project", action="append")
    r = sub.add_parser("run")
    r.add_argument("--set", choices=["dev", "held-out", "all"], default="all")
    r.add_argument("--project", action="append")
    r.add_argument("--bin", default=DEFAULT_BIN)
    r.add_argument("-o", "--output")
    r.add_argument("--gate", action="store_true", help="exit 1 unless the stage-1 gate passes")
    z = sub.add_parser("freeze")
    z.add_argument("--project", action="append", required=True)
    z.add_argument("--reason")
    args = parser.parse_args()
    sets = load_sets()
    if args.command == "fetch":
        for p in selected(sets, args.project, args.set):
            fetch(p)
        return 0
    if args.command == "freeze":
        for p in selected(sets, args.project, "all"):
            freeze(p, args.reason)
        return 0
    args.bin = os.path.abspath(args.bin)
    return run(args)


if __name__ == "__main__":
    sys.exit(main())
