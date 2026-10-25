# Phase 6 limitations

What the model checker does not do, gets wrong, or only approximates.
General CommitRift limitations: [`../LIMITATIONS.md`](../LIMITATIONS.md).

## What a result means

* `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` covers the declared actors, arguments,
  initial rows, isolation level, crashes, retries and context switches —
  nothing else. It does **not** prove an application race-free: other
  inputs, more actors, longer schedules, other operations or other
  deployments are not covered.
* `COUNTEREXAMPLE_FOUND` is a possible execution of the *model*. It becomes
  evidence about PostgreSQL only after `--validate-postgres` confirms the
  schedule, and evidence about the *application* never in Phase 6: the
  original code is not run, and whether the requests really run concurrently
  in production is not known.
* Invariants are declared by people; a wrong invariant gives a meaningless
  verdict. None is inferred.

## Language subset (TypeScript extraction)

Supported subset: [`CONCURRENT-IR.md`](CONCURRENT-IR.md). Not supported
(lowered to `Unknown`, making the result `UNSUPPORTED` if reachable): loops,
`try`/`catch`/`finally` (no exception handling inside an actor), `switch`,
`break`/`continue`, closures other than transaction callbacks and
`Promise.all` elements, `.then` chains, timers, generators, functions
wrapped by higher-order helpers (`validatedAction(…, async () => …)`), class
methods, recursion, calls beyond the inline depth (3), dynamic imports,
`??`, raw SQL (`db.execute`, `sql` beyond simple arithmetic in `SET`),
`findMany`, joins, multi-row `values`, `.returning()` contents other than
its length, select arrays without `.limit(1)`, projections (the full row is
read), `orderBy` (rows are taken in primary-key order — an assumption the
report lists). Column names must equal property names.

## SQL and database subset

* Statements: point/predicate `SELECT … LIMIT 1` (primary-key order),
  `count(*)`, `UPDATE`, `INSERT` (one row, `ON CONFLICT DO NOTHING`),
  `DELETE`, `FOR UPDATE` (+ `SKIP LOCKED`, `NOWAIT`). Not modeled: joins,
  subqueries, aggregates other than `count`, `ORDER BY` other than the key,
  `LIMIT` other than 1, `RETURNING` values, `ON CONFLICT DO UPDATE`,
  `FOR SHARE` / `FOR NO KEY UPDATE` / `FOR KEY SHARE`, sequences and
  `serial`/default values, generated columns, foreign keys, triggers,
  database functions, `NOW()`, savepoints and nested transactions,
  `lock_timeout`/`statement_timeout`, advisory locks, `LISTEN`/`NOTIFY`.
* Constraints: primary keys, unique (no partial or expression indexes),
  NOT NULL, simple CHECK. Types: the model is untyped (integers, strings,
  booleans, null); no decimals, timestamps, JSON or arrays; integer overflow
  is an error.
* **Statement atomicity**: a statement runs atomically unless it waits.
  Interleavings between a statement's snapshot and its row visits are not
  explored; for single-row statements the outcomes coincide with explored
  orders, for multi-row statements they may not (a multi-row `UPDATE` that
  races with a commit in the middle of its scan).
* Plans are not modeled; PostgreSQL's locking side effects of plans (e.g.
  SIREAD granularity) differ (below).

## Isolation

* READ COMMITTED and REPEATABLE READ: validated on PostgreSQL 17.11 and
  15.19 for the matrix in [`POSTGRES-VALIDATION.md`](POSTGRES-VALIDATION.md)
  (1 695 schedules each, 0 disagreements); other versions not validated.
* SERIALIZABLE is a **serialization-graph abstraction**, not SSI. It allows
  every history whose logical serialization graph is acyclic; PostgreSQL may
  abort more (measured: 60 of 70 schedules in a sequential-scan workload). A
  SERIALIZABLE counterexample may therefore not be realizable; a
  SERIALIZABLE "no counterexample" holds for the model's histories, which
  include PostgreSQL's (argued, and consistent with every measurement, not
  proven). Read-only and deferrable transaction optimizations are not
  modeled.
* Mixed isolation levels in one scenario follow the same rules per
  transaction; SERIALIZABLE edges only connect SERIALIZABLE transactions
  (as in PostgreSQL), but this combination was not differentially tested.
* Deadlock victims: any member of a cycle (over-approximation of
  PostgreSQL's timer race); deadlock detection timing is not modeled beyond
  that.

## Concurrency and environment

* Actors are separate processes and connections unless declared in one
  process. No connection-pool limits, no queueing for connections, no
  network partitions, no clock, no time-outs, no ordering guarantees of a
  real queue beyond what retries express.
* A crash is a single event at a transition boundary; partially sent
  statements and partially performed external effects are equivalent to
  "before" or "after". Server crashes (losing committed data) are not
  modeled: committed data is durable.
* Retries: whole operations and whole transactions; single-statement retry
  and scheduled reconciliation are not built in.
* External effects are a log; the effect itself (what a payment provider
  does) is only what the scenario declares about deduplication.

## Exploration

* Bounded: small numbers of actors (≤ 4 by default, tested up to 4) and
  steps. The state space grows exponentially with actors; 4 read-modify-write
  purchasers already have 3 002 states and ~10⁹ schedules.
* Partial-order reduction preserves reachable states only without a
  context-switch bound and only as far as the independence relation is
  valid; validity is tested (1 500 random scenarios, an audit of every
  independent pair in every reachable state), not proven. It reduces
  transitions, not states; the wall-time saving is small.
* Minimization is greedy and budgeted: a minimized witness is valid and
  violating, not necessarily the shortest.
* Memory accounting is an estimate of the visited table only.

## PostgreSQL validation

* Two versions (17.11 on macOS arm64, 15.19 on Linux aarch64); no x86-64
  run.
* The oracle replays the model's statements as SQL; it does not run the
  TypeScript/Drizzle code. Drizzle's own SQL generation is therefore not
  validated by Phase 6 (it is by the Phase 4 replay tests, sequentially).
* Deadlock victim selection is steered with delays relative to
  `deadlock_timeout` — the only timing-based coordination; a slow machine
  could make it disagree (reported as a disagreement, never forced).
* A statement predicted to finish is allowed 250 ms of reported lock wait
  before it counts as blocked (to absorb the server's wake-up latency).

## Validation tooling

* `jsonschema` was not installed on the measurement machine; reports were
  validated with the built-in strict subset validator
  (`scripts/validate-schemas.py --builtin`), which rejects any schema keyword
  it does not implement. Validation with `jsonschema` was not run in Phase 6.

## Corpus

* No original application was model-checked: the only real-world scenario is
  an adapted isolated re-write (saas-starter invitations). No finding is
  human-reviewed or maintainer-confirmed.
