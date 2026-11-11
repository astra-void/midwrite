# Output formats

Every command writes human-readable text by default. Machine-readable
output carries a `kind` and a `schemaVersion`; a schema version changes
only when the output uses fields of the newer version, so older consumers
keep working on output that does not use new features.

## Static analysis (`check`)

| format | content | schema |
| --- | --- | --- |
| text | diagnostics with source excerpt, failure sequence, help, summary line | — |
| JSON (`--format json`) | `schemaVersion` 1: `tool`, `config`, `summary` (incl. `summary.analysis` counts), `diagnostics`, `failures` | described in [`DIAGNOSTICS.md` § JSON output](../DIAGNOSTICS.md#json-output) |
| SARIF 2.1.0 (`--format sarif`) | one result per diagnostic; `partialFingerprints` carry the baseline fingerprint | [`DIAGNOSTICS.md` § SARIF output](../DIAGNOSTICS.md#sarif-output) |
| baseline file (`--write-baseline`) | fingerprints of current findings | — |

Diagnostic fields (`code`, `severity`, `confidence`, `pathCertainty`,
`evidenceStrength`, `effects`, `failureSequence`, `evidence`,
`limitations`, `recovery`, `valueRelation`, `fingerprint`, `kind:
"potentialHazard"`, `reproduced: false`) are documented in
[`DIAGNOSTICS.md`](../DIAGNOSTICS.md).

## JSON kinds

| `kind` | versions | produced by | JSON Schema |
| --- | --- | --- | --- |
| `midwrite.traces` | 1, 2 | `trace --format json` | [v1](schemas/trace-report.v1.schema.json), [v2](schemas/trace-report.v2.schema.json) (adds `valueRelation`); [format](trace-format.md) |
| `midwrite.verification` | 1, 2 | `replay --format json` | [v1](schemas/verification-report.v1.schema.json), [v2](schemas/verification-report.v2.schema.json) (isolation backend, recovery observation, surfaced faults); [protocol](../architecture/replay-protocol.md#reports) |
| `midwrite.regression` | 1 | `replay --baseline … --regression-output` | [v1](schemas/regression-report.v1.schema.json) |
| `midwrite.model-check` | 1–5 | `model-check --format json` | [v1](schemas/model-check-report.v1.schema.json), [v2](schemas/model-check-report.v2.schema.json) (source verification), [v3](schemas/model-check-report.v3.schema.json) (crash/recovery, schema facts), [v4](schemas/model-check-report.v4.schema.json) (in-flight, pools, worlds), [v5](schemas/model-check-report.v5.schema.json) (source-DPOR, connection loss) |
| `midwrite.discovery` | 1 | `discover --format json` (`nextStep`, then `coverage` per candidate, `world` per scenario and world counters in `budgets` were added without a version change) | [v1](schemas/discovery-report.v1.schema.json) (every shape of version 1, [below](#discovery-json)) |
| `midwrite.counterexample` | 1 | `discover --model-check --emit-scenarios`, one per `MODEL_COUNTEREXAMPLE` candidate | [v1](schemas/counterexample.v1.schema.json) |

All schemas are in [`schemas/`](schemas/).

`scripts/validate-schemas.py FILE...` chooses the schema by `kind` and
`schemaVersion`, validates with the `jsonschema` package (or `--builtin`, a
strict subset validator that refuses unknown schema keywords), and checks
that a deliberately corrupted copy is rejected. A `kind`/`schemaVersion`
without a published schema is an error. `--schema FILE` validates every
report against FILE instead (used to show that a report of another version
is rejected by a schema).

Fields added after the last model-check schema file are optional only (`SchemaColumn.generation`, `ExtractedSchema.imports`,
`StmtKind::Insert.returningColumns`, `Op::Unknown.defines`, discovery
`index.generatedColumns`; a value `{"$generated": …}` in models with
generated columns), so the published model-check schemas still apply.
Spot check for this page: the JSON output for
`fixtures/apps/request-app/scenarios/claim-race.json` (`schemaVersion`
3) and `fixtures/model/replicas/replica-read-your-writes.json`
(`schemaVersion` 1), produced by an earlier binary, validates against
the v3 and v1 schemas with `--builtin` (0 errors; corrupted copies
rejected).

## Model-check JSON (outline)

`kind`, `schemaVersion`, `modelVersion`, `tool`, `scenario`, `database`,
`actors`, `bounds` (declared bounds, reduction, strategy), `exploration`
(verdict; metrics: states, transitions, terminal states, `visitedBytes`;
timings unless `--no-timing`), `invariants` (status `violated`,
`holds-within-bounds`, `not-established`), `counterexample` (witness steps
with source references), `evidence` (`level`, `summary`), `runtime`
(PostgreSQL validation), `assumptions`, `diagnostics`, `extraction`,
`schemaFacts`, and when requested `sourceVerification`.

Evidence `level` values (kebab-case): `none`, `model-counterexample`,
`postgres-confirmed-schedule`, `postgres-did-not-reproduce`,
`runtime-inconclusive`, `original-code-reproduced`,
`original-code-crash-reproduced`, `original-code-recovery-reproduced`,
`original-code-invariant-held`, `original-code-in-flight-failure-reproduced`,
`original-code-connection-loss-reproduced`
([evidence levels](../concepts/evidence-levels.md)).

## Discovery JSON

`kind: "midwrite.discovery"`, `schemaVersion` 1, `tool`, `project`,
`mode`, `algorithm`, `executesApplicationCode` (always `false`), `index`
(entries, resources, generated columns, unknown-operation breakdown),
`summary` (candidates by class, status and property origin, model
counterexamples, counterexamples against hypotheses, deduplicated
findings), `budgets`, `candidates`, `rejected`, `entries`, `notes`; with
`--labels`, `evaluation`; without `--no-timing`, `timings`.

JSON Schema:
[`discovery-report.v1.schema.json`](schemas/discovery-report.v1.schema.json)
(draft-07, `$id` `midwrite.discovery.v1`). It is derived from the
serializing Rust types (`crates/midwrite-discover/src/report.rs`,
`candidate.rs`, `synth.rs`, `worlds.rs`, `eval.rs`, `effects.rs`) and is
strict where they are: every object has `additionalProperties: false`,
fields the serializer always writes are required, fields it skips when
empty must be non-empty when present, and the closed vocabularies are
enums:

| field | values |
| --- | --- |
| candidate `status`, `summary.byStatus` keys | `STATIC_CANDIDATE`, `MODEL_COUNTEREXAMPLE`, `NO_VIOLATION_WITHIN_BOUNDS`, `INCONCLUSIVE`, `UNSUPPORTED`, `BLOCKED` |
| property `origin`, invariant `origin`, `summary.byPropertyOrigin` keys | `SCHEMA_ENFORCED`, `SOURCE_ASSERTED`, `USER_DECLARED`, `FRAMEWORK_CONTRACT`, `HEURISTIC_HYPOTHESIS` |
| `class`, `summary.byClass` keys | `check-then-act`, `status-transition`, `read-modify-write`, `write-skew`, `duplicate-processing`, `external-effect-gap`, `ambiguous-commit`, `lock-order`, `resubmitted-write`, `parent-child`, `return-value` |
| assumption `kind` / `origin` | `invocation`, `concurrency`, `delivery`, `retry`, `fault`, `external`, `precondition`, `bounds`, `foreign-key-locks` (older binaries only), `loop`, `caught-error`, `request` / `source-derived`, `declared`, `framework`, `discovery-default`, `model-limitation` (older binaries only) |
| scenario `model.verdict`, world `verdict` | `COUNTEREXAMPLE_FOUND`, `NO_COUNTEREXAMPLE_WITHIN_BOUNDS`, `INCONCLUSIVE`, `UNSUPPORTED`, `NOT_LOADED` |
| scenario `model.evidence` | the model-check evidence levels above |
| `coverage.mode` / world `status` | `single-world`, `relationship-search` / `explored`, `unreached`, `invalid`, `unsupported`, `duplicate`, `not-checked`, `skipped-budget`, `skipped-after-counterexample` |
| `coverage.domainExhaustive`, `executesApplicationCode` | always `false` |

Property `spec` is tagged by `template` (`atMostOne`, `atLeast`,
`nonNegative`, `maxExternal`, `serialCell`, `noFailedActors`,
`returnsArgument`, `noOrphan`, `formula`; template fields keep their Rust
snake_case names, e.g. `group_by`, `per_request`, `parent_column`), value
sources by `kind`, and invariant formulas are structured
(`{"ge": [term, term]}`, terms `{"count": {"table", "where", "column"}}`,
...). Model values are null, booleans, integers, strings, lists or objects
(rows and the `$generated`, `$undefined`, `$map`, `$set` forms); there are
no floating-point values. Free-form text (titles, findings, details,
schedule lines, the debug rendering of an invariant's formula in
`scenarios[].invariants[].formula`, labels copied from `--labels`) is
typed as string only.

**Versions.** Version 1 was extended twice without a version change: first
`nextStep` on every candidate; then `coverage` (with
`--model-check`), `world` (with world search) and the five world counters
of `budgets` (`maxWorldsPerVariant`, `worldsBuilt`, `worldsInvalid`,
`worldsDuplicate`, `worldsSkippedForBudget`). The schema's top-level
`anyOf` accepts exactly these three shapes — without `nextStep`, with
`nextStep`, and with world search — and rejects mixtures (some world counters without the
others, `coverage` or `world` without the world counters, a candidate
without `nextStep` next to world counters). `schemaVersion` must be 1;
`scripts/validate-schemas.py` reports any other version as having no
schema.

The reproduction artifact `<scenario>.counterexample.json`
([`counterexample.v1.schema.json`](schemas/counterexample.v1.schema.json))
is a separate file, not part of the report: the candidate, class, variant,
world, violated property and its origin, the synthesized initial state and
actor arguments, assumptions, bounds, algorithm, schedule, final terms,
FNV-64 digests of the source files, `evidence` (always "model counterexample
(not an original-code reproduction)"), the `midwrite model-check`
command, the `--verify-source --allow-execution` command when the scenario
supports it (else `null`), `status` (always `MODEL_COUNTEREXAMPLE`) and
`finding`.

The text output (`--format text`, the default) is not a stable interface.
After a candidate's next step it prints a declaration skeleton for
a `BLOCKED` candidate (placeholders where only the developer knows the
rule), the contract fields for the inputs of an `INCONCLUSIVE` one, and,
with `--emit-scenarios DIR`, the `midwrite model-check` commands and the
reproduction artifact of a `MODEL_COUNTEREXAMPLE`; repeated unsupported
constructs are listed once with all their locations.

## HTML

`report`, `model-check --format html` and `discover --format html` write a
self-contained page with no scripts: a Content-Security-Policy with
`script-src 'none'`, every string escaped (pinned by tests that poison
every string field of fresh output), timelines for schedules, crashes,
retries, connection losses and external-effect channels.

## Stability

Fingerprints, diagnostic codes (`MW1001`–`MW9001`, `MW7001`–`MW7103`),
baseline files and schemas are kept across releases; compatibility checks
compare the outputs of the previous and the new binary ([evidence index § 7](../research/evidence-index.md#7-compatibility-and-regression-identity)).
