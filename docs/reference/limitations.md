# Limitations (current)

**This is the single current limitations document.** It consolidates
[`docs/LIMITATIONS.md`](../LIMITATIONS.md) (Phases 1–7, static analyzer)
and the per-phase files [6](../phase6/LIMITATIONS.md),
[7](../phase7/LIMITATIONS.md), [8](../phase8/LIMITATIONS.md),
[9](../phase9/LIMITATIONS.md), [10](../phase10/LIMITATIONS.md),
[11](../phase11/LIMITATIONS.md), [12](../phase12/LIMITATIONS.md) and
[13](../phase13/LIMITATIONS.md), applying what later phases lifted. Those
files remain the historical record and carry the details and test names.

State: Phase 13 working tree (base commit `32efbca`, Phase 13 uncommitted;
[`docs/phase13/FINAL-REPORT.md`](../phase13/FINAL-REPORT.md) is being
completed). Each entry names the phase document it comes from. When a phase
document and this page disagree, the phase document's measurement is the
record; please report the discrepancy.

Contents:
[supported and verified](#supported-and-verified) ·
[supported, model only](#supported-model-only) ·
[approximate](#approximate) ·
[unsupported source constructs](#unsupported-source-constructs) ·
[unsupported PostgreSQL behavior](#unsupported-postgresql-behavior) ·
[unsupported runtime drivers and environments](#unsupported-runtime-drivers-and-environments) ·
[missing independent verification](#missing-independent-verification) ·
[lifted limitations](#limitations-lifted-by-later-phases) ·
[Phase 14 changes](#phase-14-changes)

## What any result means

* A static diagnostic is a **potential** hazard (`"reproduced": false`).
  Midwrite cannot know whether the application tolerates the partial
  state or repairs it elsewhere.
* `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` covers the declared actors, arguments,
  initial rows, isolation level, faults and bounds — nothing else.
* `COUNTEREXAMPLE_FOUND` is a possible execution of the model. Whether the
  requests really run concurrently in production is not known.
* `original-code-reproduced` holds for the harness inputs and declared
  preconditions, the schedule, the PostgreSQL version and a disposable
  database of the run; not for production traffic.
* No result proves consistency. Midwrite does not replace database
  constraints, integration tests or fault-injection testing.
* Invariants are declared or labeled hypotheses; a wrong invariant gives a
  meaningless verdict.

## Supported and verified

Behavior that is modeled **and** checked against something other than the
model. "PostgreSQL differential" = the model's SQL replayed in lockstep on
PostgreSQL; "original code" = the first-party application's unchanged code
run under the schedule. Numbers: [evidence index](../research/evidence-index.md).

| behavior | verified by | scope of the verification |
| --- | --- | --- |
| READ COMMITTED / REPEATABLE READ snapshots, row locks, unique waits, EvalPlanQual, `ON CONFLICT DO NOTHING`, `SKIP LOCKED`/`NOWAIT`, crash by session kill, steered deadlock victims | PostgreSQL differential (Phase 6) | 1 695 schedules on 17.11 (macOS) and 15.19 (Linux), small tables |
| joins, projections, sequences with rollback, `INSERT … RETURNING` | PostgreSQL differential (Phase 7); original code (team-app, 5/5 runs) | 102 schedules on 17.11; first-party fixture |
| crash, retry, redelivery, external-effect doubles, real 40001 retries | original code (Phase 8) | 21 first-party scenarios, node-postgres and postgres-js |
| lost acknowledgment of a COMMIT or autocommit write (three fates), shared-process task crashes, pool starvation (pool of 1) | original code through the protocol proxy (Phase 9) | first-party `ledger-app`, node-postgres |
| broken connection under a live process; postgres-js in-flight faults | original code (Phase 10) | first-party fixtures; node-postgres 8.23.1, postgres-js 3.4.5 |
| automatically discovered counterexamples | original code (Phase 11) | 10 scenarios of the first-party commerce application |
| generated keys (`$defaultFn`, `defaultRandom`, database defaults), `RETURNING` into foreign keys, multi-module schemas | Drizzle/PostgreSQL probes (6/6 programs) and original code (Phase 12) | first-party `workspace-app` |
| row-lock modes and foreign-key checks (`NO ACTION`/`RESTRICT`, immediate) | PostgreSQL differential (543 schedules), independent ground truth (707 cells), original code (Phase 13) | 17.11; first-party `workspace-app` |
| `INSERT … ON CONFLICT (target) DO UPDATE` | PostgreSQL differential (396 schedules), original code (Phase 13) | 17.11; first-party `request-app` |
| return values of the main task; crash between the last statement and the response | original code (Phase 13) | first-party `request-app`, 3/3 runs each |
| schema facts read from Drizzle source | catalog comparison on a disposable database (Phase 8) | first-party schemas, with injected drift |

Phase 13's runtime verification ran on macOS only (PostgreSQL 17.11); its
Linux results are not in `reports/phase13/` at the time of writing.

## Supported, model only

Modeled, but not confirmed against PostgreSQL or original code:

* **Read replicas and replica staleness** (Phase 13). One asynchronous
  replica with unbounded lag; no PostgreSQL streaming replication was run;
  the lockstep oracle and original-code verification refuse replica
  schedules. A replica counterexample cannot go beyond
  `model-counterexample` ([Phase 13 replica consistency](../phase13/REPLICA-CONSISTENCY.md)).
* **Connection loss** other than for a COMMIT or an autocommit write: a
  loss in the middle of a transaction or during a read is refused at
  runtime (UNSUPPORTED); only a closed socket is realized (no timeouts, no
  `57P01`/`57P02`, no server that closes first, no partial frames, no TLS)
  ([Phase 10 limitations](../phase10/LIMITATIONS.md#live-connection-failures)).
* **External-effect response loss** (the provider acted, the client did
  not hear): modeled only as a crash after the effect; the runtime does not
  withhold effect responses ([Phase 9](../phase9/LIMITATIONS.md#in-flight-semantics)).
* **Pools of capacity > 1, application wait cycles, real PostgreSQL
  deadlocks in original code**: not run on original code; deadlocks are
  covered by the model-SQL replay matrix only ([Phase 9 final report § 5](../phase9/FINAL-REPORT.md#5-original-code-verification-scope)).
* **postgres-js** with bounded pools, REPEATABLE READ/SERIALIZABLE, shared
  processes: not run ([Phase 10](../phase10/LIMITATIONS.md#postgres-js)).
* **SERIALIZABLE counterexamples**: may not be realizable (see
  [approximate](#approximate)).
* **External applications**: saas-starter's invitation race is a model
  counterexample confirmed with model SQL only (not executed); no corpus
  application was executed in any phase.

## Approximate

Stated approximations; each is printed as an assumption or documented.

| approximation | consequence | source |
| --- | --- | --- |
| SERIALIZABLE as a serialization-graph abstraction, not SSI | PostgreSQL may abort more (60 of 70 schedules of a sequential-scan workload); a SERIALIZABLE counterexample may not be realizable; mixed isolation levels not differentially tested | [Phase 6](../phase6/LIMITATIONS.md#isolation) |
| statement atomicity unless the statement waits | a multi-row statement racing a commit mid-scan may have outcomes the model does not explore | [Phase 6](../phase6/LIMITATIONS.md#sql-and-database-subset) |
| deadlock victim: any member of a cycle | over-approximates PostgreSQL's timer race | [Phase 6](../phase6/LIMITATIONS.md#isolation) |
| a waiting statement waits for the first conflicting holder, then re-checks (multixact member order); deadlock detection sees that one edge | stated as an approximation in Phase 13; the lock ground truth replay (707 cells) found no disagreement | [Phase 13](../phase13/LIMITATIONS.md#approximations-stated) |
| rows without `orderBy` returned in primary-key order | PostgreSQL promises no order; the oracle sends `ORDER BY` the key | [Phase 6 concurrent IR](../phase6/CONCURRENT-IR.md) |
| `generated-values-distinct`: generated values never collide; generated rows ordered by generation event | no collision mode; a verdict depending on a collision is not checked | [Phase 12](../phase12/LIMITATIONS.md#assumptions-printed-with-results) |
| `undefined` and `null` are one value | — | [Phase 13](../phase13/LIMITATIONS.md#not-modeled) |
| the model releases a crashed transaction's locks at once | PostgreSQL does so only when the backend notices; crash plans set `client_connection_check_interval = 50ms`, without it a run is inconclusive | [Phase 8](../phase8/LIMITATIONS.md#crash-and-recovery) |
| retries other than the application's own loop are played by the coordinator under a declared mechanism | contracts, not evidence about a real queue or client; single-statement retry is not expressible | [Phase 8](../phase8/LIMITATIONS.md#crash-and-recovery) |
| recovery probes run one schedule | bounded evidence, not a proof of safety | [Phase 8](../phase8/LIMITATIONS.md#crash-and-recovery), [Phase 9](../phase9/LIMITATIONS.md#runtime-verification) |
| reductions (sleep sets, DPOR, source-DPOR) | evidence-checked against the unreduced search, not proved; some footprint accesses (`RowRead`, `LockCheck`, `Wait`, `Seq`, `Budget`) can be deleted without any workload noticing; replica reads are treated as dependent on every primary commit (sound, not minimal) | [Phase 10](../phase10/LIMITATIONS.md#reductions), [Phase 13](../phase13/LIMITATIONS.md#approximations-stated) |
| state space exponential in actors | measured ≈ 7× per actor (Phase 8 workload); 8 actors finish on the Phase 9 benchmark workload (4.5 M states), 9 not run | [Phase 8](../phase8/LIMITATIONS.md#scale), [Phase 9](../phase9/LIMITATIONS.md#state-space-exploration) |
| minimization is greedy and budgeted | a minimized witness is valid and violating, not necessarily the shortest | [Phase 6](../phase6/LIMITATIONS.md#exploration) |
| runtime lock-wait detection polls `pg_stat_activity` after a 150 ms settle time | a briefly waiting statement may be observed as completing | [Phase 7](../phase7/LIMITATIONS.md#runtime-verification) |
| runtime results compare modeled columns only | other columns (timestamps) are shown, not compared | [Phase 7](../phase7/LIMITATIONS.md#runtime-verification) |
| static analyzer: loops as zero or one iteration for effects; catch handlers entered as if the `try` failed before its effects; `Promise.all` elements concurrent; paths feasible unless simple facts contradict; every awaited operation may fail | false positives and negatives documented in [`docs/LIMITATIONS.md`](../LIMITATIONS.md#approximations-in-the-path-model) | Phases 1–5 |
| configured receiver names and member paths trusted as written; no type information | a binding merely named like a receiver is trusted (use `importedFrom`) | [`docs/LIMITATIONS.md`](../LIMITATIONS.md#recognition-trust-model) |
| concurrency of two invocations | declared, `framework`, or `discovery-default` ("NOT established from source") | [Phase 11](../phase11/LIMITATIONS.md#discovery) |
| discovery's argument/row correlation | a heuristic stated as a note, chosen by foreign-key evidence | [Phase 11](../phase11/LIMITATIONS.md#state-synthesis) |
| Drizzle `casing` option | the extractor assumes the property name and records the assumption | [Phase 12](../phase12/LIMITATIONS.md#not-modeled-reported-as-unknown-or-as-not-modeled-facts) |

## Unsupported source constructs

### Static analyzer (`check`)

From [`docs/LIMITATIONS.md`](../LIMITATIONS.md) (Phases 1–5; unchanged by
Phases 6–13, whose static output stayed byte-identical):

* methods on values of unknown class (parameters, SDK clients, factory
  results), methods with an analyzed override or dynamic reassignment,
  callbacks run by unanalyzed code, `.then/.catch/.finally` continuation
  bodies;
* captured variables and transaction handles inside callbacks;
* dynamic `import()`, CommonJS `require`, packages in `node_modules`,
  imports that do not resolve (ambiguous modules or exports, exports that
  are not function declarations such as `export const f = wrap(...)`);
* calls neither awaited nor returned from an `async` function; effects
  deeper than `--max-call-depth` (3) unless inert;
* handles from unconfigured calls or calls with arguments
  (`getDb("replica")`), handle arguments that are not direct identifiers
  (`save(tx ?? db)`), `let`-bound aliases, destructured transaction
  handles, non-inline transaction callbacks;
* raw SQL (`db.execute(sql…)`), `db.batch(...)`, other ORMs, SQLite
  executors (`.run()`, `.all()`, `.get()`);
* top-level module code, labeled `break`/`continue`, generator
  resumption; optional calls are treated as unconditional;
* more than `--max-paths` (512) paths in a function (the rest truncated,
  `MW9001`);
* unconfigured external effects before the first recognized commit
  (silent unless `--policy audit`).

### Concurrency extractor (`model-check`, `discover`)

Still unknown or unsupported after Phase 13 (sources:
[Phase 13 limitations](../phase13/LIMITATIONS.md#not-modeled),
[Phase 13 source resolution](../phase13/SOURCE-RESOLUTION.md#not-implemented-with-the-reason),
[Phase 13 advanced Drizzle § 4](../phase13/ADVANCED-DRIZZLE.md#4-not-done-in-phase-13),
[Phase 12](../phase12/LIMITATIONS.md#not-modeled-reported-as-unknown-or-as-not-modeled-facts),
[Phase 11](../phase11/LIMITATIONS.md#extraction-inherited),
[Phase 7](../phase7/LIMITATIONS.md#extraction)):

* multi-row `values([...])`, re-executing a stored builder,
  `` sql`default` `` and other SQL values in `values(...)`;
* row values of `UPDATE`/`DELETE … RETURNING` (counts and presence are
  modeled); a helper returning a mutation's rows
  (`return db.update(...).returning()`);
* higher-order wrappers whose callback argument mapping is not known
  (`withRetry(() => …)`, `export const f = wrap(async () => …)`,
  call-returned functions, function factories); only the innermost
  wrapper of a chain is analyzed;
* inherited methods (`class B extends A`), constructors that do more than
  store their parameters, objects modified anywhere in their module, `this`
  of an entry that is itself a method, dynamic dispatch;
* arrays, `Map`/`Set`, `Array.prototype.includes/map/filter/some`, loops
  over collections of unknown length; RegExp; string building (template
  literals with interpolation, `+` on strings, `.slice`,
  `.toLowerCase`); destructuring with defaults, nesting or rest;
* timestamp, JSON and array columns (dropped with a fact; a primary key of
  such a type refuses its table); decimals;
* closures over enclosing-function variables; `.then`, timers,
  generators, recursion, inspecting caught errors;
* a conditional expression inside `and(...)` used as a predicate;
  predicates with `OR`/`IN`/`LIKE`/functions are not satisfied by state
  synthesis;
* raw SQL, subqueries, aggregates other than `count`, `orderBy` semantics,
  `RIGHT`/`FULL`/`CROSS` joins, joins with locks, `findMany` (Phase 6
  lists it as unknown; no later phase lists it as supported);
* package calls: `bcryptjs`/`bcrypt` and `zod` are named by a verified
  contract but stay unknown (their results are not representable); every
  other package call is unknown package behavior;
* unrecognized default callbacks (`customIdService.generate()`,
  `process.env.X`, callbacks with side effects), `$onUpdate`,
  `default(sql…)` other than `gen_random_uuid()`, `uuid_generate_v4()`,
  dynamic schema declarations, Drizzle `alias()` self-joins, spread or
  helper-built columns, custom types;
* union handle types (`Database | Transaction`) as entry parameters;
  `executeOnReplica`/`transactionOnReplica` and other members of a
  replicated database outside the declared contract;
* the React `cache` contract applies only to `cache` imported from
  `react`, and only at the first call;
* return-value properties are never inferred; they must be declared;
* entry discovery skips `export default`, export lists and re-exports
  ([Phase 11](../phase11/LIMITATIONS.md#discovery); no later phase lists
  this as changed).

Discovery-specific: properties are hypotheses unless declared or
schema-enforced; `SOURCE_ASSERTED` is never produced; recovery
requirements ("an event is not lost") are never synthesized; write skew
needs a declared invariant; pairing covers check-then-act and
read-modify-write only and truncates resources with more than six writers;
no detection family pairs a parent `DELETE` with a child `INSERT`
([Phase 13 evaluation](../phase13/EVALUATION.md)).

## Unsupported PostgreSQL behavior

**Not every PostgreSQL feature is modeled.** Refused (the scenario does
not load; never approximated):

* `ON DELETE`/`ON UPDATE` `CASCADE`, `SET NULL`, `SET DEFAULT` when a
  statement would run them; `DEFERRABLE INITIALLY DEFERRED` foreign keys
  when the referencing table is written;
* replica reads without the `replicas` feature; writes or row locks on a
  replica;
* an invariant over the return value of an actor whose return expression
  is not modeled.

Not modeled ([Phase 6](../phase6/LIMITATIONS.md#sql-and-database-subset),
[Phase 12](../phase12/LIMITATIONS.md), [Phase 13](../phase13/LIMITATIONS.md#not-modeled)):

* `SET CONSTRAINTS`, `MATCH FULL`, partial-index upsert arbiters
  (`targetWhere`), `SKIP LOCKED`/`NOWAIT` with a shared lock strength;
* partial and expression unique indexes as constraints (a partial index is
  a recorded protection only), `NULLS NOT DISTINCT` on nullable columns,
  CHECK expressions beyond the simple subset, enum domains, triggers,
  row-level security, database functions, `NOW()`;
* savepoints and nested transactions in the model;
  `lock_timeout`/`statement_timeout`/`idle_in_transaction_session_timeout`,
  advisory locks, `LISTEN`/`NOTIFY`;
* several replicas with different lags, synchronous replication
  (`synchronous_commit = remote_apply`), hot-standby query cancellation;
* server crashes that lose committed data; partially sent statements;
  pipelined statements (one in-flight operation per connection); orphaned
  COMMITs complete atomically;
* real pools' idle timeouts, connection recycling, `maxUses`, health
  checks, priority; start-up arrival orders other than declaration order;
* PostgreSQL versions other than 17.11 and 15.19 (16 and 14 not run);
  PostgreSQL 14 or newer is required for crash plans.

## Unsupported runtime drivers and environments

* Original-code verification: Node.js + Drizzle 0.45.4 with node-postgres
  8.23.1 or postgres-js 3.4.5; other postgres-js versions are refused
  (coordination relies on `Query.prototype.handle`); Neon, Bun SQL, other
  drivers and ORMs are not coordinated; Bun is not coordinated for source
  verification ([Phase 7](../phase7/LIMITATIONS.md#runtime-verification),
  [Phase 8](../phase8/LIMITATIONS.md#postgres-js),
  [Phase 10](../phase10/LIMITATIONS.md#postgres-js)).
* Replay harnesses: Node.js ≥ 22 (compiled JavaScript; replay never
  compiles or installs) and Bun ≥ 1.2; the driver adapter covers
  node-postgres only; statements are classified by their leading keyword
  (writes in CTEs or database functions are not checkpoint boundaries)
  ([`docs/LIMITATIONS.md` § Phase 4](../LIMITATIONS.md#replay-harness-generation-and-recovery-phase-4)).
* postgres-js: cursors, `forEach`, streams unsupported; driver-internal
  catalog reads are not held; lock waits attributed by `application_name`;
  `sql.end()` without a timeout did not return through the fault proxy
  (cause not established); the application must resolve the same
  `postgres` module instance as the harness.
* node-postgres connection loss: harnesses attach a client `error`
  listener (without one the process dies, recorded as diverged); broken
  client recognition relies on driver internals verified on 8.23.1.
* Protocol proxy needs a Unix socket and the harness's own pool
  configuration; an application opening its own connection string is not
  covered. Pool capacity in the scenario must equal the harness pool's
  `max`.
* Retries of an actor that shares a process with others are refused;
  connection loss requires one running task in the actor.
* Effects must reach a provider seam the harness can replace; the double's
  deduplication is declared; key expiry, partial provider failures and
  provider-side retries are not modeled.
* Framework internals are not modeled: the coordinator plays the HTTP
  client; error responses are not a separate step; the response step is
  inserted only for actors declared (or found) to be retried after a crash.
* Isolation: macOS `sandbox-exec`, Linux bubblewrap. It protects against
  accidents in a trusted harness, not malicious code; on Linux the harness
  project and runtime are readable inside the sandbox; on macOS processes
  that leave the process group can outlive the run (detached processes are
  swept since Phase 8); no CPU, memory or temporary-disk cap beyond
  per-worker rlimits.
* Platforms: Windows not supported and not run; x86_64 not run; the Linux
  runs are a Debian 12 container on the same Apple Silicon machine;
  GitHub CI and release workflows exist but no run is recorded in the phase
  reports.

## Missing independent verification

* **No independent human review performed** — in any phase, of any
  finding, label set, semantics, protocol or scenario.
* Phase 2's precision review was done by the Phase 2 implementer from
  source alone; Phase 3's blind re-review of 18 findings was an automated
  separate model session, not a human.
* Phase 11's first-party benchmark has the same author as the engine and
  was written with the detector families in mind (an upper bound, not an
  estimate); its held-out set stopped being held out after the first
  scoring.
* Phase 12's evaluation set is fresh and frozen but written by the
  implementation's author (the file is named
  [`INDEPENDENT-EVALUATION.md`](../phase12/INDEPENDENT-EVALUATION.md) but
  states it is not independent).
* Phase 13's frozen set and PostgreSQL lock ground truth were produced by
  an AI agent without access to the implementation: independence from the
  code, not from the project; the author knew the Phase 12 supported
  subset, so it is fresh but not blind. Its post-feedback score is not
  held out.
* No external, corpus or private application was executed; no corpus
  finding is claimed as a defect; no maintainer was contacted. The Phase 8
  blind review packets remain unreviewed.
* Model-only results (replicas) have no measured counterpart.
* Unresolved intermittent test failures are recorded, not hidden: the
  Phase 6 oracle test `deadlock_workloads` (once on Linux, Phase 8) and
  `claim-skip-locked-2/serializable` (Phase 9).

## Limitations lifted by later phases

| earlier limitation | lifted in | how |
| --- | --- | --- |
| class methods, callbacks not followed by `check` | 5 | known-class dispatch, callback contexts, inert summaries ([Phase 5](../phase5/FINAL-REPORT.md)) |
| `try`/`catch`/`finally`, `switch`, `??`, joins, projections, `RETURNING`, wrapped Server Actions, loops over literal arrays not extracted (Phase 6) | 7 | [Phase 7 final report](../phase7/FINAL-REPORT.md) |
| crashes and retries not executed against original code; postgres-js not coordinated (Phase 7) | 8 | real SIGKILL and restart; postgres-js 3.4.5 adapter |
| tables only declared in scenarios (Phase 7) | 8 | Drizzle schema facts from source |
| a kill while a released COMMIT runs not planned; client ignorance not represented; no connection pools; actors in one process refused (Phase 8) | 9 | in-flight operations, possible worlds, pools, shared processes ([Phase 9 § resolved](../phase9/LIMITATIONS.md#resolved-or-narrowed-by-phase-9)) |
| `LostCommitted`/`LostNotCommitted` with a live process not realized; postgres-js in-flight unsupported; Phase 9 DPOR last-race defect | 10 | [Phase 10 § lifted](../phase10/LIMITATIONS.md#lifted-in-phase-10) (COMMIT and autocommit writes only) |
| generated primary keys refused, blocking most real schemas; one schema module per scenario; extraction re-indexed the project per entry; helper return-slot location wrong; a local assigned only by an unsupported operation made the scenario not load (Phase 11) | 12 | generated values, multi-module schemas, shared project context, provenance correction, `Op::Unknown.defines` |
| foreign keys not modeled (incl. the measured FK-lock divergence); return values; crash before the response; read replicas; `ON CONFLICT DO UPDATE`; `FOR SHARE`/`FOR NO KEY UPDATE`/`FOR KEY SHARE`; exported `const` function aliases; anonymous package calls (Phase 6–12) | 13 | [Phase 13 § lifted](../phase13/LIMITATIONS.md#lifted-in-phase-13) — replicas model only, FKs `NO ACTION`/`RESTRICT` immediate only |

## Phase 14 changes

Details and the full lists: [Phase 14 limitations](../phase14/LIMITATIONS.md).

* **Lifted:** callbacks of project wrappers (`withRetry(() => …)`) are
  inlined where called; zod `parse`/`safeParse` for a documented schema
  subset; bounded arrays, strings, `undefined`, `Map`/`Set` (feature
  `js-values`); multi-row `INSERT`, `inArray`/`notInArray`, conditional
  predicates; `timestamp`/`timestamptz`/`date` (feature `timestamps`) and
  `json`/`jsonb` (feature `json`) subsets; `nanoid()`/`randomUUID()` as
  generated values; ambiguous commit of a retried transaction; the
  `parent-child` and `return-value` (declared only) families.
* **Still refused:** `values(obj)` of a parameter object, unlisted zod
  checks, array mutation, `for`/`while` loops, framework request objects,
  time arithmetic and clock ordering, JSON comparisons and `->>`, nested
  conditional predicates, multi-row statements in flight.
* **New stated assumptions:** `clock-readings-distinct`; `now()` is the
  transaction start time; `as const` arrays are not mutated; the Phase 14
  first-party harness handles node-postgres client errors.
* **Partial unique indexes** are still not modeled; a counterexample on a
  table with one is now INCONCLUSIVE instead of a finding.
* **Synthesis:** an initial value can coincide with the value an actor
  writes (hiding a stale read); arguments are not explored over `null`.
* **Verified only in the model:** replica staleness (no replication run).
  **Verified on PostgreSQL:** multi-row inserts, predicates, timestamps and
  JSON in lockstep (669 schedules, 0 disagreements besides the known
  one-way SSI difference); two new first-party original-code slices
  (`docs/phase14/FINAL-REPORT.md`).

## Phase 15 changes

Details: [Phase 15 limitations](../phase15/LIMITATIONS.md).

* **Lifted:** synthesized initial values coinciding with written values
  (now varied: `stored-distinct`/`stored-equal` worlds); `null`/`undefined`
  arguments where the annotation admits them; branch-boundary and stored
  guard-boundary values; non-key arguments differing between actors;
  string-literal union arguments; guards over read rows and arguments that
  synthesized inputs failed (satisfied with stated preconditions);
  declared `atMostOne` keys over conditionally tested columns; guarded
  relative updates with a declared property.
* **Still not covered:** exhaustive value domains (never claimed:
  `domainExhaustive` is always false); combinations of relationships; more
  than two actors; nullability without an annotation; zod `strictObject`,
  formats, `nullable`/`optional`/`default`; `for`/`while` loops;
  `req.json()`; time arithmetic; JSON projections; predicates hoisted into
  locals; multi-row `FOR UPDATE`; properties over generated identifiers.
* **New INCONCLUSIVE rule:** a counterexample on a table with a `NULLS NOT
  DISTINCT` unique rule over a nullable column (not enforced by the
  model) is INCONCLUSIVE; PostgreSQL confirmed the rule is needed.
* **Verified on PostgreSQL:** 28 counterexample worlds in lockstep (166
  outcomes, 0 disagreements); two input-dependent first-party defects
  reproduced on the original code (3/3 each). **Model only:** the new
  read-your-writes counterexamples through a replica.

## Phase 16 changes

Details: [Phase 16 limitations](../phase16/LIMITATIONS.md).

* **Lifted:** zod `strictObject`/`strict`/`passthrough`, `optional`,
  `nullable`, `default`, `uuid`, `email`, a regex subset and arrays (zod 3
  and zod 4, version-aware); bounded `for`/`while`/`do…while` loops;
  inspecting a caught database error's SQLSTATE; `await request.json()` and
  `Response.json`; predicates hoisted into locals or chosen by a
  conditional; multi-row `SELECT … FOR UPDATE/SHARE` with `NOWAIT` and
  `SKIP LOCKED`; `UNIQUE … NULLS NOT DISTINCT` (the Phase 15 INCONCLUSIVE
  rule now applies only when the rule involves an unmodeled column);
  properties over generated identifiers.
* **Still not covered:** zod refinements (`refine`, `superRefine`),
  transforms, preprocess, coerce, unions; `for...in`, `for await`, labeled
  jumps, loops beyond 8 iterations (TRUNCATED), backoff delays; errors from
  lost connections, application error classes (`instanceof MyError`),
  error messages; a request body that is not valid JSON; Server Actions
  with `FormData`, tRPC; `orderBy` other than the primary key, multi-row
  mode with joins; partial unique indexes; collation-aware equality;
  `UPDATE … RETURNING` row values; more than two actors.
* **Approximate:** lock order of a multi-row lock without
  `.orderBy(primaryKey)` (the model uses key order; PostgreSQL may use
  physical order, measured 24 of 51 schedules differing on an out-of-order
  heap); `generatedBy` matches any generation event of the actor and
  generator, not a specific occurrence.
* **Verified on PostgreSQL 17.11:** NULLS NOT DISTINCT (1542 lockstep
  replays, 0 disagreements), multi-row locks (2124 schedules, 0
  disagreements); a retry-loop duplicate reproduced on the original code.
  **Model only:** replica counterexamples.

