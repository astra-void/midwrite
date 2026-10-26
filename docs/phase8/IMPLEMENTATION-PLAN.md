# Phase 8 implementation plan and what was done

The plan was written after the audit ([`BASELINE.md`](BASELINE.md)) and
followed the order the task sets: the primary vertical slice first
(original function → crash/retry counterexample → SIGKILL → fresh worker →
PostgreSQL observation → invariant), then summaries, schema, driver,
optimization, regression and documentation. Each stage was finished as a
vertical slice with tests before the next one started.

| stage | plan | status |
| --- | --- | --- |
| A. Baseline | gates, digest, preserved binaries, timings | done ([`BASELINE.md`](BASELINE.md)) |
| E. Crash/retry runtime (first, by priority) | plan steps `Crash`/`Restart`, checkpoints observed (announced-and-held, lock wait), SIGKILL of the process group, PostgreSQL session termination observed, new process per attempt, channel test doubles for external effects, evidence classification, recovery probes for safe alternatives | done ([`CRASH-RETRY-SEMANTICS.md`](CRASH-RETRY-SEMANTICS.md), [`ORIGINAL-CODE-RECOVERY.md`](ORIGINAL-CODE-RECOVERY.md)) |
| G. End-to-end recovery scenarios | A payment/response lost, B dual write vs outbox/relay, C job redelivery, D 40001 retry; vulnerable and safe variants | done (21 scenarios, 5/5 runs each) |
| C. Parameterized summaries | canonical memo key for the IR inliner (exact, with counter replay); path-summary instantiation across call instances; profiling-driven cost reductions | done ([`PARAMETERIZED-SUMMARIES.md`](PARAMETERIZED-SUMMARIES.md)) |
| B. Shared semantic facts | one chain decomposition and syntax facts for the three TypeScript readers; differential on every scenario and corpus project | done for the listed categories; the rest is listed below ([`SEMANTIC-FACTS.md`](SEMANTIC-FACTS.md)) |
| D. Schema extraction | Drizzle `pgTable`/`pgEnum`/`pgSchema` facts without execution; scenario `from` with conflicts and visible overrides; catalog comparison on a disposable PostgreSQL | done ([`SCHEMA-EXTRACTION.md`](SCHEMA-EXTRACTION.md), [`SCHEMA-FIDELITY.md`](SCHEMA-FIDELITY.md)) |
| F. postgres-js | adapter on the existing coordinator, version-pinned | done ([`POSTGRES-JS-ADAPTER.md`](POSTGRES-JS-ADAPTER.md)) |
| H. Optimization | profiled first; copy-on-write explorer state, shared IR strings, cached summary facts, fast hasher | done ([`STATE-SPACE-OPTIMIZATION.md`](STATE-SPACE-OPTIMIZATION.md), [`PERFORMANCE.md`](PERFORMANCE.md)) |
| I. Regression and documentation | full test matrix (macOS, Linux container), corpus identity, reports, schemas | done ([`FINAL-REPORT.md`](FINAL-REPORT.md)) |

## Deliberately not done

* Semantic-fact categories not migrated: abstract values and returns,
  transaction handles, async/exceptional completion. The two frontends model
  them for different purposes (path enumeration vs concurrent transitions)
  and their implementations share no code worth unifying without a
  behavioral change; the migration stopped at recognition
  ([`SEMANTIC-FACTS.md`](SEMANTIC-FACTS.md)).
* A loader hook to coordinate a `postgres` import an application resolves
  from a different `node_modules`: not needed for the tested structure
  (one module instance), documented as a limitation.
* `ON CONFLICT DO UPDATE`, savepoints, cursors, pipelined `sql.begin(...)`
  arrays, statement-level retries, pool exhaustion: refused or unsupported,
  as before ([`LIMITATIONS.md`](LIMITATIONS.md)).
* Running the external saas-starter application or its migrations: not
  authorized; its schema is reported from static extraction only.
