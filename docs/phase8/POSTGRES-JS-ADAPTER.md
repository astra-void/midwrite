# postgres-js adapter

`runtime/node/source.js` (`coordinatePostgresJs`, `sourcePostgresUrl`),
`crates/commitrift-replay/src/source.rs` (coordinator: transaction
configuration, backend lookup by `application_name`).

The same coordinator, worker lifecycle, authorization, isolation, plans and
reports serve both drivers; only the worker-side adapter differs.

```
Runtime coordinator (Rust)
   ├── node-postgres adapter: coordinatePool (client.query)   [Phase 7]
   └── postgres-js adapter:   coordinatePostgresJs             [Phase 8]
```

## Where statements are held

postgres-js offers no public hook that can delay a statement: `debug` is a
synchronous callback, and wrapping `sql.unsafe` would miss what the driver
sends itself — `begin()` runs `BEGIN` through `sql.unsafe` but `COMMIT`,
`ROLLBACK` and `SAVEPOINT` through tagged templates on a transaction-scoped
`sql`. Every query, however created, goes through one place before it
reaches a connection: `Query.prototype.handle()` (called by
`then`/`catch`/`finally`/`execute`, after `await 1`). The adapter wraps it:
announce the statement (text, parameters, application call site), wait for
the coordinator's release, then call the original handler. Completion is
the query promise's own settlement (`Promise.prototype.then` on the query,
which does not execute it again), never the call of a tagged template
(a tagged template that is never awaited is never executed or announced —
tested).

This is an internal of postgres-js. The dependence is explicit:

* only verified versions are accepted (`POSTGRES_JS_VERSIONS = ["3.4.5"]`,
  the version saas-starter locks); another version is **refused**
  (`the_postgres_js_adapter_refuses_unverified_versions`);
* the prototype is checked to have the expected methods before it is
  patched; it is patched once per module instance.

## Driver behavior and how it is handled

| postgres-js behavior | handling |
| --- | --- |
| reads the type catalog when a connection opens (`fetchArrayTypes`), through a query whose handler is the connection's own `execute` | not held (it precedes the application's statements on that connection and would block it); reported as `driverInternal` |
| sets the isolation level in a statement of its own (`begin`, then `set transaction isolation level ...`) | released together with the BEGIN the plan released; reported as `transactionConfig` |
| no backend pid exposed per query | lock waits and session ends are looked up by the execution's `application_name` (`commitrift-actor-<name>#<attempt>`), which the URL sets; with several non-idle sessions of one execution the wait is not attributed (no false lock wait) |
| affected rows in `result.count`, column origins in `result.columns` (`table`, `number`), rows as arrays with `.values()` (what Drizzle uses) | reported like node-postgres results |
| cursors, `forEach`, streams | reported **unsupported** (run inconclusive) |
| pipelined `sql.begin(sql => [q1, q2])`, savepoints, `reserve()` | not exercised; savepoints are unsupported in the model; pipelines would be announced statement by statement |
| no Unix socket in a URL host | `sourcePostgresUrl()` returns `postgres:///db?application_name=...` and sets `PGHOST`/`PGPORT`/`PGUSER` (postgres-js falls back to them when the URL has no host); the application's `postgres(process.env.POSTGRES_URL)` stays unchanged |

## Regression tests

`commitrift-cli/tests/phase8.rs`:

* `the_postgres_js_adapter_reproduces_what_the_pg_adapter_does`: the same
  invite double submit on both drivers — same statements, observed rows,
  final rows and verdict (timestamps of unmodeled `now()` columns aside);
* `postgres_js_crashes_lock_waits_and_serialization_retries_are_coordinated`:
  crash after charge + retry, a lock wait found by `application_name` and a
  crash while waiting, a real 40001 with the application's retry loop and
  the separate `SET TRANSACTION` statements;
* `the_postgres_js_adapter_refuses_unverified_versions`
  (`runtime/node/test/postgres-js.test.js`).

Covered by the scenario evidence (5/5 each): SELECT/INSERT, transactions,
rollback after failure, concurrent transactions, lock waits, query errors
(40001 SQLSTATE through Drizzle's error wrapping), generated ids, crash
after commit-free operations and before COMMIT, retry after crash.

## Installation

`postgres@3.4.5` was installed from the npm registry for this fixture
(`fixtures/phase8/pgjs-app/package-lock.json`, integrity
`sha512-cDWgoah1Gez9rN3H4165peY9qfpEo+SA61oQv65O3cRUE1pOEoJWwddwcqKE8XZYjbblOJlYDlLV4h67HrEVDg==`);
the other packages came from the local npm cache. It is a test dependency
of a fixture, not a dependency of CommitRift.
