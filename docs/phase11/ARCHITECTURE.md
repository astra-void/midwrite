# Phase 11 architecture: automatic scenario synthesis

```
original TypeScript project
  │  commitrift-analyzer::entries        entry candidates, imports, transaction helpers
  │  commitrift-analyzer::schema         Drizzle tables, keys, foreign keys, defaults
  ▼
project index (commitrift-discover::project)
  │  a probe scenario loaded by the model checker's loader fixes the
  │  table bindings: discovery binds exactly what a scenario will model
  ▼
extraction (extract.rs)        the loader's own request per entry, memoized,
  │                            optionally cached by import closure
  ▼
effect summaries (effects.rs)  db effects with value provenance, guards,
  │                            reachability, transactions, try/handlers, unknowns
  ▼
candidates (detect.rs)         8 detectors + pairing through a resource index;
  │                            protections recorded, rejections recorded
  ▼
properties (candidate.rs)      every property has an Origin and a Strength
  ▼
scenarios (synth.rs)           arguments, rows found by running each actor
  │                            alone in the engine, invariants, variants;
  │                            written in the existing scenario format
  ▼
model checking (pipeline.rs)   report::check, counterexample validation
  ▼                            (strict replay, violation, operations exercised)
ranking, deduplication, report (report.rs: JSON, text, script-free HTML)
  ▼
optional, separate, authorized: commitrift model-check <emitted scenario>
                                --verify-source --allow-execution
```

## Principles

1. **No second semantics.** Discovery never lowers TypeScript and never
   evaluates SQL. Entries are extracted by the request the scenario loader
   makes; rows are found by the model's engine; scenarios are loaded and
   validated by the scenario loader; verdicts come from `report::check`.
   An emitted scenario is an ordinary scenario file:
   `commitrift model-check` gives the same verdict and state count
   (`crates/commitrift-cli/tests/phase11.rs`).
2. **Static by default, budgeted model checking, separately authorized
   execution.** `commitrift discover` never executes application code. With
   `--model-check` it explores the synthesized models within per-scenario
   and total budgets. Running original code stays where it was: `commitrift
   model-check --verify-source --allow-execution` on an emitted scenario.
3. **Provenance before precision.** Every property carries its origin
   (`SCHEMA_ENFORCED`, `SOURCE_ASSERTED`, `USER_DECLARED`,
   `FRAMEWORK_CONTRACT`, `HEURISTIC_HYPOTHESIS`); every assumption carries
   its origin (`source-derived`, `declared`, `framework`,
   `discovery-default`); no later step changes them. A counterexample
   against a hypothesis is reported as such.
4. **Unsupported is visible.** Unknown operations of every entry are
   listed and categorized; a candidate with an unknown operation on its
   path is never reported safe; refused tables, failed extractions and
   unsatisfiable states are reported with the reason.

## The boundary between static discovery and declared contracts

| from source (static) | declared (`commitrift-contracts.json`) |
| --- | --- |
| entries and their syntactic kind | whether an exported function is called concurrently (`concurrent`) |
| wrapper invocation and guards | the invocation of a wrapper whose source is unknown (`invocation`, `preconditions`) |
| database effects, predicates, keys, transactions, locks | which calls are external effects (`externals`) and whether a channel deduplicates by key (`dedup`) |
| transaction helpers without a loop (one attempt) | helpers with a retry loop (`transactionHelpers` with `attempts`) |
| retry-after-failure code (`try`/`catch` repeating a write) | at-least-once delivery (`delivery`), client retries after a crash (`retry`) |
| unique constraints, keys, foreign keys, defaults | business invariants (`invariants`, templated) |
| | values that validation code accepts (`values`), harness inputs (`harness`) |

The file format is in `CONTRACTS.md`.

## Where each topic is documented

The requested documents were consolidated where they overlap:

| topic | document |
| --- | --- |
| entries, effect summaries, **resource identity** | `EFFECT-INDEX.md` |
| detectors, pairing, feasibility, **ranking and deduplication** | `CANDIDATE-DISCOVERY.md` |
| property origins, strengths, templates | `INVARIANT-SYNTHESIS.md` |
| arguments, rows, **scenario construction** and variants | `STATE-SYNTHESIS.md` |
| extraction completeness, counterexample validation, runtime | `SOURCE-MODEL-FIDELITY.md` |
| contracts file | `CONTRACTS.md` |
| corpus | `REAL-WORLD-EVALUATION.md` |
| labeled benchmarks | `PRECISION-RECALL.md` |
| costs | `PERFORMANCE.md` |
| authorization and privacy | `SECURITY.md` |

## Code

| crate / module | role |
| --- | --- |
| `commitrift-analyzer/src/entries.rs` (new) | entry candidates, imports, transaction helpers |
| `commitrift-analyzer/src/concurrency.rs` | one fix: excerpts never split a multi-byte character |
| `commitrift-discover` (new crate) | `contracts`, `project`, `extract`, `effects`, `candidate`, `detect`, `synth`, `pipeline`, `report`, `eval` |
| `commitrift-model/src/invariant.rs`, `source_verify.rs` | outcome terms (`actors`) evaluated after an original-code run without crashes or retries (no earlier scenario uses them: no earlier result changes) |
| `commitrift-cli/src/discover.rs` (new) | `commitrift discover` |
