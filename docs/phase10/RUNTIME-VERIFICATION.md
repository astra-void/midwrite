# Original-code verification matrix (Phase 10)

All runs execute **first-party** applications written for this purpose
(`fixtures/phase10/ledger-live` on node-postgres 8.23.1,
`fixtures/phase10/ledger-pgjs` on postgres-js 3.4.5, both with Drizzle ORM
0.45.4), with `--allow-execution`, each run on its own disposable PostgreSQL
cluster in a private directory, through the test-only proxy. No external
application was executed.

Command (`scripts/phase10-evidence.py OUT N`): every scenario of
`fixtures/phase10/scenarios`, `model-check --verify-source --allow-execution
--source-repeat N`, reports saved per scenario with a `SUMMARY.md`. A scenario
counts only when **all** N runs were followed and matched the model (for a
counterexample: reproduced, invariant violated on the observed state; for a
probe: recovery reproduced, invariant held). Final numbers:
`reports/phase10/evidence/runtime-final/SUMMARY.md` (macOS) and
`reports/phase10/linux/phase10-runtime/SUMMARY.md` (Linux).

## Coverage of the dimensions

The Cartesian product was not run; the cases below were chosen so that each
value of each dimension is exercised by at least one case where it matters.

| dimension | values covered | cases |
| --- | --- | --- |
| driver | node-postgres 8.23.1, postgres-js 3.4.5 | 13 + 9 |
| fault | connection broken under a live process (answered-and-withheld, not-reached, held-for-later); process killed with the operation in flight | 19 + 3 |
| operation | COMMIT, autocommit INSERT | 18 + 4 |
| process | one task per process; two tasks of one process (pool of 2) | 21 + 1 |
| recovery | blind retry in the process, unique request key, reconciliation (look-then-insert); the three crash cases retry in a new process | 14 + 4 + 4 (3 of them in a new process) |
| database isolation | READ COMMITTED; REPEATABLE READ; SERIALIZABLE | 19; 1; 2 |
| pool | default (10), one connection, two connections | |
| driver configuration | client error handler attached; none (the process dies) | 21; 1 |

Not run: postgres-js with a bounded pool; REPEATABLE READ / SERIALIZABLE on
postgres-js; a shared process on postgres-js; a lock wait or deadlock
together with a connection loss; external applications.

## What each case records

Model verdict and counterexample schedule; the plan executed against the
original code; per statement the model's operation, its source location and
the SQL Drizzle sent; the wire events (redacted); the loss or crash evidence
(fate model vs run, server answer, backend state, session end, the driver's
error, the worker's survival, statements on the broken client, connections
used afterwards); PostgreSQL's durable rows after the loss and at the end;
each actor's outcome; the invariant on the observed state; the evidence level.

## Results (macOS, five runs per scenario)

The first full campaign (18 scenarios, `reports/phase10/evidence/runtime/`)
and the final campaign (22 scenarios with the final binary,
`reports/phase10/evidence/runtime-final/`) are summarized in `FINAL-REPORT.md`
§Runtime. In both, every scenario expected to reproduce did so in 5 of 5
runs, and `live-blind-retry-unhandled-error-event` diverged in 5 of 5 runs
with the driver's unhandled `error` event quoted.

Repetition matters for the held-for-later cases (the COMMIT reaches the
server only after the application's next step): their order is enforced by
the coordinator (the held frames are delivered only at the planned step), so
the repetitions test that the enforcement holds, not that a race was lucky.

## Reading the results

* A reproduction is one schedule of one first-party application, run N times.
  It shows that the model's counterexample is real for this code and these
  driver versions; it says nothing about other schedules, versions or
  applications.
* "Recovery reproduced" (a probe) shows one schedule with the fault in which
  the invariant held on the original code; it is not a proof that the
  recovery is correct in general — the model's bounded search is the broader
  (still bounded) claim.
* The driver's error is evidence of what the application was told, not of
  what PostgreSQL did; the physical outcome comes from the proxy and
  PostgreSQL only.
