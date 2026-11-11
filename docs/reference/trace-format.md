# Failure trace format

`midwrite trace [PATHS] --format json` prints one document of kind
`midwrite.traces` (written as `commitrift.traces` before the product
rename). Schema version 1 is the base; version 2 only adds the optional
`valueRelation` per trace, and version 1 reports still load. JSON Schemas:
[v1](schemas/trace-report.v1.schema.json),
[v2](schemas/trace-report.v2.schema.json). Code: `midwrite-core::trace`.

```json
{
  "schemaVersion": 2,
  "kind": "midwrite.traces",
  "tool": { "name": "midwrite", "version": "…" },
  "config": "midwrite.json",
  "summary": { "findings": 3, "traces": 3, "complete": 2, "approximated": 1, "incomplete": 0, "skipped": 0 },
  "traces": [ /* FailureTrace */ ],
  "skipped": [ { "findingId": "…", "code": "MW9001", "function": "…", "location": { … }, "reason": "…" } ]
}
```

Every diagnostic of the same analysis appears exactly once, in `traces` or
in `skipped` (MW9001 diagnostics have no trace: they describe what the
analysis could not interpret, not a failure path). Finding ids are the
diagnostics' `fingerprint`s from `check --format json` with the same inputs
and configuration (including `-2`, `-3` suffixes). `--finding <ID>` keeps
only the trace whose `findingId` or `id` is `ID`; `midwrite report` renders
the document as HTML.

## How a trace is built

The detector already chose the best-ranked path for every finding; the
trace builder re-runs the same enumeration with database reads kept as
events (reads never fork paths and are ignored when paths merge, so the set
and order of paths and every diagnostic are identical —
`traces_do_not_change_the_diagnostics`). Every event becomes one step;
nothing is invented (no step without an IR event, no argument values).
`validate` checks the trace against the transaction model (indices,
ordering, simulation agreeing with step states, first operation durable
and second not performed); a trace that fails is downgraded to
`incomplete` with the reason.

## FailureTrace

| field | meaning |
| --- | --- |
| `id` | `tr-` + 16 hex digits: FNV-1a of the finding id, the boundary kind, `afterStep`, `beforeStep` and every step's kind and label. No positions: moving code keeps the id. |
| `findingId` | the diagnostic's fingerprint |
| `code`, `function`, `location` | copied from the diagnostic (`{file, line, column, endLine, endColumn}`, 1-based) |
| `verification` | always `STATIC_CANDIDATE` |
| `completeness` | `complete`; `approximated` (concurrent or unawaited operations, or an approximated catch-handler entry, on the path); `incomplete` (an operation with unknown semantics inside the failure window, unsupported control flow, or a failed validation). An incomplete trace is not a reproduction plan. |
| `steps` | every event on the reported path, in execution order, from function entry |
| `injection` | the failure boundary |
| `simulation` | the abstract model's prediction for a process crash at the boundary |
| `assumptions` | what must hold for the trace to describe a real execution (auto-commit, commit on callback completion, recognition basis, branch decisions taken together) |
| `limitations` | trace-specific, then the diagnostic's own |
| `valueRelation` | (v2, optional) as in diagnostics ([value analysis](../architecture/value-analysis.md#effect-relationships-valuerelation)) |

## TraceStep

| field | meaning |
| --- | --- |
| `index` | position on the path, from 0 |
| `kind` | `dbRead`, `dbWrite`, `txBegin`, `txCommit`, `txRollback`, `external`, `unknown`, `branch`, `return`, `unsupported` |
| `label`, `description` | source label (`db.update(orders)`), or the branch decision in words |
| `location` | its own file and position (an operation inlined from another module keeps its file) |
| `callChain` | `[{callee, label, location}]`, outermost first, for operations reached through summarized calls |
| `transaction`, `parentTransaction` | for transaction steps the transaction (`tx#0`); for a write, the transaction whose handle it uses; the parent of a savepoint |
| `handle`, `table`, `operation`, `category`, `bestEffort`, `execution` | as recognized |
| `role` | `completedFirst`, `notYetPerformed`, `stagedWrite`, `transaction`, `inWindow` |
| `atFailure` | state if the process dies right after `injection.afterStep`: `taken`, `completed`, `committed`, `staged` (lost with the open transaction), `rolledBack`, `performed`, `notReached` |

## InjectionPoint

| field | meaning |
| --- | --- |
| `boundary` | `afterCommitBeforeExternal` (MW1001), `afterExternalBeforeCommit` (MW1002), `afterExternalInsideTransaction` (MW1003), `betweenCommits` (MW1004), `betweenExternalEffects` (MW1005) |
| `afterStep` | the last step completed when the failure happens (the commit, or the external effect that completes first) |
| `beforeStep` | the operation that has not happened (for MW1003 the transaction's end) |
| `window` | steps strictly between them; an unknown operation here makes the trace incomplete |
| `checkpoints` | `[{name, step, resource}]`: the replay checkpoint right after `afterStep` and the one right before `beforeStep` (`after-db-commit`, `before-db-write`, `before-tx-commit`, `after-/before-event-publish`, `after-/before-external-effect`). Hints only: a harness decides where its checkpoints are ([replay protocol](../architecture/replay-protocol.md#checkpoints)). |
| `openTransactions` | root transactions open at the boundary |

## Simulation

`verification` is always `SIMULATED` and `failureMode` always
`processKill`. `durable` lists committed writes and performed external
effects, `lost` the writes staged in a transaction open at the boundary,
`notPerformed` everything after it; `claim` states the prediction in one
sentence; `notModeled` lists what is left out (exception handlers and
rollbacks, unknown operations, database failure modes, everything outside
the function). Model rules: a write commits immediately unless it uses the
handle of a transaction open at that point, in which case it is staged in
that transaction's root; a root commit makes its staged writes durable; a
rollback discards the writes of the rolled-back transaction and those
nested in it; at the crash every staged write is lost.

## What a trace proves

Under the analysis' model and its stated assumptions, the reported path
exists in the source and a failure at the boundary leaves the predicted
partial state. It does not show that the path is reachable with real inputs
(branch conditions are not solved, only contradictions pruned), that the
operations behave as recognized, that the partial state violates an
application rule, or that nothing outside the function repairs it. Runtime
evidence requires [`midwrite replay`](../architecture/replay-protocol.md).

## Example

```text
$ midwrite trace fixtures/replay/order-vulnerable/src --config fixtures/replay/order-vulnerable/midwrite.json
tr-… MW1001 … in `completeOrder` (fixtures/replay/order-vulnerable/src/orders.ts:11:9) [STATIC_CANDIDATE; Complete]
    0. dbWrite     Committed   db.update(orders) @ 9:9 <- completes first
       ---- failure injection boundary (after-db-commit / before-event-publish) ----
    1. external    NotReached  queue.publish("order.completed", { orderId }) @ 11:9 <- not yet performed
  simulated: If the process dies right after step 0: durable or visible: `db.update(orders)` (step 0); never performed: `queue.publish("order.completed", { orderId })` (step 1).
```
