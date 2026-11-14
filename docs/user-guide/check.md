# `midwrite check`: static failure-path analysis

`check` finds moments at which state becomes **durable** (a database
commit) or **visible** (a configured external effect) in each function, and
reports every place where a failure between two of them leaves partial
state. It parses and analyzes; it never imports, evaluates or executes
project code, makes no network requests and has no telemetry.

Model: [failure model](../concepts/failure-model.md). Diagnostics catalog:
[`docs/DIAGNOSTICS.md`](../DIAGNOSTICS.md). All flags:
[CLI reference](../reference/cli.md#check).

## Typical use

```bash
midwrite check .                                  # default: text, policy recommended, fail on warning
midwrite check apps/web/src --config midwrite.json
midwrite check . --format json > findings.json
midwrite check . --format sarif > findings.sarif
midwrite check . -v                               # also evidence, general limitations, every repeated finding in full
```

A run analyzes all given files as one project, so imports between them are
followed. Directories are searched recursively for `.ts`, `.tsx`, `.mts`,
`.cts`, skipping `node_modules`, `.git`, `dist`, `build`, `out`, `.next`,
`.nuxt`, `.output`, `.turbo`, `.vercel`, `.svelte-kit`, `coverage` and
`.d.ts` files; explicitly named files are always analyzed. `"exclude"` in
the configuration skips more ([configuration](../reference/configuration.md)).

## Diagnostics

| Code | Default severity (`recommended`) | Meaning |
| --- | --- | --- |
| `MW1001` | warning | database commit followed by an independent external effect |
| `MW1002` | warning | external effect followed by an independent database commit |
| `MW1003` | warning | external effect executed inside a database transaction before commit |
| `MW1004` | info; warning when a write bypasses its enclosing transaction | independent database commits |
| `MW1005` | info | independent external effects |
| `MW9001` | info | analysis incomplete: unknown or unsupported semantics (not a finding) |

Every static finding is a potential hazard (`"kind": "potentialHazard"`,
`"reproduced": false`). Confidence combines `pathCertainty` and
`evidenceStrength` ([`DIAGNOSTICS.md` § Confidence](../DIAGNOSTICS.md#confidence)).

## Policies

| Policy | Warnings | Info |
| --- | --- | --- |
| `recommended` (default) | MW1001, MW1002, MW1003; MW1004 when a write bypasses its enclosing transaction | other MW1004, MW1005, MW9001; unrecognized calls after a commit grouped into one MW9001 per function |
| `strict` | every hazard | MW9001, one per call |
| `audit` | as `strict` | as `strict`, plus unknown calls before the first recognized operation |

Codes and meanings do not change between policies; only severity (and so
the exit status) does.

## Inline suppressions

Silence a rule at a finding's reported line with a `// midwrite-ignore`
comment and a required reason:

```ts
await db.update(orders).set({ status: "paid" });
// midwrite-ignore MW1001: compensated by nightly reconciliation job
await queue.publish("order.completed", { id });
```

Put the comment immediately above the reported line, or at the end of that
line. A standalone comment targets exactly the next line; blank lines break
adjacency. The directive takes one known rule ID, including `MW9001`, and
suppresses only that rule at that primary location. Other rules and lines
remain reported. For a finding reported at a summarized call, put the
directive at the caller's reported line.

Suppressed findings are omitted from text, JSON, SARIF, failure traces and
new baselines, and do not count toward `--fail-on`. Operations stay in the
IR and call summaries. The reason is recorded in source; Midwrite does not
verify the claimed recovery mechanism. Strings, block comments and comments
that merely mention the directive are not suppression directives. There is
no file-wide or function-wide suppression.

A missing or whitespace-only reason, unknown rule, wildcard or list of rules
is rejected as a file parse error with the comment's position. That file is
not analyzed; the run remains incomplete (exit status 3 unless another
finding reaches `--fail-on`), even with `--fail-on never`. Syntax and read
errors cannot be suppressed.

## Baselines (adopting it in CI)

Step by step, from the first scan to a new finding in CI:
[adopting `midwrite check`](adoption.md).

```bash
midwrite check . --write-baseline midwrite-baseline.json   # once; commit the file
midwrite check . --baseline midwrite-baseline.json          # fails only on new findings
```

Entries are matched by a fingerprint of code, file, function and the
operations' compact source labels, including related call sites. Line and
column numbers are excluded, even from generated anonymous-function names,
so adding comments, shifting lines or reflowing member chains keeps the ID.
SARIF carries the same ID as `partialFingerprints["midwrite/v1"]`; traces
use it as `findingId`. Parse failures are never suppressed.

Changing the rule, function name or operation labels changes the ID. File
moves change the ID, but baselines still match a unique file-independent
identity. Findings with identical inputs get occurrence suffixes (`-2`,
`-3`, …) in source order; inserting or removing an identical occurrence can
reassign those suffixes. Use the same analysis root across runs to keep file
paths consistent. Older baselines and replay selectors for anonymous
callbacks need refreshing once after the position-normalization fix;
named-function IDs are unchanged.

## GitHub code scanning

Run the scan from the repository root to keep paths and fingerprints
consistent between CI runs. Each SARIF result includes the same stable
fingerprint as JSON in `partialFingerprints["midwrite/v1"]`, plus its
diagnostic code, severity, source range and related effects.

Add these steps to a GitHub Actions job after checking out the repository
and [installing Midwrite](../getting-started/installation.md). Give the job
`contents: read` and `security-events: write` permissions:

```yaml
permissions:
  contents: read
  security-events: write

steps:
  # Check out your repository and install midwrite before these steps.
  - name: Analyze failure paths
    run: midwrite check . --format sarif > midwrite.sarif
  - name: Upload code scanning results
    if: ${{ !cancelled() }}
    uses: github/codeql-action/upload-sarif@v4
    with:
      sarif_file: midwrite.sarif
      category: midwrite
```

The upload step runs even when findings make the scan exit with status 1.
For an annotation-only scan, add `--fail-on never`; read and parse errors
still produce an incomplete run and exit status 3. With `--baseline`,
existing findings remain in SARIF with `baselineState: "unchanged"` and
only new findings count toward the failure threshold.

GitHub supports these uploads for public repositories. Alerts can appear
on pull requests when the reported lines are in the diff. See GitHub's
[upload guide](https://docs.github.com/en/code-security/how-tos/find-and-fix-code-vulnerabilities/integrate-with-existing-tools/upload-sarif-file)
and [SARIF support](https://docs.github.com/en/code-security/reference/code-scanning/sarif-files/sarif-support).

## Exit status

`0` clean; `1` a diagnostic at or above `--fail-on`; `2` usage,
configuration or input error; `3` some files could not be read or parsed
(and no diagnostic reached the threshold). A file that does not parse is
never analyzed.

## What it follows

Calls it can resolve, through depth-bounded summaries (`--max-call-depth`,
default 3): functions of the same file or imported from analyzed files
(relative imports, `tsconfig` `paths`, workspace packages), methods of
receivers whose class is known, callbacks passed to analyzed functions,
immediately invoked functions; calls of functions it proves inert are
resolved at any depth. Paths per function are capped (`--max-paths`,
default 512; exceeding it adds `MW9001`). Full table:
[supported patterns](../reference/supported-patterns.md#static-analyzer-check-trace-ir).

## Related commands

| Command | Purpose |
| --- | --- |
| `midwrite trace [PATHS] [--finding ID] [--format json]` | the failure trace behind each finding (static candidate) — [format](../reference/trace-format.md) |
| `midwrite ir [PATHS] [--format json]` | the Effect IR the frontend derived |
| `midwrite report TRACE.json -o trace.html` | self-contained, script-free HTML |
| `midwrite init-replay` / `midwrite replay` | turn one trace into a runtime replay harness ([source verification](source-verification.md#single-request-replay-harnesses), [protocol](../architecture/replay-protocol.md)) |

## Accuracy

The reviews of `check` findings on real projects are a first review by the
implementer of the cross-module analysis, from source code alone (103
findings, 97 with a
decided label; the window existed in 92, 27 were judged likely bugs), and a
blind automated re-review of 18 of them by a separate model session. Neither is an independent human review, and no maintainer
confirmed a finding. Numbers and caveats:
[evidence index](../research/evidence-index.md#1-static-analysis-accuracy).
Read findings as review candidates.
