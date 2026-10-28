# Phase 14 security

Unchanged guarantees, and what Phase 14 adds to them.

* **Discovery never executes application code.** The new extractor work
  (closures, zod schemas, arrays/strings/`Map`/`Set`, multi-row inserts,
  predicates, timestamps, package generators) reads source text only. A zod
  schema is read from its builder chain, never constructed; a `nanoid()` or
  `crypto.randomUUID()` call is modeled from a written contract
  (`analyzer/src/packages.rs`), never run.
* **Original code ran only for first-party fixtures**: the new
  `fixtures/phase14/request-app` (authorized harness `verify/harness.ts`,
  manifest `commitrift-source.json`), and the Phase 12/13 fixtures in the
  regression suite, on private disposable PostgreSQL 17.11 clusters (unix
  socket, `listen_addresses=''`), deleted afterwards. Every run used
  `--verify-source --allow-execution` explicitly. The Phase 14 harness
  states one environment assumption in its source: it handles errors of
  checked-out node-postgres clients (without it, a broken connection during
  a COMMIT kills the worker; that run is kept as evidence).
* **The Drizzle SQL probe** (`fixtures/phase14/drizzle-app/probe/`) calls
  drizzle-orm's `.toSQL()`: it renders SQL text without connecting to any
  database.
* **No corpus or private application was executed.** midday was analyzed
  statically at its pinned revision; school-ledger and unnamed-ai-project
  from read-only copies without `node_modules`, in a private scratch
  directory; their working trees were not modified.
* **Privacy.** Reports about the private projects contain aggregate counts
  and generic blocker categories only (`REAL-WORLD-RESULTS.md`): no paths,
  identifiers, selectors or SQL. Evidence files of first-party runs contain
  absolute paths of the analysis machine (as in Phases 12 and 13).
* **The new evaluation set** (`fixtures/phase14/eval-app`) is static
  TypeScript with no `node_modules`; it is analyzed, never run.
* **No package was installed** into any project, no production database was
  contacted, nothing was committed, pushed or published. Report HTML stays
  script-free and escaped (the new "Next step" paragraph goes through the
  same escaping; Phase 6/11 tests pass).
