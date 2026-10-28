# Roadmap: open problems

This page collects the open items that the phase reports themselves name.
It is a list of known gaps, not a schedule or a commitment. Phase 14 is in
progress; its plan and results will be recorded by the lead engineer in its
own phase documents, and the [limitations reference](../reference/limitations.md#phase-14-changes)
will be updated at the end of Phase 14.

## Source coverage (blocks recall on real code)

From the Phase 13 frozen evaluation set and corpus
([evaluation](../phase13/EVALUATION.md), [advanced Drizzle § 4](../phase13/ADVANCED-DRIZZLE.md#4-not-done-in-phase-13),
[source resolution](../phase13/SOURCE-RESOLUTION.md#not-implemented-with-the-reason),
[school-ledger coverage](../phase13/SCHOOL-LEDGER-COVERAGE.md)):

* multi-row `values([...])` (atomicity, per-row defaults and conflicts
  need a pending statement with a row cursor);
* timestamp, JSON and array columns; arrays, `Map`/`Set` and string
  building in the value layer;
* higher-order wrappers with unknown callback argument mapping
  (`withRetry(() => …)`), inherited methods;
* a detection family pairing a parent `DELETE` with a child `INSERT`;
* return-value properties: today they must be declared; none is inferred;
* opaque fresh values (bcrypt hashes) and declared validity assumptions
  (zod) so that package calls with verified contracts can be represented;
* row values of `UPDATE`/`DELETE … RETURNING`.

## Model and PostgreSQL semantics

* `ON DELETE CASCADE`/`SET NULL`/`SET DEFAULT`, deferred constraints,
  `SET CONSTRAINTS` (refused today) ([Phase 13 FK locks § 5](../phase13/POSTGRESQL-FK-LOCKS.md#5-not-modeled-refused-never-approximated));
* several replicas with different lags, synchronous replication, and a
  measured counterpart for the model-only replica results
  ([Phase 13 replica consistency](../phase13/REPLICA-CONSISTENCY.md));
* server-side timeouts as fates of a held COMMIT
  ([Phase 9 § 12](../phase9/FINAL-REPORT.md#12-remaining-research-problems));
* pool generators for random differential testing and start-up arrival
  orders (same);
* a correctness argument for the reductions: the summary argument, source
  sets with sleep sets under conditional enabling, generalization of the
  audited relation, completeness of the canonical state
  ([Phase 10 POR correctness § 6](../phase10/POR-CORRECTNESS.md));
* more PostgreSQL versions (16, 14 not run) and plan shapes; SERIALIZABLE
  beyond the one-way abstraction.

## Runtime verification

* Drivers beyond node-postgres 8.23.1 and postgres-js 3.4.5; postgres-js
  with pools, REPEATABLE READ/SERIALIZABLE and shared processes
  ([Phase 10 limitations](../phase10/LIMITATIONS.md#postgres-js)).
* Pool capacity > 1, application wait cycles and real deadlocks on
  original code.
* Connection loss beyond a closed socket for a COMMIT or autocommit write.
* Phase 13 runtime evidence on Linux.

## Evidence and release

* **Independent human review** of findings, labels and semantics (none has
  been performed in any phase).
* An authorized reproduction in an external application with human review
  ([Phase 4 final report](../phase4/FINAL-REPORT.md),
  [Phase 9 § 12](../phase9/FINAL-REPORT.md#12-remaining-research-problems)).
* Release blockers recorded in Phase 4: Git history and a verified remote; a
  green GitHub CI run including required Linux replay; a tagged release
  run; a security review; a human-reviewed precision baseline; crates.io
  and npm packaging decisions ([Phase 4 release readiness](../phase4/RELEASE-READINESS.md)).
  A local Git history exists since the start of Phase 7 (first commit
  `72c789e`, "import CommitRift through Phase 6"); the phase reports record
  no GitHub CI run, no push and no published package.
* Windows and x86_64 runs.

## Documentation

* Keep this documentation layer current: update
  [limitations](../reference/limitations.md),
  [evidence index](../research/evidence-index.md) and the
  [CLI reference](../reference/cli.md) at the end of each phase, and run
  `python3 scripts/check-doc-links.py --historical report`.
