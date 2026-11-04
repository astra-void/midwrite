#!/usr/bin/env python3
"""Check relative links in Midwrite's Markdown documentation.

    python3 scripts/check-doc-links.py [--historical {fail,report,skip}] [--quiet] [FILE...]

Scans README.md, CONTRIBUTING.md and every docs/**/*.md (or the files given)
for inline links `[text](target)`, reference definitions `[ref]: target`
and images, and checks every *relative* target:

* the file or directory exists (relative to the linking file; a leading `/`
  means the repository root);
* a `#fragment` on a Markdown target (or a bare `#fragment`) names a heading
  of that file (GitHub's slug rules) or an explicit `<a name|id="...">`.

External links (`http:`, `https:`, `mailto:`, ...) are not fetched. Links in
fenced code blocks and inline code spans are ignored.

Files under docs/phaseN/ are historical records. With `--historical report`
their broken links are printed but do not change the exit status; with
`skip` they are not checked. Default: `fail` (they count like any other).

Exit status: 0 no counted broken link, 1 at least one, 2 usage error.
Standard library only.
"""

from __future__ import annotations

import argparse
import re
import sys
import unicodedata
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parent.parent
HISTORICAL = re.compile(r"^docs/phase\d+/")
EXTERNAL = re.compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:")
FENCE = re.compile(r"^\s{0,3}(`{3,}|~{3,})")
HEADING = re.compile(r"^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$")
SETEXT = re.compile(r"^\s{0,3}(=+|-+)\s*$")
ANCHOR_TAG = re.compile(r"<a\s[^>]*\b(?:name|id)\s*=\s*[\"']([^\"']+)[\"']", re.I)
REF_DEF = re.compile(r"^\s{0,3}\[([^\]]+)\]:\s*(<[^>]*>|\S+)")


def strip_code_spans(line: str) -> str:
    """Blank out inline code spans (any backtick run length) so links inside
    them are ignored; keeps column positions."""
    out = list(line)
    i = 0
    while i < len(line):
        if line[i] == "`":
            j = i
            while j < len(line) and line[j] == "`":
                j += 1
            run = line[i:j]
            end = line.find(run, j)
            if end == -1:
                i = j
                continue
            for k in range(i, end + len(run)):
                out[k] = " "
            i = end + len(run)
        else:
            i += 1
    return "".join(out)


def slugify(text: str) -> str:
    """GitHub-style heading slug."""
    # Rendered text: drop images, keep link text, drop code backticks and
    # emphasis markers, drop inline HTML tags.
    text = re.sub(r"!\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"\[([^\]]*)\]\[[^\]]*\]", r"\1", text)
    text = re.sub(r"<[^>]+>", "", text)
    text = text.replace("`", "")
    text = text.strip().lower()
    out = []
    for ch in text:
        if ch in (" ", "-"):
            out.append("-")
        elif ch == "_":
            out.append("_")
        elif ch.isalnum():
            out.append(ch)
        else:
            cat = unicodedata.category(ch)
            if cat.startswith("M"):  # combining marks stay with their letter
                out.append(ch)
            # everything else (punctuation, symbols, emphasis markers) is dropped
    return "".join(out)


def iter_lines_outside_fences(text: str):
    fence = None
    for number, line in enumerate(text.splitlines(), start=1):
        m = FENCE.match(line)
        if m:
            marker = m.group(1)
            if fence is None:
                fence = marker[0] * len(marker)
                continue
            if marker[0] == fence[0] and len(marker) >= len(fence):
                fence = None
                continue
        if fence is None:
            yield number, line


_anchor_cache: dict[Path, set[str]] = {}


def anchors_of(path: Path) -> set[str]:
    if path in _anchor_cache:
        return _anchor_cache[path]
    anchors: set[str] = set()
    counts: dict[str, int] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        _anchor_cache[path] = anchors
        return anchors
    previous = None
    for _, line in iter_lines_outside_fences(text):
        title = None
        m = HEADING.match(line)
        if m:
            title = m.group(2)
        elif previous is not None and previous.strip() and SETEXT.match(line) and not previous.lstrip().startswith(("|", "-", "*", ">")):
            title = previous.strip()
        if title is not None:
            slug = slugify(title)
            n = counts.get(slug, 0)
            anchors.add(slug if n == 0 else f"{slug}-{n}")
            counts[slug] = n + 1
        for a in ANCHOR_TAG.findall(line):
            anchors.add(a)
        previous = line
    _anchor_cache[path] = anchors
    return anchors


def find_inline_targets(line: str):
    """Yield (column, raw target) for `[..](target)` and `![..](target)`."""
    i = 0
    while True:
        start = line.find("](", i)
        if start == -1:
            return
        # find the matching ')' allowing one level of nested parentheses and <...>
        j = start + 2
        depth = 0
        if j < len(line) and line[j] == "<":
            end = line.find(">", j)
            if end == -1:
                i = start + 2
                continue
            target = line[j + 1 : end]
            yield start + 1, target
            i = end + 1
            continue
        k = j
        while k < len(line):
            c = line[k]
            if c == "(":
                depth += 1
            elif c == ")":
                if depth == 0:
                    break
                depth -= 1
            elif c == " " and depth == 0:
                # title follows: target ends here
                break
            k += 1
        target = line[j:k].strip()
        if target:
            yield start + 1, target
        i = k + 1


def check_target(src: Path, target: str):
    """Return None if fine, else a reason string."""
    target = target.strip()
    if target.startswith("<") and target.endswith(">"):
        target = target[1:-1]
    if not target or EXTERNAL.match(target):
        return None
    path_part, _, fragment = target.partition("#")
    path_part = unquote(path_part)
    if path_part == "":
        dest = src
    elif path_part.startswith("/"):
        dest = (ROOT / path_part.lstrip("/")).resolve()
    else:
        dest = (src.parent / path_part).resolve()
    if path_part:
        try:
            dest.relative_to(ROOT)
        except ValueError:
            return f"target leaves the repository: {path_part}"
        if not dest.exists():
            return f"no such file or directory: {path_part}"
    if fragment:
        if dest.is_dir() or dest.suffix.lower() not in (".md", ".markdown"):
            return None  # anchors into non-Markdown files are not checked
        if unquote(fragment) not in anchors_of(dest) and fragment not in anchors_of(dest):
            return f"no heading or anchor `#{fragment}` in {dest.relative_to(ROOT)}"
    return None


def check_file(path: Path):
    problems = []
    text = path.read_text(encoding="utf-8")
    for number, raw in iter_lines_outside_fences(text):
        line = strip_code_spans(raw)
        m = REF_DEF.match(line)
        if m:
            reason = check_target(path, m.group(2))
            if reason:
                problems.append((number, m.group(2), reason))
            continue
        for _, target in find_inline_targets(line):
            reason = check_target(path, target)
            if reason:
                problems.append((number, target, reason))
    return problems


def default_files() -> list[Path]:
    files = [ROOT / "README.md", ROOT / "CONTRIBUTING.md"]
    files += sorted((ROOT / "docs").rglob("*.md"))
    return [f for f in files if f.is_file()]


def category(rel: str) -> str:
    if HISTORICAL.match(rel):
        return "historical"
    if rel in ("README.md", "CONTRIBUTING.md"):
        return "root"
    return "docs"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("files", nargs="*", type=Path, help="Markdown files (default: README.md, CONTRIBUTING.md, docs/**/*.md)")
    parser.add_argument("--historical", choices=["fail", "report", "skip"], default="fail",
                        help="how to treat broken links in docs/phaseN/ (default: fail)")
    parser.add_argument("--quiet", action="store_true", help="print only the summary")
    args = parser.parse_args()

    files = [f.resolve() for f in args.files] if args.files else default_files()
    totals = {"root": [0, 0], "docs": [0, 0], "historical": [0, 0]}  # files, broken links
    counted = 0
    for f in files:
        if not f.is_file():
            print(f"error: not a file: {f}", file=sys.stderr)
            return 2
        rel = f.relative_to(ROOT).as_posix()
        cat = category(rel)
        if cat == "historical" and args.historical == "skip":
            continue
        problems = check_file(f)
        totals[cat][0] += 1
        totals[cat][1] += len(problems)
        fails = not (cat == "historical" and args.historical == "report")
        if fails:
            counted += len(problems)
        if not args.quiet:
            for number, target, reason in problems:
                tag = "" if fails else " [historical, not counted]"
                print(f"{rel}:{number}: broken link `{target}`: {reason}{tag}")
    print(
        "checked {} file(s): README/CONTRIBUTING {} broken in {} file(s); docs (current) {} broken in {} file(s); "
        "historical docs/phaseN {} broken in {} file(s){}".format(
            sum(v[0] for v in totals.values()),
            totals["root"][1], totals["root"][0],
            totals["docs"][1], totals["docs"][0],
            totals["historical"][1], totals["historical"][0],
            " (reported only)" if args.historical == "report" else (" (skipped)" if args.historical == "skip" else ""),
        )
    )
    return 1 if counted else 0


if __name__ == "__main__":
    sys.exit(main())
