#!/usr/bin/env python3
"""Independent human review of the evaluation labels and upstream reports (DEV-162).

Every label in corpora/eval/labels/ and every upstream report was written by
an AI agent. This script prepares a fixed sample for a person to review, checks
the person's review record, summarizes it, and applies the adjudicated result.
It never produces a verdict itself.

    scripts/human-review.py sample            write corpora/review/sample.json
    scripts/human-review.py packet [--reveal] write a reading packet under reports/review/packet/
    scripts/human-review.py template --reviewer NAME
                                              write an empty record under corpora/review/records/
    scripts/human-review.py check RECORD [--complete]
    scripts/human-review.py summarize RECORD... [-o OUT]
    scripts/human-review.py adjudicate RECORD... [--resolutions FILE] [--apply]
    scripts/human-review.py upstream-status   refresh corpora/review/upstream-status.json (gh, read-only)

The protocol is docs/development/human-review.md.
"""

import argparse
import datetime
import hashlib
import json
import math
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
SETS = ROOT / "corpora/eval/sets.json"
UPSTREAM = ROOT / "corpora/upstream/manifest.json"
REVIEW = ROOT / "corpora/review"
SAMPLE = REVIEW / "sample.json"
UPSTREAM_STATUS = REVIEW / "upstream-status.json"
PACKET = ROOT / "reports/review/packet"

SALT = "DEV-162"
# Tier 2 takes this share of each dev project's true positives and intended
# risks (at least MIN_PER_PROJECT, or all when fewer), and KNOWN_PER_PROJECT
# known positives per dev project.
SHARE = 0.10
MIN_PER_PROJECT = 2
KNOWN_PER_PROJECT = 3

HAZARD = ("real", "not-real", "unclear")
ACTIONABLE = ("unhandled-bug", "accepted-or-mitigated", "not-applicable", "unclear")
CLASSES = ("true-positive", "intended-risk", "false-positive", "unclear")


def load(path):
    return json.loads(Path(path).read_text())


def dump(path, data):
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    Path(path).write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n")


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def rank(project, ident):
    """Order independent of label content, so the sample cannot be steered."""
    return hashlib.sha256(f"{SALT}/{project}/{ident}".encode()).hexdigest()


def head_revision():
    out = subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True)
    return out.stdout.strip() or None


# ---------------------------------------------------------------- sampling

_LOCATIONS = {}


def report_location(project, finding_id):
    """Location of a label that carries none (old midday labels), from the newest eval report."""
    if not _LOCATIONS:
        for path in sorted((ROOT / "reports/eval").glob("*.json"), reverse=True):
            report = load(path)
            for p in report.get("projects") if isinstance(report.get("projects"), list) else []:
                if not isinstance(p, dict):
                    continue
                for h in p.get("findings", []):
                    if h.get("file"):
                        _LOCATIONS.setdefault((p.get("name"), h["findingId"]), {
                            "code": h.get("code"), "file": h["file"], "line": h.get("line"),
                            "function": h.get("function"),
                            "locationFrom": str(path.relative_to(ROOT))})
    return _LOCATIONS.get((project, finding_id))


def finding_item(project, labels, finding, stratum, tier, relabeled):
    if "file" not in finding:
        located = report_location(project["name"], finding["findingId"])
        if located is None:
            sys.exit(f"error: no location for {project['name']} finding {finding['findingId']}")
        finding = {**located, **finding}
    return {
        "id": f"{project['name']}:finding:{finding['findingId']}",
        "kind": "finding",
        "stratum": stratum,
        "tier": tier,
        "project": project["name"],
        "set": project["set"],
        "revision": labels["revision"],
        "code": finding["code"],
        "file": finding["file"],
        "line": finding["line"],
        "function": finding["function"],
        "relabeledAfterTuning": relabeled,
        "ai": {"classification": finding["classification"], "notes": finding.get("notes", "")},
    }


def known_item(project, labels, known, stratum, tier, removed=False):
    return {
        "id": f"{project['name']}:known-positive:{known['id']}",
        "kind": "known-positive",
        "stratum": stratum,
        "tier": tier,
        "project": project["name"],
        "set": project["set"],
        "revision": labels["revision"],
        "file": known["file"],
        "line": known.get("functionStartLine"),
        "endLine": known.get("functionEndLine"),
        "function": known.get("function"),
        "ai": {
            "classification": "removed" if removed else "valid",
            "pattern": known.get("pattern"),
            "firstEffect": known.get("firstEffect"),
            "secondEffect": known.get("secondEffect"),
            "consequence": known.get("consequence"),
            "confidence": known.get("confidence"),
        },
    }


def take(pool, project, count):
    return sorted(pool, key=lambda pair: rank(project, pair[0]))[:count]


def build_sample():
    sets = load(SETS)
    items, taken = [], set()

    def add(item):
        if item["id"] not in taken:
            taken.add(item["id"])
            items.append(item)

    for project in sets["projects"]:
        labels = load(ROOT / project["labels"])
        name, heldout = project["name"], project["set"] == "held-out"
        relabeled = {f for c in labels["corrections"] if c["type"] == "new-findings-reviewed"
                     for f in c["findings"]}
        corrected = {c["findingId"] for c in labels["corrections"]
                     if c["type"] == "finding-classification-correction"}
        corrected_known = {k for c in labels["corrections"] if c["type"] == "pattern-correction"
                           for k in c["knownPositives"]}
        findings = labels["findings"]

        def fitem(f, stratum, tier):
            return finding_item(project, labels, f, stratum, tier, f["findingId"] in relabeled)

        # Tier 1: everything a public number or an earlier correction rests on.
        for f in findings:
            if f["findingId"] in corrected:
                add(fitem(f, "corrected", 1))
        for k in labels["knownPositives"]:
            if k["id"] in corrected_known:
                add(known_item(project, labels, k, "corrected", 1))
        for c in labels["corrections"]:
            if c["type"] == "remove-invalid-known-positive":
                add(known_item(project, labels, c["previous"], "corrected", 1, removed=True))
        if heldout:
            for f in findings:
                add(fitem(f, "held-out-finding", 1))
            for k in labels["knownPositives"]:
                add(known_item(project, labels, k, "held-out-known-positive", 1))
            continue
        for f in findings:
            if f["classification"] == "false-positive":
                add(fitem(f, "dev-false-positive", 1))
            elif f["classification"] == "unclear":
                add(fitem(f, "dev-unclear", 1))

        # Tier 2: a fixed-rule sample of the dev positives.
        for cls, stratum in (("true-positive", "dev-true-positive"),
                             ("intended-risk", "dev-intended-risk")):
            pool = [(f["findingId"], f) for f in findings if f["classification"] == cls]
            count = min(len(pool), max(MIN_PER_PROJECT, math.ceil(len(pool) * SHARE)))
            for _, f in take(pool, name, count):
                add(fitem(f, stratum, 2))
        pool = [(k["id"], k) for k in labels["knownPositives"]]
        for _, k in take(pool, name, KNOWN_PER_PROJECT):
            add(known_item(project, labels, k, "dev-known-positive", 2))

    upstream = load(UPSTREAM)
    for project in upstream["projects"]:
        reviews = load(ROOT / project["reviews"])["reviews"]
        by_issue = {}
        for r in reviews:
            if r["classification"] == "reported":
                by_issue.setdefault(r["upstream"], []).append(r)
        for url, rs in sorted(by_issue.items()):
            issue = next(p for p in sorted((ROOT / "reports/upstream/issues").glob("*.json"))
                         if load(p).get("url") == url)
            meta = load(issue)
            add({
                "id": f"{project['name']}:upstream-report:{issue.stem}",
                "kind": "upstream-report",
                "stratum": "upstream-report",
                "tier": 1,
                "project": project["name"],
                "set": "upstream",
                "revision": project["revision"],
                "repository": project["repository"],
                "issue": url,
                "title": meta["title"],
                "body": meta["bodyFile"],
                "findings": [{"fingerprint": r["fingerprint"], "code": r["code"], "file": r["file"],
                              "line": r["line"], "function": r["function"],
                              "sourceUrl": r.get("sourceUrl")} for r in rs],
                "ai": {"classification": "reported", "reviewer": load(ROOT / project["reviews"])["reviewer"]},
            })

    strata = {}
    for item in items:
        key = item["stratum"]
        strata.setdefault(key, {"tier": item["tier"], "items": 0})
        strata[key]["items"] += 1
    return {
        "schemaVersion": 1,
        "kind": "midwrite.human-review-sample",
        "$comment": "Fixed sample for independent human review (DEV-162). Generated by "
                    "scripts/human-review.py sample; do not edit by hand. Items are ordered by "
                    f"sha256('{SALT}/<project>/<id>'), which does not depend on any label. Tier 1 "
                    "must be reviewed before any human-validated number is published; tier 2 is a "
                    "representative sample of the dev positives.",
        "rule": {"salt": SALT, "share": SHARE, "minPerProject": MIN_PER_PROJECT,
                 "knownPerProject": KNOWN_PER_PROJECT},
        "inputs": {str(p.relative_to(ROOT)): sha256_file(p) for p in sorted(
            [SETS, UPSTREAM]
            + [ROOT / p["labels"] for p in load(SETS)["projects"]]
            + [ROOT / p["reviews"] for p in upstream["projects"]])},
        "strata": strata,
        "items": items,
    }


def cmd_sample(args):
    sample = build_sample()
    dump(SAMPLE, sample)
    print(f"{len(sample['items'])} items -> {SAMPLE.relative_to(ROOT)}")
    for key, s in sample["strata"].items():
        print(f"  tier {s['tier']}  {key:<24} {s['items']}")


def current_sample():
    sample = load(SAMPLE)
    stale = [p for p, digest in sample["inputs"].items()
             if not (ROOT / p).exists() or sha256_file(ROOT / p) != digest]
    return sample, stale


# ------------------------------------------------------------------ packet

def corpus_root(item):
    if item["kind"] == "upstream-report":
        project = next(p for p in load(UPSTREAM)["projects"] if p["name"] == item["project"])
    else:
        project = next(p for p in load(SETS)["projects"] if p["name"] == item["project"])
    return ROOT / project["path"]


def excerpt(root, file, first, last):
    path = root / file
    if not path.is_file():
        return f"(source not found: {path.relative_to(ROOT)}; fetch the corpus at the pinned revision)"
    lines = path.read_text(errors="replace").split("\n")
    first, last = max(1, first), min(len(lines), last)
    return "\n".join(f"{n:5} {lines[n - 1]}" for n in range(first, last + 1))


def render(item, reveal):
    out = [f"## {item['id']}", "",
           f"- stratum: {item['stratum']} (tier {item['tier']})",
           f"- project: {item['project']} ({item['set']}) at `{item['revision']}`"]
    root = corpus_root(item)
    if item["kind"] == "finding":
        out.append(f"- {item['code']} at `{item['file']}:{item['line']}` in `{item['function']}`")
        out += ["", "```ts", excerpt(root, item["file"], item["line"] - 25, item["line"] + 10), "```"]
    elif item["kind"] == "known-positive":
        ai = item["ai"]
        out.append(f"- `{item['file']}:{item['line']}-{item['endLine']}` `{item['function']}`")
        out.append(f"- claimed first effect: {ai['firstEffect']}")
        out.append(f"- claimed second effect: {ai['secondEffect']}")
        out.append(f"- claimed consequence: {ai['consequence']}")
        last = min(item["endLine"] or item["line"] + 60, (item["line"] or 1) + 120)
        out += ["", "```ts", excerpt(root, item["file"], item["line"] or 1, last), "```"]
    else:
        out.append(f"- issue: {item['issue']} ({item['title']})")
        out.append(f"- report text: `{item['body']}`")
        for f in item["findings"]:
            out.append(f"- {f['code']} `{f['file']}:{f['line']}` in `{f['function']}` {f.get('sourceUrl') or ''}")
    if reveal:
        out += ["", "<details><summary>AI label</summary>", "",
                "```json", json.dumps(item["ai"], indent=2, ensure_ascii=False), "```", "</details>"]
    out.append("")
    return "\n".join(out)


def cmd_packet(args):
    sample, stale = current_sample()
    if stale:
        print("warning: labels changed since the sample was drawn: " + ", ".join(stale), file=sys.stderr)
    PACKET.mkdir(parents=True, exist_ok=True)
    suffix = "revealed" if args.reveal else "blind"
    for tier in (1, 2):
        items = [i for i in sample["items"] if i["tier"] == tier]
        header = [f"# DEV-162 review packet, tier {tier} ({suffix})", "",
                  f"{len(items)} items. Write each verdict into your record before opening the "
                  "revealed packet. Protocol: docs/development/human-review.md.", ""]
        path = PACKET / f"tier{tier}.{suffix}.md"
        path.write_text("\n".join(header + [render(i, args.reveal) for i in items]))
        print(path.relative_to(ROOT))


# ------------------------------------------------------------------ record

def cmd_template(args):
    sample, _ = current_sample()
    slug = "".join(c if c.isalnum() else "-" for c in args.reviewer.lower()).strip("-")
    path = REVIEW / "records" / f"{slug}.json"
    if path.exists():
        sys.exit(f"error: {path.relative_to(ROOT)} exists")
    dump(path, {
        "schemaVersion": 1,
        "kind": "midwrite.human-review-record",
        "reviewer": {"name": args.reviewer, "kind": "human",
                     "independence": "", "toolsUsed": ""},
        "attestation": False,
        "sampleSha256": sha256_file(SAMPLE),
        "midwriteRevision": head_revision(),
        "started": datetime.date.today().isoformat(),
        "completed": None,
        "items": [{"item": i["id"], "revision": i["revision"], "hazard": None, "actionable": None,
                   "blind": None, "rationale": "", "codeRefs": []} for i in sample["items"]],
    })
    print(path.relative_to(ROOT))


def validate(record, sample, complete=False):
    """Return a list of problems; an empty list means the record is usable."""
    problems = []
    reviewer = record.get("reviewer") or {}
    if reviewer.get("kind") != "human" or not (reviewer.get("name") or "").strip():
        problems.append("reviewer must name a person with kind 'human'")
    if not (reviewer.get("independence") or "").strip():
        problems.append("reviewer.independence must state the reviewer's relation to Midwrite and the labels")
    if record.get("attestation") is not True:
        problems.append("attestation must be true: the verdicts are the reviewer's own, not generated by a tool")
    by_id = {i["id"]: i for i in sample["items"]}
    seen = set()
    for entry in record.get("items", []):
        ident = entry.get("item")
        item = by_id.get(ident)
        if item is None:
            problems.append(f"{ident}: not in the sample")
            continue
        if ident in seen:
            problems.append(f"{ident}: reviewed twice")
        seen.add(ident)
        if entry.get("hazard") is None and entry.get("actionable") is None:
            continue  # not reviewed (yet)
        if entry.get("revision") != item["revision"]:
            problems.append(f"{ident}: reviewed revision {entry.get('revision')} is not the sampled {item['revision']}")
        if entry.get("hazard") not in HAZARD:
            problems.append(f"{ident}: hazard must be one of {HAZARD}")
        if entry.get("actionable") not in ACTIONABLE:
            problems.append(f"{ident}: actionable must be one of {ACTIONABLE}")
        if entry.get("hazard") == "real" and entry.get("actionable") == "not-applicable":
            problems.append(f"{ident}: a real hazard is either an unhandled bug, accepted/mitigated, or unclear")
        if entry.get("hazard") == "not-real" and entry.get("actionable") not in ("not-applicable", None):
            problems.append(f"{ident}: a hazard that is not real has actionable 'not-applicable'")
        if not isinstance(entry.get("blind"), bool):
            problems.append(f"{ident}: blind must say whether the verdict was written before reading the AI label")
        if not (entry.get("rationale") or "").strip():
            problems.append(f"{ident}: rationale is required")
        if entry.get("hazard") != "unclear" and not entry.get("codeRefs"):
            problems.append(f"{ident}: at least one code reference (path:line) is required")
    if complete:
        missing = [i["id"] for i in sample["items"] if i["tier"] == 1 and i["id"] not in
                   {e["item"] for e in record.get("items", []) if e.get("hazard") is not None}]
        if missing:
            problems.append(f"{len(missing)} tier-1 items are not reviewed (first: {missing[0]})")
        if not record.get("completed"):
            problems.append("completed date is missing")
    return problems


def load_checked(path, complete=False):
    sample, stale = current_sample()
    record = load(path)
    if record.get("sampleSha256") != sha256_file(SAMPLE):
        sys.exit(f"error: {path} was made for another sample")
    problems = validate(record, sample, complete)
    if problems:
        sys.exit(f"error: {path}:\n  " + "\n  ".join(problems))
    return sample, record, stale


def cmd_check(args):
    for path in args.records:
        _, record, stale = load_checked(path, args.complete)
        done = sum(1 for e in record["items"] if e.get("hazard") is not None)
        print(f"{path}: ok, {done} of {len(record['items'])} items reviewed")
        if stale:
            print("  note: labels changed since the sample was drawn: " + ", ".join(stale))


# --------------------------------------------------------------- summarize

def human_class(entry):
    if entry["hazard"] == "not-real":
        return "false-positive"
    if entry["hazard"] == "unclear" or entry["actionable"] == "unclear":
        return "unclear"
    return "true-positive" if entry["actionable"] == "unhandled-bug" else "intended-risk"


def wilson(hits, total, z=1.96):
    if total == 0:
        return None
    p = hits / total
    centre = (p + z * z / (2 * total)) / (1 + z * z / total)
    half = z * math.sqrt(p * (1 - p) / total + z * z / (4 * total * total)) / (1 + z * z / total)
    return [round(max(0.0, centre - half), 3), round(min(1.0, centre + half), 3)]


def ratio(hits, total):
    return {"value": round(hits / total, 3) if total else None, "hits": hits, "total": total,
            "wilson95": wilson(hits, total)}


def summarize(sample, records):
    by_id = {i["id"]: i for i in sample["items"]}
    reviewed = {}
    for record in records:
        for e in record["items"]:
            if e.get("hazard") is not None:
                reviewed.setdefault(e["item"], []).append((record["reviewer"]["name"], e))
    strata, confusion, disagreements, between = {}, {}, [], []
    heldout = {"findings": 0, "reviewed": 0, "hazard": 0, "bug": 0, "decided": 0,
               "aiHazard": 0, "aiBug": 0, "aiDecided": 0}
    for item in sample["items"]:
        s = strata.setdefault(item["stratum"], {"tier": item["tier"], "items": 0, "reviewed": 0,
                                                "agree": 0, "hazardAgree": 0, "blind": 0})
        s["items"] += 1
        if item["kind"] == "finding" and item["set"] == "held-out":
            heldout["findings"] += 1
        entries = reviewed.get(item["id"], [])
        if not entries:
            continue
        s["reviewed"] += 1
        verdicts = {human_class(e) for _, e in entries}
        if len(verdicts) > 1:
            between.append({"item": item["id"], "verdicts": {n: human_class(e) for n, e in entries}})
        name, entry = entries[0]
        verdict = human_class(entry)
        s["blind"] += all(e["blind"] for _, e in entries)
        ai = item["ai"]["classification"]
        if item["kind"] == "finding":
            confusion.setdefault(ai, {}).setdefault(verdict, 0)
            confusion[ai][verdict] += 1
            agree = verdict == ai
            hazard_agree = (ai == "false-positive") == (entry["hazard"] == "not-real") and "unclear" not in (ai, verdict)
        elif item["kind"] == "known-positive":
            agree = hazard_agree = (entry["hazard"] == "real") == (ai == "valid")
        else:
            agree = hazard_agree = entry["hazard"] == "real" and entry["actionable"] == "unhandled-bug"
        s["agree"] += agree
        s["hazardAgree"] += hazard_agree
        if not agree:
            disagreements.append({"item": item["id"], "stratum": item["stratum"], "ai": ai,
                                  "human": verdict, "hazard": entry["hazard"],
                                  "actionable": entry["actionable"], "reviewer": name,
                                  "rationale": entry["rationale"], "codeRefs": entry["codeRefs"]})
        if item["kind"] == "finding" and item["set"] == "held-out":
            heldout["reviewed"] += 1
            if verdict != "unclear":
                heldout["decided"] += 1
                heldout["hazard"] += verdict in ("true-positive", "intended-risk")
                heldout["bug"] += verdict == "true-positive"
            if ai != "unclear":
                heldout["aiDecided"] += 1
                heldout["aiHazard"] += ai in ("true-positive", "intended-risk")
                heldout["aiBug"] += ai == "true-positive"
    for s in strata.values():
        s["agreement"] = ratio(s["agree"], s["reviewed"])
        s["hazardAgreement"] = ratio(s["hazardAgree"], s["reviewed"])
    complete = heldout["reviewed"] == heldout["findings"]
    precision = {
        "complete": complete,
        "reviewedHeldOutFindings": heldout["reviewed"],
        "heldOutFindings": heldout["findings"],
    }
    if complete:
        precision["humanHazardPrecision"] = ratio(heldout["hazard"], heldout["decided"])
        precision["humanBugPrecision"] = ratio(heldout["bug"], heldout["decided"])
        precision["aiLabelHazardPrecision"] = ratio(heldout["aiHazard"], heldout["aiDecided"])
        precision["aiLabelBugPrecision"] = ratio(heldout["aiBug"], heldout["aiDecided"])
    else:
        precision["note"] = ("not computed: human-validated held-out precision needs every "
                             "held-out finding reviewed")
    return {
        "schemaVersion": 1,
        "kind": "midwrite.human-review-summary",
        "sampleSha256": sha256_file(SAMPLE),
        "reviewers": [{"name": r["reviewer"]["name"], "independence": r["reviewer"]["independence"],
                       "started": r.get("started"), "completed": r.get("completed"),
                       "midwriteRevision": r.get("midwriteRevision")} for r in records],
        "itemsReviewed": len(reviewed),
        "itemsInSample": len(sample["items"]),
        "strata": strata,
        "confusion": {"$comment": "rows: AI label, columns: human verdict (findings only)", **confusion},
        "heldOutPrecision": precision,
        "disagreementsWithAiLabels": disagreements,
        "disagreementsBetweenReviewers": between,
    }


def cmd_summarize(args):
    pairs = [load_checked(p) for p in args.records]
    summary = summarize(pairs[0][0], [r for _, r, _ in pairs])
    if args.output:
        dump(args.output, summary)
    print(f"{summary['itemsReviewed']} of {summary['itemsInSample']} items reviewed by "
          + ", ".join(r["name"] for r in summary["reviewers"]))
    print(f"{'stratum':<24} {'tier':>4} {'reviewed':>9} {'label agrees':>13} {'hazard agrees':>14}")
    for key, s in summary["strata"].items():
        print(f"{key:<24} {s['tier']:>4} {s['reviewed']:>4}/{s['items']:<4} "
              f"{s['agree']:>13} {s['hazardAgree']:>14}")
    p = summary["heldOutPrecision"]
    if p["complete"]:
        print(f"held-out hazard precision: human {p['humanHazardPrecision']['value']} "
              f"(AI labels {p['aiLabelHazardPrecision']['value']}); bug precision: human "
              f"{p['humanBugPrecision']['value']} (AI labels {p['aiLabelBugPrecision']['value']})")
    else:
        print(f"held-out precision: {p['note']} ({p['reviewedHeldOutFindings']}/{p['heldOutFindings']})")
    print(f"{len(summary['disagreementsWithAiLabels'])} disagreements with AI labels, "
          f"{len(summary['disagreementsBetweenReviewers'])} between reviewers")


# -------------------------------------------------------------- adjudicate

def final_verdicts(sample, records, resolutions):
    """One verdict per reviewed item; reviewer disagreements need a resolution."""
    by_item = {}
    for record in records:
        for e in record["items"]:
            if e.get("hazard") is not None:
                by_item.setdefault(e["item"], []).append((record["reviewer"]["name"], e))
    final, unresolved = {}, []
    for ident, entries in by_item.items():
        classes = {human_class(e) for _, e in entries}
        if len(classes) == 1:
            final[ident] = {"classification": classes.pop(), "by": [n for n, _ in entries],
                            "entries": [e for _, e in entries]}
        elif ident in resolutions:
            r = resolutions[ident]
            if r.get("classification") not in CLASSES or not r.get("by") or not r.get("rationale"):
                sys.exit(f"error: resolution for {ident} needs classification, by and rationale")
            final[ident] = {"classification": r["classification"], "by": [r["by"]],
                            "entries": [e for _, e in entries], "resolution": r}
        else:
            unresolved.append(ident)
    return final, unresolved


def adjudicate(sample, records, resolutions, record_paths, today):
    """Return {label path: new label data} and upstream issue updates; nothing is written."""
    final, unresolved = final_verdicts(sample, records, resolutions)
    by_id = {i["id"]: i for i in sample["items"]}
    sets = {p["name"]: p for p in load(SETS)["projects"]}
    labels, issues, changed, confirmed = {}, {}, [], {}
    for ident, verdict in final.items():
        item = by_id[ident]
        entry = verdict["entries"][0]
        evidence = sorted({ref for e in verdict["entries"] for ref in e["codeRefs"]})
        reason = " | ".join(e["rationale"] for e in verdict["entries"])
        if verdict.get("resolution"):
            reason += " | resolution: " + verdict["resolution"]["rationale"]
        if item["kind"] == "upstream-report":
            path = ROOT / item["body"].replace(".md", ".json")
            data = issues.setdefault(path, load(path))
            data["humanReview"] = {"hazard": entry["hazard"], "actionable": entry["actionable"],
                                   "reviewers": verdict["by"], "date": today,
                                   "records": record_paths, "rationale": reason, "codeRefs": evidence}
            continue
        project = sets[item["project"]]
        path = ROOT / project["labels"]
        data = labels.setdefault(path, load(path))
        confirmed.setdefault(path, [])
        if item["kind"] == "finding":
            fid = ident.split(":", 2)[2]
            label = next(f for f in data["findings"] if f["findingId"] == fid)
            if label["classification"] == verdict["classification"]:
                confirmed[path].append(fid)
                continue
            previous = dict(label)
            label["classification"] = verdict["classification"]
            data["corrections"].append({
                "date": today, "by": ", ".join(verdict["by"]) + " (human review, DEV-162)",
                "humanReviewed": True, "type": "human-adjudication", "findingId": fid,
                "previous": previous, "classification": verdict["classification"],
                "reason": reason, "evidence": evidence, "records": record_paths})
            changed.append(ident)
        else:
            kid = ident.split(":", 2)[2]
            valid = entry["hazard"] == "real"
            present = next((k for k in data["knownPositives"] if k["id"] == kid), None)
            if valid == (present is not None):
                confirmed[path].append(kid)
                continue
            if present is not None:
                data["knownPositives"].remove(present)
            data["corrections"].append({
                "date": today, "by": ", ".join(verdict["by"]) + " (human review, DEV-162)",
                "humanReviewed": True, "type": "human-adjudication", "knownPositive": kid,
                "valid": valid, "previous": present, "reason": reason, "evidence": evidence,
                "records": record_paths,
                **({} if present is not None or not valid else
                   {"note": "restoring a removed known positive is done by hand from `previous` of the earlier correction"})})
            changed.append(ident)
    for path, ids in confirmed.items():
        data = labels[path]
        if ids:
            data["corrections"].append({
                "date": today, "by": "human review (DEV-162)", "humanReviewed": True,
                "type": "human-confirmed", "items": sorted(ids), "records": record_paths,
                "reason": "The human verdict matches the existing label; the label is unchanged."})
        reviewed = sorted(set(ids) | {c.get("findingId") or c.get("knownPositive")
                                      for c in data["corrections"]
                                      if c["type"] == "human-adjudication" and c["date"] == today})
        data["labeling"]["humanReview"] = {"date": today, "records": record_paths,
                                           "itemsReviewed": len(reviewed),
                                           "scope": "the DEV-162 sample, not every label"}
    return labels, issues, changed, unresolved


def cmd_adjudicate(args):
    pairs = [load_checked(p, complete=False) for p in args.records]
    sample = pairs[0][0]
    resolutions = load(args.resolutions) if args.resolutions else {}
    paths = [str(Path(p).resolve().relative_to(ROOT)) for p in args.records]
    labels, issues, changed, unresolved = adjudicate(
        sample, [r for _, r, _ in pairs], resolutions, paths, datetime.date.today().isoformat())
    print(f"{len(changed)} labels change, {len(unresolved)} reviewer disagreements unresolved")
    for ident in changed:
        print(f"  change {ident}")
    for ident in unresolved:
        print(f"  unresolved {ident} (add it to --resolutions)")
    if not args.apply:
        print("dry run; pass --apply to write the label files")
        return
    if unresolved:
        sys.exit("error: resolve every reviewer disagreement before applying")
    for path, data in {**labels, **issues}.items():
        dump(path, data)
    sets = {p["name"]: p for p in load(SETS)["projects"]}
    for name, project in sets.items():
        if project["set"] == "held-out" and ROOT / project["labels"] in labels:
            print(f"refreeze: scripts/eval.py freeze --project {name} "
                  f"--reason 'DEV-162 human adjudication ({', '.join(paths)})'")


# ---------------------------------------------------------- upstream status

def cmd_upstream_status(args):
    reports = []
    for path in sorted((ROOT / "reports/upstream/issues").glob("*.json")):
        meta = load(path)
        repo, number = meta["url"].split("github.com/")[1].split("/issues/")
        out = subprocess.run(["gh", "issue", "view", number, "-R", repo, "--json",
                              "state,stateReason,closedAt,labels,comments"],
                             capture_output=True, text=True)
        if out.returncode != 0:
            sys.exit(out.stderr)
        issue = json.loads(out.stdout)
        reports.append({
            "report": str(path.relative_to(ROOT)), "url": meta["url"], "state": issue["state"],
            "stateReason": issue["stateReason"] or None, "closedAt": issue["closedAt"],
            "labels": [l["name"] for l in issue["labels"]],
            "comments": len(issue["comments"]),
            "maintainerConfirmed": None,
        })
    dump(UPSTREAM_STATUS, {
        "schemaVersion": 1, "kind": "midwrite.upstream-status",
        "$comment": "Public state of the upstream reports. maintainerConfirmed stays null until a "
                    "maintainer says in the issue that the failure is real (true) or not (false); "
                    "set it by hand with a link to that comment. An open issue without a reply is "
                    "unconfirmed.",
        "checkedAt": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "reports": reports,
    })
    for r in reports:
        print(f"{r['url']}: {r['state']}, {r['comments']} comments")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("sample").set_defaults(run=cmd_sample)
    p = sub.add_parser("packet")
    p.add_argument("--reveal", action="store_true", help="include the AI labels (read only after your verdicts)")
    p.set_defaults(run=cmd_packet)
    p = sub.add_parser("template")
    p.add_argument("--reviewer", required=True)
    p.set_defaults(run=cmd_template)
    p = sub.add_parser("check")
    p.add_argument("records", nargs="+")
    p.add_argument("--complete", action="store_true", help="also require every tier-1 item")
    p.set_defaults(run=cmd_check)
    p = sub.add_parser("summarize")
    p.add_argument("records", nargs="+")
    p.add_argument("-o", "--output")
    p.set_defaults(run=cmd_summarize)
    p = sub.add_parser("adjudicate")
    p.add_argument("records", nargs="+")
    p.add_argument("--resolutions")
    p.add_argument("--apply", action="store_true")
    p.set_defaults(run=cmd_adjudicate)
    sub.add_parser("upstream-status").set_defaults(run=cmd_upstream_status)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
