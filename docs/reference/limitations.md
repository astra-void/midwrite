# Limitations (current)

**This is the single current limitations document.** It covers the static
analyzer, the concurrency extractor, discovery, the model checker and
runtime verification. The static analyzer's detailed list (false
positives and negatives with examples, call summaries, replay) is in
[`docs/LIMITATIONS.md`](../LIMITATIONS.md); the topic pages linked from
each entry carry the details. Measured numbers are in the
[evidence index](../research/evidence-index.md); the phase records they
come from are archived ([development history](../HISTORY.md)). When a
topic page and this page disagree, please report the discrepancy.

Contents:
[supported and verified](#supported-and-verified) ·
[supported, model only](#supported-model-only) ·
[approximate](#approximate) ·
[unsupported source constructs](#unsupported-source-constructs) ·
[unsupported PostgreSQL behavior](#unsupported-postgresql-behavior) ·
[unsupported runtime drivers and environments](#unsupported-runtime-drivers-and-environments) ·
[missing independent verification](#missing-independent-verification)

Broad HTTP-effect rules can flag read-only network requests. The optional
[`httpMethods` filter](configuration.md#narrowing-http-effects) narrows literal
fetch-style methods, also through a helper that spreads its options parameter
into the call; unresolved options remain unknown, and a POST query (or an
OAuth token exchange) may still be read-only.

Joined collections have a bounded source model: a single-use `const`
initialized by `map`/`flatMap` and awaited with `Promise.all` is connected
to its callbacks. Branching inline callbacks can be sampled for a second
element to expose mixed update/insert batches; this does not enumerate all
elements or completion orders. Ordinary loops remain bounded to zero or
one iteration. Captured transaction handles in arbitrary array callbacks
remain unsupported.

`closedWorld` and `resolvedResult` are explicit configuration assumptions
([configuration](configuration.md#factory-receivers-and-resolved-results)).
Neither proves application invariants, library behavior, or the absence of
callers outside the analyzed source. Unrelated but harmless writes can
still be reported. The bounded [returned-row pass](../architecture/returned-row-facts.md)
can carry schema-backed string-column identities through source helpers
and known Result transformations. Missing schemas, custom decoders,
undefined assignments, escaped/mutated rows, unrecognized transformations
and exhausted budgets do not prove a guard unreachable. Runtime triggers,
custom driver parsers and monkeypatching of library APIs are outside this
source-level relation.

## What any result means

* Inline suppressions apply to all findings of one rule on the targeted
  primary line in `check` and `trace`. They do not apply to discovery or
  concurrency model checking, and their reasons are unverified assertions
  ([inline suppressions](../user-guide/check.md#inline-suppressions)).

* Static finding fingerprints exclude source positions, including generated
  anonymous-function positions. Identical code/file/function/effect-label
  combinations use occurrence suffixes; inserting or removing an identical
  occurrence can reassign those IDs. See [finding identity](../user-guide/check.md#baselines-adopting-it-in-ci).

* A static diagnostic is a **potential** hazard (`"reproduced": false`).
  Midwrite cannot know whether the application tolerates the partial
  state or repairs it elsewhere.
* `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` covers the declared actors, arguments,
  initial rows, isolation level, faults and bounds — nothing else. In
  discovery, `domainExhaustive` is always false: no verdict claims all
  input values ([world search](../architecture/world-search.md#coverage-and-what-a-verdict-claims)).
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
PostgreSQL ([validation](../architecture/postgres-model.md#validation-against-postgresql));
"original code" = the first-party application's unchanged code run under
the schedule ([runtime verification](../architecture/runtime-verification.md)).
Sizes of each check: [evidence index](../research/evidence-index.md).

| behavior | verified by | scope of the verification |
| --- | --- | --- |
| READ COMMITTED / REPEATABLE READ snapshots, row locks, unique waits, EvalPlanQual, `ON CONFLICT DO NOTHING`, `SKIP LOCKED`/`NOWAIT`, crash by session kill, steered deadlock victims | PostgreSQL differential | PostgreSQL 17.11 (macOS) and 15.19 (Linux), small tables |
| joins, projections, sequences with rollback, `INSERT … RETURNING` | PostgreSQL differential; original code (`team-app`) | 17.11; first-party fixture |
| crash, retry, redelivery, external-effect doubles, real 40001 retries | original code | first-party scenarios, node-postgres and postgres-js |
| lost acknowledgment of a COMMIT or autocommit write (three fates), shared-process task crashes, pool starvation (pool of 1) | original code through the protocol proxy | first-party `ledger-app`, node-postgres |
| broken connection under a live process; postgres-js in-flight faults | original code | first-party fixtures; node-postgres 8.23.1, postgres-js 3.4.5 |
| automatically discovered counterexamples | original code | first-party commerce, workspace, request and retry applications |
| generated keys (`$defaultFn`, `defaultRandom`, database defaults), `RETURNING` into foreign keys, multi-module schemas | Drizzle/PostgreSQL probes and original code | first-party `workspace-app` |
| row-lock modes and foreign-key checks (`NO ACTION`/`RESTRICT`, immediate) | PostgreSQL differential, independent lock ground truth, original code | 17.11; first-party `workspace-app` |
| `INSERT … ON CONFLICT (target) DO UPDATE` | PostgreSQL differential, original code | 17.11; first-party `request-app` |
| return values of the main task; crash between the last statement and the response | original code | first-party `request-app` |
| multi-row inserts, `inArray`/conditional predicates, timestamps and JSON subsets | PostgreSQL differential | 17.11 |
| input-dependent counterexamples found by world search | PostgreSQL differential; original code | 17.11; first-party worlds app |
| `UNIQUE … NULLS NOT DISTINCT`; multi-row `SELECT … FOR UPDATE/SHARE` with `NOWAIT`/`SKIP LOCKED` | PostgreSQL differential | 17.11 (NULLS NOT DISTINCT needs PostgreSQL ≥ 15) |
| zod validation subset | differential against the pinned zod 3 and zod 4 runtimes | [zod](zod.md#verification-against-the-real-zod) |
| retry loops (a duplicate after a lost COMMIT acknowledgment) | original code | first-party retry fixture |
| schema facts read from Drizzle source | catalog comparison on a disposable database | first-party schemas, with injected drift |

Not every later check was repeated on Linux; the evidence index names the
platform of each.

## Supported, model only

Modeled, but not confirmed against PostgreSQL or original code:

* **Read replicas and replica staleness.** One asynchronous replica with
  unbounded lag; no PostgreSQL streaming replication was run; the lockstep
  oracle and original-code verification refuse replica schedules. A
  replica counterexample cannot go beyond `model-counterexample`
  ([read replica](../architecture/postgres-model.md#read-replica)).
* **Connection loss** other than for a COMMIT or an autocommit write: a
  loss in the middle of a transaction or during a read is refused at
  runtime (UNSUPPORTED) ([fault injection](../architecture/fault-injection.md#not-covered)).
* **External-effect response loss** (the provider acted, the client did
  not hear): modeled only as a crash after the effect; the runtime does not
  withhold effect responses.
* **Pools of capacity > 1, application wait cycles, real PostgreSQL
  deadlocks in original code**: not run on original code; deadlocks are
  covered by the model-SQL replay matrix only.
* **postgres-js** with bounded pools, REPEATABLE READ/SERIALIZABLE, shared
  processes: not run.
* **SERIALIZABLE counterexamples**: may not be realizable (see
  [approximate](#approximate)).
* **Generated-identity properties** are reported `not-evaluated` on
  original code, never held or violated there: which actor generated a
  value is not observable in a run ([invariants](../concepts/invariants.md#generated-identity-and-quantifiers)).
* **External applications**: saas-starter's invitation race is a model
  counterexample confirmed with model SQL only (not executed); no corpus
  application has been executed.

## Approximate

Stated approximations; each is printed as an assumption or documented.

| approximation | consequence | details |
| --- | --- | --- |
| SERIALIZABLE as a serialization-graph abstraction, not SSI | PostgreSQL may abort more (sequential scans take relation SIREAD locks); a SERIALIZABLE counterexample may not be realizable; read-only and deferrable optimizations not modeled; mixed isolation levels not differentially tested | [isolation](../architecture/postgres-model.md#isolation) |
| statement atomicity unless the statement waits | a multi-row statement racing a commit mid-scan may have outcomes the model does not explore | [model semantics](../concepts/model-semantics.md) |
| deadlock victim: any member of a cycle | over-approximates PostgreSQL's timer race | [row locks](../architecture/postgres-model.md#row-locks) |
| a waiting statement waits for the first conflicting holder, then re-checks (multixact member order); deadlock detection sees that one edge | the independent lock ground truth found no disagreement | [row locks](../architecture/postgres-model.md#row-locks) |
| lock order of a multi-row lock without `.orderBy(primaryKey)` is key order | PostgreSQL may lock in physical order; on an out-of-order heap some schedules differ | [multi-row locks](../architecture/postgres-model.md#multi-row-reads-and-locks) |
| rows without `orderBy` returned in primary-key order | PostgreSQL promises no order; the oracle sends `ORDER BY` the key | [storage and statements](../architecture/postgres-model.md#storage-and-statements) |
| `generated-values-distinct`: generated values never collide; generated rows ordered by generation event | no collision mode; a verdict depending on a collision is not checked | [generated values](../concepts/value-semantics.md#generated-values) |
| `clock-readings-distinct`; `now()` is the transaction start time | time arithmetic and clock ordering are not modeled; a `new Date()` held in a local and used by several statements stays unknown; `mode: "string"` timestamps, `date` with `mode: "date"` and `.default(sql\`now()\`)` are not modeled | [clock readings](../concepts/value-semantics.md#clock-readings-feature-timestamps) |
| generated identity: `generatedBy` matches any generation event of the actor and generator | package-call sites share one `call` generator; `returned` reads only the latest completed attempt; no quantifier over generation events or attempts | [invariants](../concepts/invariants.md#generated-identity-and-quantifiers) |
| the model releases a crashed transaction's locks at once | PostgreSQL does so only when the backend notices; crash plans set `client_connection_check_interval = 50ms`, without it a run is inconclusive | [crashes](../architecture/runtime-verification.md#crashes) |
| retries other than the application's own loop are played by the coordinator under a declared mechanism | contracts, not evidence about a real queue or client; single-statement retry is not expressible | [failure model](../concepts/failure-model.md#retries-not-interchangeable) |
| recovery probes run one schedule | bounded evidence, not a proof of safety | [runtime verification](../architecture/runtime-verification.md#retries-effects-probes) |
| `maxInFlight` is a budget per fault event, at most 8 operations in flight per event | whether a backend completes a *blocked* autocommit statement of a dead client is not modeled | [lost acknowledgments](../concepts/failure-model.md#lost-acknowledgments) |
| twin analysis of a lost operation | another actor's knowledge of the lost operation is not modeled; other clients see only the database | [possible worlds](../concepts/model-semantics.md#possible-worlds-and-knowledge) |
| reductions (sleep sets, DPOR, source-set DPOR) | evidence-checked against the unreduced search, not proved; some footprint accesses (`RowRead`, `LockCheck`, `Wait`, `Seq`, `Budget`) can be deleted without any workload noticing; replica reads are dependent on every primary commit (sound, not minimal); terminal states are compared modulo the order of external effects on different channels, so a property depending on that order is neither expressible nor preserved | [partial-order reduction](../architecture/partial-order-reduction.md#what-is-preserved) |
| DPOR (`--dpor`, `--source-dpor`) with `always` invariants or `maxContextSwitches` | refused; sleep sets run instead and the report says why; no search checks liveness or fairness | [partial-order reduction](../architecture/partial-order-reduction.md#searches) |
| `maxMemoryBytes` is checked against `visitedBytes` (table, interner, edges, nodes), not RSS | the stack and the minimizer are outside it; no disk spilling and no lossy (bitstate) mode | [state identity](../concepts/model-semantics.md#state-identity) |
| state space exponential in actors | measured ≈ 7× per actor on team-app; 8 actors (≈ 4.5 M states) finish, 9 not run | [benchmarks](../development/benchmarks.md) |
| minimization is greedy and budgeted | a minimized witness is valid and violating, not necessarily the shortest | [counterexamples](../architecture/model-checker.md#counterexamples) |
| lockstep oracle timing | a statement expected to finish may show a lock wait for up to 250 ms; deadlock victims are steered by timing relative to `deadlock_timeout` (a slow machine can disagree; reported, never forced); interleavings inside one statement cannot be compared on a real server | [lockstep oracle](../architecture/postgres-model.md#the-lockstep-oracle) |
| runtime lock-wait detection polls `pg_stat_activity` after a 150 ms settle time | a briefly waiting statement may be observed as completing | [runtime verification](../architecture/runtime-verification.md) |
| runtime results compare modeled columns only | other columns (timestamps) are shown, not compared | [runtime verification](../architecture/runtime-verification.md) |
| static analyzer: loops as zero or one iteration for effects; catch handlers entered as if the `try` failed before its effects; `Promise.all` elements concurrent; paths feasible unless simple facts contradict; every awaited operation may fail | false positives and negatives | [`docs/LIMITATIONS.md`](../LIMITATIONS.md#approximations-in-the-path-model) |
| configured receiver names and member paths trusted as written; no type information | a binding merely named like a receiver is trusted (use `importedFrom`) | [`docs/LIMITATIONS.md`](../LIMITATIONS.md#recognition-trust-model) |
| effect relationships (`relationship`, `valueRelation`) | descriptive only; `noRelationEstablished` is not independence and `provenDisjoint` is never produced (predicates are not interpreted) | [value analysis](../architecture/value-analysis.md#effect-relationships-valuerelation) |
| concurrency of two invocations | declared, `framework`, or `discovery-default` ("NOT established from source") | [scenario synthesis](../architecture/scenario-synthesis.md) |
| discovery's argument/row correlation | a heuristic stated as a note, chosen by foreign-key evidence | [state synthesis](../architecture/scenario-synthesis.md#state-synthesis) |
| a synthesized state is valid only with respect to the modeled constraints and the source's foreign keys | triggers, RLS and CHECK constraints the extractor does not read can make a synthesized state impossible in the real database | [state synthesis](../architecture/scenario-synthesis.md#state-synthesis) |
| branch-outcome coverage comes from solo runs | concurrent schedules may take other branch directions; it orders worlds and is not a correctness measure | [world search](../architecture/world-search.md#coverage-and-what-a-verdict-claims) |
| a world reported invalid | no valid state was found within the bounds; never proof that none exists | [world search](../architecture/world-search.md#worlds) |
| controls and hypothetical variants are checked in one world only | world search applies to actual variants | [world search](../architecture/world-search.md#worlds) |
| Drizzle `casing` option | the extractor assumes the property name and records the assumption | [schema extraction](../architecture/schema-extraction.md) |

## Unsupported source constructs

### Static analyzer (`check`)

Array-cardinality exclusions use native array methods and side-effect-free
scalar counts. They are not a model of arbitrary proxies, array subclasses
or hidden user-defined coercions. Mutation, escaping aliases, unknown
predicates and visible coercion/getter hooks abandon this proof; sparse
partitions supply only an upper bound
([cardinality guards](../architecture/value-analysis.md#array-cardinality-guards)).

Details and examples: [`docs/LIMITATIONS.md`](../LIMITATIONS.md). The
model-checking work left the static analyzer's output unchanged.

* methods on values of unknown class (parameters, SDK clients, factory
  results), methods with an analyzed override or dynamic reassignment,
  callbacks run by unanalyzed code, `.then/.catch/.finally` continuation
  bodies;
* constructor parameter properties (`constructor(private db: Database)`)
  are not bound to instance fields; getters and setters are ignored (an
  accessor makes its object open); class dispatch assumes a closed world
  (subclasses only in the analyzed files);
* mutable captured bindings and shared object contents. Immutable lexical
  bindings, callbacks and recognized transaction handles are preserved
  within 16 captures and three nested environments; deeper captures widen
  to unknown. More than 8 callback contexts per callee bypasses the cache
  without discarding callback effects
  ([value analysis](../architecture/value-analysis.md#soundness-assumptions));
* promises stored and awaited later, `for await`, timers firing;
* dynamic `import()`, CommonJS `require`, packages in `node_modules`,
  imports that do not resolve (ambiguous modules or exports, exports that
  are not function declarations such as `export const f = wrap(...)`);
* calls neither awaited nor returned from an `async` function; effects
  deeper than `--max-call-depth` (3) unless inert. A synchronous inert
  factory's return value can resolve a later call, but its plain call does
  not introduce awaited effects. Returned-function dispatch requires a
  single known function and no possible implicit return. Stable module
  factory bindings, aliases and default exports are interpreted only
  through inert initializer chains; mutable bindings, cycles, temporal
  dead zones, unawaited async factories and effectful initializers stay
  unknown. This does not analyze general module initialization effects;
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

What the extractor supports is listed in
[supported patterns](supported-patterns.md#concurrency-extractor-model-check-discover);
an unsupported construct becomes an `Unknown` operation and the scenario
is UNSUPPORTED or INCONCLUSIVE, never silently approximated.

* **Values:** `values(obj)` of a parameter object, re-executing a stored
  builder, `` sql`default` `` and other SQL values in `values(...)`; array
  mutation; RegExp outside the zod subset; destructuring with defaults,
  nesting or rest; decimals and array columns (dropped with a fact; a
  primary key of such a type refuses its table); time arithmetic and clock
  ordering; JSON comparisons and `->>`; collation-aware string equality
  ([value semantics](../concepts/value-semantics.md#approximations-kept)).
* **Statements:** row values of `UPDATE`/`DELETE … RETURNING` (counts and
  presence are modeled); a helper returning a mutation's rows
  (`return db.update(...).returning()`); raw SQL, subqueries, aggregates
  other than `count`, `orderBy` other than the primary key, multi-row mode
  with joins, `RIGHT`/`FULL`/`CROSS` joins, joins with locks, `findMany`;
  multi-row statements in flight.
* **Predicates:** predicates with `OR`/`LIKE`/functions are not satisfied
  by state synthesis. Held predicates are refused when a `let` is
  reassigned anywhere in the module (a textual check), for clock readings,
  nested conditionals, helpers that do more than `return <pred>`, async
  helpers, and operators re-exported or renamed through project modules
  ([held predicates](../architecture/source-extraction.md#drizzle-statements)).
* **Functions:** higher-order wrappers whose callback mapping is not known
  (call-returned functions, function factories, `export const f =
  wrap(async () => …)`); closures that are returned or stored; a closure
  used as an entry wrapper; dynamically created functions, functions in
  maps and class fields holding arrow functions must be selected by
  anchor; inherited methods (`class B extends A`), constructors that do
  more than store their parameters, objects modified anywhere in their
  module, `this` of an entry that is itself a method, dynamic dispatch;
  `.then`, timers, generators, recursion. The wrapper code around a
  callback entry is outside the model: its reads are recorded, a write
  there makes a runtime run inconclusive.
* **Loops** ([loops and errors](../architecture/loops-and-errors.md)):
  `for...in`, `for await`, labeled jumps; loops beyond 8 iterations
  (`MAX_UNROLL` 8 and `MAX_UNROLL_PRODUCT` 64 are constants, not options)
  are TRUNCATED; a `while` bound needs a known-integer counter start (a
  parameter or a database value gives TRUNCATED); writes after a TRUNCATED
  loop are blocked conservatively by discovery; backoff delays are not
  modeled.
* **Caught errors:** errors from lost connections, application error
  classes (`instanceof MyError`) and error messages are not modeled. Error
  shapes are verified only for drizzle-orm 0.45 with pg 8 or postgres 3.4;
  with drizzle-orm 0.45, `e.code` is undefined for every database error
  (the SQLSTATE is on `e.cause.code`), so an `e.code === "40001"` retry
  never runs.
* **Requests:** a second `request.json()`, `request.text()`,
  `formData()`, `arrayBuffer()`, `headers`, `url`/search params, passing
  `request` to a helper; a body that is not valid JSON; Server Actions
  with `FormData`; tRPC. `Response.redirect`, `Response.error` and
  streaming bodies are not modeled; `headers` and `statusText` are
  accepted but not modeled
  ([requests and responses](../architecture/source-extraction.md#requests-and-responses)).
* **zod** ([zod](zod.md#limitations)): refinements (`refine`,
  `superRefine`), transforms, preprocess, coerce, unions; elements of
  rebuilt arrays are opaque to synthesis; output key order is not modeled;
  the version lookup stays under the project root (Yarn PnP or a
  path-aliased zod gives an unknown version and version-dependent
  constructs are refused); `zod/v4` from zod 3.x is refused.
* **Packages:** `bcryptjs`/`bcrypt` are named by a verified contract but
  stay unknown; every other package call outside the documented contracts
  is unknown package behavior.
* **Schemas** ([schema extraction](../architecture/schema-extraction.md)):
  unrecognized default callbacks (`customIdService.generate()`,
  `process.env.X`, callbacks with side effects), `$onUpdate`,
  `default(sql…)` other than the recognized generators, dynamic schema
  declarations, Drizzle `alias()` self-joins, spread or helper-built
  columns, custom types; partial and expression unique indexes (a
  counterexample on a table with one is INCONCLUSIVE).
* **Handles:** union handle types (`Database | Transaction`) as entry
  parameters; `executeOnReplica`/`transactionOnReplica` and other members
  of a replicated database outside the declared contract; the React
  `cache` contract applies only to `cache` imported from `react`, and only
  at the first call.
* Return-value properties are never inferred; they must be declared.
* Entry discovery skips `export default`, export lists and re-exports.
* The static analyzer and the extractor share recognition and function
  identity only; each lowers TypeScript separately.

Discovery-specific ([scenario synthesis](../architecture/scenario-synthesis.md),
[world search](../architecture/world-search.md)):

* properties are hypotheses unless declared or schema-enforced;
  `SOURCE_ASSERTED` is never produced; recovery requirements ("an event is
  not lost") are never synthesized; write skew needs a declared invariant;
* pairing covers check-then-act and read-modify-write (plus the
  parent-child and declared return-value families) and truncates
  resources with more than six writers; more than two actors are not
  synthesized;
* state synthesis satisfies only the missing base row and first-level
  joined rows of a joined read; count reads are satisfied by adding rows
  until the guard passes (at most 16 iterations);
* guard satisfaction handles only comparisons of a read row's property or
  an argument path with a literal, integer orderings and `includes` of a
  literal; other guards (package calls, computed comparisons) stop
  synthesis, with the location and failure named;
* nullability comes only from annotations; literal-union annotations are
  followed through relative imports and re-exports only, not tsconfig
  `paths` aliases or packages; stored boundary worlds come only from
  integer comparisons in guards over read rows (time and text orderings
  are not varied); combinations of relationships are not explored;
* synthesis can leave an argument invalid when a regex's shortest example
  violates another check on the same string.

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

Not modeled ([refused or not modeled](../architecture/postgres-model.md#refused-or-not-modeled)):

* `SET CONSTRAINTS`, `MATCH FULL`, partial-index upsert arbiters
  (`targetWhere`), `SKIP LOCKED`/`NOWAIT` with a shared lock strength;
* partial and expression unique indexes as constraints (a partial index is
  a recorded protection only); a `NULLS NOT DISTINCT` rule involving a
  column the model does not represent (a counterexample on such a table is
  INCONCLUSIVE); collations and type-specific equality of unique keys;
* CHECK expressions beyond the simple subset, enum domains, triggers,
  row-level security, database functions;
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
  PostgreSQL 14 or newer is required for crash plans, 15 or newer for
  `NULLS NOT DISTINCT`.

## Unsupported runtime drivers and environments

* **Original-code verification:** Node.js + Drizzle 0.45.4 with
  node-postgres 8.23.1 or postgres-js 3.4.5; other postgres-js versions
  are refused (coordination relies on `Query.prototype.handle`); Neon, Bun
  SQL, other drivers and ORMs are not coordinated; Bun is not coordinated
  for source verification
  ([runtime verification](../architecture/runtime-verification.md)).
* **Crash planning:** crashes are planned only at observable checkpoints
  (an announced-and-held operation, or a confirmed lock wait). Refused: a
  crash while several tasks of one actor run (`Promise.all`), and a crash
  of an autocommit statement whose lock the model released but has not
  resumed. Statements without an application stack frame are linked by
  kind and table only; call-chain restoration across an un-awaited
  `return helper()` works only when unambiguous, otherwise the run may be
  inconclusive.
* **Replay harnesses** (single request): Node.js ≥ 22 (compiled
  JavaScript; replay never compiles or installs) and Bun ≥ 1.2; the driver
  adapter covers node-postgres only; statements are classified by their
  leading keyword (writes in CTEs or database functions are not
  checkpoint boundaries); savepoint statements, callback-style and cursor
  queries pass through without checkpoints. `processKill` kills only the
  client process (PostgreSQL keeps running: no database or machine crash);
  `exception` runs the application's `catch`/`finally` and rollbacks, so it
  can look harmless where a crash is not. Replay trusts the harness to
  emit a checkpoint only after its operation completed
  (`verifyBeforeInjection` can check this). `init-replay` writes a
  skeleton only: observations, invariants, schema, inputs and the scenario
  call are manual; its `tsconfig.json` suits plain `NodeNext` projects;
  the vendored replay client must be refreshed by hand after an upgrade
  ([replay protocol](../architecture/replay-protocol.md#failure-modes),
  [`docs/LIMITATIONS.md`](../LIMITATIONS.md#replay-runtimes-isolation-harness-generation-and-recovery)).
* **postgres-js:** cursors, `forEach`, streams unsupported; `sql.reserve()`,
  `listen`/`notify` and pipelined `sql.begin` arrays not exercised;
  driver-internal catalog reads are not held; lock waits attributed by
  `application_name`; `sql.end()` without a timeout did not return through
  the fault proxy (cause not established); the application must resolve
  the same `postgres` module instance as the harness.
* **node-postgres connection loss:** harnesses attach a client `error`
  listener (without one the process dies, recorded as diverged); broken
  client recognition relies on driver internals verified on 8.23.1.
* **Live connection loss** covers only a closed client socket after a
  COMMIT or autocommit write. Timeouts, `57P01` admin shutdown, TLS
  errors, partial frames, a server that closes first, and a connection
  broken in the middle of a transaction are not covered. A lock wait or
  deadlock combined with a connection loss was not run on either driver
  ([fault injection](../architecture/fault-injection.md#not-covered)).
* **Protocol proxy:** needs a Unix socket and the harness's own pool
  configuration; an application opening its own connection string is not
  covered. Pool capacity in the scenario must equal the harness pool's
  `max`. COPY, function-call messages and frames over 8 MiB mark the
  connection unsupported (the run is inconclusive); TLS/GSS encryption is
  never negotiated, so a client that requires it fails.
* Retries of an actor that shares a process with others are refused;
  connection loss requires one running task in the actor.
* Effects must reach a provider seam the harness can replace; the double's
  deduplication is declared; key expiry, partial provider failures and
  provider-side retries are not modeled.
* Framework internals are not modeled: the coordinator plays the HTTP
  client; error responses are not a separate step; the response step is
  inserted only for actors declared (or found) to be retried after a crash.
* **Isolation:** macOS `sandbox-exec` (deprecated by Apple), Linux
  bubblewrap. It protects against accidents in a trusted harness, not
  malicious code. On Linux the harness project and runtime are readable
  inside the sandbox, including secrets in project files (e.g. `.env`)
  and a live Unix socket inside the visible paths; on macOS processes that
  leave the process group can outlive the run (detached processes are
  swept); another process of the same user can connect to the private
  PostgreSQL socket during a run (trust authentication); no CPU, memory or
  temporary-disk cap beyond per-worker rlimits
  ([security](security.md)).
* **Platforms:** Windows builds have static analysis only (no `replay`)
  and get a smoke test, not the test suite. Required replay runs in CI on
  Linux x64 only; the macOS and Linux arm64 replay evidence comes from the
  development machine and its Debian 12 container, and the release run's
  full suite on the other targets. macOS (BSD tar) archives are not
  bit-reproducible. The npm packages were published by hand; their macOS
  and Linux binaries are byte-identical to the release archives, the
  Windows ones are not
  ([release status](../development/release-status.md)).

## Missing independent verification

* **No independent human review performed** — of any finding, label set,
  semantics, protocol or scenario.
* The static-finding precision review was done by the implementer from
  source alone; its blind re-review was an automated separate model
  session, not a human.
* The first-party discovery benchmark (p11) has the same author as the
  engine and was written with the detector families in mind (an upper
  bound, not an estimate); its held-out set stopped being held out after
  the first scoring.
* The p12 evaluation set is fresh and frozen but written by the
  implementation's author (not independent).
* The p13–p16 frozen sets and the PostgreSQL lock ground truth were
  produced by AI agents without access to the implementation:
  independence from the code, not from the project; the authors knew the
  supported subset of their time, so the sets are fresh but not blind.
  Post-feedback scores are not held out
  ([evaluation datasets](../research/evaluation-datasets.md)).
* After an original-code run, outcome terms (`actors`) are evaluated only
  when the plan has no crash, loss, restart or delayed completion and
  every observed outcome equals the model's.
* No external, corpus or private application was executed; no corpus
  finding is claimed as a defect; no maintainer was contacted. The blind
  review packets for first-party recovery findings remain unreviewed.
* Model-only results (replicas) have no measured counterpart.
* Unresolved intermittent test failures are recorded, not hidden: the
  oracle test `deadlock_workloads` (once, on Linux) and
  `claim-skip-locked-2/serializable`.
