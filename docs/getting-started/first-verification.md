# First verification: from a candidate to evidence

This walkthrough uses first-party fixtures of this repository. Steps 1–3
only read files and explore a model in memory. Steps 4 and 5 start a
private PostgreSQL and, in step 5, execute application code; they are
described but were **not run** for this documentation.

Outputs were produced by the release binary built from the current tree on
2026-10-27 (`commitrift 0.1.0`, Phase 13 working tree, pre-Phase 14),
with `--no-timing` so that they are reproducible.

The workflow and what each step can claim:
[verification workflow](../user-guide/interpreting-results.md#the-verification-workflow),
[evidence levels](../concepts/evidence-levels.md).

## 1. Model-check a declared scenario

A scenario names actors (functions extracted from TypeScript, or declared
programs), their arguments, a schema with initial rows, an isolation level,
bounds and invariants ([format](../user-guide/model-check.md#scenario-files)).
`fixtures/concurrency/scenarios/inventory-oversell.json` runs two
`purchase` requests for the last unit of a product:

```text
$ commitrift model-check fixtures/concurrency/scenarios/inventory-oversell.json --no-timing
inventory-oversell: COUNTEREXAMPLE_FOUND
  isolation read-committed · 2 actor(s) · 11 state(s), 10 transition(s) (sleep-set reduction on)
...
Schedule (6 transitions, 2 context switches, minimized and replayed):

    1  B        SELECT products WHERE (@id = "product-1") → {id="product-1", stock=1}    [src/purchase.ts:8:25]
    1  B        CHECK (!_row1 or (_row1.stock <= 0)) → false    [src/purchase.ts:12:7]
    2  A        SELECT products WHERE (@id = "product-1") → {id="product-1", stock=1}    [src/purchase.ts:8:25]
    2  A        CHECK (!_row1 or (_row1.stock <= 0)) → false    [src/purchase.ts:12:7]
    3  A        UPDATE products SET stock = 0 WHERE (@id = "product-1") → 1 row    [src/purchase.ts:16:9]
    4  A        INSERT orders {id="order-A", product="product-1"} → 1 row    [src/purchase.ts:21:9]
    4  A        return (success)
    5  B        UPDATE products SET stock = 0 WHERE (@id = "product-1") → 1 row    [src/purchase.ts:16:9]
    6  B        INSERT orders {id="order-B", product="product-1"} → 1 row    [src/purchase.ts:21:9]
    6  B        return (success)
...
Invariant `successful-purchases-within-initial-stock` (final state):
  purchase:ok <= initial(products[id="product-1"].stock)
    purchase:ok = 2
    initial(products[id="product-1"].stock) = 1

Result:
  VIOLATED

Evidence: ModelCounterexample — a schedule found by the model checker; it has not been run on a real database
...
warning CP7001: invariant `successful-purchases-within-initial-stock` can be violated by a bounded schedule of 6 transitions (2 context switches); evidence: model-counterexample
```

Exit status 1. The same purchase with `SELECT … FOR UPDATE`
(`inventory-row-lock.json`) exits 0:

```text
$ commitrift model-check fixtures/concurrency/scenarios/inventory-row-lock.json --no-timing
inventory-row-lock: NO_COUNTEREXAMPLE_WITHIN_BOUNDS
  isolation read-committed · 2 actor(s) · 30 state(s), 34 transition(s) (sleep-set reduction on)

Evidence: None — no invariant violation exists in any behavior within the declared bounds (this does not prove the application race-free)
```

Invariants can also read what callers were told (Phase 13). In
`fixtures/phase13/request-app/scenarios/claim-race.json` two `claimTask`
calls both return `true`; the table alone ends in a legal state:

```text
$ commitrift model-check fixtures/phase13/request-app/scenarios/claim-race.json --no-timing
claim-race: COUNTEREXAMPLE_FOUND
...
Invariant `at-most-one-caller-told-it-claimed` (final state):
  ok returning true <= 1
    ok returning true = 2
```

## 2. Discover candidates without writing a scenario

`commitrift discover` indexes a project, pairs effects that may interact,
attaches a property whose origin is always stated, synthesizes initial
states and scenarios, and with `--model-check` explores them. It never
executes application code.

```text
$ commitrift discover fixtures/phase13/eval-app --model-check --format text --no-timing
commitrift discover: eval-app (26 modules, 41 entry candidates, 35 extracted, 22 effectful)
mode: discover+model-check; application code executed: no; search: sleep-set partial-order reduction (default)
candidates: 13 (5 model counterexamples, 1 against hypotheses; 3 deduplicated findings)

Candidate CR-SYNTH-95604C  [MODEL_COUNTEREXAMPLE]
  concurrent check-then-insert on `teams` (ownerId = $ownerId)
  Entry:
    src/services/teams.ts#createTeamShared (exported-function, direct)
  Conflict:
    [check] SELECT teams WHERE ownerId = $ownerId  src/services/teams.ts:33:30
    [act] INSERT teams  src/services/teams.ts:35:29
  Resource: teams(ownerId = $ownerId)
  Property: A user owns at most one team (product rule).
    origin: USER_DECLARED
    why: declared in commitrift-contracts.json
  Scenario actual [Actual]: synthesis satisfied — COUNTEREXAMPLE_FOUND (16 states, violates `one-team-per-owner`)
  Scenario distinct-key [Control]: synthesis satisfied — NO_COUNTEREXAMPLE_WITHIN_BOUNDS (40 states)
  Status: MODEL_COUNTEREXAMPLE
...
Important: a model counterexample against a HEURISTIC_HYPOTHESIS is not a confirmed defect; a bounded run without counterexample is not a proof.
```

The property's origin decides what a counterexample means
([invariants](../concepts/invariants.md)): here it is `USER_DECLARED` in
`fixtures/phase13/eval-app/commitrift-contracts.json`. Without
`--model-check` the same command builds the scenarios and stops
(`STATIC_CANDIDATE`). The 13 candidates and 5 counterexamples above equal
the `post-feedback-1` scoring recorded in
`reports/phase13/eval/post-feedback-1.json`.

To keep the synthesized scenarios and re-run one by hand:

```bash
commitrift discover fixtures/phase13/eval-app --emit-scenarios /tmp/scenarios
commitrift model-check /tmp/scenarios/<id>-actual.json
```

## 3. Explain and render

```bash
commitrift explain CP7001
commitrift model-check SCENARIO.json --format html -o report.html   # script-free HTML timeline
commitrift discover ROOT --model-check --format html -o discovery.html
```

## 4. Optional: replay the model's SQL on PostgreSQL

```bash
commitrift model-check fixtures/concurrency/scenarios/inventory-oversell.json --validate-postgres
```

Starts a private, disposable PostgreSQL cluster (Unix socket only) and
replays the counterexample's statements in lockstep, one session per actor.
It never runs application code. A matching replay is
`PostgresConfirmedSchedule`; a mismatch is `CP7101`; a missing server is
`CP7102`. Needs the PostgreSQL server binaries. Not run for this page.

## 5. Optional: run the counterexample against the original code

```bash
(cd fixtures/phase7/team-app && npm ci --ignore-scripts && npm run build)   # as scripts/phase12-linux-docker.sh does
commitrift model-check fixtures/phase7/scenarios/team-app-invite-double-submit.json \
  --verify-source --allow-execution --source-repeat 5
```

This **executes** the first-party application through its authorized
harness (`commitrift-source.json`), one isolated worker per actor, holding
and releasing each SQL statement Drizzle sends in the counterexample's
order against a disposable PostgreSQL. The best result is
`original-code-reproduced` (or `original-code-crash-reproduced` for crash
and retry schedules); a disagreement is reported statement by statement
(`CP7103`). Requirements, guarantees and limits:
[source verification](../user-guide/source-verification.md). Not run for
this page; the recorded runs of this fixture are in
[Phase 7](../phase7/FINAL-REPORT.md) (5 of 5 runs, `original-code-reproduced`).

## What you have and have not shown

* Steps 1–2: a schedule of the **model** violates a property under the
  scenario's assumptions. Not a confirmed bug.
* Step 4: PostgreSQL behaves as the model predicts for that schedule of the
  model's SQL.
* Step 5: the original code, under that schedule, on a disposable database
  with the harness's inputs, violates the property. It does not show that
  the schedule happens in production.

Next: [interpreting results](../user-guide/interpreting-results.md),
[limitations](../reference/limitations.md).
