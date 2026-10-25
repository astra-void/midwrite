# Phase 6 baseline

Recorded on 2026-10-25, before any Phase 6 code was written, on one Apple
Silicon Mac (macOS, arm64). Raw output: `reports/phase6/baseline/`.

## Quality gates (Phase 5 code, unmodified)

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **300 passed, 0 failed, 0 ignored** (replay prerequisites present: PostgreSQL 17.11 Homebrew, Node.js 22.23.2, Bun) |
| `cargo build --release` | ok |

The Phase 5 release binary was preserved before any change
(`sha256 71ad0af6…64ca`) and is the "Phase 5" binary of every Phase 5 vs
Phase 6 comparison in [`PERFORMANCE.md`](PERFORMANCE.md) and
[`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md). Phase 5 corpus results are
in `reports/phase5/corpus/` and were re-measured in Phase 6 with both
binaries on the same machine.

## What the existing IR can and cannot provide

Inspected: `crates/commitrift-core/src/ir.rs` (Effect IR),
`analysis/` (path analysis, traces), `absint/` (values, provenance),
`trace.rs` (Failure Trace IR), `commitrift-replay` (isolated runtime
replay).

Reusable as is:

* **Module resolution and function discovery** of the analyzer (relative
  imports, tsconfig `paths`, workspace packages) and the configuration
  (`databaseReceivers`, `externalEffects`).
* **Execution classes** of effects (`Awaited`, `Returned`, `FireAndForget`,
  `Concurrent`) — the distinctions Phase 6 needs for intra-actor ordering.
* **Effect identities and locations** (`EffectId` per function, exact
  source ranges) and **diagnostic fingerprints**: Phase 6 links every
  extracted operation to them for traceability.
* **The isolated PostgreSQL cluster** of `commitrift-replay`
  (`postgres::Cluster`, `safety::ReplayRoot`): private socket, no TCP,
  removed on drop.

Not representable in the Effect IR (hence a separate concurrency IR):

* **What a statement does**: the Effect IR knows that `db.update(products)`
  writes `products`, not its predicate (`WHERE id = $productId`), its `SET`
  expressions (`stock = product.stock - 1` vs. `stock = stock - 1`), the
  values inserted, `ON CONFLICT`, `FOR UPDATE`, or `.returning()` results.
  These determine every concurrency outcome.
* **Data flow from reads to later decisions** at the level of row fields
  (`if (!product || product.stock <= 0) throw`): value provenance records
  *where* a value comes from, not its concrete content.
* **Transaction isolation levels** (`isolationLevel` options are not read),
  **snapshots**, **row versions**, **locks** and **waiting**.
* **Multiple executions** of the same function, **crashes as events** with
  durable consequences, **retries**, and **external-effect logs**.
* **Constraints**: unique keys, NOT NULL, CHECK (Phase 1–5 never needed
  them; Phase 6 does not infer them either — they are declared).

Concurrency behaviors not represented anywhere before Phase 6: interleaving
of two requests, lost updates, write skew, check-then-act on shared rows,
lock waits and deadlocks, serialization failures, duplicate external effects
through redelivery or retry, idempotency enforcement.

## Measurements

Phase 5 static analysis performance is re-measured with the preserved
binary in [`PERFORMANCE.md`](PERFORMANCE.md) (the recorded Phase 5 values:
midday 0.209 s / 96 MiB at 8 threads; school-ledger 0.153 s / 89 MiB).
