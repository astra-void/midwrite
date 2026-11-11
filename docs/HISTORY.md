# Development history

Midwrite was built in sixteen development phases. Each phase asked one
question, wrote a baseline, design notes, evaluations and a final report,
and kept its raw evidence (benchmark output, differential matrices,
original-code runs, corpus counts, review packets). The product was called
CrashProof in Phases 1–2 and CommitRift from Phase 3 until it was renamed
Midwrite after Phase 16; the phase records use the name of their time.

The current documentation describes the system as it is, by topic. The
phase-by-phase records are no longer in the working tree. Everything
still valid in them — design decisions, semantics, formal assumptions,
limitations, security properties, evaluation methodology and dataset
definitions — was folded into the topic documents linked from the
[documentation index](README.md). What was not carried over is history:
per-phase baselines, implementation plans, measurements of a given binary,
and final reports.

## Reading the archived records

The records are preserved, unchanged, in the git tag
**`history/phase-records`** (commit `87186c0`). The tag holds
`docs/phaseN/` (design notes and reports per phase) and `reports/phaseN/`
(raw evidence per phase).

```bash
# list what a phase recorded
git ls-tree -r --name-only history/phase-records -- docs/phase13 reports/phase13

# read one document
git show history/phase-records:docs/phase13/FINAL-REPORT.md

# restore a phase's raw evidence into the working tree (do not commit it)
git checkout history/phase-records -- reports/phase13
```

Numbers in those records belong to the binary of that phase and are not
re-measured. The [evidence index](research/evidence-index.md) summarizes
the headline results and names, for each, the phase record that holds the
raw evidence and the tests and fixtures in the current tree that exercise
the same behavior. A few report files are still used as test inputs and
were moved to [`fixtures/reports/`](../fixtures/reports); the report JSON
Schemas of every version are in [`reference/schemas/`](reference/schemas).
The evaluation sets frozen in Phases 11–16 are in
[`fixtures/eval/`](../fixtures/eval)
([evaluation datasets](research/evaluation-datasets.md)).

## Phases

### Phase 1

The static analyzer (no separate record): Oxc-based frontend, the Effect
IR, failure-path analysis between database commits and external effects,
the first diagnostics and text, JSON and SARIF output. See
[ARCHITECTURE.md](ARCHITECTURE.md).

### Phase 2

Real-world evaluation of `check` on four projects (two public, two
private) with a manual precision review; cross-module analysis (imported
functions, call summaries with parameter binding), policies (`recommended`,
`audit`) and baselines for CI. Record: `docs/phase2/`, `reports/phase2/`.

### Phase 3

Rename CrashProof → CommitRift. Failure traces (`trace`), opt-in runtime
replay of a single request against a disposable PostgreSQL with a real
process kill after a verified commit, verification reports and the HTML
view. Record: `docs/phase3/`, `reports/phase3/`.

### Phase 4

Replay on Node.js with the real Drizzle ORM, Linux isolation with
bubblewrap, `init-replay` harness generation, recovery contracts, the
regression report and release readiness. Record: `docs/phase4/`,
`reports/phase4/`.

### Phase 5

Bounded interprocedural abstract interpretation for `check`: the value
layer, heap and aliases, class and method resolution, callbacks,
asynchronous structure, inertness summaries and effect relationships.
Record: `docs/phase5/`, `reports/phase5/`.

### Phase 6

The explicit-state model checker (`model-check`): concurrent IR, the
PostgreSQL isolation model (READ COMMITTED, REPEATABLE READ,
SERIALIZABLE), row locks, crashes and retries, invariants, sleep-set
partial-order reduction, minimized counterexamples, and the lockstep
differential oracle against PostgreSQL. Record: `docs/phase6/`,
`reports/phase6/`.

### Phase 7

Model extraction from TypeScript source: source selectors and function
identity, wrapped Server Action callbacks, Drizzle joins, projections,
sequences and `RETURNING`, `try`/`catch`/`finally` and `switch`; compact
path summaries; first original-code verification (`--verify-source`) on a
disposable database. Record: `docs/phase7/`, `reports/phase7/`.

### Phase 8

Crash and retry against original code (real SIGKILL, new process, effect
test doubles), the postgres-js adapter, Drizzle schema extraction and
schema-fidelity checks against the catalog, parameterized shared
summaries, state-space optimizations. Record: `docs/phase8/`,
`reports/phase8/`.

### Phase 9

Lost acknowledgments: in-flight operations whose fate the client does not
know, client knowledge and possible worlds, processes, tasks and
connection pools, the protocol fault-injection proxy, classic DPOR and
compact visited-state storage. Record: `docs/phase9/`, `reports/phase9/`.

### Phase 10

Source-set DPOR with a summarizing state cache; adversarial,
metamorphic, mutation and audit testing of every reduction against the
unreduced search; the formal assumptions of the reductions; live
connection loss under a running process; postgres-js in-flight faults.
Record: `docs/phase10/`, `reports/phase10/`.

### Phase 11

Automatic scenario synthesis (`discover`): entry index, effect summaries,
detectors, property candidates with stated provenance, state synthesis,
contracts (`midwrite-contracts.json`), model checking within budgets and
counterexample validation; the first first-party and held-out evaluation
sets. Record: `docs/phase11/`, `reports/phase11/`.

### Phase 12

Schema fidelity and generated values (`$defaultFn`, `defaultRandom`,
database defaults, `RETURNING` into foreign keys), multi-module schemas,
cross-module resolution through repositories and barrels, a shared
project analysis context; a fresh frozen evaluation set. Record:
`docs/phase12/`, `reports/phase12/`.

### Phase 13

Row-lock modes and foreign-key checks (with an independent PostgreSQL
ground truth), return values and the response step, read replicas,
`ON CONFLICT DO UPDATE`, source-resolution work on the real corpus; an
evaluation set written without access to the implementation. Record:
`docs/phase13/`, `reports/phase13/`.

### Phase 14

JavaScript values (`undefined`, strings, arrays, `Map`/`Set`), closures and
project retry wrappers, zod validation (first subset), multi-row inserts,
`inArray` and conditional predicates, timestamps and JSON subsets, the
replica round trip, parent-child and return-value candidate families,
next-step guidance; a documentation consolidation. Record:
`docs/phase14/`, `reports/phase14/`.

### Phase 15

Typed input domains, a feedback-guided search over initial worlds,
guard-satisfying state synthesis with stated preconditions, coverage
accounting in four dimensions, reproduction artifacts, a fresh frozen
evaluation set. Record: `docs/phase15/`, `reports/phase15/`.

### Phase 16

zod object validation for zod 3 and 4 (checked differentially against the
real runtimes), bounded and retry loops, caught database errors,
`request.json()` and `Response.json`, held Drizzle predicates, multi-row
row locks, `NULLS NOT DISTINCT`, properties over generated identifiers,
the discovery-report JSON Schema, quality gates and the Closed Alpha
package. Record: `docs/phase16/`, `reports/phase16/`.

### After Phase 16

The product was renamed Midwrite (crates, binary, diagnostic codes,
report kinds and configuration file names), a dev / held-out precision
and recall harness was added
([evaluation harness](development/evaluation-harness.md)), and the
phase-numbered layout of documentation, tests, fixtures and scripts was
replaced by topic names; the records above were moved to the tag.
