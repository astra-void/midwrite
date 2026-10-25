# Phase 6 final report

Question: *can CommitRift find a concrete bounded schedule under which
independently correct-looking operations violate a declared invariant, and
explain why it is possible?*
Measured answer: **yes, for a small, precisely documented subset.** From
TypeScript source, the model checker finds lost updates, write skew,
check-then-act duplicates, double processing and duplicate charges after a
crash and retry. It shows each as a minimized, replayed schedule with
source locations. Replaying those schedules on PostgreSQL 17.11 reproduced
every one: every predicted statement outcome matched. Across 1 695
differential schedules, the model's isolation semantics agreed with
PostgreSQL 17.11 (macOS) and 15.19 (Linux) in all of them, after two model
corrections that the differential tests forced. Ordinary `commitrift check` is byte-for-byte
unchanged and no slower.

Measured on 2026-10-25 on one Apple Silicon Mac (macOS arm64, PostgreSQL
17.11) and in a Debian 12 container (Linux 7.0.12, aarch64, PostgreSQL
15.19) on the same machine. Raw data: `reports/phase6/`.

## 1. What was built

| Area | Implementation | Document |
| --- | --- | --- |
| Concurrency IR | actors, tasks (`Promise.all`, fire-and-forget), statements with transaction ownership, branches, external effects, in-process state, `Unknown`; source provenance, Effect IR effect ids and linked findings on every operation | [`CONCURRENT-IR.md`](CONCURRENT-IR.md) |
| Extraction | `TsActorProvider` (analyzer): Drizzle subset → IR, helper inlining across modules, `Unknown` for everything else | same |
| Database model | multi-version rows, snapshots, row locks, unique/NOT NULL/CHECK, EvalPlanQual, unique-key waits, `ON CONFLICT DO NOTHING`, `SKIP LOCKED`/`NOWAIT` | [`DATABASE-MODEL.md`](DATABASE-MODEL.md) |
| Isolation | PostgreSQL READ COMMITTED and REPEATABLE READ; SERIALIZABLE as a conservative serialization-graph abstraction; deadlock victims as nondeterministic transitions | [`POSTGRES-ISOLATION.md`](POSTGRES-ISOLATION.md) |
| Exploration | bounded DFS/BFS, deterministic, full canonical-state deduplication, every limit reported | [`SCHEDULE-EXPLORATION.md`](SCHEDULE-EXPLORATION.md) |
| Reduction | sleep sets over footprint-based conditional independence, per-state explored sets | [`PARTIAL-ORDER-REDUCTION.md`](PARTIAL-ORDER-REDUCTION.md) |
| Failures | process crashes at every boundary; operation retries, at-least-once redelivery, transaction retries; optional vs guaranteed retries; idempotency by declared channel dedup | [`FAILURE-AND-RETRY.md`](FAILURE-AND-RETRY.md) |
| Scenarios | versioned JSON scenario format, full validation, data-only assertion language | [`INVARIANTS.md`](INVARIANTS.md) |
| Counterexamples | witness from replay, completion, greedy minimization validated by replay, evidence levels, codes CP7001–CP7102, JSON schema, script-free HTML timeline | [`COUNTEREXAMPLES.md`](COUNTEREXAMPLES.md) |
| PostgreSQL oracle | lockstep replay of model schedules on a private cluster, one session per actor, server-reported blocking, crash by killing sessions, steered deadlock victims | [`POSTGRES-VALIDATION.md`](POSTGRES-VALIDATION.md) |
| CLI | `commitrift model-check scenario.json [--format text|json|html] [--validate-postgres] [--no-por] [--strategy] [--no-minimize] [--no-timing]`; `commitrift report` renders model-check JSON; `commitrift explain CP7001` | — |

New code: `crates/commitrift-model/` (engine, oracle, reports),
`crates/commitrift-analyzer/src/concurrency.rs`,
`crates/commitrift-cli/src/model_check.rs`; fixtures
`fixtures/concurrency/` (a TypeScript project, 14 scenarios, one adapted
corpus reproduction with 2 scenarios); schema
`docs/phase6/schemas/model-check-report.v1.schema.json`. The only change to
Phase 1–5 code paths: the per-file fixture harness skips
`fixtures/concurrency/` (like `projects/` and `replay/`), and
`scripts/validate-schemas.py` knows the new schema (plus a strict built-in
validator).

## 2. Correctness

| Gate | macOS arm64 (PostgreSQL 17.11) | Linux aarch64, Debian 12 container (PostgreSQL 15.19, bubblewrap) |
| --- | --- | --- |
| `cargo fmt --all -- --check` | clean (`reports/phase6/final/`) | clean (`reports/phase6/linux/`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | clean (rustc 1.99.1) |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **381 passed, 0 failed, 0 ignored**, nothing skipped (300 Phase 1–5 + 81 Phase 6) | **381 passed, 0 failed, 0 ignored**, nothing skipped — the PostgreSQL differential matrix and the SSI one-way test pass against PostgreSQL 15.19 as well |
| `cargo build --release`, packaging | ok | ok; packaged binary analyzes the fixtures |
| Node + Drizzle replay (Phase 4 harnesses) | unchanged tests pass | 16/16, `--repeat 5` and generated harness 5/5 as declared |
| Schema validation | 16 model-check reports, 0 errors, corrupted copies rejected (built-in validator; `jsonschema` not installed) | — |

The Linux wrapper's final status 3 comes from its packaging smoke step
(`check fixtures` exits 3 on the intentionally unparsable
`fixtures/unsupported/syntax_error.ts`), as in Phases 4 and 5; it is not a
test failure.

Phase 6 test suites (in addition to the 300 Phase 1–5 tests, all still
passing): model unit tests; `engine.rs` (workload verdicts); `properties.rs`
(transition invariants, exploration properties, metamorphic tests);
`por.rs` (reduction vs. full exploration, independence audit, 150 random
seeds in the gate, 1 500 measured); `pg_matrix.rs` and `postgres.rs`
(PostgreSQL differential); analyzer `concurrency.rs` (extraction); CLI
`model_check.rs` (end to end).

**Bugs the tests found in Phase 6 code** (all fixed, each now covered by a
test):

1. SERIALIZABLE failed transactions at `COMMIT`; PostgreSQL fails them at
   the statement that completes the dangerous structure (differential).
2. SERIALIZABLE `LIMIT 1` reads were over-wide; PostgreSQL did not abort a
   `SKIP LOCKED` job claim the model rejected (differential).
3. A conflicting `INSERT … ON CONFLICT DO NOTHING` had no footprint, so
   reduction treated it as independent of an uncommitted delete that makes
   it wait (random seed 5).
4. A waiting READ COMMITTED statement keeps its snapshot; its order relative
   to commits was ignored by the independence relation (seed 86, found by
   the independence audit).
5. Re-expanding cached states with incomparable sleep sets explored
   transitions twice (seed 92).
6. Retries were forced (a state with a retry enabled was never final);
   retries are now optional unless declared guaranteed.
7. The evidence summary claimed the invariant was violated "on a real
   server" even when the invariant reads actor outcomes or external effects;
   now scoped to what the replay observed.

## 3. Results

Fixture scenarios extracted from `fixtures/concurrency/project` (TypeScript,
Drizzle style):

| Scenario | Verdict | PostgreSQL replay |
| --- | --- | --- |
| inventory oversell (read-modify-write, autocommit) | counterexample, 6 transitions | confirmed |
| same in a transaction, READ COMMITTED | counterexample | confirmed |
| same in a transaction, REPEATABLE READ | no counterexample within bounds (40001) | — |
| atomic conditional `UPDATE` | no counterexample within bounds | — |
| `SELECT … FOR UPDATE` | no counterexample within bounds | — |
| doctors write skew, RC / RR | counterexample | confirmed (both) |
| doctors write skew, SERIALIZABLE | no counterexample within bounds | — |
| redemption check-then-act, no constraint | counterexample | confirmed |
| redemption, declared UNIQUE(code) / atomic `ON CONFLICT` | no counterexample within bounds | — |
| job double processing (at-least-once delivery) | counterexample | confirmed |
| payment, crash after charge, client retry, provider ignores keys | counterexample (5 transitions) | confirmed (database part) |
| same, provider declared to dedup by key | no counterexample within bounds (under that assumption) | — |
| saas-starter invitations (adapted reproduction) | counterexample | confirmed |
| same with a hypothetical UNIQUE(team_id, email) | no counterexample within bounds | — |

PostgreSQL differential matrix: 60 configurations, **1 695 schedules, 0
disagreements**. One documented difference by design: SERIALIZABLE reads
through sequential scans. There PostgreSQL aborted 60 of 70 schedules the
model allows, and never the reverse.

## 4. Performance

[`PERFORMANCE.md`](PERFORMANCE.md). `commitrift check`: no measurable
wall-time change (midday 0.206–0.209 s on both binaries), peak RSS +0–3 MiB
(larger binary), identical output. Model checker: 2–4 actors in 0.2–68 ms; fixture scenarios under 1 ms, about 0.95 s with PostgreSQL validation;
sleep sets cut explored transitions by 28–46 % at 3–4 actors but wall time
by only 10–20 %. Phase 5 debt investigated: deeper call chains cost mainly
path analysis over inlined bodies. Compact effect summaries are the
recommended next step; the default depth is unchanged.

## 5. Corpus

[`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md). The default analysis is
unchanged on all four projects (byte-identical reports). One real-world
candidate, saas-starter's invitation check-then-act, could not be extracted
as written (wrapped action). An adapted isolated reproduction violates the
invariant and PostgreSQL confirms the schedule; that is evidence level 2 of
4. No human review and no maintainer confirmation exist.

## 6. Definition of done

| # | Requirement | Status |
| --- | --- | --- |
| 1 | Rust explicit-state model checker | yes (`commitrift-model`) |
| 2 | ≥ 2 concurrent actors | yes (tested to 4; random 2–3) |
| 3 | extracts or accepts database transitions | both (TypeScript subset; declared DSL) |
| 4 | explores multiple legal bounded schedules | yes (DFS/BFS/reduced; validated against PostgreSQL) |
| 5 | transaction-local vs committed state | yes (versions, snapshots; property tests) |
| 6 | PostgreSQL isolation subset | RC, RR validated; SER conservative abstraction, validated with one documented one-way difference |
| 7 | detects a genuine modeled violation | yes (7 fixture counterexamples + 1 adapted corpus case) |
| 8 | valid counterexample witness | yes (replayed; minimized witnesses re-validated) |
| 9 | POR preserves tested violation reachability | yes (equal reachable states on 14 workloads and 1 500 random scenarios; independence audit) |
| 10 | crash and retry within bounds | yes (crash, operation retry, redelivery, transaction retry) |
| 11 | predictions compared with real PostgreSQL | yes (1 695 schedules + every fixture counterexample) |
| 12 | unsupported semantics not presented as verified | `Unknown` ops → `UNSUPPORTED`; resource limits → `INCONCLUSIVE` |
| 13 | sequential analysis still works | yes (300 Phase 1–5 tests; corpus byte-identical) |
| 14 | `check` incurs no model-checking cost | yes (measured) |
| 15 | regressions measured | yes |
| 16 | limitations documented | [`LIMITATIONS.md`](LIMITATIONS.md) |

## 7. Remaining work

1. More PostgreSQL versions and plan shapes (validated: 17.11 and 15.19,
   small tables).
2. Extraction coverage: higher-order action wrappers, `try`/`catch`, loops,
   joins, `RETURNING` values, `serial` defaults. These are what blocked the
   original saas-starter function.
3. A runtime replay of the *original application code* under a controlled
   multi-request schedule (evidence level 3). It needs the Phase 4 harness
   to coordinate several requests.
4. Wall-time gain from reduction: compute footprints without applying
   sleeping transitions.
5. Compact summaries for the static analyzer's depth problem (§4).
6. Independent human review of the new candidate.
