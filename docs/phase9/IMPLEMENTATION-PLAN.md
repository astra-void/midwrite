# Phase 9 implementation plan and what was done

The stage order was fixed by the task (A–J); where time or risk forced a
choice, the priority was: in-flight COMMIT semantics → a real original-code
ack-loss reproduction → shared-process crash → verified DPOR → memory
reduction. All five were completed; the table says what each stage ended up as.

| stage | content | where | status |
| --- | --- | --- | --- |
| A | baseline: Phase 8 gates, digest, saved Phase 8 binary, measured cost of the Phase 8 model | `BASELINE.md`, `reports/phase9/baseline/` | done, identical to the Phase 8 report |
| B | formal model: operation lifecycle, physical state vs application knowledge, orphans, worlds | `FORMAL-MODEL.md`, `OPERATION-LIFECYCLE.md`, `epistemic.rs`, `engine.rs`, `state.rs` | done |
| C | epistemic analysis: possible worlds, twin analysis, canonicalization with knowledge | `EPISTEMIC-SEMANTICS.md`, `AMBIGUOUS-COMMIT.md`, `twin.rs` | done |
| D | protocol fault-injection proxy (PostgreSQL wire v3, test-only) | `PROTOCOL-FAULT-INJECTION.md`, `commitrift-replay/src/proxy.rs` | done |
| E | original-code in-flight reproduction (held COMMIT, withheld answer, SIGKILL, retry, durable state) | `SOURCE-REPRODUCTION.md`, `source.rs`, `runtime/node/source.js` (protocol 4) | done for `pg`/drizzle; postgres-js not covered |
| F | shared processes, tasks, bounded pools, stuck-state classification | `PROCESS-AND-TASK-MODEL.md`, `CONNECTION-POOL.md` | done |
| G | dynamic partial-order reduction with differential validation | `DYNAMIC-POR.md`, `tests/dpor.rs` | done; a memory-for-time trade, not a speedup (`PERFORMANCE.md`) |
| H | compact visited-state storage with exact identity | `STATE-SPACE-STORAGE.md`, `keytable.rs`, `state.rs::Interner` | done |
| I | first-party failure-oriented scenarios A–G, CLI/report/HTML/schema 4 | `fixtures/phase9/`, `docs/phase9/schemas/`, `commitrift-cli` | done |
| J | regression, benchmarks, cross-platform, docs, final report | `CORPUS-EVALUATION.md`, `PERFORMANCE.md`, `FINAL-REPORT.md` | see `FINAL-REPORT.md` for what ran |

## Order in which the code landed (commits on `phase-8`)

1. `a9a7aac` in-flight operations, pending server completion, twin worlds
2. `768977a` in-flight COMMIT verification against original code (proxy, protocol 4)
3. `63645e5` dynamic partial-order reduction and its differential tests
4. `52744af` compact visited-state keys, bounded pools, stuck classification
5. `0d25375` shared-process tasks, pool and stuck-state verification on original code; HTML, schema, docs
6. the commit after it: storage profile, varint key table, benchmark and identity scripts, remaining documents

## Decisions that shaped the work

* **A server completion is a transition of its own.** Treating "the COMMIT
  reached the server" as instantaneous hid the reconciliation race (read, then
  retry, then the held COMMIT completes). Orphans with `ServerComplete` fix
  that and are what the twin analysis and the runtime `held-for-later-delivery`
  fate are built on (`OPERATION-LIFECYCLE.md`).
* **No approximation of unsupported fates.** A live process with a broken
  connection (`LostCommitted`/`LostNotCommitted`) has no runtime realization;
  the plan step is refused with UNSUPPORTED rather than replaced by a crash.
* **DPOR stays opt-in** and is refused when an `always` invariant is present;
  sleep sets remain the default because they are faster on every workload
  measured.
* **Exact state identity.** Keys are compared byte for byte; the hash only
  selects a bucket (`STATE-SPACE-STORAGE.md`).
* **Schema 4 is additive over 3**, and reports without Phase 9 features keep
  their earlier schema version and bytes.
