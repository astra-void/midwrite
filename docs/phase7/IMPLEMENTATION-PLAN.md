# Phase 7 implementation plan and what became of it

Order followed: the vertical slice first (original wrapped action →
extraction → counterexample → original code on PostgreSQL), then compact
summaries, then the remaining source constructs, then corpus, performance
and documentation. Scope was cut where noted; nothing below "not done" is
claimed elsewhere.

| Stage | Plan | Outcome |
| --- | --- | --- |
| A | baseline, function identity for anonymous functions/callbacks | done: [`BASELINE.md`](BASELINE.md), [`SOURCE-IDENTITY.md`](SOURCE-IDENTITY.md) |
| B | wrapper-aware actor extraction, first-party Server Action app | done: [`CALLBACK-RESOLUTION.md`](CALLBACK-RESOLUTION.md); `fixtures/phase7/team-app`, `fixtures/phase7/wrappers` |
| C | compact summaries, differential vs legacy | done (exact, now the default): [`COMPACT-SUMMARIES.md`](COMPACT-SUMMARIES.md) |
| D | exceptions, branches, bounded loops | done for a documented subset: [`CONTROL-FLOW.md`](CONTROL-FLOW.md) |
| E | projection, JOIN, RETURNING, generated values | done; validated against PostgreSQL and real Drizzle: [`DRIZZLE-EXTRACTION.md`](DRIZZLE-EXTRACTION.md) |
| F | original-source scheduler | done: [`RUNTIME-SCHEDULER.md`](RUNTIME-SCHEDULER.md) |
| G | end-to-end verification | done: [`DIFFERENTIAL-VERIFICATION.md`](DIFFERENTIAL-VERIFICATION.md) |
| H | corpus and performance | done: [`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md), [`PERFORMANCE.md`](PERFORMANCE.md) |
| I | documentation, quality gates | [`FINAL-REPORT.md`](FINAL-REPORT.md) |

## Not done, by milestone

* **M2 shared semantic lowering**: not started. The two frontends remain
  separate; they share the function identity only.
* **M3 parameterized summaries across call sites**: the compact summary is
  per inlined call scope. Cross-site reuse would need summaries before
  inlining with relabeled IR identities; not attempted.
* **M4** loops over collections of unknown length, `?.` calls, `.then`
  chains, timers, labeled jumps, `try` inside transaction callbacks,
  exits through `finally`, inspecting a caught error: all stay `Unknown`.
* **M5** `ON CONFLICT DO UPDATE`, aggregates other than `count`,
  `returning({...})` projections, multi-row `values`, `RIGHT/FULL/CROSS`
  joins, joins with row locks, schema facts *derived from Drizzle schema
  files* (the scenario still declares tables; `sqlName` maps columns).
* **M7** crashes, retries and deadlock-victim steering against original
  code (the plan builder refuses such counterexamples as `unsupported`);
  connection-pool limits are not modeled.
* **M10** PostgreSQL 15 on macOS (not installed); Linux run: see
  [`POSTGRESQL-MATRIX.md`](POSTGRESQL-MATRIX.md).
* **M11** state-space optimizations (interning, cached footprints): profiled
  numbers are reported, no optimization was made.
* **M14** the HTML report lists the extraction facts and the
  statement-by-statement original-code comparison (escaped, script-free),
  but has no graphical per-actor timeline of runtime events or lock waits.
