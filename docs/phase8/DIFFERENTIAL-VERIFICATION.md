# Differential verification in Phase 8

Three comparisons stay separate; passing one implies nothing about the
others.

| | comparison | Phase 8 additions |
| --- | --- | --- |
| A | abstract model vs PostgreSQL running the **model's** SQL (`--validate-postgres`, Phase 6) | unchanged; effects are not database operations (the oracle skips them) |
| B | Drizzle semantics vs the SQL Drizzle actually generates (per statement, Phase 7) | **schema**: extracted Drizzle schema vs the catalog of the database the migrations create ([`SCHEMA-FIDELITY.md`](SCHEMA-FIDELITY.md)); postgres-js's generated SQL compared the same way as node-postgres's |
| C | original TypeScript execution vs the model's schedule (Phase 7) | crashes (checkpoint, SIGKILL, session termination, committed rows after the crash), restarts (new process, attempt), external effects (channel, key, payload, deduplication, actor and attempt), executions per actor, the invariant on observed rows **and** the observed effect log |

## What a crash run compares

| model | runtime | difference → |
| --- | --- | --- |
| the actor's next operation at the crash | the operation the worker announced (held) | diverged |
| a statement waiting for a lock | `pg_stat_activity` wait right before the kill | inconclusive if not waiting |
| process gone | `waitpid`: signal 9 | inconclusive |
| uncommitted work discarded | sessions of the execution ended (10 s) | inconclusive |
| committed rows after the crash | modeled tables read after the sessions ended | crash check differs → not reproduced |
| retry = new execution | new pid, attempt + 1, previous outcome `killed`/`returned`/`threw` | not observed → not reproduced |
| effect log (channel, key, actor#attempt, deduplicated) | the channel double's log | not reproduced |
| executions per actor | worker processes per actor | not reproduced |
| invariant | evaluated on the observed final rows and effect log | — |

**Ambiguous outcomes are not resolved by assumption**: a kill is only
issued at a checkpoint where the outcome is determined (operation held, or
statement waiting); "no client response = no commit" and "submitted =
committed" are never used — durable state is read from PostgreSQL after the
sessions ended. A COMMIT in flight is never killed (the model has no such
transition).

**Unexpected operations are never skipped**: a statement or effect the
model does not expect at that point diverges the run; statements after the
plan are `beyond-plan` and make the run not reproduced; a write, effect or
transaction control outside the modeled function makes it inconclusive;
driver statements (postgres-js catalog reads) are reported separately and
are read-only.

## Static-analysis exactness

| check | result |
| --- | --- |
| `check` json/sarif/text + `trace` json, Phase 8 vs Phase 7 binary, 4 projects × depths 3/6/12 × path modes shared/compact/legacy | **144/144 identical** |
| path differential (legacy vs compact, legacy vs shared), 22 022 function analyses × depths 3/6/8/12 | **0 differences** |
| model-check reports of all Phase 6/7 scenarios, Phase 8 vs Phase 7 binary | **17/17 identical** (tool version aside) |
| model-checker states/transitions on the benchmark workloads, Phase 8 vs Phase 7 | identical (asserted by the benchmark) |

## PostgreSQL versions

macOS: PostgreSQL 17.11. Linux container: PostgreSQL 15.19
([`FINAL-REPORT.md`](FINAL-REPORT.md)). PostgreSQL 14+ is required for
crash plans (`client_connection_check_interval`); NULLS NOT DISTINCT is
compared from 15. The SERIALIZABLE model remains a documented subset of SSI
(Phase 6); none of the Phase 8 scenarios relies on it.
