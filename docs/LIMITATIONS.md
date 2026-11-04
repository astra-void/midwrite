# Limitations

> **Current limitations:** [`reference/limitations.md`](reference/limitations.md)
> consolidates this page and every phase's `LIMITATIONS.md`, with the
> limitations later phases lifted. This page is kept as the detailed record
> for the static analyzer (Phases 1–7) and is not updated per phase.

Midwrite is a narrow analysis. This page lists what it gets wrong or does
not attempt, so that a clean run is not mistaken for a guarantee. Many
entries are pinned by tests or fixtures (named in parentheses). Measured
rates on real projects are in `docs/phase2/CORPUS-EVALUATION.md`.

## What a result means

* A diagnostic is a **potential** partial-completion hazard derived from code
  structure. Midwrite cannot know whether the application tolerates the
  partial state, or whether a retry, reconciliation job or idempotent consumer
  outside the function repairs it.
* No diagnostic is reproduced at runtime (`"reproduced": false`). Runtime
  evidence exists only for findings exercised by an authorized replay
  harness, in a separate verification report, against invariants the harness
  declares; see [`phase3/`](phase3/FINAL-REPORT.md) and
  [`phase4/`](phase4/FINAL-REPORT.md). Runtime reproductions so far are of
  first-party fixtures (including one on the real Drizzle ORM) and of one
  real-world pattern in an isolated re-write; no bug in an original
  application has been runtime-confirmed.
* No result proves consistency. Midwrite does not replace database
  constraints, integration tests, or fault-injection testing.
* In the Phase 2 review of 103 findings on four real projects (97 with a
  decided label), the reported window existed in 92 cases, but only 27 were
  judged likely bugs; most of the rest were deliberate (best-effort
  notifications, operations repaired by job retries, orderings documented in
  the code). Read findings as review candidates. The review was done by the
  Phase 2 implementer from source code alone, not by the projects'
  maintainers, and nothing was reproduced at runtime. A blind automated
  re-review of 18 of those findings in Phase 3 (a separate model session,
  not a human) agreed with 3 of the 7 sampled "likely bug" labels; see
  [`phase3/PRECISION.md`](phase3/PRECISION.md).

## Support levels

| Level | Patterns |
| --- | --- |
| Fully supported (tested end to end) | Drizzle writes/reads/transactions on recognized receivers, including handles returned by a configured call (`getDb()`); savepoints; `tx` vs `db` inside callbacks; same-file and imported function summaries (relative imports, named/aliased/default/namespace imports, `export { } from`, `export *` barrels); methods of `const` object-literal services; transaction handles and root handles passed as direct identifier arguments or configured calls; contradictory simple conditions, including literals returned by an inline transaction callback; baselines; policies; best-effort effects. |
| Supported within a documented subset | `tsconfig.json` `paths`/`baseUrl` (single `*`, relative `extends` only); workspace packages (`package.json` `name` + `exports` string targets, single-`*` patterns, conditions `types`/`import`/`module`/`default`/`require`/`node`, or `main`/`module`/`types`); constant conditions on `const` literals. |
| Supported within a documented subset (Phase 5) | Methods of receivers whose class is known (`new C()`, `this` without analyzed overrides, static, `super`), across modules; callbacks invoked by analyzed functions (per call-site context, ≤ 8 contexts per callee); IIFEs; resolved calls inside `Promise.all`; `Promise.all(xs.map(cb))` / `xs.forEach(cb)`; inertness summaries with modeled builtins; return-value facts and property facts of unescaped objects; value provenance and `valueRelation`; loop value fixpoints. See [`phase5/`](phase5/FINAL-REPORT.md). |
| Heuristic | Configured names and member paths (trusted as written); `Promise.all` treated as concurrent; loops as zero or one iteration for effects; catch handlers entered as if the `try` failed before its effects; method dispatch under a package superclass (recorded assumption, medium confidence); `map`/`forEach`/`flatMap` with a function argument assumed to run it per element without awaiting; modeled globals assumed not replaced; `console` output not an effect. |
| Unsupported | Methods on values of unknown class (parameters, SDK clients, factory results), callbacks run by unanalyzed code, `.then/.catch/.finally` continuation bodies, captured variables and transaction handles inside callbacks, dynamic `import()`, CommonJS `require`, `node_modules` packages, handles from unconfigured calls or calls with arguments (`getDb("replica")`), raw SQL, other ORMs, labeled `break`/`continue`, generators' resumption. |

## Known false positives

| Case | Why | Workaround |
| --- | --- | --- |
| A parameter or global named like a configured receiver (`db: Map<...>`) | Bare names are trusted unless the same file shows a contrary initializer (`analysis.rs: known_false_positive_parameter_named_like_receiver`). | Use `importedFrom`. |
| A configured member path that holds something else | Only the names along the path are checked; such findings are capped at `medium` confidence. | Configure member paths only where they always denote the database or client. |
| Correlated conditions on members of objects from parameters or unknown calls, calls, or reassigned variables (`if (input.a) w(); if (!input.a) p();`) | Only variables that are never reassigned, and (Phase 5) properties of unescaped objects without accessors (literals, rows, analyzed callees' results), are correlated (`phase2.rs: members_calls_and_complex_conditions_stay_unknown`, `phase5.rs: member_tests_on_unknown_objects_are_not_correlated`). | — |
| Correlation through database state (`getOrCreate()` whose insert cannot run because the row was just created) | Database contents are not modeled (one reviewed false positive in the corpus). | — |
| Correlation through return values other than literals and object-literal properties (`return computeOk()`), or when the result escapes (`audit(result)`) | Phase 5 correlates a summarized helper's returned literals and literal properties with the caller's `const` binding (`vulnerable/return_value_correlation.ts`); anything else is not a fact (`phase5.rs: an_escaping_result_is_not_correlated`). | — |
| An object service whose properties are changed outside its file, or through `Object.assign` | Only assignments and `delete` of its properties in its own file disable it. | — |
| Adjacent but unrelated operations (a notification, then an unrelated update; a write in one loop and an enqueue for other records in a later loop) | A failure between *any* two adjacent durable or visible points is reported; whether the second is owed because of the first is not known (four reviewed false positives). | — |
| Effects already compensated elsewhere (job retries, webhook redelivery, idempotent relays, documented manual recovery) | Not visible to the analysis (most of the reviewed "intended risk" findings). | Declare them (`externalEffects[].recovery`, function-level `recoveryContracts` with cited evidence: the finding stays, as `info` under `recommended`), baseline them, or use the `recommended` policy for MW1004/MW1005. |
| Swallowed failures | An error caught and ignored is treated like any other path. | — |
| Test setup and seed scripts | They commit in sequence on purpose; in one project they produced all MW1004 findings. | `"exclude": ["tests/**", "scripts/**"]`. |

## Known false negatives

| Case | Behavior |
| --- | --- |
| Effects inside methods whose receiver's class is not known (`service.save()` with `service` a parameter or a factory result), methods with an analyzed override or dynamic reassignment, callbacks run by unanalyzed code | Not followed. A call after a recognized commit becomes MW9001 with the reason; one before the first commit is silent (reported under `--policy audit`). Known receivers are followed since Phase 5 (`vulnerable/class_this_dispatch.ts`, `unsupported/class_override_dispatch.ts`, `projects/class_hierarchy`). |
| Callees that are resolved but not inert and perform no recognized operation (`render()` returning a package call, `BaseProcessor.updateProgress` calling BullMQ) | One unknown effect for the call, as in Phase 4; the detail says which inner call prevents inertness, but the inner unknown is not shown as its own trace step. |
| Captured values in callbacks (`items.map((i) => tx.update(...))`, `{ id }` from the enclosing function) | The callback's own lowering does not know the enclosing frame: a captured transaction handle is not recognized (the call stays unknown) and captured values have unknown provenance. |
| Effects more than `--max-call-depth` (3) calls deep | Still unknown unless the chain is inert. Measured on school-ledger: depth 6 would add 261 findings; the default is unchanged pending review (`phase5/CORPUS-EVALUATION.md`). |
| Imports that do not resolve: packages outside the analyzed files, unsupported aliases, ambiguous modules or exports, exports that are not function declarations (`export const f = wrap(...)`) | The call stays an unknown effect whose detail says why (`projects/missing_module`, `projects/ambiguous_exports`, `phase2.rs: ambiguous_module_candidates_are_not_chosen`). |
| Calls neither awaited nor returned from an `async` function (`notify(id);`) | Not summarized: silent (`unsupported/unawaited_helper.ts`, `projects/unawaited`). Resolved calls inside `Promise.all` are summarized since Phase 5 (concurrent, low confidence). |
| Callees without a recognized operation that are not proven inert, recursive calls, calls more than `--max-call-depth` (default 3) levels deep, callees with unsupported control flow | Not summarized; the call stays an unknown effect, MW9001 after a commit (`unsupported/local_helper_unknown.ts`, `unsupported/recursive_helper.ts`, `unsupported/deep_helpers.ts`). Inert callees are resolved at any depth and in recursion since Phase 5 (`safe/inert_helpers.ts`, `projects/pure_helper`). A recursive call cut inside an inlined callee is not reported separately (`projects/recursive_imports`). |
| Helpers that return an unexecuted query builder for the caller to await | The helper executes nothing itself, so its summary is empty: MW9001 after a commit. |
| Handle arguments that are not a direct identifier, configured path or configured call (`save(tx ?? db)`, `save(getDb())` without a `"getDb()"` entry, spread arguments) | The callee's writes on that parameter become unknown effects (`projects/unknown_handle`); never assumed to be root writes. |
| `tx.rollback()`, raw SQL and transactions on an *unconfigured* parameter of an inlined callee | Not modeled through the parameter. |
| Database handles obtained from a call that is not configured (`getDb().insert(...)` without a `"getDb()"` entry), or from a call with arguments | Not recognized. With `"getDb()"` in `databaseReceivers`, direct chains, `const` aliases and arguments are recognized (`vulnerable/call_returned_handle.ts`). |
| Unconfigured external effects before the first recognized commit | Silent by default; `--policy audit` reports unknown calls before the first recognized operation. |
| Member receivers whose path is not configured (`this.repo.db`) | MW9001 with the path to configure (`unsupported/unconfigured_member_receivers.ts`). |
| Member receivers reached through aliases or destructuring (`const database = this.db`), computed or private members | Not matched against member paths. |
| Loops | Effects modeled as zero or one iteration (no MW1004 between iterations); values are iterated to a fixpoint with widening. |
| Top-level module code | Not analyzed. |
| Raw SQL (`db.execute(sql\`...\`)`), `db.batch(...)` | MW9001; the SQL is not parsed. |
| Writes via `let`-bound aliases or reassigned variables | Only `const` aliases are followed. |
| Destructured transaction handles, non-inline transaction callbacks | MW9001 (`unsupported/destructured_handle.ts`, `unsupported/opaque_transaction.ts`). |
| Implicit failure points inside a `try` | The catch handler is entered as if the failure happened before the block's first effect, or at an explicit `throw`. |
| Other ORMs, other databases, SQLite executors (`.run()`, `.all()`, `.get()`) | Not recognized. Drizzle on PostgreSQL only. |

## Unsupported constructs and truncation

| Construct | Handling |
| --- | --- |
| Labeled `break` / `continue` | The path stops and MW9001 is reported if the function has recognized effects (`unsupported/labeled_break.ts`). |
| More than `--max-paths` paths in a function (default 512) | The first paths are analyzed, the function is counted in `summary.analysis.truncatedFunctions`, and MW9001 is reported. Findings from such a function are partial. Inlined callees count toward the caller's budget. |
| Optional calls (`queue?.publish()`) | Treated as unconditional. |
| Generators, `yield` | Analyzed like ordinary functions; resumption is not modeled. |
| `with` statements | Lowered without special scoping. |
| Unawaited `db.transaction(...)` | Lowered as a transaction with `fireAndForget` execution; findings that depend on its commit get at most `medium` confidence (`phase2.rs: transaction_model_audit`). |

## Recognition trust model

* `databaseReceivers` and `externalEffects` are trusted declarations. Matching
  uses the root identifier after following `const` aliases and renamed imports.
* An entry may be a member path (`this.db`, `ctx.db`, `stripe.charges`,
  `this.queue`). Its root is resolved like a bare name (a contrary initializer
  disables it; `importedFrom` checks the root's import), `this` is never
  resolved, and the properties after the root are trusted as written. The
  longest configured path that matches wins. Such matches are reported as the
  weakest basis, `member-path match`, and cap confidence at `medium`.
* Name-based matches are disabled when the binding is declared in the same file
  with an obviously different value: a literal, object or array literal, `new`
  expression, function, or class.
* `importedFrom` compares the import specifier verbatim (`"@/db"` and
  `"../db"` are different modules); path aliases are not resolved for this
  check (they are for following calls).
* Handles created by `drizzle(...)` imported from a `drizzle-orm*` module are
  recognized by provenance regardless of name.
* There is no type information. A Drizzle-looking chain on a recognized
  receiver is assumed to be Drizzle. A Drizzle-looking read or write on a
  parameter becomes a database operation only where a caller passes a
  recognized handle for that parameter.

## Call summaries

* An awaited call (or an `async` return) of a function declared in the same
  file, or imported from an analyzed file, is analyzed by inlining that
  function's summary: its own lowered body, with its own calls inlined in
  turn, at most `--max-call-depth` levels deep and never recursively.
  `let`/`var` bindings, shadowed names and function declarations that are
  reassigned anywhere in the file are not followed.
* Arguments are bound to parameters only for database handles passed as
  direct identifiers or configured receiver paths. Other values are not
  tracked, the callee's `this` is unrelated to the caller's, and branch facts
  of the callee never constrain the caller.
* An operation the callee performs only on some of its paths (counting paths
  that end in a `throw`) is reported with at most `medium` confidence. When
  the callee has more paths than `--max-paths`, no operation counts as
  performed on every path.
* A hazard between two operations of the same inlined call is reported only in
  the callee's own analysis, and unknown calls inside one inlined call are
  reported in the caller once, only when the caller's own commit or
  transaction precedes them.
* Inlined bodies count toward the caller's `--max-paths` budget.

## Approximations in the path model

* Paths are assumed feasible unless two branch decisions contradict each
  other on a variable that is never reassigned (`flag`/`!flag`, different
  literals in `===` tests or `switch` cases, `const` literals). There is no
  general constraint solving: `a > 1` and `a <= 1` are both feasible.
* Every awaited operation may fail; the analysis does not distinguish
  operations that cannot fail.
* `Promise.all` elements are treated as started in source order; their
  completion order is undetermined (findings get `low` confidence).
* Unawaited external calls and transactions are placed where they start;
  their completion is unordered with respect to later operations (`medium`
  confidence).
* When a callee returns early on some paths, the caller keeps only one
  representative of the callee paths that have the same effects; the path
  conditions shown are those of that representative.

## Failure traces and runtime replay (Phase 3)

* A failure trace is the reported path, not proof that it is reachable:
  branch conditions are not solved (only contradictions are pruned), and
  operations are what recognition says they are.
* `incomplete` traces (an unknown operation inside the failure window,
  unsupported control flow) are not reproduction plans. They are common in
  large projects (midday 47 of 180, school-ledger 93 of 123).
* Checkpoint names in traces are hints; the harness decides where
  checkpoints are, and replay trusts that a checkpoint is reported only after
  its operation completed (`verifyBeforeInjection` can check this).
* The crash simulation models only a process crash; exceptions run
  application handlers that the model does not execute.
* Replay supports one database provider (a disposable local PostgreSQL
  cluster). Phase 4 status of the other Phase 3 limits is in the next
  section.
* `RUNTIME_NOT_REPRODUCED` holds for the harness, its declared invariants and
  the recorded environment only.
* Recovery declarations (`recovery`) are assertions, never verified; the
  `relationship` field is a naming heuristic that did not separate bugs from
  non-bugs on the reviewed corpus and is never used for severity.
* Class methods and `this.method()` are still not followed.

## Replay, harness generation and recovery (Phase 4)

* Runtimes: Node.js ≥ 22 (compiled JavaScript; replay never compiles or
  installs) and Bun ≥ 1.2. The driver adapter covers node-postgres (`pg`)
  only; postgres.js, Neon, Bun SQL and other drivers need their own adapter.
  Statements are classified by their leading keyword: writes inside CTEs or
  database functions are not checkpoint boundaries.
* Isolation: macOS `sandbox-exec` and Linux bubblewrap with unprivileged user
  namespaces. On Linux, the harness project and the runtime installation are
  readable inside the sandbox, and a live Unix socket inside them would be
  reachable; on macOS, processes that leave the harness's process group can
  outlive the run. Neither platform limits CPU or memory.
* Linux CI on GitHub-hosted runners and the release workflow have not run
  yet; Linux was exercised in an unprivileged Debian container.
* `midwrite init-replay` generates a skeleton; observations, invariants,
  schema, inputs and the scenario call are always manual. Its `tsconfig.json`
  suits projects that compile with plain `NodeNext` settings.
* Recovery contracts are assertions with cited evidence; Midwrite does not
  verify that the mechanism exists, covers the window or runs after a crash.
  Runtime recovery observations hold for the harness only.
* The 140 incomplete corpus traces are all caused by unknown calls inside the
  failure window; their number did not change in Phase 4.
* No human review of corpus findings exists yet (a blind review workflow is
  ready: `midwrite-corpus review-packet`).

## Kinds of evidence (Phase 5)

| Kind | Meaning | Where it shows |
| --- | --- | --- |
| Recognized operation | a Drizzle write/read or configured external effect matched by syntax, provenance or configuration (never by types) | `effects`, `evidenceStrength` |
| Must-effect (within the model) | performed on every modeled path of the callee (`SummaryOrigin.onEveryPath`) | full confidence; otherwise ≤ medium |
| May-effect | performed on some modeled paths | ≤ medium, "only on some of its paths" note |
| Inferred call target | resolved by name/import (Phase 2), or by dispatch, callback context or IIFE (Phase 5) | `CallSite.resolution` in IR and traces |
| Assumed target | dispatch that relies on an unanalyzed superclass or an escaped receiver | `resolution.assumption`, ≤ medium, limitation note |
| Proven inert | the callee and everything it may run perform nothing durable or visible on any modeled path, given the modeled builtins | `CallScope.inert`; no MW9001 |
| Configured assumption | `databaseReceivers`, `externalEffects`, `bestEffort`, `recovery`, `recoveryContracts` | evidence and limitations; never verified |
| Unknown behavior | an operation whose semantics are not known | `Unknown` effect, MW9001, incomplete trace |
| Abstract execution prediction | everything above: a static, bounded model of some paths | every diagnostic and trace (`STATIC_CANDIDATE`) |
| Runtime-observed evidence | observed PostgreSQL state in an authorized replay | verification reports only; never feeds back into static confidence |

## Phase 5 soundness assumptions

1. Closed world for classes: subclasses exist only in the analyzed files.
2. Modeled globals (`Promise`, timers, `Math`, `JSON`, `Object.keys`, …) are
   not replaced; `console` output is not a consistency-relevant effect;
   `drizzle-orm` query helpers (`eq`, `sql`, …) build expressions only.
3. Property reads run no code (getters are ignored except that an accessor
   makes its object open; member facts are restricted to objects without
   accessors that nothing visible can change).
4. Prototype and instance mutation are detected only syntactically in the
   declaring files (`this.m =`, `C.prototype.m =`, `Object.defineProperty`
   and friends).
5. Recognized effects do not mutate their arguments.
6. Module-level data is static (`Static` atoms are neither inputs nor
   unknown).
7. All Phase 1–4 assumptions still hold.

## Phase 6: concurrency model checking

`midwrite model-check` is opt-in and separate from `check`. Its results
hold only for the declared scenario (actors, data, isolation level, bounds)
and the model's semantics; SERIALIZABLE is a conservative serialization-graph
abstraction, and the TypeScript and SQL subsets are small. Full list:
[`phase6/LIMITATIONS.md`](phase6/LIMITATIONS.md).

## Phase 7: source selectors and original-code verification

Wrapped callbacks are modeled from their own body; the wrapper's guards are
declared preconditions, not proofs. `original-code-reproduced` holds for the
harness inputs, the schedule and the disposable database of the run, not
for production traffic. Only Node.js + `pg` + Drizzle are coordinated;
crashes and retries are not executed against original code. Full list:
[`phase7/LIMITATIONS.md`](phase7/LIMITATIONS.md).
