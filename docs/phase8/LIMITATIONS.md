# Phase 8 limitations

Everything Phase 7 lists (`docs/phase7/LIMITATIONS.md`) still holds unless
it is marked resolved below.

## Resolved by Phase 8

* Crash, retry and redelivery schedules against original code (real
  SIGKILL, new process per execution, PostgreSQL session termination
  observed).
* External effects at runtime (coordinator-hosted test doubles).
* postgres-js source verification (version 3.4.5).
* Schema facts from Drizzle definitions, compared with scenarios and with a
  database catalog.
* Inliner expansions shared across depths; path summaries shared across
  call instances.
* Worker resource limits; detached processes are found and killed.

## Crash and recovery

* A crash is planned only at observable checkpoints: an announced-and-held
  operation, or a statement waiting for a lock. A kill while a released
  statement runs (a COMMIT whose acknowledgment is lost in flight) is not
  planned: the model has no such transition, and its outcome would be
  ambiguous. "Lost acknowledgment" is represented as a crash after the
  operation and before the next one.
* An autocommit statement whose lock was released in the model but not yet
  resumed is refused (PostgreSQL may have committed it).
* Crashes while several tasks of one actor run (`Promise.all`) are refused.
* The model releases a crashed transaction's locks at once; PostgreSQL
  does so only if the backend notices the lost client. Source verification
  sets `client_connection_check_interval = 50ms`; on a default server a
  crashed transaction can keep its locks (measured: > 10 s).
* Retries other than the application's own transaction-retry loop are
  played by the coordinator under a **declared** mechanism (client,
  queue, supervisor). They are contracts, not evidence about a real queue
  or client. Retrying a single SQL statement is not expressible.
* Actors declared in the same process (shared memory, crash together) are
  not supported for source verification (one process per actor).
* The transaction-retry helper is bound by a **declared** contract
  (`extraction.transactionRetries`); its loop is not analyzed statically
  (it runs during verification, which checks the contract on the schedule).
* Async call-chain restoration (`return helper()` without `await`) is
  unambiguous-only and per execution; an ambiguous chain is left as is
  (the statement may then be classified outside the model → inconclusive).
* Recovery probes run one schedule (the shortest with the required
  events); they are bounded evidence, not proofs of safety. Example: the
  idempotent checkout holds under one request with a crash and a retry,
  and fails with two concurrent requests.

## External effects

* Effects must reach a provider seam the harness can implement; effects
  issued by libraries the harness cannot replace are not observed.
* The double's deduplication (`none`, `byKey`) is declared; key expiry,
  partial failures of providers, and provider-side retries are not modeled.

## postgres-js

* Coordination relies on an internal of postgres-js
  (`Query.prototype.handle`); other versions are refused until verified.
* Driver-internal catalog reads are not held (reported).
* Lock waits are attributed by `application_name`; with several concurrent
  statements of one execution a wait is not attributed.
* Cursors, `forEach`, streams: unsupported. Savepoints: unsupported in the
  model. Pipelined `sql.begin` arrays and `sql.reserve()` were not
  exercised.
* The application must resolve the same `postgres` module instance as the
  harness (one `node_modules`); a loader hook for other layouts was not
  built.

## Schema extraction

* Only literal declarations and the documented builders; dynamic names,
  spread columns, helpers that build columns, `casing` options and custom
  types are reported as unsupported, not extracted.
* CHECK expressions and partial-index predicates are kept as text: never
  interpreted, compared by presence/count only.
* Foreign keys, CHECK constraints, enum domains, expression and
  application defaults are not modeled; the report says so per fact.
* `NULLS NOT DISTINCT` on nullable columns is not modeled.

## Summaries and semantic facts

* Semantic-fact migration stopped at recognition (chains, keys, paths);
  values, transaction handles and completion semantics remain per frontend.
* Path-summary sharing is within one function's analysis; summaries are
  not shared across functions.
* Lowering and resolution costs (≈ 60 % of midday) are unchanged.

## Scale

* Model-checking state space still grows ≈ 7× per actor; 6 actors of the
  team-app action take ≈ 3.8 s and ≈ 740 MiB (visited set).
* Memory is not reduced by the Phase 8 optimizations.

## Platform and evidence

* PostgreSQL 17.11 (macOS) and 15.19 (Linux container); 16 and 14 were not
  run. x86_64 was not available.
* The Linux run is a container on the same Apple Silicon machine, not
  separate hardware.
* The Phase 6 oracle test `deadlock_workloads` failed once on Linux (the
  second full container run): PostgreSQL chose the other transaction as
  the deadlock victim than the model's. The oracle steers the victim by
  timing (the victim's statement is sent 1.5 × `deadlock_timeout` after the
  first waiter is observed); a late first waiter can run detection itself.
  It passed in the next full run and in 20 of 20 targeted repetitions
  (10 alone, 10 with its sibling tests; `reports/phase8/linux-deadlock-repeat/`),
  and in every macOS run. Not fixed in Phase 8 (Phase 6 code, unchanged).
* `the_html_report_escapes_crash_recovery_and_schema_fields` reads a
  generated report (`reports/phase8/model-check/checkout-crash-after-charge.json`);
  if the evidence run that writes it was blocked, the test fails until the
  report is regenerated.
* No independent human review; review packets are unreviewed.
* The external saas-starter application was not executed; its Phase 6/7
  finding remains a model counterexample confirmed only for model SQL.
