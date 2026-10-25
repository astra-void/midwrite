# Demo: from a static finding to runtime evidence

This walkthrough needs no knowledge of the analyzer's internals. It takes
about 15 seconds after the build.

**Prerequisites.** A Rust toolchain (1.97+). For the runtime part: macOS,
[Bun](https://bun.sh) 1.2 or newer, and the PostgreSQL server binaries
(`brew install postgresql@17`; or set `COMMITRIFT_PG_BIN` to the directory
containing `initdb`, `pg_ctl`, `postgres`, `psql`). No Docker, no running
database, no network access by the demo application.

## One command

```bash
scripts/demo.sh            # writes demo-output/
```

Without Bun or PostgreSQL it runs the static part and prints
`Runtime part SKIPPED: missing …`.

## Step by step

### 1. The application

`fixtures/replay/order-vulnerable/src/orders.ts`:

```ts
export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  await queue.publish("order.completed", { orderId });
}
```

`db` is a small Drizzle-shaped adapter over Bun's built-in PostgreSQL client
and `queue` a fake message queue that appends to a durable log file
(`fixtures/replay/shared/`). The adapters, not the application, report replay
checkpoints.

### 2. Static finding

```bash
cargo build --release
H=fixtures/replay/order-vulnerable
target/release/commitrift check $H/src --config $H/commitrift.json
```

reports `warning[CP1001]: database commit followed by an independent
external effect`. Nothing was executed.

### 3. Failure trace

```bash
target/release/commitrift trace $H/src --config $H/commitrift.json
```

```text
    0. dbWrite     Committed   db.update(orders) @ 9:9 <- completes first
       ---- failure injection boundary (after-db-commit / before-event-publish) ----
    1. external    NotReached  queue.publish("order.completed", { orderId }) @ 11:9 <- not yet performed
  simulated: If the process dies right after step 0: durable or visible: `db.update(orders)` (step 0); never performed: `queue.publish(...)` (step 1).
```

This is a `STATIC_CANDIDATE` plus a `SIMULATED` prediction: still nothing
executed.

### 4. Authorize and run the harness

```bash
target/release/commitrift replay $H                 # shows the plan; runs nothing (exit 2)
target/release/commitrift replay $H --allow-execution --case kill-after-db-commit
```

The supervisor creates a disposable PostgreSQL cluster in a private
directory, runs a control scenario (no injection: order paid, one event,
invariant holds), then runs the case from a fresh database. When the
harness reports `after-db-commit`, the supervisor checks from a separate
session that the status `paid` is committed, then kills the process with
SIGKILL. It observes:

```text
    before:        orderStatus="pending" publishedEvents=0
    after:         orderStatus="paid" publishedEvents=0
  run 1: RUNTIME_REPRODUCED — declared invariant violated: `paid-order-has-event` …
```

The invariant ("a paid order has its event") is declared by the harness in
`commitrift-replay.json`; CommitRift does not invent it.

### 5. All cases, repeated, and the safe version

```bash
target/release/commitrift replay $H --allow-execution --repeat 3 --output vulnerable.json
target/release/commitrift replay fixtures/replay/order-outbox --allow-execution --repeat 3 --output outbox.json
```

Vulnerable: 3 cases reproduce (kill after the commit; exception after the
commit; kill before publishing in the version with a `catch` compensation),
3 do not (kill before the write; kill after the publish; *exception* before
publishing in the compensating version). Outbox: none reproduce at the same
failure points; the intent survives in the outbox and the relay publishes
it, with a **duplicate delivery** when the relay dies between publishing and
acknowledging.

### 6. View

```bash
target/release/commitrift report vulnerable.json -o vulnerable.html
```

Open the file in any browser: static predictions (dashed, "not executed")
next to runtime observations (checkpoint log, state before/after, invariant
results). The page contains no script and loads nothing.

## Where to go next

* [`REPLAY-PROTOCOL.md`](REPLAY-PROTOCOL.md) to write a harness for your
  own code.
* [`SECURITY.md`](SECURITY.md) before running any harness.
* [`REPRODUCTION-RESULTS.md`](REPRODUCTION-RESULTS.md) for the recorded
  results.
