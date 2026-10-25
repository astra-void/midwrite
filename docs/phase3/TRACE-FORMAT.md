# Failure trace format (schema version 1)

`commitrift trace [PATHS] --format json` prints one JSON document:

```json
{
  "schemaVersion": 1,
  "kind": "commitrift.traces",
  "tool": { "name": "commitrift", "version": "0.1.0" },
  "config": "commitrift.json",
  "summary": { "findings": 3, "traces": 3, "complete": 2, "approximated": 1, "incomplete": 0, "skipped": 0 },
  "traces": [ /* FailureTrace */ ],
  "skipped": [ { "findingId": "…", "code": "CP9001", "function": "…", "location": { … }, "reason": "…" } ]
}
```

Every diagnostic of the same analysis appears exactly once, either in
`traces` or in `skipped`. Finding ids are the diagnostics' `fingerprint`s from
`commitrift check --format json` with the same inputs and configuration
(including the `-2`, `-3` suffixes for identical findings). `schemaVersion`
is bumped on any incompatible change; new optional fields are not
incompatible.

`--finding <ID>` keeps only the trace whose `findingId` or `id` is `ID`.
`commitrift report <file>` renders the document as HTML.

## FailureTrace

| Field | Type | Meaning |
| --- | --- | --- |
| `id` | string | `tr-` + 16 hex digits: FNV-1a of `trace/v1`, the finding id, the boundary kind, `afterStep`, `beforeStep`, and every step's kind and label. No positions: moving code keeps the id. |
| `findingId` | string | The diagnostic's fingerprint. |
| `code`, `function`, `location` | | Copied from the diagnostic. `location` = `{file, line, column, endLine, endColumn}`, 1-based. |
| `verification` | `"STATIC_CANDIDATE"` | Always, for traces produced by the analysis. |
| `completeness` | `complete` \| `approximated` \| `incomplete` | `approximated`: concurrent or unawaited operations, or an approximated catch-handler entry, on the path. `incomplete`: an operation with unknown semantics inside the failure window, unsupported control flow on the path, or a failed validation (the first limitation says so). An incomplete trace is not a reproduction plan. |
| `steps` | TraceStep[] | Every event on the reported path, in execution order, from function entry. |
| `injection` | InjectionPoint | The failure boundary. |
| `simulation` | Simulation | The abstract model's prediction for a process crash at the boundary. |
| `assumptions` | string[] | What must hold for the trace to describe a real execution (auto-commit, commit on callback completion, recognition basis, branch decisions taken together, unknown invariants). |
| `limitations` | string[] | Trace-specific limitations, then the diagnostic's own. |

## TraceStep

| Field | Meaning |
| --- | --- |
| `index` | Position on the path, from 0. |
| `kind` | `dbRead`, `dbWrite`, `txBegin`, `txCommit`, `txRollback`, `external`, `unknown`, `branch`, `return`, `unsupported`. |
| `label` | Source label (`db.update(orders)`), the transaction's call (`db.transaction(…)`), or the branch decision in words. |
| `description` | What the step is (`database update on \`orders\``, ...). |
| `location` | Its own file and position (an operation inlined from another module keeps its file). |
| `callChain` | `[{callee, label, location}]`, outermost first, when the operation is reached through summarized calls. |
| `transaction` | For transaction steps, the transaction (`tx#0`). For a write, the transaction whose handle it uses; it is staged only while that transaction is open. |
| `parentTransaction` | For a nested transaction (savepoint). |
| `handle`, `table`, `operation`, `category`, `bestEffort`, `execution` | As recognized: the handle as written, Drizzle table identifier, `insert`/`update`/`delete`, external-effect category, configured best-effort, `awaited`/`returned`/`fireAndForget`/`concurrent`. |
| `role` | `completedFirst`, `notYetPerformed`, `stagedWrite`, `transaction`, `inWindow` (strictly between the boundary's two operations). |
| `atFailure` | State when the process dies right after `injection.afterStep`, per the model: `taken` (branch), `completed`, `committed`, `staged` (in a transaction still open: rolled back by the crash), `rolledBack`, `performed`, `notReached`. |

Database reads appear only on explored paths; a branch that contains only
reads does not fork paths, and its reads are not listed.

## InjectionPoint

| Field | Meaning |
| --- | --- |
| `boundary` | `afterCommitBeforeExternal` (CP1001), `afterExternalBeforeCommit` (CP1002), `afterExternalInsideTransaction` (CP1003), `betweenCommits` (CP1004), `betweenExternalEffects` (CP1005). |
| `afterStep` | The last step completed when the failure happens: the commit (a `dbWrite` outside a transaction, or the `txCommit` of a transaction) or the external effect that completes first. |
| `beforeStep` | The operation that has not happened (for CP1003 the transaction's end). |
| `window` | Steps strictly between them. A failure at any of them leaves the same partial state unless one is itself an effect, which is why unknown operations there make the trace incomplete. |
| `description` | The boundary in words. |
| `checkpoints` | `[{name, step, resource}]`: the replay checkpoint right after `afterStep` and the one right before `beforeStep`. Names: `after-db-commit` (resource: table), `before-db-write`, `before-tx-commit`, `after-event-publish`/`before-event-publish` (message-publish effects), `after-external-effect`/`before-external-effect` (other categories). A harness decides where its checkpoints are; these are hints. |
| `openTransactions` | Root transactions open at the boundary (their staged writes are lost by a crash). |

## Simulation

| Field | Meaning |
| --- | --- |
| `verification` | Always `"SIMULATED"`. |
| `failureMode` | Always `"processKill"`: only a crash is simulated. |
| `durable` | Steps whose effect survives: committed writes, performed external effects. |
| `lost` | Writes staged in a transaction open at the boundary. |
| `notPerformed` | Writes, commits and external effects after the boundary. |
| `claim` | The prediction in one sentence. |
| `notModeled` | What the model leaves out: exception handlers and rollbacks (an injected exception runs them), unknown operations, database failure modes, everything outside the function. |

Model rules: a write commits immediately unless it uses the handle of a
transaction open at that point, in which case it is staged in that
transaction's root; a root commit makes its staged writes durable; a rollback
discards the writes of the rolled-back transaction and the transactions
nested in it; at the crash every staged write is lost.

## What a trace proves and does not prove

A trace shows that, *under the analysis' model and its stated
assumptions*, the reported path exists in the source and a failure at the
boundary leaves the predicted partial state. It does not show that the path
is reachable with real inputs, that the operations behave as recognized
(recognition is syntactic and configuration-based), that the partial state
violates an application rule, or that nothing outside the function repairs
it. Runtime evidence requires `commitrift replay` (see
[`REPLAY-PROTOCOL.md`](REPLAY-PROTOCOL.md)).

## Example

```text
$ commitrift trace fixtures/replay/order-vulnerable/src --config fixtures/replay/order-vulnerable/commitrift.json
tr-ad006afba49993a8 CP1001 b46c663b51050e0a in `completeOrder` (fixtures/replay/order-vulnerable/src/orders.ts:11:9) [STATIC_CANDIDATE; Complete]
    0. dbWrite     Committed   db.update(orders) @ 9:9 <- completes first
       ---- failure injection boundary (after-db-commit / before-event-publish) ----
    1. external    NotReached  queue.publish("order.completed", { orderId }) @ 11:9 <- not yet performed
  simulated: If the process dies right after step 0: durable or visible: `db.update(orders)` (step 0); never performed: `queue.publish("order.completed", { orderId })` (step 1).
```

The full JSON for this harness is in
`reports/phase3/replay/order-vulnerable.traces.json`.
