# Phase 7 limitations

In addition to [`../phase6/LIMITATIONS.md`](../phase6/LIMITATIONS.md) and
[`../LIMITATIONS.md`](../LIMITATIONS.md), which still apply.

## What a result means

* `original-code-reproduced` is evidence about **this** application code,
  these inputs and declared preconditions, this schedule, this PostgreSQL
  version and a disposable database. It does not say the schedule occurs in
  production, that the declared preconditions hold there, or that other
  schedules behave alike. Two first-party scenarios reached it (5/5 runs
  each); no external application was executed.
* Declared contracts (`entry.invocation`, `entry.preconditions`) are
  assumptions. The harness *establishes* them for a run (it supplies the
  session and valid input); nothing proves they hold for real requests.
* The wrapper's own code around a callback is not modeled; its reads are
  observed at runtime and listed, its writes make a run inconclusive.

## Extraction

* The concurrency extractor and the static analyzer still lower TypeScript
  separately (shared function identity only).
* Wrapper analysis is bounded (one returned function, direct calls); only
  the innermost wrapper of a nested chain is analyzed.
* Closures over enclosing-function variables are unknown (a callback
  factory's parameters are not bound).
* Not modeled: loops of unknown length, `try` inside transaction callbacks,
  exits through `finally`, inspecting caught errors, `.then`, timers,
  generators, dynamic dispatch, `this` method dispatch in extracted code,
  `ON CONFLICT DO UPDATE`, `returning({...})`, aggregates other than
  `count`, `orderBy` semantics, multi-row `values`, subqueries, raw SQL,
  `RIGHT`/`FULL`/`CROSS` joins, joins with locks, `undefined` vs `null`
  in inserted values.
* Tables are declared in the scenario (schema facts are not derived from
  Drizzle `pgTable` definitions); column-name mapping (`sqlName`) is
  declared.
* Generated values are concrete only relative to the declared sequence
  start of a disposable database.

## Runtime verification

* Node.js + node-postgres (`pg`) + Drizzle only; `postgres-js` (used by
  saas-starter), other ORMs and Bun are not coordinated.
* Crashes, retries and deadlock victims are not executed against original
  code; such counterexamples are refused as unsupported.
* One process per actor; connection-pool exhaustion and queueing are not
  exercised.
* Statement attribution needs Drizzle builder calls (or a synchronous
  driver call) in application code; statements without an application
  frame are linked by kind and table only, and the report says so.
* Lock waits are detected by polling `pg_stat_activity` after a 150 ms
  settle time; a statement that waits only briefly within that time is
  observed as completing.
* Results compare modeled columns only; other columns (timestamps) are
  shown but not compared.

## Compact summaries

Per inlined call instance; not reused across call sites; calls whose facts
interact with the caller are walked the legacy way. They make deeper
analysis cheaper but do not change what deeper analysis finds; the default
depth stays 3 pending review of the extra findings.

## Validation

PostgreSQL 17.11 (macOS) and 15.19 (Linux container), arm64 only; small
tables; SERIALIZABLE remains a conservative abstraction; no human-reviewed
precision baseline exists for any Phase 7 result.
