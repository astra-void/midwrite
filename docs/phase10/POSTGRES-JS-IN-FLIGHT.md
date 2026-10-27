# postgres-js: in-flight and connection-loss verification

Phase 8 verified postgres-js crash/retry at driver checkpoints; Phase 9's wire
faults ran on node-postgres only. Phase 10 runs both fault families on
postgres-js for a documented subset.

## Version

`postgres` **3.4.5** only (`runtime/node/source.js::POSTGRES_JS_VERSIONS`,
unchanged). The adapter holds statements in `Query.prototype.handle`, an
undocumented internal; any other version is refused before the application
loads ("postgres-js X is not supported by source verification"), never run
with incomplete observation.

## Application

`fixtures/phase10/ledger-pgjs`: the **same** `src/app/payments.ts` as the
node-postgres fixture (`fixtures/phase10/ledger-live`), with
`src/lib/db/drizzle.ts` creating a postgres-js client from `POSTGRES_URL`
(the saas-starter pattern) and Drizzle 0.45.4's `postgres-js` adapter.

## Protocol observations

* postgres-js sends every statement, including `begin` and `commit`, over the
  extended protocol (`Parse`/`Bind`/`Describe`/`Execute`/`Sync`). The proxy
  finds a COMMIT at its `Parse` and holds or forwards the whole message group;
  the server's `CommandComplete COMMIT` is told apart from the completion of
  an ordinary statement by its tag, and from `ReadyForQuery` by its type.
* A new postgres-js connection first reads `pg_catalog.pg_type` (array
  types). The first run of an autocommit scenario armed the proxy's "next
  statement" fault on that read (`CommandComplete SELECT 297`) instead of the
  application's INSERT. Statement faults now target only data-modifying
  statements (`proxy::is_write_sql`), the only kind the model puts in flight.
  node-postgres sends no such read, which is why Phase 9 never hit it.
* The worker cannot see postgres-js's individual connections or their backend
  pids: faults are armed by `application_name` (one per actor execution),
  and the loss evidence has no "later backend pids" for postgres-js.

## Driver behavior observed

| | postgres-js 3.4.5 |
| --- | --- |
| error after the connection broke (COMMIT or INSERT in flight) | `write CONNECTION_CLOSED <replay-root>/px/.s.PGSQL.5432` (the socket path is redacted in reports) |
| process | survives; postgres-js emits no unhandled event |
| ROLLBACK after the failed COMMIT inside `sql.begin()` | none observed (the COMMIT is outside `begin()`'s try block) |
| the retry | a new connection; the transaction and the insert match the model |
| `sql.end()` at process shutdown | without a timeout it did not return within the run's deadline (30 s), **also in processes whose connections had no fault** (the restarted worker of a crash scenario); with `end({ timeout: 5 })` it returned after 5 s. The Phase 8 postgres-js harness, whose connections do not go through the proxy, ends normally. The slow shutdown is therefore an interaction of postgres-js with the proxy path, not a consequence of the broken connection; its cause was not established. It costs ≈ 5 s per postgres-js run (median 6.4 s against 1.3 s for node-postgres) |

## Cases verified (five runs each; `RUNTIME-VERIFICATION.md`)

| fault | case | result |
| --- | --- | --- |
| connection broken, process alive | COMMIT answered and withheld, blind retry | reproduced (duplicate) |
| | COMMIT never forwarded, blind retry | recovery reproduced (one row) |
| | autocommit INSERT answered and withheld | reproduced (duplicate) |
| | autocommit INSERT never forwarded | recovery reproduced |
| | unique request key, COMMIT held for later delivery | recovery reproduced (one row) |
| | look-then-insert after the error, COMMIT completed after the look | reproduced (duplicate) |
| process killed, operation in flight (Phase 9 fault) | COMMIT answered and withheld, retry in a new process | reproduced |
| | COMMIT held, delivered after the restarted client's look | reproduced |
| | COMMIT never reached, idempotent retry | recovery reproduced |

## Not covered

* Other postgres-js versions (refused).
* `sql.reserve()`, `listen`/`notify`, cursors, `forEach`, streams (reported
  unsupported by the adapter, as in Phase 8).
* Pool replacement with a bounded `max` (no pool-of-one postgres-js scenario).
* A connection loss in the middle of a transaction (not after its COMMIT).
