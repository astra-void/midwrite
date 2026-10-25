# Node.js runtime and real Drizzle ORM replay

Phase 4 milestones 1 and 2. Results: [`FINAL-REPORT.md`](FINAL-REPORT.md),
raw reports `reports/phase4/replay/drizzle-node.json` (macOS) and
`reports/phase4/linux/drizzle-node-isolated.json` (Linux).

## 1. One replay engine, two runtimes

Nothing in the supervisor is runtime-specific: the JSON-lines checkpoint
protocol over a Unix socket, the deterministic acknowledgment (the harness
blocks at a checkpoint until the supervisor answers or kills it), the
failure modes, the PostgreSQL supervision, deadlines, cleanup and reports are
the Phase 3 ones ([`../phase3/REPLAY-PROTOCOL.md`](../phase3/REPLAY-PROTOCOL.md)).
Phase 4 adds:

| Piece | What |
| --- | --- |
| `runtime/node/index.js` (+ `index.d.ts`) | The protocol client as one dependency-free ECMAScript module that runs unchanged on Node.js and Bun. `runtime/typescript/commitrift-replay.ts` now re-exports it, so the Phase 3 Bun harnesses and the Node harnesses share **one** implementation. |
| `runtime/node/pg.js` | `instrumentPgPool(pool)`: wraps one node-postgres pool so that checkpoints are emitted at the database effect boundaries of whatever library sends the queries (Drizzle here). `replayPoolConfig()` points `pg` at the run's private socket. |
| `runtime/node/event-log.js` | `publishToEventLog(topic, payload, { id })`: the fake external transport (fsync'ed append, `before/after-event-publish`). |
| Manifest `runtime` | Optional `"runtime": "node" \| "bun"`. When declared: `command[0]` must start that runtime; its version must be at least Node.js 22.0 / Bun 1.2 (else the replay is *blocked*, exit 3); the entry point must exist inside the harness directory, else blocked with "compile the harness first" (replay never installs or builds); and an entry point that still contains a `COMMITRIFT-TODO` marker is refused. |
| `done.injected` | The runtime reports whether the error a scenario ended with *is or wraps* the injected fault (it follows `error.cause`); the report records it per run as `injectedFaultSurfaced`. |

### Execution strategy for TypeScript on Node.js

Harnesses are **compiled with `tsc` (TypeScript 5.9.3, pinned in the
lockfile) and run as JavaScript** (`"command": ["node", "dist/replay/harness.js"]`).
Rejected alternatives: Node's built-in type stripping (only for erasable
syntax, unflagged only in recent releases, no `node_modules`); third-party
loaders (an extra moving part inside every harness process). The minimum is
**Node.js 22** (the oldest LTS line still maintained on 2026-10-25); tested
with v22.23.2 on macOS and Linux. Replay does not compile: `npm ci && npm
run build` is the developer's step, and the supervisor refuses a missing
entry point with that instruction.

### Package name

`@commitrift/replay` is a **workspace-local, private** package
(`"private": true`), consumed through `file:` dependencies. On 2026-10-25
the npm registry returned 404 for `commitrift`, `@commitrift/replay` and
`commitrift-replay` (unpublished); ownership of an npm scope `@commitrift`
could not be verified (the organization page is not public). Nothing was
published.

## 2. Checkpoints from the driver, not from application code

Phase 3 kept checkpoints out of analyzed code (an awaited `checkpoint()`
next to a commit becomes a CP9001). With the real ORM this is done one layer
down: `instrumentPgPool` wraps `pool.query`, `pool.connect()` and the
clients it returns, and classifies each SQL statement Drizzle sends by its
leading keyword:

| Checkpoint (label) | Emitted |
| --- | --- |
| `before-db-write` (table) | before an INSERT/UPDATE/DELETE is sent |
| `after-db-commit` (table) | after an auto-commit write was acknowledged |
| `after-tx-write` (table) | after a write inside an open transaction |
| `before-tx-commit` (`transaction`) | before COMMIT is sent |
| `after-db-commit` (`transaction`) | after PostgreSQL acknowledged COMMIT **as a commit** |
| `after-tx-rollback` (`transaction`) | after ROLLBACK, or after a COMMIT that PostgreSQL answered with `ROLLBACK` |

What counts as committed comes from PostgreSQL's reply: COMMIT of an
aborted transaction returns the command tag `ROLLBACK` and never yields
`after-db-commit`; a COMMIT that fails with an error (the deferred-constraint
case below) yields no checkpoint and the error propagates unchanged.
Verified against the installed sources: Drizzle 0.45.4's node-postgres
session calls `client.query(config, params)` (promise style) and runs
transactions on a client from `pool.connect()` with literal `begin` /
`commit` / `rollback`; `pg-pool`'s own `pool.query` uses callback style
internally, which the wrapper passes through, so nothing is emitted twice.
Not understood (passed through without a checkpoint): writes hidden in CTEs
or functions, savepoint statements, callback-style and cursor queries.

One integration detail surfaced at runtime: Drizzle wraps an error raised
inside a driver call in `DrizzleQueryError` with the original as `cause`.
The runtime therefore searches the cause chain for the `InjectedFault`;
without that, an injected exception would have been indistinguishable from
an application error in the report (found by the first test run, fixed,
covered by `runtime/node/test/runtime.test.js`).

## 3. The fixture: `fixtures/replay/drizzle-node`

A small order service on **Node.js, `drizzle-orm` 0.45.4 and `pg` 8.23.1**
(exact versions in `package-lock.json`), a disposable PostgreSQL database,
and the event-log transport. The application code (`src/`) has no replay
code; it has two seams a real process entry point would also have:
`usePool(pool)` (`src/pool.ts`) and `useTransport(fn)` (`src/queue.ts`).
`replay/harness.ts` is the only replay-aware file: it wires the instrumented
pool and the event log, then imports the application.

```text
src/schema.ts     Drizzle pgTable definitions (DDL with constraints: schema.sql)
src/pool.ts       the pool seam          src/database.ts  db = drizzle(pool, { schema })
src/queue.ts      queue.publish → transport
src/service.ts    scenarios A–D          src/relay.ts     outbox relay
replay/harness.ts entry point (compiled to dist/replay/harness.js)
```

Static analysis of `src/` (no replay code involved) reports exactly:
CP1001 `completeOrder` (`4f53a6215ac16913`), CP1003 `shipOrder`
(`fdb2734b893502d8`), CP1002 `relayOutbox` (`70af7a37dbb95d80`); nothing for
the transactional `recordPayment` or the outbox `confirmOrder`. All three
traces are complete. Pinned in `crates/commitrift-analyzer/tests/traces.rs`.

## 4. Scenarios and results

16 cases, one control run (`allFlows`: all four flows, all 7 invariants
hold). Fresh database per run. Same outcome on macOS (sandbox-exec) and
Linux (bubblewrap); `a-kill-after-db-commit` additionally 5/5 with
`--repeat 5` on both.

| Case | Failure | Observed after the failure | Level |
| --- | --- | --- | --- |
| **A** `a-kill-after-db-commit` | SIGKILL at `after-db-commit (orders)`; a separate session saw `paid` first | order-1 `paid`, `order.completed` events **0** | **RUNTIME_REPRODUCED** |
| A `a-exception-after-db-commit` | exception, same point (surfaces as `DrizzleQueryError` caused by `InjectedFault`) | `paid`, 0 events | RUNTIME_REPRODUCED |
| A `a-kill-before-db-write` / `a-kill-after-event-publish` | negative | `pending`/0, `paid`/1 | RUNTIME_NOT_REPRODUCED |
| **B** `b-exception-before-tx-commit` | exception before COMMIT; separate session saw `pending`, 0 payments while paused | `pending`, 0 payments; `after-tx-rollback` emitted, no commit claimed | RUNTIME_NOT_REPRODUCED |
| B `b-kill-after-tx-write` | SIGKILL with both writes staged | `pending`, 0 payments (rolled back by PostgreSQL) | RUNTIME_NOT_REPRODUCED |
| B `b-kill-after-tx-commit` | SIGKILL after COMMIT acknowledged | `paid`, 1 payment | RUNTIME_NOT_REPRODUCED |
| **C** `c-exception-after-event-publish` | exception after the publish inside the transaction | order-3 `pending`, 0 shipments, `order.shipped` **1** | **RUNTIME_REPRODUCED** |
| C `c-kill-after-event-publish` | SIGKILL, same point | same | RUNTIME_REPRODUCED |
| C `c-commit-fails-after-publish` | **no injection**: duplicate tracking number violates a deferred unique constraint, PostgreSQL rejects COMMIT (`shipments_tracking_number_key`) | same: event out, transaction rolled back | RUNTIME_REPRODUCED |
| C `c-kill-before-event-publish` | negative | nothing durable | RUNTIME_NOT_REPRODUCED |
| **D** `d-kill-before-tx-commit` | SIGKILL with state + outbox row staged | nothing durable | RUNTIME_NOT_REPRODUCED |
| D `d-kill-after-tx-commit` | SIGKILL after commit; recovery: relay | after: `confirmed`, 1 pending outbox row, 0 events; after relay: 1 event | RUNTIME_NOT_REPRODUCED |
| D `d-kill-relay-before-publish` | relay killed before publishing; recovery: relay | row pending, 0 events → 1 event | RUNTIME_NOT_REPRODUCED |
| D `d-kill-relay-after-publish-before-ack` | relay killed between publish and marking (its CP1002 window) | 1 event, row pending → after relay: 2 events, **1 duplicate**, row published | RUNTIME_NOT_REPRODUCED |
| D `d-exception-relay-after-publish-before-ack` | same with exception | same | RUNTIME_NOT_REPRODUCED |

The outbox keeps durable intent at every failure point and delivers
**at least once**; the relay's own window produces a duplicate, which the
declared contract accepts because events carry ids. Exactly-once delivery
is neither claimed nor observed.

## 5. Static-to-runtime correspondence

Each case names its finding (`finding: {code, function, file}`); replay
re-analyzes `src/` from the harness directory, links the case to exactly one
finding and embeds its trace. A kill at a trace boundary checkpoint is
reported `onTraceBoundary`; negative cases `outsideTraceBoundary`. The chain
is the Phase 3 one: static candidate → selected finding → authorized replay
→ observed evidence. A runtime result never changes the finding's
confidence; it is reported in the verification report only.

**Scope of the evidence.** These runs show what this fixture does with the
real Drizzle ORM, node-postgres and PostgreSQL under the injected
conditions. They do not show that any other application has the same bug,
nor anything about PostgreSQL crashes (only the client process is killed).

## 6. Running it

```bash
cd fixtures/replay/drizzle-node && npm ci && npm run build && cd -
commitrift replay fixtures/replay/drizzle-node                    # plan only
commitrift replay fixtures/replay/drizzle-node --allow-execution  # 17 runs, ~10 s on the test machine
```

Tests: `real_drizzle_on_node_reproduces_the_dual_write_and_respects_transactions`,
`the_drizzle_dual_write_reproduces_in_every_repeated_run`,
`node_harnesses_are_isolated_like_bun_harnesses`,
`a_declared_node_runtime_without_a_compiled_entry_point_is_blocked`,
`the_node_runtime_protocol_client_passes_its_own_tests` (9 `node --test`
cases: protocol order, deterministic acknowledgment, exception vs. other
errors, cause chains, statement classification, COMMIT answered by
ROLLBACK, no COMMIT sent after an injected fault, callback pass-through).
