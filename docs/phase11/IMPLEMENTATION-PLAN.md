# Implementation plan (as executed)

| stage | done | where |
| --- | --- | --- |
| A baseline | clean-tree gates, Phase 10 binary saved, inspection | `BASELINE.md` |
| B index | entries, imports, transaction helpers; probe-scenario table bindings; memoized loader-identical extraction; effect summaries | `entries.rs`, `project.rs`, `extract.rs`, `effects.rs` |
| — fixture | commerce application and labels written before any detector | `fixtures/phase11/` |
| C discovery | check-then-act and read-modify-write first, then status transition, write skew, duplicate processing, external-effect gap (declared retry, retried transaction), ambiguous commit, lock order; pairing | `detect.rs` |
| D provenance | origins, strengths, templates, declared invariants | `candidate.rs`, `contracts.rs` |
| E states | engine-driven seeding, foreign keys, joins, correlation, contradictions | `synth.rs` |
| F scenarios | existing format, variants, deterministic files | `synth.rs`, `pipeline.rs` |
| G model checking | budgets, algorithm recorded, validation, ranking, dedup | `pipeline.rs` |
| H runtime | vertical slice `inviteMember` reproduced on original code, then 10 scenarios × 3 | `crates/commitrift-cli/tests/phase11.rs`, `scripts/phase11-evidence.py` |
| I coverage | remaining families, held-out labels and the fixes they prompted | `PRECISION-RECALL.md` |
| J corpus | four projects, static + bounded model checking, no execution | `REAL-WORLD-EVALUATION.md` |
| K regression | identity checks, full suite, timings | `FINAL-REPORT.md`, `PERFORMANCE.md` |
| L documentation | this directory | |

Not done: Linux run, independent review, generated-primary-key modeling,
shared resolver across extractions.
