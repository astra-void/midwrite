# Developer alpha readiness (Phase 15)

| aspect | state | evidence |
| --- | --- | --- |
| installation | build from source (`cargo build --release`); Rust 1.97+; Node 22 and PostgreSQL 15+ only for the optional runtime steps; no packages published | `docs/getting-started/installation.md` |
| command discoverability | `commitrift --help` lists the subcommands; `discover --help` documents `--max-worlds`, `--all-worlds`, `--explain-coverage` | CLI |
| first-run workflow | `discover <app> --model-check` → candidate with status, property origin, input exploration, schedule exploration, next step | quickstart, validated with the final binary |
| error quality | every candidate has a first blocker; unsatisfiable synthesis now names where the actor stopped and with what failure; unmodeled protections are named | `CONSTRAINT-SYNTHESIS.md` |
| report readability | a no-counterexample result says how many worlds, which relationships, whether schedules completed, and that general safety is not established; `--explain-coverage` lists every world and every gap | `VERDICT-SEMANTICS.md` |
| deterministic output | JSON, text and emitted scenarios byte-identical across runs (test `world_fingerprints_differ_with_the_inputs_and_reports_are_deterministic`) | tests |
| resource budgets | `--max-worlds`, `--max-states`, `--max-total-states`, `--max-wall-ms`, `--max-memory-bytes`, `--max-candidates`; every skip reported | `FEEDBACK-GUIDED-SEARCH.md` |
| unsupported-feature explanations | first blocker per candidate; unknown operations by category per project | discovery report |
| cross-platform | macOS arm64 full gates; Linux: see `FINAL-REPORT.md` §8 | |
| source-execution authorization | unchanged: `--verify-source --allow-execution` + a user-written harness | `SECURITY.md` |
| reproducible counterexamples | `--emit-scenarios` writes a `*.counterexample.json` per finding (world, rows, arguments, assumptions, bounds, schedule, source fingerprint, commands) | `FEEDBACK-GUIDED-SEARCH.md` |
| real-world useful findings | **none**: 0 model-checkable candidates on midday and school-ledger (more precise blockers only); saas-starter unchanged (1 heuristic counterexample) | `REAL-WORLD-EVALUATION.md` |

## Release blockers (honest list)

1. **Real-world model-checkability is still zero** on the two larger
   corpus projects. school-ledger is now blocked by zod `strictObject`
   (after its authorization guards were satisfied); midday by properties
   over generated identifiers and undeclared multi-row invariants.
2. **Recall on realistic code** is moderate on fresh input-domain cases
   (0.308 held out, 0.615 after feedback) and low on the Phase 14 set
   (0.278); the misses are mostly unsupported source constructs (loops,
   request bodies, zod formats, time arithmetic).
3. **No independent human review** of any label or result; no external
   user has run the tool.
4. **Replica counterexamples are model-only.**
5. Windows is not tested.

## Recommendation

Suitable for a **closed developer alpha** on first-party Drizzle/PostgreSQL
code written in the supported subset, with the coverage section read as
part of every result. Not ready for an open release: the tool still
cannot verify the realistic corpus projects end to end.
