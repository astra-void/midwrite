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
midwrite check . -v                               # also evidence, path certainty, evidence strength, limitations
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

## Baselines (adopting it in CI)

```bash
midwrite check . --write-baseline midwrite-baseline.json   # once; commit the file
midwrite check . --baseline midwrite-baseline.json          # fails only on new findings
```

Entries are matched by a fingerprint of code, file, function and the
operations' source labels (no line numbers); SARIF carries it as
`partialFingerprints`. Parse failures are never suppressed.

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
| `midwrite trace [PATHS] [--finding ID] [--format json]` | the failure trace behind each finding (static candidate) — [format](../phase3/TRACE-FORMAT.md) |
| `midwrite ir [PATHS] [--format json]` | the Effect IR the frontend derived |
| `midwrite report TRACE.json -o trace.html` | self-contained, script-free HTML |
| `midwrite init-replay` / `midwrite replay` | turn one trace into a runtime replay harness ([source verification](source-verification.md#single-request-replay-harnesses)) |

## Accuracy

The reviews of `check` findings on real projects are the Phase 2 review by
the Phase 2 implementer, from source code alone (103 findings, 97 with a
decided label; the window existed in 92, 27 were judged likely bugs), and a
blind automated re-review of 18 of them in Phase 3 by a separate model
session. Neither is an independent human review, and no maintainer
confirmed a finding. Numbers and caveats:
[evidence index](../research/evidence-index.md#1-static-analysis-accuracy).
Read findings as review candidates.
