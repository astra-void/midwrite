# Phase 7 baseline

Captured on 2026-10-25/26 before any Phase 7 change, on the machine all
Phase 7 measurements use: Apple Silicon (arm64), macOS 27.0.1, rustc 1.98.1,
Node.js 22.23.2, PostgreSQL 17.11 (Homebrew). Raw logs:
`reports/phase7/baseline/`.

## Quality gates (Phase 6 code, unchanged)

| Gate | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean (exit 0) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **381 passed, 0 failed, 0 ignored** — matches the Phase 6 report |
| `cargo build --release` | ok |

Replay dependencies (PostgreSQL server binaries, Node.js ≥ 22,
`fixtures/replay/drizzle-node/node_modules`) were present; nothing was
skipped.

The Phase 6 release binary was preserved before any rebuild
(`commitrift-phase6`, SHA-256 in
`reports/phase7/baseline/phase6-binary.sha256`) and is the reference for
every same-machine comparison in [`PERFORMANCE.md`](PERFORMANCE.md) and
[`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md).

## What the audit found

* **Function selection.** The concurrency extractor found entry functions
  by *name* among top-level declarations only; the static analyzer already
  named anonymous functions by position (`<anonymous@line:col>`). Wrapped
  callbacks (`validatedActionWithUser(schema, async (...) => ...)`) could
  not be selected at all.
* **Duplicated frontends.** `concurrency.rs` (≈2 900 lines) re-implemented
  module loading, import resolution, Drizzle chain flattening and predicate
  lowering separately from `lower.rs`/`recognize.rs`. The two share the
  resolver and the configuration, not the semantics.
* **Repeated traversal.** The path walker (`core/analysis/paths.rs`) sends
  *each* trace that reaches an inlined call through the callee's body
  separately; with *k* incoming traces a callee is walked *k* times. Phase 6
  profiling had located the depth cost there ("path analysis +114 % from
  depth 3 to 6" on school-ledger).
* **Lost source semantics in lowering** (each is fixed in Phase 7 and
  covered by a test):
  * an `Expr::Unknown` inside a local operation (`let x = a ?? b`) made the
    actor *fail* at evaluation — unsupported source silently became a
    modeled exception instead of `UNSUPPORTED`;
  * `a || b` / `a && b` in value position evaluated to a boolean (the
    JavaScript-mode `Or` returns `true`/`false`), not to `a` or `b`;
  * `select(...)` projections were ignored (full row read, recorded as an
    assumption), joins, `RETURNING` rows, generated keys and defaults were
    unsupported, `rows[0]` was unknown;
  * `try`/`catch`/`finally`, `switch` and loops were all `Unknown`;
  * the oracle replayed model SQL only; Drizzle's own SQL was never
    observed under a concurrent schedule.
