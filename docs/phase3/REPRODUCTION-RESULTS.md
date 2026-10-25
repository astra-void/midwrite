# Runtime reproduction results

All runs below were executed on 2026-10-25 on one machine (macOS, aarch64),
PostgreSQL 17.11 (Homebrew), Bun 1.3.3, network isolation through
`sandbox-exec`, with `commitrift replay --allow-execution --repeat 5`. The
complete reports, with every run's checkpoint log, observations, invariant
results and redacted harness output, are in `reports/phase3/replay/`
(`*.json`, `*.txt`, and `*.html` rendered by `commitrift report`).

Every case ran 5 times, each time from a freshly created database with the
same setup file and an empty event log. **All 14 cases gave the same
verdict and the same observed state in all 5 runs**, every injection was
delivered at the selected checkpoint (process kills ended with signal 9,
injected exceptions with exit code 1), and every observed state matched the
trace's prediction. The control runs (no injection) satisfied the declared
invariants in all three harnesses.

None of this is a claim about a real deployment. The harnesses are
first-party test applications; their invariants are declared by their
author.

## 1. `order-vulnerable`: the CP1001 window, reproduced

Application (`fixtures/replay/order-vulnerable/src/orders.ts`):

```ts
export async function completeOrder(orderId: string) {
  await db.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
  await queue.publish("order.completed", { orderId });
}
```

Static finding: CP1001 in `completeOrder`, trace `tr-c649a775ffafff7d`
(complete), boundary `after-db-commit` (orders) / `before-event-publish`.
Declared contract `paid-order-has-event`: when `orderStatus = 'paid'`,
`publishedEvents >= 1`.

| Case | Failure | Pre-injection check (separate session) | Before | After | Verdict (5/5) |
| --- | --- | --- | --- | --- | --- |
| `kill-after-db-commit` | SIGKILL at `after-db-commit` | `orderStatus = 'paid'` holds | pending, 0 events | **paid, 0 events** | **RUNTIME_REPRODUCED** |
| `exception-after-db-commit` | exception at `after-db-commit` | holds | pending, 0 | paid, 0 | RUNTIME_REPRODUCED |
| `kill-before-db-write` (negative) | SIGKILL at `before-db-write` | — | pending, 0 | pending, 0 | RUNTIME_NOT_REPRODUCED |
| `kill-after-event-publish` (negative) | SIGKILL at `after-event-publish` | — | pending, 0 | paid, 1 | RUNTIME_NOT_REPRODUCED |
| `compensation-exception-before-publish` | exception at `before-event-publish` in `completeOrderWithCompensation` | holds | pending, 0 | pending, 0 (the `catch` reverted the status) | RUNTIME_NOT_REPRODUCED |
| `compensation-kill-before-publish` | SIGKILL at the same point | holds | pending, 0 | **paid, 0** (the `catch` never ran) | **RUNTIME_REPRODUCED** |

What this establishes, for this harness: the commit was durable (visible from
another session before the kill, and still there after it), the event was
absent, the operation did not complete (the process died at the
checkpoint), and the declared invariant failed — the same way in 5 of 5
runs. The last two rows show why exception and crash are separate modes: an
in-process compensation handles the exception and does nothing for the crash.

## 2. `order-outbox`: the safe counterpart

```ts
export async function completeOrder(orderId: string) {
  await db.transaction(async (tx) => {
    await tx.update(orders).set({ status: "paid" }).where(eq(orders.id, orderId));
    await tx.insert(outbox).values({ topic: "order.completed", payload: { orderId } });
  });
}
// relay.ts: for each pending row: queue.publish(topic, payload, { id: `outbox-${row.id}` }); then mark it published.
```

Static analysis: no finding for `completeOrder`; one CP1002 for the relay
(publish, then mark published: the at-least-once window). Declared contract:
a paid order has its event published **or** still owed by a pending outbox
row; an outbox row exists only for a paid order; every event carries an id
(duplicates are allowed, consumers deduplicate).

| Case | Failure | After the failure | After recovery (`relay` run again) | Verdict (5/5) |
| --- | --- | --- | --- | --- |
| `kill-before-tx-commit` | SIGKILL at `before-tx-commit` (both writes staged; pre-check from another session: status still `pending`) | pending, 0 outbox rows, 0 events | — | NOT_REPRODUCED |
| `kill-after-tx-commit` | SIGKILL at `after-db-commit` (transaction) — the point that is a bug in the vulnerable version | paid, 1 pending row, 0 events | paid, 0 pending, 1 event | NOT_REPRODUCED |
| `kill-relay-before-publish` | SIGKILL at the relay's `before-event-publish` | paid, 1 pending, 0 events | 1 event | NOT_REPRODUCED |
| `kill-relay-after-publish-before-ack` | SIGKILL at the relay's `after-event-publish` (the relay's CP1002 boundary; pre-check: 1 event published) | paid, 1 pending, 1 event | **2 deliveries, 1 distinct id (1 duplicate)**, 0 pending | NOT_REPRODUCED |
| `exception-relay-after-publish-before-ack` | exception at the same point | paid, 1 pending, 1 event | 2 deliveries, 1 duplicate | NOT_REPRODUCED |

Here the intent survived every failure and the relay completed it. The
outbox does **not** give exactly-once delivery: a crash between publishing
and acknowledging produced a duplicate delivery in every run, which the
declared contract accepts only because events carry an id that consumers can
deduplicate. `RUNTIME_NOT_REPRODUCED` means these invariants held in these
runs, not that the design is proven safe.

## 3. Real-world pattern: midday invoice finalization (isolated reproduction)

**This is an isolated reproduction of a suspected pattern, not a
reproduction in midday.** `fixtures/replay/invoice-pattern/` re-writes the
code shape of the Phase 2 finding `34107b6f5fb66fb4` (midday
`apps/api/src/rest/routers/invoices.ts:487-499` at revision
`51587319f26a0ffaa9dfccab1920373cb65689b7`; the same shape at
`apps/api/src/mcp/tools/invoices.ts:1141-1152` and `:1606-1614`): a shared
query helper `updateInvoice(db, {status: "unpaid"})` in another module
auto-commits, then the route enqueues `generate-invoice` with `triggerJob`.
midday itself was not executed; BullMQ/Redis is replaced by the replay event
log. The static analysis of the re-written code reports the same CP1001
through the same cross-module parameter binding.

The invariant is an *assumption* drawn from code comments ("Trigger invoice
generation (and sending if create_and_send)"), not confirmed by the
maintainers.

| Case | Failure | After | After recovery | Verdict (5/5) |
| --- | --- | --- | --- | --- |
| `kill-after-status-commit` | SIGKILL after the status commit | `unpaid`, 0 jobs | — | RUNTIME_REPRODUCED |
| `enqueue-fails-after-status-commit` | exception before the enqueue (e.g. queue unreachable) | `unpaid`, 0 jobs | — | RUNTIME_REPRODUCED |
| `client-retry-repairs` | SIGKILL after the status commit, then the same request again | `unpaid`, 0 jobs | `unpaid`, 1 job (invariant holds) | RUNTIME_REPRODUCED (the bad state was observed before the retry) |

Re-reading midday's source for this report: no reconciliation that re-enqueues
generation for finalized invoices was found, but a client retry (shown above)
or a manual re-send from the dashboard would repair the state, and nothing
here measures whether either happens. The finding remains a **plausible,
unconfirmed** issue in midday. No upstream issue was filed.

## Findings verified at runtime

| Finding | Status |
| --- | --- |
| `order-vulnerable` CP1001 `completeOrder` (first-party fixture) | RUNTIME_REPRODUCED (5/5 kill, 5/5 exception) |
| `order-vulnerable` CP1001 `completeOrderWithCompensation` | RUNTIME_REPRODUCED for a crash (5/5), RUNTIME_NOT_REPRODUCED for an exception (5/5) |
| `order-outbox` CP1002 `relayOutbox` | RUNTIME_NOT_REPRODUCED against its contract (5/5 kill, 5/5 exception); duplicate delivery observed |
| midday `34107b6f5fb66fb4` (and the two MCP variants) | Pattern reproduced in isolation (5/5); **not** reproduced in midday; not confirmed by maintainers |
| Every other corpus finding | Not runtime-verified (static candidates only) |

## Reproducing these results

```bash
scripts/demo.sh                     # build, static analysis, traces, replays, HTML views
cargo test -p commitrift-cli --test replay
```
