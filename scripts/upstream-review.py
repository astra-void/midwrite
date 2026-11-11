#!/usr/bin/env python3
"""Reproduce the pinned upstream review without executing repository code."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "corpora/upstream/manifest.json"


def digest_tree(root):
    digest = hashlib.sha256()
    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if ".git" in relative.parts or "node_modules" in relative.parts:
            continue
        if path.is_symlink():
            payload = ("symlink:" + str(path.readlink())).encode()
        elif path.is_file():
            payload = path.read_bytes()
        else:
            continue
        digest.update(str(relative).encode() + b"\0" + hashlib.sha256(payload).digest())
    return digest.hexdigest()


def fetch(project):
    destination = ROOT / project["path"]
    if destination.exists():
        assert digest_tree(destination) == project["sourceTreeSha256"], str(destination)
        return
    url = f"https://codeload.github.com/{project['repository']}/tar.gz/{project['revision']}"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent) as scratch:
        archive = Path(scratch) / "source.tar.gz"
        with urllib.request.urlopen(url, timeout=180) as response:
            archive.write_bytes(response.read())
        extracted = Path(scratch) / "source"
        extracted.mkdir()
        with tarfile.open(archive) as stream:
            stream.extractall(extracted, filter="data")
        roots = list(extracted.iterdir())
        assert len(roots) == 1 and roots[0].is_dir(), url
        assert digest_tree(roots[0]) == project["sourceTreeSha256"], url
        roots[0].rename(destination)


def run(project, binary, output, phase):
    source = ROOT / project["path"]
    assert digest_tree(source) == project["sourceTreeSha256"], str(source)
    config = project["beforeConfig"] if phase == "before" else project["config"]
    expected_config = project["beforeConfigSha256"] if phase == "before" else project["configSha256"]
    assert hashlib.sha256((ROOT / config).read_bytes()).hexdigest() == expected_config, config
    result = subprocess.run(
        [str(binary), "check", project["scanPath"], "--config", config,
         "--format", "json", "--fail-on", "never"],
        cwd=ROOT, capture_output=True, text=True, check=False,
    )
    assert result.returncode == project["exitCode"], result.stderr or result.stdout
    report = json.loads(result.stdout)
    assert digest_tree(source) == project["sourceTreeSha256"], "source changed"
    output.mkdir(parents=True, exist_ok=True)
    (output / f"{project['name']}.{phase}.json").write_text(result.stdout)
    print(project["name"], report["summary"]["filesAnalyzed"], "files;", len(report["diagnostics"]), "diagnostics")


def verify(project):
    ledger = json.loads((ROOT / project["reviews"]).read_text())
    for phase in ("before", "after"):
        report = json.loads((ROOT / f"reports/upstream/{project['name']}.{phase}.json").read_text())
        expected = {d["fingerprint"] for d in report["diagnostics"]}
        reviews = [r for r in ledger["reviews"] if phase in r["phases"]]
        assert len(reviews) == len(expected), (project["name"], phase, "duplicate/missing reviews")
        assert {r["fingerprint"] for r in reviews} == expected, (project["name"], phase)
        assert all(r["notes"] and r["classification"] in {
            "reported", "false-positive", "intended-risk", "needs-triage", "analysis-limitation"
        } for r in reviews)
        assert not ledger["humanReviewed"], "AI review must not be labeled human review"
        for review in reviews:
            if review["classification"] == "reported":
                assert review.get("upstream"), review
    print(project["name"], "every diagnostic has a review record")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("fetch", "run", "verify"))
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/midwrite")
    parser.add_argument("--output", type=Path, default=ROOT / ".corpora/upstream/rerun")
    parser.add_argument("--phase", choices=("before", "after"), default="after")
    args = parser.parse_args()
    manifest = json.loads(MANIFEST.read_text())
    for project in manifest["projects"]:
        if args.action == "fetch":
            fetch(project)
        elif args.action == "run":
            run(project, args.binary.resolve(), args.output.resolve(), args.phase)
        else:
            verify(project)
    if args.action == "verify":
        issues = [json.loads(path.read_text()) for path in (ROOT / "reports/upstream/issues").glob("*.json")]
        reported = {issue["url"] for issue in issues if issue["status"] == "reported"}
        assert len(reported) >= 3, "Stage 2 requires at least three distinct upstream reports"
        print(f"Stage-2 report-count gate: PASS ({len(reported)} distinct reports; source review only)")


if __name__ == "__main__":
    main()
