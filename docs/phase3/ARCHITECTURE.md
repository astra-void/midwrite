# Phase 3 architecture: from static finding to runtime evidence

Phase 3 adds one path on top of the Phase 2 analyzer:

```text
static finding ──► failure trace ──► controlled injection ──► observed state ──► verification report ──► HTML view
 (check)            (trace)            (replay, opt-in)          (replay)            (replay --output)       (report)
```

Every arrow is a separate command, and only `replay` executes code. The
Phase 2 analysis (Effect IR, paths, detection, diagnostics) is unchanged:
on the whole corpus Phase 3 reports exactly the same findings, fingerprints
and coverage counters (`reports/phase3/phase2-vs-phase3.txt`).

## Crates

```text
commitrift-core       Effect IR, analysis, diagnostics, config            (no parser, no runtime)
  ├─ trace            Failure Trace IR, abstract crash simulation, validation
  └─ verification     Verification levels, failure modes, report types (serde only)
commitrift-analyzer   TypeScript frontend + project analysis               (depends on core)
commitrift-replay     Runtime replay: manifests, disposable PostgreSQL,    (depends on core only)
                      supervisor, observations, invariants
commitrift-cli        check, explain, ir, trace, report, replay           (replay behind the default
                                                                           `replay` cargo feature)
commitrift-corpus     dev tool: corpus runs, comparison, precision, memory profile
runtime/typescript    replay protocol client for harnesses (no dependencies)
```

`commitrift-core` and `commitrift-analyzer` do not depend on
`commitrift-replay`. `commitrift check` never calls into it; the CLI can be
built without it (`--no-default-features`). The viewer reads reports through
the serde types in `commitrift-core`, so it needs no runtime code either.

## 1. Failure Trace IR (`commitrift-core::trace`)

A `FailureTrace` is one candidate execution path behind one diagnostic,
derived from the path the analysis actually reported:

* `detect` already chose, for every finding, the best-ranked path; Phase 3
  only records its index (`Finding::trace`).
* `trace_function_in` re-runs the same enumeration with database reads kept
  as events (they never fork paths and are ignored when paths are merged, so
  the set and order of paths, and every diagnostic, are identical — checked by
  `traces_do_not_change_the_diagnostics`).
* Every event of the path becomes one `TraceStep`, in order: database reads
  and writes, transaction begin/commit/rollback, external effects, unknown
  operations, branch decisions, transaction-callback returns, unsupported
  control flow. Steps carry the source label, file and position, the call
  chain for operations inlined from other functions or modules, the
  transaction a write is staged in, table, operation, handle, category.
  Nothing is invented: no step without an IR event, no argument values.
* The **injection boundary** sits right after the operation the finding says
  completes first and before the one it says has not happened
  (`afterStep`/`beforeStep`, plus the steps in between). It names the
  checkpoints a harness can use (`after-db-commit`, `before-event-publish`,
  ...); the names are a protocol convention, not AST positions.
* `simulate_crash` runs the analysis' transaction model over the steps for a
  process crash right after `afterStep`: which writes are durable, which are
  lost with an open transaction, which effects never happen. That result is
  labeled `SIMULATED` and is never stronger than the trace.
* `validate` checks the trace against the model (indices, ordering, the
  simulation agreeing with the step states, the first operation durable and
  the second not performed). A trace that fails is downgraded to
  `incomplete` with the reason. No fixture or corpus trace fails (389 traces
  over the fixtures and four corpus projects; `reports/phase3/trace-coverage.txt`).
* `completeness`: `complete`, `approximated` (concurrency, unawaited
  operations, an approximated catch entry), or `incomplete` (unknown
  operations inside the failure window, unsupported control flow). midday has
  47 incomplete traces out of 180, school-ledger 93 of 123: the unknown calls
  that CP9001 already reports sit inside those windows.
* Identity: `id = hash(finding id, boundary, step kinds and labels)`; no line
  numbers, so moving code keeps it. The finding id is the diagnostic
  fingerprint after the report's duplicate disambiguation
  (`report::order_findings`, shared by reports, baselines and traces).

CP9001 diagnostics have no trace: they describe what the analysis could not
interpret, not a failure path.

## 2. Replay protocol and supervisor (`commitrift-replay`)

A **harness** is a directory with `commitrift-replay.json` (validated before
anything runs), an instrumented application, and SQL files for its initial
state. `commitrift replay <dir>` without `--allow-execution` prints the plan
and exits 2.

For one invocation the supervisor:

1. creates a private 0700 directory, short enough for Unix socket paths;
2. creates a PostgreSQL cluster in it (`initdb`, trust authentication for a
   local user) and starts it with `listen_addresses=''`: Unix socket only, in
   that directory;
3. chooses network isolation (macOS `sandbox-exec`: no IP traffic, no Unix
   socket outside the directory, no file writes outside it) or refuses;
4. runs the **control** scenario without injection from a fresh database; if
   it does not complete or violates an invariant, every case is
   `INCONCLUSIVE` (the harness or its contract is wrong);
5. for every selected case and repetition: creates a fresh database, applies
   the setup files, observes the state (`before`), starts the harness with a
   cleared environment, and answers its checkpoints over a Unix socket. At the
   selected checkpoint it first runs the case's `verifyBeforeInjection`
   queries from a separate session (e.g. the write is visible, i.e.
   committed), then injects: `processKill` sends SIGKILL to the harness'
   process group while it is blocked waiting for the answer; `exception`
   answers `throw`. It then waits for the harness' database sessions to end,
   observes (`after`), runs `recovery` scenarios without injection if any,
   observes again (`afterRecovery`), evaluates the invariants and drops the
   database;
6. stops the cluster and deletes the directory (also on early return, via
   `Drop`).

Because the harness blocks at a checkpoint until the supervisor answers,
the kill can only land at that checkpoint: no race between notification and
termination, no reliance on timing. Deadlines bound connection, every message
and the process lifetime.

## 3. Verification evidence model (`commitrift-core::verification`)

| Level | Produced by | Meaning |
| --- | --- | --- |
| `STATIC_CANDIDATE` | `trace` | A possible failure path from source analysis. Nothing executed. |
| `SIMULATED` | `trace` (`simulation`) | The abstract transaction model applied to the trace. Nothing executed. |
| `RUNTIME_REPRODUCED` | `replay` | The failure was injected at the selected checkpoint of an authorized harness, and a declared invariant was observed to fail. |
| `RUNTIME_NOT_REPRODUCED` | `replay` | The run completed as specified and every declared invariant held. Not a proof of safety. |
| `INCONCLUSIVE` | `replay` | No outcome: setup failure, timeout, checkpoint never reached, failed pre-injection check, failing control run, runs that disagree. |

These levels are a separate axis from `confidence`: a trace never raises a
diagnostic's confidence, and `check` output is unchanged (`"reproduced":
false` as before). Runtime results live only in verification reports, each
tied to a harness, its manifest hash and its environment.

A runtime level is decided per run, in this order: a problem (protocol,
setup, pre-injection check) → `INCONCLUSIVE`; timeout → `INCONCLUSIVE`;
injection requested but the checkpoint never reached → `INCONCLUSIVE`;
process kill requested but the process did not die by SIGKILL →
`INCONCLUSIVE`; a run without injection that did not complete →
`INCONCLUSIVE`; a recovery scenario that did not complete →
`INCONCLUSIVE`; any invariant violated at any stage → `RUNTIME_REPRODUCED`;
otherwise `RUNTIME_NOT_REPRODUCED`. A case's level is its runs' common
level; runs that disagree make it `INCONCLUSIVE`.

The analyzer may *suggest* checkpoints and predictions, but invariants come
only from the harness author: a missing event is a bug only if the declared
contract says so.

## 4. Recovery declarations and effect relationships (static, descriptive)

* `externalEffects[].recovery` (`bestEffort`, `reconciled`,
  `idempotentConsumer`) are assertions in configuration. They can lower a
  severity under `recommended`, never remove a finding, and are listed in the
  diagnostic's `recovery` with `"source": "configuration", "verified": false`.
  Properties proven by analysis (a write in the same transaction as the
  state change: no finding at all) and properties observed at runtime (a
  verification report) stay in their own places.
* `relationship` describes whether the arguments of the two operations refer
  to a common variable (by name, within one function). On the reviewed
  corpus it does not separate bugs from non-bugs (see `PRECISION.md`), so it
  is reported in JSON only and never used for severity.

## 5. Viewer (`commitrift report`)

One self-contained HTML file per report, generated in Rust. No script, no
external resource; a Content-Security-Policy forbids both. Every string from
a report is escaped. Static predictions are drawn with dashed borders and
labeled "not executed"; runtime observations with solid borders and their
checkpoint log, observations by stage, invariant results and harness output.

## Memory change

Phase 3 also shrinks the IR: function and block bodies are `shrink_to_fit`
after lowering (the vectors' growth slack was ~60% of midday's retained IR
bytes), and `Effect::recognition` is boxed (most nodes are not recognized
effects, and every node is as large as the largest variant): `Node` went
from 208 bytes (Phase 2) to 176. Details in `PERFORMANCE.md`.
