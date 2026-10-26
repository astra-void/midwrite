# Connection pool

## Model (documented subset)

`"pools": {"<process>": <capacity>}` in a scenario; see `FORMAL-MODEL.md` §5
for the transition semantics. What is modeled, and how it is checked:

| aspect | modeled as | test |
| --- | --- | --- |
| capacity, leased, idle | `PoolState::leased`, capacity from `Model::pools` | `connections_are_granted_in_arrival_order` |
| pending acquisition requests | `PoolState::queue`, FIFO; `Lease::{None, Waiting, Held}` per task | same |
| release | commit, rollback, abort, task end, broken connection hand the connection to the head of the queue | same, `a_lost_acknowledgment_frees_the_broken_connection` |
| termination of a connection | the lost-acknowledgment break (`LostCommitted`) drops the connection; a crash empties the pool | `a_crash_ends_every_task_of_the_process_and_empties_its_pool` |
| transaction ownership | `Txn::leased`: the transaction holds its connection to the end | `a_pool_of_one_starves_a_task_that_waits_for_its_own_parent` |
| a task needing a connection it has not got | not enabled; it waits | same |
| pool state in state identity | encoded (trailer `0xB0`, per task lease byte, per transaction flag) | `pool_state_is_part_of_the_state_and_reduction_stays_sound` |
| reduction | `Access::Pool(p)`: operations of one pool are dependent | same (independence audit + full-vs-reduced states) |

Not modeled: acquisition timeout (`connectionTimeoutMillis`), idle timeout,
pool shutdown (`pool.end()`), errors while connecting, `min`, pipelining,
`sql.reserve()`. A timeout would turn a starvation into an error: the model
reports the starvation, not the timeout the application may configure.

## Distinguishing what is stuck, and what is not

| situation | what the model does | classified as |
| --- | --- | --- |
| lock cycle between transactions | `Deadlock` transition: PostgreSQL's detector aborts a victim (any member) | not stuck; SQLSTATE 40P01 |
| every remaining task waits for a pool connection | no transition enabled, tasks running | `PoolStarvation` |
| a statement waits for a lock whose holder waits (via `Promise.all`) for that statement | no transition enabled; PostgreSQL sees one waiter | `ApplicationWaitCycle` |
| process crash | `Crash`/`CrashInFlight` | not stuck |
| a wait without a cause the model knows | no transition enabled | `Unexplained` (stays INCONCLUSIVE) |

`StuckKind` is reported under `exploration.stuck`; with a bounded pool, or
`options.termination: true`, a stuck state is a counterexample of the implicit
invariant `termination` with a witness (`stuck tasks = N`).
`a_database_deadlock_is_not_pool_starvation` and
`a_lock_wait_on_the_applications_own_transaction_is_an_application_wait_cycle`
pin the difference.

## Drivers

| | node-postgres (`pg.Pool`) | postgres-js |
| --- | --- | --- |
| acquisition | `pool.connect()`/`pool.query()`; FIFO `_pendingQueue`; `max` | internal connection pool per client; queries are queued per connection; `sql.reserve()` hands out one |
| observable at runtime | `totalCount`, `idleCount`, `waitingCount`, `options.max` — reported by the worker on the coordinator's `pool?` probe | not exposed by the module the harness can reach |
| order of service | FIFO (modeled) | not established |
| release | `client.release()`; queued request served synchronously | not modeled |
| pipelining | none | yes (not modeled) |

Only node-postgres pools are modeled and verified; a scenario with `pools`
plus postgres-js is not supported. The two drivers are *not* assumed to behave
alike.

## Runtime verification

* The harness's pool is the application's own configuration
  (`new pg.Pool({ max: 1 })` in `verify/harness-pool1.ts`); the coordinator
  compares the scenario's declared capacity with the `max` the worker reports
  at start-up and refuses a run whose pool differs
  (`the_declared_pool_must_be_the_harnesss_pool`).
* A model *wait for a connection* at a crash is checked with the pool's own
  counters (waiting ≥ 1, idle = 0) and the task being silent.
* A *stuck state* is confirmed by quiescence (no statement announced or
  completed for ≥ 800 ms, never fewer than 4× the settle time), unfinished
  tasks, and the pool's counters (pool starvation) or `pg_stat_activity` +
  `pg_blocking_pids` (application-level wait cycle). The worker is then killed
  to end the run. Quiescence is *evidence*, not a proof that nothing will ever
  proceed: a timeout-based liveness claim is bounded by construction, and the
  report says "no statement was announced or completed for N ms".

First-party result (`ledger-pool-starvation`): the audit insert written
through the pool inside an open transaction with `max: 1` — model stuck after 2
transitions; the original code: pool counters `[waiting 1, idle 0, total 1]`
after 806 ms of silence; `stuck.status = agree`. With `max: 2`
(`ledger-pool-of-two`) the model finds nothing to run.
