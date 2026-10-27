# Phase 12 final report: schema fidelity, cross-module resolution, shared analysis context

Phase 12 targets the source-to-model pipeline, not the verification engine:
real Drizzle schemas with generated keys, schemas split over modules,
repositories and barrels, and the repeated project work that dominated
discovery. Every number below comes from a file under `reports/phase12/`.

**Snapshot.** Base commit `6657faf` (Phase 11 committed; clean tree; the
license edits were already committed). Phase 12 changes are **uncommitted**.
macOS: Darwin 27.0.0 arm64, rustc 1.98.1, Node 22.23.2, PostgreSQL server
17.11 (client tools 18.6). Nothing was committed, pushed or published.

Documents (the plan's 17 consolidated into 10): `BASELINE.md`,
`ARCHITECTURE.md` (architecture, schema composition, cross-module resolution,
shared semantic index, incremental invalidation),
`GENERATED-VALUE-SEMANTICS.md` (generated values, Drizzle defaults,
RETURNING), `POSTGRESQL-DIFFERENTIAL.md` (PostgreSQL/Drizzle differential,
source-model fidelity), `REAL-WORLD-COVERAGE.md`, `INDEPENDENT-EVALUATION.md`,
`PERFORMANCE.md`, `SECURITY.md`, `LIMITATIONS.md`, this report.

## 1. Implementation

* **Generated values** (`model::value::Value::Gen`, `db::ColumnDefault::Generated`):
  identity is the generation event `(generator, actor, per-actor counter)`;
  equality is identity; no order; assigned when the INSERT starts executing;
  a retry, resubmission or rolled-back attempt generates new values; never
  initial data; part of the canonical state only when used. Explicit
  assumption `generated-values-distinct` (no collision exploration).
* **Generator classes** (`schema_facts::GeneratorClass`): application-,
  database-, sequence-, identity-generated, constant, unknown — every default
  classified with provenance. Recognized application generators are a narrow
  import-checked list; `$defaultFn(helper)` from another project module is
  resolved; `customIdService.generate()`, `process.env.X`, look-alike names,
  unbound `crypto.randomUUID` and shadowed `crypto` are not.
* **RETURNING**: `returning({ alias: t.col })` → `RETURNING col` plus a local
  projection; generated keys flow by identity into later inserts and foreign
  keys.
* **Schema composition**: scenarios list several schema modules and name
  tables `module#binding`; tables bind by **resolved declaration**
  (aliases, barrels) instead of by name; foreign keys resolve by declaration;
  duplicates and ambiguity are refused, not merged; discovery emits the
  dependency slice of modules.
* **Cross-module resolution**: re-exports (`export {…} from`, `export *`,
  `export * as`), const object repositories, class instances with
  constructor-injected handles (`this.db`), `this.m()`, `getDb()`-style
  factories and typed `db` parameters recognized from declared
  `drizzle-orm` types; transaction handles survive repository calls.
* **Shared project context** (`analyzer::context::ProjectContext`): one
  module discovery, one resolver, sources read once, compact module
  summaries, memoized export resolution, closures and type facts — reused by
  every extraction of a run.
* **Incremental cache**: with declaration identity, an entry's key covers
  only the schema declarations its import closure reaches; a per-run memo
  digests each file once.
* **Discovery**: new `resubmitted-write` family; distinct-key controls vary
  the parameters behind read keys; synthesis never seeds rows the execution
  creates, gives `uuid` parameters valid UUIDs, covers every table it seeds;
  ambiguous-commit retries are reachable under their fault; a
  `foreign-key-locks` model-limitation assumption where PostgreSQL's FK
  check lock can make a model schedule unrealizable.
* **Unsupported results stay unsupported**: an unsupported statement that
  would assign a local records it (`Op::Unknown.defines`) so scenarios load
  and the path ends as unsupported (no invented value).
* **Provenance**: a helper's return slot is attributed to its call site.

## 2. Correctness

### Quality gates (final tree)

Source digest (`scripts/phase8-digest.py`, 702 files):
`803fbbd12f6bc6f1ccd9ddef2e28a7c12b7709e9402d380f4fa753abab6c9881` (equal
before and after the test run, and on Linux); release binary sha256
`7da740b2f5f3618f…` (`reports/phase12/final/`).

| gate | macOS arm64 |
| --- | --- |
| `cargo fmt --all -- --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace --no-fail-fast` | exit 0: **606 passed, 0 failed, 3 ignored** (25.6 min, `reports/phase12/final/test.txt`) |
| `cargo build --release` | exit 0 |

606 = the 583 of the Phase 11 baseline (re-established at the start:
`BASELINE.md`) + 23 new. Ignored: `regenerate_fixtures`, `dump_seed`,
`dump_ir` (debugging tools, as before). A mid-phase checkpoint run
(`reports/phase12/checkpoint/test.txt`) had 603 passed and 2 failures —
the `@expect` fixture harness walking the new fixtures, and a panic when
wrapper analysis resolved a method call before the entry's frame existed —
both fixed before the final run. A first final run at digest
`c2fb78df…` (606 / 0 / 3 on both platforms, `test-run1-c2fb78df.txt`,
`reports/phase12/linux-run1-c2fb78df/`) was superseded after the Linux run's
report-schema validation found the `schemaFacts[].module` field; the gates
above are the rerun after that fix.

### Compatibility (Phase 11 binary vs Phase 12, same inputs)

| | result |
| --- | --- |
| static analysis | **144 / 144** corpus outputs byte-identical (4 projects × depths 3, 6, 12 × 3 path modes × json, sarif, text, trace) — `reports/phase12/final/corpus-identity.txt` |
| model checking | **56 / 56** Phase 6–9 scenarios with identical verdicts, state and transition counts, schedules and text reports; JSON byte-identical for 43; the other 13 differ only in `exploration.metrics.visitedBytes` (9: the 4-byte generation counter in each actor state) and the source attribution of the helper return-slot event (4: the intended provenance correction) — `model-identity-classified.txt` |
| Phase 11 benchmarks | first-party model prediction TP 8 FP 1 FN 0 TN 9 (0.889 / 1.000) and static 0.800 / 1.000 — unchanged; held-out (post-fix) TP 7 FP 0 FN 1 TN 4 (1.000 / 0.875) — unchanged (`reports/phase12/regression/phase11-benchmarks.md`) |
| report schemas | model-check schemas unchanged (every Phase 12 runtime report validates against v3 — `reports/phase12/final/evidence-schemas.txt`; a first Linux run caught a `schemaFacts[].module` field v3 forbids, now internal-only, and a test pins it); additive optional fields only: `SchemaColumn.generation`, `ExtractedSchema.imports`, `StmtKind::Insert.returningColumns`, `Op::Unknown.defines`, discovery `index.generatedColumns`; a value `{"$generated": …}` appears only in models with generated columns; discovery cache version 2 |
| runtime verification | Phase 7–11 source-verified scenarios: covered by the final test run (section above) |

### Differential evidence

* **Drizzle/PostgreSQL probes** (10 probe programs, `POSTGRESQL-DIFFERENTIAL.md`):
  `$defaultFn` runs once per row at every execution of a builder (also for a
  failing insert); database defaults are sent as `default` and exposed by
  RETURNING; sequences are not rolled back; application keys of rolled-back
  or retried work are not reused.
* **Model vs PostgreSQL on the same TypeScript**: **6 / 6** supported probe
  programs end in the same rows (generated values matched one-to-one).
* **Lockstep oracle** with generated keys and a unique email: agrees.
* **Mismatches**: none between PostgreSQL and the generated-value semantics.
  One model limitation found (foreign-key check locks, below) and four
  harness/synthesis defects fixed (RETURNING columns, generated-value
  matching, UUID arguments, self-created parents).

### Original-code reproduction (first-party `workspace-app`, discovered automatically)

`reports/phase12/evidence/SUMMARY.md`, `--source-repeat 3`:

| candidate | evidence | runs |
| --- | --- | --- |
| check-then-act `createWorkspace` × 2 (object repository → INSERT, `$defaultFn` key, RETURNING → class repository INSERT with the returned key as FK) | `original-code-reproduced` | 3 / 3 |
| check-then-act through the wrapped Server Action (`createWorkspaceAction` × 2) | `original-code-reproduced` | 3 / 3 |
| check-then-act `createWorkspaceAction` × `createWorkspace` | `original-code-reproduced` | 3 / 3 |
| resubmitted-write `addWorkspace`: crash after the INSERT, declared resubmission — the retry **regenerated** the key (two workspaces, the membership on the second) | `original-code-crash-reproduced` | 3 / 3 |
| check-then-act `createWorkspace` × `createWorkspaceLocked`, and the same with the Server Action | **diverged** (0 / 3 each): PostgreSQL's foreign-key check on the unlocked actor's INSERT waits for the other actor's `FOR UPDATE` on the referenced `users` row; the model has no FK locks. The invariant is violated in every run, along a different schedule. Stated on the candidates (`foreign-key-locks`) and pinned by a regression test | — |

The safe counterparts are `NO_VIOLATION_WITHIN_BOUNDS`: `createWorkspaceLocked`
alone, `inviteToWorkspace` (unique index, `tx` passed to a repository),
`addWorkspaceIdempotent` (request id as key: the resubmission collides).

## 3. Real-world coverage (`REAL-WORLD-COVERAGE.md`)

| project | tables modeled (11 → 12) | tables refused | effectful entries | unknown ops | candidates | model-checkable | wall | peak RSS |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 5 → 5 | 0 → 0 | 8 → 8 | 274 → 284 | 2 → 2 | 1 → 1 | 0.06 → 0.03 s | 11 → 11 MiB |
| midday | 8 → **50** | 42 → **0** | 0 → 0 | 15 382 → 17 603 | 0 → 0 | 0 → 0 | 28.07 → **1.13 s** | 73 → 112 MiB |
| school-ledger | 9 → **30** | 21 → **0** | 0 → **21** | 9 029 → 12 894 | 0 → **9** | 0 → 0 | 3.36 → 0.53 s | 50 → 81 MiB |
| unnamed-ai-project | 7 → **9** | 2 → **0** | 1 → **3** | 1 009 → 1 009 | 0 → 0 | 0 → 0 | 0.07 → 0.03 s | 11 → 12 MiB |

**Recognition increased; verification precision did not change on the
corpus.** All 65 refused tables are modeled through the general generator
mechanism. Midday's effects stay unbound for a demonstrated reason (its
database handle is a `withReplicas` database typed through
`ReturnType<typeof …>`; 752 operations; replica reads are not modeled).
School-ledger's new effects reach 9 candidates, all honestly
`UNSUPPORTED` (each actor reaches an unsupported operation first). No new
corpus counterexample; no threshold was tuned for the corpus; no corpus
result is claimed as a defect (no independent review).

## 4. Performance (`PERFORMANCE.md`)

* midday discovery with model checking: **28.07 s → 1.13 s**; extraction
  27 323 → 873 ms; 24.03 → 0.97 ms per extracted entry, with more work per
  entry. School-ledger 3.36 → 0.53 s.
* Cache: cold 34.39 → 1.54 s, warm 7.21 → 1.06 s (1 168 hits, identical
  candidates); schema changes invalidate only entries that reach the module.
* Memory: peak RSS +39 MiB on midday (sources and summaries kept for the
  run); per-structure sizes not instrumented.

## 5. Evaluation (`INDEPENDENT-EVALUATION.md`)

Fresh 15-entry application and labels, frozen (SHA-256 recorded) before the
first scoring; same author as the implementation — **not independent**.

| | TP | FP | FN | TN | precision | recall |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Phase 12 set, **first scoring** | 4 | 0 | 3 | 6 | 1.000 | 0.571 |
| Phase 12 set, after fixes prompted by it (**not held out**) | 5 | 0 | 2 | 6 | 1.000 | 0.714 |

Stages (first scoring): schema 6/6 tables, 4/4 generated keys; 15/15
entries extracted; 14/15 effectful (the loop entry has none); 6/7 positives
with a candidate of the labeled class; every counterexample validated;
2/2 unsupported reported; original-code reproduction not measured (no
harness). Remaining false negatives are model limits: return values are not
modeled (`claimTask`), and there is no crash point between a single
statement's commit and the response (`recordSignup`).

## 6. Cross-platform

Both platforms ran the same source (digest `803fbbd1…`, 702 files).

| | macOS arm64 (Darwin 27.0.0) | Linux aarch64 (Debian 12 container, `scripts/phase12-linux-docker.sh`) |
| --- | --- | --- |
| toolchain | rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 | rustc 1.99.1, Node 22.23.4, PostgreSQL 15.19, bubblewrap 0.8.0 |
| fmt / clippy `-D warnings` | 0 / 0 | 0 / 0 |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **606 passed, 0 failed, 3 ignored** | **606 passed, 0 failed, 3 ignored** |
| release build | 0 (`7da740b2…`) | 0 (`13366dec…`) |
| Phase 12 original-code evidence | 4 / 6 reproduced 3 / 3; 2 diverged (foreign-key lock) | identical: 4 / 6 reproduced 3 / 3; 2 diverged |
| report-schema validation of the runtime reports | 0 errors | 0 errors |

Covered on both by the test suite: UUID/default extraction, application
default generation (Drizzle probes against PostgreSQL 17.11 and 15.19),
multi-module schema resolution, cross-module call resolution, RETURNING,
Source-DPOR regressions (Phase 10 suites), existing original-code replay
(Phase 7–11 suites), the new generated-ID runtime scenarios, report schema
validation, HTML escaping and cache invalidation. Raw logs:
`reports/phase12/linux/`. The evidence script exits 1 on both platforms by
design (not every counterexample reproduced: the two diverging pairs).
Windows was not run.

## 7. Tests added

* analyzer `tests/phase12.rs` (10): default classification with negative
  cases; imported `$defaultFn` helpers; identity binding through aliases and
  barrels; ambiguous and cyclic re-exports; database factories and typed
  parameters (and a look-alike that is not one); object and class
  repositories (and modified objects, inherited methods); transaction
  handles through repositories; projected RETURNING; return-slot provenance;
  multi-module scenario loading and refusals.
* model `tests/phase12.rs` (7): generated-value equality/order/serialization;
  RETURNING identity into a child row; distinct values across actors and
  after rollback; regeneration on retry (model counterexample); generation
  counters in the state identity; refusal as initial data; PostgreSQL
  lockstep oracle with generated keys.
* CLI `tests/phase12.rs` (5): the Drizzle/PostgreSQL differential (6
  programs + runtime facts); shapes the model does not represent are
  unknown; discovery on the workspace application (tables, controls,
  locked/unique/idempotent counterparts); original-code reproduction of the
  generated-ID counterexamples (3 × 2 runs); the foreign-key-lock divergence
  is stated and detected.
* discover `tests/discovery.rs` (1): a schema-module change invalidates only
  the entries that reach it; cached = uncached.
* Updated: the `@expect` harness excludes `fixtures/phase12`;
  `TableBinding`/`StmtKind::Insert` construction in existing tests gained the
  new optional fields.

## 8. Limitations (`LIMITATIONS.md`)

Generated values are assumed distinct (no collision mode); their order is
not PostgreSQL's; foreign-key locks, read replicas, return values, a
response event, multi-row inserts, stored builders, `$onUpdate`, inherited
methods and package calls are not modeled; the evaluation set is not
independent and there was no independent review; original-code evidence is
first-party only.
