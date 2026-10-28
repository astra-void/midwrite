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
| `commitrift.traces` | 1, 2 | `trace --format json` | [v1](../phase3/schemas/trace-report.v1.schema.json), [v2](../phase5/schemas/trace-report.v2.schema.json) (adds `valueRelation`) |
| `commitrift.verification` | 1, 2 | `replay --format json` | [v1](../phase3/schemas/verification-report.v1.schema.json), [v2](../phase4/schemas/verification-report.v2.schema.json) |
| `commitrift.regression` | 1 | `replay --baseline … --regression-output` | [v1](../phase4/schemas/regression-report.v1.schema.json) |
| `commitrift.model-check` | 1–5 | `model-check --format json` | [v1](../phase6/schemas/model-check-report.v1.schema.json) (Phase 6), [v2](../phase7/schemas/model-check-report.v2.schema.json) (source verification), [v3](../phase8/schemas/model-check-report.v3.schema.json) (crash/recovery, schema facts), [v4](../phase9/schemas/model-check-report.v4.schema.json) (in-flight, pools, worlds), [v5](../phase10/schemas/model-check-report.v5.schema.json) (source-DPOR, connection loss) |
| `commitrift.discovery` | 1 | `discover --format json` (Phase 15 adds `coverage` per candidate, `world` per scenario and world counters in `budgets`; additive) | no published JSON Schema |
| `commitrift.counterexample` | 1 | `discover --emit-scenarios` (Phase 15), one per `MODEL_COUNTEREXAMPLE` candidate | no published JSON Schema |

`scripts/validate-schemas.py FILE...` chooses the schema by `kind` and
`schemaVersion`, validates with the `jsonschema` package (or `--builtin`, a
strict subset validator that refuses unknown schema keywords), and checks
that a deliberately corrupted copy is rejected.

Fields added after the last schema file: Phase 12 added optional fields
only (`SchemaColumn.generation`, `ExtractedSchema.imports`,
`StmtKind::Insert.returningColumns`, `Op::Unknown.defines`, discovery
`index.generatedColumns`; a value `{"$generated": …}` in models with
generated columns) and kept the model-check schemas unchanged
([Phase 12 final report § compatibility](../phase12/FINAL-REPORT.md#compatibility-phase-11-binary-vs-phase-12-same-inputs)).
Phase 13's schema status is not yet recorded in `docs/phase13/`. Spot
check for this page: the JSON output for
`fixtures/phase13/request-app/scenarios/claim-race.json` (`schemaVersion`
3) and `fixtures/phase13/model/replica-read-your-writes.json`
(`schemaVersion` 1), produced by the pre-Phase-14 binary, validates against
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

## Discovery JSON (outline)

`kind: "commitrift.discovery"`, `schemaVersion` 1, `tool`, `project`,
`mode`, `algorithm`, `executesApplicationCode` (always `false`), `index`
(entries, resources, generated columns, unknown-operation breakdown),
`summary` (candidates by class, status and property origin, model
counterexamples, counterexamples against hypotheses, deduplicated
findings), `budgets`, `candidates`, `rejected`, `entries`, `notes`; with
`--labels`, scores.

## HTML

`report`, `model-check --format html` and `discover --format html` write a
self-contained page with no scripts: a Content-Security-Policy with
`script-src 'none'`, every string escaped (pinned by tests that poison
every string field of fresh output), timelines for schedules, crashes,
retries, connection losses and external-effect channels.

## Stability

Fingerprints, diagnostic codes (`CP1001`–`CP9001`, `CP7001`–`CP7103`),
baseline files and schemas are kept across phases; each phase's
compatibility check compares the outputs of the previous and the new
binary ([evidence index § 7](../research/evidence-index.md#7-compatibility-and-regression-identity)).
