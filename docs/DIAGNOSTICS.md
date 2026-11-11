# Diagnostics

Codes are stable: a code's meaning will not change. `midwrite explain <CODE>`
prints the same documentation from the binary.

| Code | Catalog severity (`strict`) | `recommended` (default) | Kind | Title |
| --- | --- | --- | --- | --- |
| MW1001 | warning | warning | potentialHazard | database commit followed by an independent external effect |
| MW1002 | warning | warning | potentialHazard | external effect followed by an independent database commit |
| MW1003 | warning | warning | potentialHazard | external effect executed inside a database transaction before commit |
| MW1004 | warning | warning when a write bypasses its enclosing transaction or uses a handle after the transaction ended; otherwise info | potentialHazard | independent database commits |
| MW1005 | warning | info | potentialHazard | independent external effects |
| MW9001 | info | info | analysisLimitation | analysis incomplete: unsupported or unknown effect semantics |

No code has `error` severity: every finding is a potential hazard derived
from code structure, not a confirmed violation of an application invariant.
`error` is reserved for that (future) case.

**Policies** (`--policy`) change severities, never codes or their meaning.
`strict` uses the catalog severities. `recommended`
keeps cross-resource hazards and transaction bypasses as warnings and reports
broad atomicity checks (two sequential writes, two sequential external
effects) as `info` review candidates: in the first corpus review most of
them were real windows that the code accepts on purpose, and they far
outnumber the cross-resource findings (two sequential effects on the same
resource are usually ordered deliberately, while a database/external pair
cannot be made atomic by any transaction). The cost is that reviewed true
positives among sequential writes are `info` under `recommended`; use
`--policy strict` to make them fail CI. `recommended` also reports the
unrecognized calls of a function that are relevant only because of their
context (after a commit, inside a transaction with writes) as one MW9001 per
function, listing the others in `limitations`. `audit` is `strict` plus a
MW9001 for unknown calls *before* the first recognized operation of a path
(possible unconfigured effects).

**Best-effort effects.** An `externalEffects` rule with `"bestEffort": true`
declares that the application accepts the effect sometimes not being
performed. Under `recommended`, a MW1001 or MW1005 whose effect that may not
happen is best-effort is `info`, with a limitation saying so; MW1002 and
MW1003 (the effect already happened) and every other policy are unchanged.
`"firstArgument"` narrows a rule to calls whose first argument is that string
literal, so `triggerJob("notification", …)` can be best-effort while
`triggerJob("generate-invoice", …)` is not.

**Operations from summaries.** When an operation comes from the summary of a
called function (`await notify(id)` where `notify` publishes), declared in the
same file or imported from another analyzed module, the diagnostic belongs to
the calling function and its *primary location* is the call (`notify(id)`),
not the line inside `notify`. The operation keeps its own location (and its
own file) in `effects`, a `callSite` entry names the call and where the
callee is declared, and `evidence` states that the operation was taken from
the summary and through which chain of calls. A hazard entirely inside one
callee is reported once, for the callee. Unknown calls inside one inlined
call are reported once per call.

**Resolved methods, callbacks and concurrent calls.** The called function may also be a method found by
dispatch on a receiver of known class, a callback passed to an analyzed
function, an immediately invoked function expression, or a `Promise.all`
element (its operations are then `concurrent`, confidence `low`). When a
method was resolved under an assumption (an unanalyzed superclass, an
escaped receiver), confidence is at most `medium` and `limitations` names the
assumption ([value analysis](architecture/value-analysis.md)). Calls of functions proven inert are not unknown calls and
produce no MW9001. The detail of a remaining unknown call says why its callee
is not inert or not resolved ("may dispatch to an override in `Loud`",
"calls `job.updateProgress(…)`, whose target is not known").

**Alternative paths.** When the same operation (for MW1001, the external
effect that may not happen) pairs with different operations on different
paths, one diagnostic is reported, for the best-ranked path; the other
operations are listed in `limitations` ("On other paths a different operation
precedes it ...").

## MW1001: database commit followed by an independent external effect

A write commits (on its own, or with a completed transaction), and afterwards
the function performs an external effect. A failure in between leaves the
database changed without the effect, and nothing records that it is owed.

```ts
await db.update(orders).set({ status: "paid" });
await queue.publish("order.completed", { id });
```

*Mitigation:* transactional outbox (insert the event in the same transaction,
publish from a retrying relay), or reconciliation from database state.
*Primary location:* the external effect.

## MW1002: external effect followed by an independent database commit

An external effect is performed before a write commits. If the write fails or
the process dies, other systems have observed an effect for state that does
not exist.

```ts
await queue.publish("order.completed", { id });
await db.update(orders).set({ status: "paid" });
```

*Mitigation:* commit first and drive the effect from the commit (outbox); if
the effect must come first (charging a card), persist a pending record before
it and reconcile. *Primary location:* the external effect.

## MW1003: external effect executed inside a database transaction before commit

An external effect runs inside a transaction callback that writes. The effect
is visible immediately; the writes become durable only at commit and may still
roll back. Consumers can also read before the commit.

```ts
await db.transaction(async (tx) => {
  await tx.update(orders).set({ status: "paid" });
  await queue.publish("order.completed", { id });
});
```

*Mitigation:* insert an outbox row with `tx` instead, and publish after commit
from a relay. (Moving the call after the transaction turns it into MW1001.)
*Primary location:* the external effect.

## MW1004: independent database commits

Two writes commit independently: outside any transaction, in different
transactions, or a write inside a `tx` callback that uses `db`.

```ts
await db.update(orders).set({ status: "paid" });
await db.insert(payments).values({ orderId: id });
```

*Mitigation:* one transaction, every write on the transaction handle.
*Primary location:* the write that bypasses a transaction if there is one,
otherwise the second commit.

## MW1005: independent external effects

Two configured external effects in sequence; a failure between them performs
only the first.

*Mitigation:* drive them from durable state (outbox, durable workflow) and make
each one idempotent. *Primary location:* the second effect.

## MW9001: analysis incomplete

Not a finding. Midwrite found something next to recognized effects that it
does not understand and makes no claim about it:

* an awaited call matching no configuration entry, after a recognized commit
  or visible effect, or inside a transaction with writes;
* raw SQL (`db.execute(...)`), unmodeled database methods (`db.batch(...)`);
* a transaction whose callback is not inline or whose handle is destructured;
* a member receiver that looks like a configured one but whose path is not
  configured (`this.repo.db` when only `db` and `this.db` are configured);
* a call of a function (same file or imported) that is not summarized: it
  performs no recognized operation, it is recursive, it lies more than
  `--max-call-depth` calls deep, it contains unsupported control flow, it
  writes only through parameters for which the call passes no recognized
  handle, or its import cannot be linked to a function (package import,
  missing module, missing, ambiguous or non-function export; the detail says
  which);
* a database operation of an inlined function on a parameter whose argument
  is not a recognized handle (`unresolvedHandle`);
* unsupported control flow (labeled `break`/`continue`), or more paths than
  `--max-paths`.

*Mitigation:* if the call is a side effect, configure it; otherwise ignore.

## Concurrency model checking (MW7001–MW7102)

Produced only by `midwrite model-check` (a separate category: the codes
above are never reused for concurrency results, and a sequential finding is
never promoted to one). Details:
[model-check guide](user-guide/model-check.md),
[interpreting results](user-guide/interpreting-results.md).

## MW7001: invariant violated by a bounded concurrent schedule

A declared invariant is violated by a schedule of the modeled actors (a
model counterexample). It holds under the scenario's assumptions: schema,
isolation level, actors, bounds. It is not a confirmed application bug.
Replay it with `--validate-postgres`.

## MW7002: model checking inconclusive

A resource limit (states, transitions, steps, wall time, memory) stopped
the search before it covered the declared bounds. Nothing is established
beyond the explored part.

## MW7003: unsupported behavior reached

The scenario reaches an operation the model does not understand (raw SQL,
an unsupported construct). Unknown behavior is never a no-op, so the
scenario cannot be verified.

## MW7101: PostgreSQL did not reproduce the counterexample

Some statement behaved differently on PostgreSQL than the model predicted.
The result must not be reported as a confirmed race.

## MW7102: PostgreSQL validation unavailable

Validation was requested but could not run. The counterexample remains a
model counterexample.

## MW7103: original code did not reproduce the counterexample

With `midwrite model-check --verify-source --allow-execution`,
the actors' original application code ran through an authorized source
harness under the counterexample's schedule against a disposable
PostgreSQL database, and the run did not reproduce the violation: the
application sent different statements (`diverged`), PostgreSQL answered
differently, an expected statement never came (`inconclusive`), the
verification could not run (`blocked`, `unsupported`), or the invariant held
on the observed rows (`not-reproduced`). The counterexample keeps its own
evidence level; `sourceVerification` lists every difference statement by
statement. See [runtime verification](architecture/runtime-verification.md).

## Confidence

Two dimensions are reported separately and combined into `confidence`:

| `pathCertainty` | Meaning |
| --- | --- |
| `straightLine` | The two operations follow each other with no branch decision, loop, unawaited operation or partial summary between them. |
| `conditional` | The sequence depends on branch decisions (listed in `pathConditions`), a loop (zero or one iteration modeled), an unawaited operation or transaction, or an operation a callee performs only on some of its paths. |
| `approximated` | Concurrent operations, or a path entering a catch handler as if the try block failed before its effects. |
| `truncated` | The function has more paths than `--max-paths` (the MW9001 about it). |
| `unknown` | A MW9001 about an operation whose semantics are unknown. |

| `evidenceStrength` (weakest operation) | Meaning |
| --- | --- |
| `provenance` | A transaction callback parameter or a `drizzle(...)` instance. |
| `configuredImport` | Matched a configuration entry with `importedFrom`. |
| `configuredName` | Matched a configured name; where the binding comes from is not checked. |
| `memberPath` | Matched a configured member path (`this.db`, `stripe.charges`); only the names are checked. |
| `unknown` | Not recognized (MW9001). |

| `confidence` | Meaning |
| --- | --- |
| `high` | `straightLine` path and no `memberPath` evidence. |
| `medium` | `conditional` path, or any `memberPath` evidence (a member-path match alone never yields `high`; configured names are not capped, because they are explicit assertions by the user). |
| `low` | `approximated` path, or a MW9001 limitation. |

Branch conditions are checked for simple contradictions only: a path whose
decisions contradict each other on a variable that is never reassigned is
not reported at all; all other conditions are assumed satisfiable together.

## Exit status

| Status | Meaning |
| --- | --- |
| 0 | No diagnostic at or above `--fail-on`, and every file was analyzed. |
| 1 | At least one diagnostic at or above `--fail-on` (default `warning`). |
| 2 | Usage error, invalid configuration, missing path, or unsupported explicit file. |
| 3 | Some files could not be read or parsed, and no diagnostic reached the threshold. |

## JSON output

`midwrite check --format json` prints one object. `schemaVersion` changes on
any incompatible change. All keys are camelCase. Every addition so far was
an optional field, so `schemaVersion` is still 1 (the trace report moved to
schema v2 for the optional `valueRelation`, see
[value analysis](architecture/value-analysis.md#effect-relationships-valuerelation)).
`tool.name` is `midwrite`. Failure traces and verification reports are
separate documents: [trace format](reference/trace-format.md),
[replay protocol](architecture/replay-protocol.md#reports).

```text
Report {
  schemaVersion: 1
  tool: { name: "midwrite", version: string }
  config: string | null                 // configuration file used; null = built-in defaults
  summary: {
    filesDiscovered, filesAnalyzed, filesFailed, functionsAnalyzed: number
    diagnostics: { error, warning, info: number }
    analysis: {                         // measured coverage (not correctness)
      functionsWithEffects, truncatedFunctions, prunedPaths, mergedPaths: number
      sameFileCalls, crossFileCallsResolved, crossFileCallsUnresolved, packageCalls, callsSummarized: number
    }
    baseline?: { new, existing, resolved: number }   // only with --baseline
  }
  diagnostics: Diagnostic[]             // sorted by file, line, column, code
  failures: FileFailure[]               // files not analyzed; sorted by file
}

Diagnostic {
  code: "MW1001" | "MW1002" | "MW1003" | "MW1004" | "MW1005" | "MW9001"
  severity: "error" | "warning" | "info"
  kind: "potentialHazard" | "analysisLimitation"
  title: string                         // from the code catalog
  message: string                       // specific to this instance
  location: SourceLocation              // primary location
  function: string                      // e.g. "completeOrder", "OrderService.complete"
  confidence: "high" | "medium" | "low"
  reproduced: false                     // always false: static findings are never runtime evidence (see midwrite replay)
  effects: RelatedEffect[]
  failureSequence: string[]             // numbered steps, "1. ..."
  pathConditions: PathCondition[]       // branch decisions the reported path takes
  evidence: string[]                    // why each operation was recognized
  mitigation: string
  limitations: string[]                 // instance-specific notes, then the code's general limitation
  pathCertainty: "straightLine" | "conditional" | "approximated" | "truncated" | "unknown"
  evidenceStrength: "provenance" | "configuredImport" | "configuredName" | "memberPath" | "unknown"
  fingerprint: string                   // stable id (16 hex digits, "-N" suffix for repeats); no positions, including anonymous-function names
  baseline?: "new" | "existing"         // only with --baseline
  recovery?: RecoveryNote[]             // recovery declarations from the configuration that bear on this finding
  relationship?: {                      // descriptive only (never changes severity):
    status: "sharedArguments" | "noSharedArgument" | "notComparable",
    shared: string[]                    //   variables both operations' arguments refer to (by name, same function)
  }
  valueRelation?: {                     // descriptive only (never changes severity or existence):
    status: "provenShared" | "possiblyRelated" | "noRelationEstablished" | "unknown",
                                        //   ("provenDisjoint" is reserved and never produced)
    shared?: [{ kind: "derivedFromResult" | "sameValue", first: string, second: string, exact?: true }]
  }                                     //   from abstract value provenance across summarized calls; see architecture/value-analysis.md
}

RecoveryNote {
  effect: string                        // label of the operation the declaration applies to
  declaration: "bestEffort" | "reconciled" | "idempotentConsumer"
  source: "configuration"               // asserted by the user
  verified: false                       // Midwrite never verifies declarations
  effectOnFinding: "informational" | "noteOnly"
}

RelatedEffect {
  role: "completedFirst" | "notYetPerformed" | "stagedWrite" | "transaction" | "unanalyzed" | "context"
      | "callSite"                      // call of a function whose summary supplied an operation
  label: string                         // short source excerpt, e.g. "db.update(orders)"
  description: string
  location: SourceLocation              // operations from a summary keep their own location
}

PathCondition { description: string, location: SourceLocation }

SourceLocation { file: string, line, column, endLine, endColumn: number }   // 1-based; columns count characters

FileFailure {
  file: string
  kind: "read" | "parse"
  message: string
  line: number | null
  column: number | null
}
```

Example (abridged):

```json
{
  "code": "MW1003",
  "severity": "warning",
  "kind": "potentialHazard",
  "title": "external effect executed inside a database transaction before commit",
  "message": "`queue.publish(\"order.completed\", { id })` (message-publish) runs inside the transaction started at 7:9 before it commits; if the transaction rolls back, the external effect remains",
  "location": { "file": "vulnerable/publish_inside_transaction.ts", "line": 9, "column": 11, "endLine": 9, "endColumn": 51 },
  "function": "completeOrder",
  "confidence": "high",
  "reproduced": false,
  "effects": [
    { "role": "transaction", "label": "db.transaction(…)", "description": "transaction; its writes become durable only when it commits", "location": { "...": "..." } },
    { "role": "stagedWrite", "label": "tx.update(orders)", "description": "database update on `orders` (staged in the transaction)", "location": { "...": "..." } },
    { "role": "completedFirst", "label": "queue.publish(\"order.completed\", { id })", "description": "external effect (message-publish); visible immediately", "location": { "...": "..." } }
  ],
  "failureSequence": [
    "1. The transaction started by `db.transaction(…)` (7:9) stages its writes without committing them: `tx.update(orders)` (8:11).",
    "2. `queue.publish(\"order.completed\", { id })` (9:11) performs an external effect (message-publish) while the transaction is still open.",
    "3. The transaction then fails to commit: ... Its staged writes roll back.",
    "4. Result: the external effect is visible while the transaction's writes may still roll back; consumers may also act before the data is committed."
  ],
  "pathConditions": []
}
```

## SARIF output

`--format sarif` emits SARIF 2.1.0 with one rule per code, one result per
diagnostic (`relatedLocations` are the related effects, possibly in other
files; `partialFingerprints["midwrite/v1"]` is the fingerprint;
`baselineState` is `new` or `unchanged` with `--baseline`; `properties` carry
`function`, `confidence`, `pathCertainty`, `evidenceStrength`, `kind`,
`reproduced` and `limitations`), and parse or read failures as tool execution
notifications.

Source paths are URI-escaped, including related-effect locations. Absolute
paths use `file:` URIs; the invocation records the working directory so
consumers can resolve them against the checkout. Lines and columns are
1-based, end columns are exclusive, and `columnKind: "unicodeCodePoints"`
matches the analyzer's Unicode character columns.

For an upload workflow, see [GitHub code scanning](user-guide/check.md#github-code-scanning).
