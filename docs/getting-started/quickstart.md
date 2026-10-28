# Quickstart: static analysis

`commitrift check` reads TypeScript and reports places where a crash,
restart, timeout or failed call between two independently committed side
effects can leave partial state. It never executes code.

The outputs below were produced by the release binary built from the
current tree on 2026-10-27 (`commitrift 0.1.0`, Phase 13 working tree,
before any Phase 14 change), run from the repository root.

## 1. Check one file

`fixtures/vulnerable/db_commit_then_publish.ts` (imports omitted; the line
numbers in the output refer to the file):

```ts
export async function completeOrder(orderId: string) {
  await db.update(orders)
    .set({ status: "paid" })
    .where(eq(orders.id, orderId));
  await queue.publish("order.completed", { orderId });
}
```

```text
$ commitrift check fixtures/vulnerable/db_commit_then_publish.ts
warning[CP1001]: database commit followed by an independent external effect
  --> fixtures/vulnerable/db_commit_then_publish.ts:12:9 (in `completeOrder`, confidence: high)
     |
  12 |   await queue.publish("order.completed", { orderId });
     |         ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  = `db.update(orders)` commits before `queue.publish("order.completed", { orderId })` (message-publish); a failure in between leaves the database changed without the external effect
  failure sequence:
    1. `db.update(orders)` (9:9) commits immediately: it is not inside a recognized transaction.
    2. The process crashes or is restarted, or the call fails, after the commit and before `queue.publish("order.completed", { orderId })` completes.
    3. `queue.publish("order.completed", { orderId })` is never performed.
    4. Result: the database change is durable but the external effect never happens, and nothing records that it is still owed.
  help: Record the intent in the same database transaction as the state change (transactional outbox) and perform the external effect from a relay that retries until it succeeds, or make a reconciliation job derive the missing effect from database state.

commitrift: analyzed 1 of 1 file(s), 1 function(s); 1 warning(s), 0 error(s), 0 info; config: built-in defaults
Findings are potential hazards, not reproduced failures. Run `commitrift explain <CODE>` for details.
```

Exit status 1: a diagnostic at or above `--fail-on warning` exists. The
built-in defaults recognize `db` as the database and `queue.publish` as an
external effect; a real project needs a [configuration](../reference/configuration.md).

## 2. Read the explanation and the trace

```bash
commitrift explain CP1001      # documentation of the code
commitrift explain             # list of codes
```

```text
$ commitrift trace fixtures/vulnerable/db_commit_then_publish.ts
tr-a812981794e25ea1 CP1001 4e50509bec00ca97 in `completeOrder` (fixtures/vulnerable/db_commit_then_publish.ts:12:9) [STATIC_CANDIDATE; Complete]
    0. dbWrite     Committed   db.update(orders) @ 9:9 <- completes first
       ---- failure injection boundary (after-db-commit / before-event-publish) ----
    1. external    NotReached  queue.publish("order.completed", { orderId }) @ 12:9 <- not yet performed
  simulated: If the process dies right after step 0: durable or visible: `db.update(orders)` (step 0); never performed: `queue.publish("order.completed", { orderId })` (step 1).

commitrift: 1 finding(s): 1 trace(s) (1 complete, 0 approximated, 0 incomplete), 0 without a trace. Traces are static candidates, not reproduced failures.
```

A trace is a static candidate, never runtime evidence
([evidence levels](../concepts/evidence-levels.md)).

## 3. Check a project

```bash
commitrift check .                         # every .ts/.tsx/.mts/.cts below ., one project
commitrift check src --format json         # JSON report (docs/DIAGNOSTICS.md#json-output)
commitrift check src --format sarif        # SARIF 2.1.0
commitrift check . --write-baseline commitrift-baseline.json   # once
commitrift check . --baseline commitrift-baseline.json         # in CI: fail only on new findings
```

On the repository's own fixtures: `commitrift check fixtures/vulnerable`
reports `analyzed 36 of 36 file(s), 87 function(s); 37 warning(s), 0
error(s), 15 info`, and `commitrift check fixtures/safe` reports `0
warning(s), 0 error(s), 1 info` (exit 0).

## Next

* [User guide: `check`](../user-guide/check.md) — policies, baselines,
  options, what the analysis follows.
* [First verification](first-verification.md) — concurrency and crash
  scenarios: discovery, model checking, optional runtime verification.
* [Interpreting results](../user-guide/interpreting-results.md).

## Discover concurrency and retry candidates (Phase 14)

`commitrift discover` indexes a project's entry points and database
effects, proposes candidates, synthesizes scenarios and, with
`--model-check`, explores them. It never executes application code. The
output below was produced by the Phase 14 release binary (2026-10-28) on the
first-party fixture `fixtures/phase14/request-app`, from the repository root:

```bash
commitrift discover fixtures/phase14/request-app --model-check --entry payments.ts
```

```text
commitrift discover: request-app (7 modules, 8 entry candidates, 2 extracted, 2 effectful)
mode: discover+model-check; application code executed: no; search: sleep-set partial-order reduction (default)
candidates: 2 (1 model counterexamples, 0 against hypotheses; 1 deduplicated findings)

Candidate CR-SYNTH-232519  [MODEL_COUNTEREXAMPLE]
  write to `payments` repeated after an unknown outcome
  Entry:
    src/services/payments.ts#settlePayment (exported-function, direct)
  Conflict:
    [first-write] INSERT payments  src/services/payments.ts:11:13
    [retried-write] INSERT payments  src/services/payments.ts:11:13
  Resource: payments(orderId, amount)
  Property: An order is paid at most once.
    origin: USER_DECLARED
    why: declared in commitrift-contracts.json
  Scenario actual [Actual]: synthesis satisfied — COUNTEREXAMPLE_FOUND (11 states, violates `one-payment-per-order`)
  Status: MODEL_COUNTEREXAMPLE
  MODEL_COUNTEREXAMPLE: a schedule within the bounds violates `one-payment-per-order` (USER_DECLARED). The property is declared by the user; the counterexample replays in the model.
  Next step: write the scenario with `--emit-scenarios <DIR>`, read the counterexample with `commitrift model-check <DIR>/<file>`; with an authorized harness, reproduce it on the original code with `--verify-source --allow-execution` (docs/user-guide/source-verification.md)

Candidate CR-SYNTH-959BBA  [NO_VIOLATION_WITHIN_BOUNDS]
  ...
  Next step: nothing to do within these bounds; a bounded search is not a proof (raise `--max-states` for a larger search)
```

The candidate says where (the INSERT inside the callback that the
application's own `withRetry` runs twice), what is shared (`payments`),
which property (declared in the project's `commitrift-contracts.json`) and
what to do next. The exit status is 0: discovery reports, it does not fail
a build. Follow the next step to see the schedule:

```bash
commitrift discover fixtures/phase14/request-app --model-check --entry payments.ts#settlePayment --emit-scenarios /tmp/commitrift-quickstart
```

```bash
commitrift model-check /tmp/commitrift-quickstart/cr-synth-232519-actual.json
```

```text
Schedule (9 transitions, 0 context switches, minimized and replayed):

    1  settlePayment BEGIN ISOLATION LEVEL READ COMMITTED    [src/services/payments.ts:10:5]
    2  settlePayment INSERT payments {amount=3, orderId=1} → 1 row    [src/services/payments.ts:11:13]
    3  settlePayment UPDATE orders SET status = "paid" WHERE (@id = 1) → 0 rows    [src/services/payments.ts:12:13]
    4  settlePayment COMMIT submitted; it reaches PostgreSQL, which will complete it; the acknowledgment never reaches the application (connection lost)    [src/services/payments.ts:10:5]
    4  settlePayment caught: connection terminated unexpectedly: the outcome of COMMIT is unknown
    5  settlePayment the server completes the COMMIT it received: committed
    6  settlePayment BEGIN ISOLATION LEVEL READ COMMITTED    [src/services/payments.ts:10:5]
    7  settlePayment INSERT payments {amount=3, orderId=1} → 1 row    [src/services/payments.ts:11:13]
    ...
Invariant `one-payment-per-order` (final state):
  count(payments[orderId=1]) <= 1
    count(payments[orderId=1]) = 2
```

(Local steps are abridged here.) This is a **model** counterexample. To
run the application's original code under this schedule against a
disposable PostgreSQL, the project needs an authorized harness (this
fixture has one) and an explicit `--verify-source --allow-execution`:
[first verification](first-verification.md) and
[source verification](../user-guide/source-verification.md). For this
fixture that run reproduces the duplicate payment (`docs/phase14/FINAL-REPORT.md`),
under the stated assumption that the server handles node-postgres client
errors.

A project without `commitrift-contracts.json` still gets candidates, but
their properties are hypotheses (`HEURISTIC_HYPOTHESIS`): a counterexample
against one is not a confirmed defect, and properties about what callers
are told (return values) are never inferred — declare them
([contracts](../user-guide/contracts.md)).

## Explore input worlds and save a counterexample (Phase 15)

Validated with the Phase 15 binary on the first-party application
`fixtures/phase15/worlds-app`, whose defects appear only for some valid
inputs: a root folder (`parentId = null`) and a quota with one export left.

1. Build (once): `cargo build --release`.
2. Analyze and model-check, listing every explored world:

   ```bash
   target/release/commitrift discover fixtures/phase15/worlds-app --model-check --entry createFolder --explain-coverage
   ```

   The candidate's property is `USER_DECLARED` (from the app's
   `commitrift-contracts.json`). The synthesized world (`base`,
   `parentId = 1`) holds; the world `null:parentId` — `null` is allowed by
   the signature `parentId: number | null` — violates the property:

   ```text
   Input exploration (relationship-search): 3 world(s) model-checked, controls included — actual configuration: 1 distinct row set(s), 2 distinct argument vector(s)
     world `base` (actual, Base): explored — NO_COUNTEREXAMPLE_WITHIN_BOUNDS
     world `null:parentId` (actual, Domain): explored — COUNTEREXAMPLE_FOUND
     world `base` (distinct-key, Base): explored — NO_COUNTEREXAMPLE_WITHIN_BOUNDS
   Schedule exploration (sleep-sets): completed for 2 world(s), truncated for 0, unsupported for 0
   Branch outcomes reached alone: 5 of 8
   Input domain exhaustive: no (every derived relationship class was explored)
   Status: MODEL_COUNTEREXAMPLE
   ```

   The exit status is 1 when a model counterexample is reported.
3. Save the scenarios and the reproduction artifact:

   ```bash
   target/release/commitrift discover fixtures/phase15/worlds-app --model-check --emit-scenarios /tmp/cr-worlds
   ```

   `/tmp/cr-worlds/cr-synth-438686-actual--null-parentid.counterexample.json`
   holds the world, rows, arguments, assumptions, bounds, schedule and
   source fingerprint.
4. Replay it in the model:

   ```bash
   target/release/commitrift model-check /tmp/cr-worlds/cr-synth-438686-actual--null-parentid.json
   ```

5. Optionally, with explicit authorization, run it against the original
   TypeScript on a disposable PostgreSQL (Node 22 and PostgreSQL 15+
   installed; the fixture's `dist/` is built with `npm run build`):

   ```bash
   target/release/commitrift model-check /tmp/cr-worlds/cr-synth-438686-actual--null-parentid.json --verify-source --allow-execution --source-repeat 3
   ```

   Result in this cycle: `original-code-reproduced`, 3/3.

A result without a counterexample reads, for example: "No violation found
within the modeled bounds … Input exploration: no counterexample among 2
explored initial world(s) of the actual configuration … The input domain
was not exhaustively explored; general application safety is not
established."
