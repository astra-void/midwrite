# Phase 5 final report

Question: *can CommitRift understand more real programs correctly —
values, objects, methods, callbacks and asynchronous structure — without
hiding what it does not understand?*
Measured answer: **yes, within a bounded and documented subset; the effect
on real-world incompleteness is modest and the cost is a ~29 % slowdown.**
Six of the 140 Phase 4 incomplete traces became complete through validated
facts; most remaining unknown calls are SDK, package or raw-SQL operations
(now named precisely) or effectful helpers beyond the depth bound.

Measured on 2026-10-25 on one Apple Silicon Mac (macOS) and in a Debian 12
container (Linux 7.0.12, aarch64) on the same machine. Raw data:
`reports/phase5/`. Baseline: [`BASELINE.md`](BASELINE.md); design:
[`ANALYSIS-DESIGN.md`](ANALYSIS-DESIGN.md).

## 1. Architecture

| Area | Implementation | Document |
| --- | --- | --- |
| Abstract domain | bounded atom sets (≤ 6) with ⊤, canonical and inline; atoms for literals, functions, classes, allocation sites, records, parameters/`this`, call and effect results, opaque values with identity, property paths (depth ≤ 3), static data | [`ABSTRACT-DOMAIN.md`](ABSTRACT-DOMAIN.md) |
| Environment | symbol-id keyed, flow-sensitive, copy-on-write; closure-written bindings are ⊤ | same |
| Transfer functions | declarations, destructuring, assignments, property reads/writes, calls, `new`, returns, branches (fork/join), try, loops | same, [`PATH-ANALYSIS.md`](PATH-ANALYSIS.md) |
| Heap | one object per allocation site; strong updates only on one non-summary object; weak otherwise; transitive selective escape; export to records | [`HEAP-AND-ALIASES.md`](HEAP-AND-ALIASES.md) |
| Context sensitivity | handles substituted on copy (Phase 2); callback identities in the memo key, only for parameters the callee may run, ≤ 8 contexts per callee with fallback | [`INTERPROCEDURAL-SUMMARIES.md`](INTERPROCEDURAL-SUMMARIES.md) |
| Function summaries | inlined effect IR (Phase 2) + return values, return facts, provenance substitution; inertness (greatest fixpoint) and parameter use (least fixpoint) over Tarjan SCCs, with budgets | same |
| Method resolution | class table across modules; exact (`new C`), `this` (no analyzed override), static, `super`; fields before prototype methods; dynamic changes and multiple targets → unresolved; package superclass → recorded assumption, ≤ medium | [`CLASS-RESOLUTION.md`](CLASS-RESOLUTION.md) |
| Async semantics | concurrent summaries of `Promise.all` elements, IIFEs, callbacks, awaited-argument placeholders (`withTimeout`), array-callback loops, deferred timers, `new Promise` executors | [`ASYNC-SEMANTICS.md`](ASYNC-SEMANTICS.md) |
| Path analysis | existing walker + member facts, cross-call return facts, liveness-keyed trace merging; loop value fixpoint (optimistic pass, Kleene iterations, widening, havoc budget) | [`PATH-ANALYSIS.md`](PATH-ANALYSIS.md) |
| Effect relationships | provenance-based `valueRelation` in diagnostics and traces (trace schema v2); descriptive only | [`EFFECT-RELATIONSHIPS.md`](EFFECT-RELATIONSHIPS.md) |
| State-explosion controls | atom bound, field depth, record size, input cap, loop iterations (3) and dry-run budget (48), specialization limit (8), SCC iteration budget, `--max-paths`, `--max-call-depth` — every limit counted and reported | all of the above |

New code: `crates/commitrift-core/src/absint/` (value, heap, env, summary,
relation), `crates/commitrift-analyzer/src/{values,classes,link,builtins}.rs`;
extended: `lower.rs`, `functions.rs`, `bindings.rs`, `exports.rs`, `lib.rs`,
`project.rs`, core `ir.rs`, `analysis/{summary,paths,detect,render,trace}.rs`,
`trace.rs`, `diagnostic.rs`, corpus tool (`traces --unknowns`, Phase 5
counters, value relations), `scripts/phase5-attribute.py`,
`docs/phase5/schemas/trace-report.v2.schema.json`.

## 2. Correctness

| | macOS arm64 | Linux aarch64 (bubblewrap) |
| --- | --- | --- |
| `cargo fmt --check`, `cargo clippy -D warnings` | clean | clean (rustc 1.99.1) |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **300 passed, 0 failed, 0 ignored** | **300 passed, 0 failed, 0 ignored** |
| `cargo build --release` | ok | ok; packaging ok |
| Node + Drizzle replay (16 cases), `--repeat 5`, generated harness (5 cases) | unchanged tests pass | 16/16 and 5/5 as declared |
| Schemas | trace v2 (5 reports) and v1, SARIF 2.1.0 (fixtures 107 results, midday 282), Phase 4 verification/regression reports: 0 errors, corrupted copies rejected | — |

300 = 245 (Phase 4) + 55: core unit +28 (domain, heap, env, solver,
relations; 6 seeded property tests), `phase5.rs` 24, `differential.rs` 3
(Node.js executions), fixtures and projects (new files checked by the
existing harness: 7 vulnerable, 3 safe-with-new-meaning, 3 unsupported, 5
differential programs, 2 project fixtures), `traces.rs` and `cli.rs`
updated for schema v2, one Phase 1 test updated for the intended
`forEach` semantics, one project fixture (`pure_helper`) updated for
inertness. The default-seccomp negative Linux check was not re-run
(isolation code unchanged).

**Metamorphic tests**: 7 behavior-preserving transformations (renamed locals,
extracted helper, pure wrapper, return through a local, method of a known
instance, helper passed as a callback, helper moved to another module and
imported under an alias) give identical normalized findings; 6
behavior-changing ones (root → transaction handle, awaited → fire-and-forget
callback, known → parameter receiver, reassignable method, sequential →
concurrent, escaping object) change the evidence as expected.

**Bug found by the corpus**: return facts on nested inlined calls were
merged away by a prefix-based rule, losing feasible paths (and CP1004s);
fixed with a liveness-based key and a regression test.

**Known false positives** (unchanged in kind): adjacent unrelated
operations; compensation outside the function; plus new findings in
functions over the path budget whose explored prefix changed (midday
`match-transactions-bidirectional.ts` `run`, partial coverage).
**Known false negatives**: methods on values of unknown class, captured
transaction handles in callbacks, `.then` continuation bodies, unawaited
calls of helpers, effects beyond the depth bound, loops (one iteration),
non-inert callees without recognized operations (one unknown, inner call
named only in the detail). Unsupported semantics and the soundness
assumptions are listed in [`../LIMITATIONS.md`](../LIMITATIONS.md).

## 3. Corpus evaluation

Full analysis: [`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md).

| Project | Diagnostics P4 → P5 | Warnings | Traces: complete / approx. / incomplete (P4 → P5) |
| --- | --- | --- | --- |
| saas-starter | 21 → 23 | 1 → 1 | 14/1/0 → 14/4/0 |
| midday | 269 → 282 | 49 → 52 | 126/7/47 → 134/14/44 |
| school-ledger | 317 → 332 | 0 → 0 | 30/0/93 → 30/0/101 |
| unnamed-ai-project | 17 → 17 | 0 → 0 | 8/0/0 → 8/0/0 |

* **Incomplete traces**: of the 140 Phase 4 incomplete traces, **6 became
  complete** (midday; concurrent read-only queries summarized), 134 remain;
  11 new incomplete traces belong to newly visible findings. 14 of the 41
  remaining midday incomplete traces now name the actual unknown operation
  (Supabase calls behind `withTimeout`, raw SQL behind `getTeamCalibration`).
* **Newly resolved calls**: calls summarized 2 523 → 5 035 (all projects);
  2 230 calls resolved as inert; 356 method calls resolved (87 under a
  recorded assumption, 18 unresolved); 159 callbacks linked; 178 concurrent
  calls summarized.
* **Diagnostics added/removed**: +51 / −21; every removed finding is a
  CP9001; every public change is explained individually (concurrent
  summaries, array callbacks, method dispatch, inertness, argument
  placeholders, path budget). New midday warnings: 3 × CP1001
  (`documents.ts:121`, `team.ts:358`, and one from a truncated function),
  3 × CP1005 (`info` under `recommended`).
* **Why school-ledger did not improve**: effectful helpers beyond depth 3
  dominate; depth 6 would add 261 findings — not changed without review.
* **Review**: 0 independent human reviews (still); new warnings should join
  the 12 prepared packets.

## 4. Performance

[`PERFORMANCE.md`](PERFORMANCE.md).

| | Phase 4 | Phase 5 |
| --- | --- | --- |
| midday, 8 threads | 0.162 s, 66 MiB | 0.209 s (+29 %), 96 MiB (+45 %) |
| school-ledger, 8 threads | 0.118 s, 73 MiB | 0.153 s (+29 %), 89 MiB (+22 %) |
| midday, 1 thread | 0.372 s, 55 MiB | 0.479 s (+29 %), 78 MiB |
| midday live heap peak | 27.4 MiB | 39.6 MiB |
| summary memo hit rate | midday 61 %, school-ledger 83 % (fewer, context-free entries) | 35 % / 67 % |

The first complete build was 1.82× slower and peaked at 51.9 MiB; profiling
led to eight optimizations without precision loss (largest: copy-on-write
state, cached lookalike paths, inline values, interned facts, releasing
lowering data). Output is identical with 1 and 8 threads. Replay is
unchanged and separate.

## 5. Remaining work

**Unresolved high-impact limitations**

1. Unknown SDK operations (Supabase, Slack, Trigger.dev, BullMQ) are now the
   dominant unknowns in midday windows; they need configuration or an
   evidence-backed provider model, not more inference.
2. The depth bound (school-ledger); raising it is cheap but changes many
   findings — needs a reviewed decision, possibly per-project.
3. Captured variables in callbacks (transaction handles, values).
4. Non-inert callees without recognized operations stay one opaque unknown
   step; showing their inner unknown calls in traces would improve
   attribution (requires a policy for unawaited inner calls).
5. Loop effects (repeated commits between iterations) and `.then`
   continuations.
6. The 29 % slowdown and 22–45 % RSS growth.

**Deferred to Phase 6**: bounded concurrency exploration of concurrent
groups (completion orders), database isolation-level modeling, repeated
loop iterations, persistent environments, SDK effect models.

**External verification still needed**: human review of new and old
warnings; any reproduction in an external application (none was attempted;
no external code was executed); a green GitHub CI run (no remote).

**Recommended next research priorities**: (1) evidence-backed SDK effect
configuration (shipped presets with tests); (2) callback closures with
captured frames; (3) a reviewed depth policy; (4) concurrency-group
exploration for the new concurrent windows; (5) performance of the value
layer.
