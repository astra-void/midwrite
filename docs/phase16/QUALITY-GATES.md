# Phase 16 quality gates: discovery report schema and CLI reliability

Track G. This note covers the schema repair and the `discover` text
output. The gate results of the whole phase, on the integrated tree, are
in [§ 10](#10-final-gate-results).

## 1. The failing gate

The Phase 13–15 evidence and Linux scripts
(`scripts/phase13-linux-docker.sh:60`, `scripts/phase14-linux-docker.sh:61`,
`scripts/phase15-linux-docker.sh:63`) run

```sh
python3 scripts/validate-schemas.py --builtin reports/phase13/evidence/*.json
```

At 1efa383 it exits 1 on macOS and Linux:

```
reports/phase13/evidence/workspace-app-discovery.json: no schema for kind/version ('commitrift.discovery', 1)
model-check-report.v3 (builtin validator): ... 0 schema errors; corrupted copy rejected: True   (x9)
model-check-report.v1 (builtin validator): replica-read-your-writes.json: ... 0 schema errors ...
exit 1
```

The report carries `"kind": "commitrift.discovery"`, `"schemaVersion": 1`,
and no schema for that kind had ever been published
(`docs/reference/output-formats.md` said so). The other ten files pass.

After this track the same command exits 0, the discovery report included:

```
discovery-report.v1 (builtin validator): workspace-app-discovery.json: 19 candidates, 0 schema errors; corrupted copy rejected: True
```

## 2. What was published

| file | `$id` | covers |
| --- | --- | --- |
| `docs/phase16/schemas/discovery-report.v1.schema.json` | `commitrift.discovery.v1` | `commitrift discover --format json`, every shape of `schemaVersion` 1 |
| `docs/phase16/schemas/counterexample.v1.schema.json` | `commitrift.counterexample.v1` | the `<scenario>.counterexample.json` reproduction artifact of `discover --model-check --emit-scenarios` |

Both are draft-07 (the draft of the nine existing CommitRift schemas) and
are registered in `scripts/validate-schemas.py` (`SCHEMAS`), the central
validation every evidence script and CI's schema step use.

### Derivation

The schema follows the serializing types, field by field:
`report.rs` (`DiscoveryReport`, `ProjectInfo`, `IndexStats`, `Summary`,
`Budgets`, `Timings`, `CandidateReport`, `ScenarioResult`, `ModelResult`,
`Validation`, `InvariantReport`, `WorldInfo`, `WorldOutcome`, `Coverage`
and its four dimension types, `EntryReport`), `candidate.rs` (`Candidate`
flattened into the candidate, `Class`, `Origin`, `Strength`, `Spec`,
`Property`, `Assumption`, `EntryRef`, `ConflictOp`, `Resource`,
`Relation`, `Environment`, `Rejected`), `synth.rs` (`Variant`,
`VariantKind`, `ActorPlan`, `SynthesisReport`), `worlds.rs`
(`WorldOrigin`), `eval.rs` (`Evaluation`, `CaseResult`, `Confusion`),
`project.rs` (`HelperNote`), `effects.rs` (`Val`), and from
`commitrift-model`: `value::Value` (untagged) and `invariant::Formula`,
`Term`, `RowRef`, `ActorsRef`, `ExternalsRef`.

Rules applied:

* `additionalProperties: false` on every object the serializer builds from
  a struct (it can emit no other key); maps (`entriesByKind`, `byStatus`,
  `rows`, `filter`, initial-state tables, rows) are typed by their values
  and, where the keys are a closed vocabulary (`byClass`, `byStatus`,
  `byPropertyOrigin`, `entriesByKind`), by `propertyNames`.
* Always-serialized fields are `required`. Fields with
  `skip_serializing_if = "Vec::is_empty"` / `"String::is_empty"` /
  `"BTreeMap::is_empty"` are optional and, when present, `minItems: 1` /
  `minLength: 1` / `minProperties: 1`; `skip_serializing_if = "Not::not"`
  booleans (`response`, `per_request`) are `const: true` when present;
  `Option` fields without `skip` (`RowRef.column`, `ActorsRef.function`,
  `reproduceOnOriginalCode`) allow `null`.
* Closed string vocabularies are enums. Rust enums give them directly
  (`Class`, `Origin`, `Strength`, `Relation`, `VariantKind`, `WorldOrigin`,
  `EntryKind`, `EvidenceLevel`). For fields typed `String` the values were
  read from every assignment site: candidate status (`pipeline.rs`
  `status_of`), world outcome status (`outcome_of`, the budget and
  after-counterexample skips), model verdict (`explore::Verdict` plus
  discovery's own `NOT_LOADED`), synthesis status, entry status and
  invocation (`detect.rs` `invocation()`; empty for unextracted entries),
  conflict roles, resource kinds, assumption kinds and origins,
  environment retry (`on-crash`, `on-failure`, `on-failure-or-crash` from
  contracts, `at-least-once` from delivery), coverage mode, the
  model algorithm (`sleep-sets`, `source-dpor:<mode>`, empty when the
  scenario does not load) and the top-level algorithm sentence.
* Invariants the code guarantees and the report promises:
  `executesApplicationCode` and `coverage.domainExhaustive` are `const:
  false`; `tool.name` is `commitrift`; candidate ids match
  `^CR-SYNTH-[0-9A-F]{6}$`, fingerprints and groups `^[0-9a-f]{16}$`;
  `branchOutcomes.relevantOutcomes` is a multiple of 2 (two per branch);
  counterexample `evidence` is the fixed "model counterexample (not an
  original-code reproduction)" and its `status` is `MODEL_COUNTEREXAMPLE`.
* Tagged unions use `oneOf` with a `const` tag (`Spec` by `template`, `Val`
  by `kind`); `Term` and `Formula` (externally tagged) are objects with
  exactly one property from a closed set (`minProperties`/`maxProperties`
  1). `Value` is recursive: null, boolean, integer, string, array of
  values or object of values — that is exactly the set of its JSON forms
  (rows and `{"$generated", "actor", "n"}`, `{"$undefined": true}`,
  `{"$map": [[k, v]]}`, `{"$set": [...]}` are all objects of values). There
  is no floating-point model value, so a float is rejected.

Free-form parts (documented in the schema's `description`s): every text
field (titles, findings, details, notes, schedule and final-term lines,
`ranking` factors, `inconclusiveIf`, helper status), the formula of
`scenarios[].invariants[]`, which is the Rust `Debug` rendering of the
formula (a string, not a structure), and `evaluation.cases[].label`,
copied from the user's `--labels` file. No `serde_json::Value` reaches
the report: the only one in these types, `ScenarioResult.reproduction`, is
`#[serde(skip)]` and is written as the separate counterexample artifact,
which has its own schema (built by `serde_json::json!` in
`pipeline.rs::reproduction`, so its schema was derived from that macro
call and the `status`/`finding` the caller adds).

### Versions

`schemaVersion` has only ever been 1, but its content grew without a
version change:

| producers | shape |
| --- | --- |
| Phase 11–13 | no `nextStep`, no `coverage`, no `world`, no world counters; Phase 12 alone states the assumption `foreign-key-locks` / `model-limitation` (retired in Phase 13, `pipeline.rs:493`) |
| Phase 14 | every candidate has `nextStep` (always serialized) |
| Phase 15 and later | `budgets` always has `maxWorldsPerVariant`, `worldsBuilt`, `worldsInvalid`, `worldsDuplicate`, `worldsSkippedForBudget`; `coverage` with `--model-check`; `world` with world search |

The report shape is not changed. The schema keeps one structural
definition and a top-level `anyOf` of three generation constraints, so a
report must be one of the three shapes as a whole: a world counter without
the other four, `coverage` or `world` without the counters, or a missing
`nextStep` next to the counters is rejected. `schemaVersion` is `const: 1`
and `kind` `const: "commitrift.discovery"`; any other version is rejected
by the schema and reported by `validate-schemas.py` as having no schema.
A future change that is not additive must bump `schemaVersion` and add a
`discovery-report.v2.schema.json`.

## 3. Validator changes (`scripts/validate-schemas.py`)

* The two schemas are registered by `kind`/`schemaVersion`.
* The built-in validator (`--builtin`, used by every Linux docker script
  and whenever the `jsonschema` package is absent) implements the extra
  draft-07 keywords the discovery schema needs: `anyOf`, `oneOf`, `not`,
  `propertyNames`, `minProperties`, `maxProperties`, `minLength`,
  `maximum`, `multipleOf`. It still refuses any keyword it does not
  implement, and now also refuses a `$ref` into another schema (the
  verification schemas reference `commitrift.traces.v1#/...`; before, the
  `oneOf` they also use was the keyword it refused). `const`/`enum` now
  compare as JSON (`true` is not `1`).
* Regression check of the validator itself: the 1efa383 script and this
  one were run with `--builtin` on each of the 404 committed files under
  `reports/` and `fixtures/` whose `kind` is a model-check, traces,
  verification or regression report. Every file that passed before passes
  (374); the three Phase 4 regression reports, refused before because of
  `oneOf`, now validate (377 pass); the 21 verification reports are still
  refused, now on the cross-schema `$ref` (they need `jsonschema`, as
  before); the remaining failures are identical before and after.
* `--schema FILE` validates every report against FILE (to show that a
  report of an unsupported version is rejected by the schema itself).
* The count printed for a discovery report is its candidates.

The `jsonschema` package is not installed on this machine (`python3 -c
"import jsonschema"` fails, Python 3.14.7), and no JSON Schema crate is in
`Cargo.lock`; nothing was added. The Rust tests therefore always run the
built-in validator, and additionally `jsonschema` when it is importable
(as in CI's schema step, which installs it). The `jsonschema` path of
these two schemas was not executed here.

## 4. Tests

`crates/commitrift-cli/tests/phase16_schema.rs` (shells out to
`python3 scripts/validate-schemas.py`; skips with a message without
`python3`, fails instead with `COMMITRIFT_REQUIRE_SCHEMA=1`):

| test | fixtures | result |
| --- | --- | --- |
| `committed_discovery_reports_validate` | the 7 committed discovery reports (`reports/phase11/evidence/{commerce-app,heldout-shop-app,heldout-team-app}.json`, `reports/phase12/evidence/workspace-app.json` and its two Linux copies, `reports/phase13/evidence/workspace-app-discovery.json`) | 0 errors each, corrupted copies rejected |
| `phase13_evidence_schema_check_passes` | the formerly failing command (11 files) | exit 0 |
| `fresh_discovery_reports_of_first_party_fixtures_validate` | 12 reports generated by the test binary: `phase15/worlds-app` (static, `--model-check`, `--max-worlds 1`, `--source-dpor`), `phase15/eval-app` (static, model-checked), `phase14/eval-app`, `phase14/values-app`, `phase13/eval-app`, `phase12/workspace-app` (model-checked), `phase11/commerce-app` (static; model-checked with `--labels`) — all six candidate statuses, coverage, worlds and `evaluation` occur | 0 errors each |
| `counterexample_artifacts_validate` | every `*.counterexample.json` those runs emit (50 on this machine) | 0 errors each |
| `earlier_shapes_of_schema_version_1_are_accepted` | the Phase 14 and Phase 11–13 shapes derived from a current static report (no Phase 14 binary output is committed) | 0 errors |
| `negative_fixtures_are_rejected` | `fixtures/phase16/schema/negative-cases.json`: 36 single mutations of real reports (32 discovery: missing required fields, invalid enums, wrong types, malformed coverage, mixed generations, unsupported versions 0 and 2, extra fields; 4 counterexample) | every case rejected, each with exactly the intended error under `--builtin`; unsupported versions also reported as "no schema" without `--schema` |
| `text_output_names_declarations_and_reproduction_files` | `phase12/workspace-app --model-check --emit-scenarios` | see § 5 |

Unit tests in `report.rs`: grouping of unsupported constructs and shell
quoting of reproduction paths.

## 5. `discover` text output

Reviewed on `fixtures/phase12/workspace-app` (BLOCKED, INCONCLUSIVE,
MODEL_COUNTEREXAMPLE, NO_VIOLATION_WITHIN_BOUNDS) and
`fixtures/phase15/eval-app` (UNSUPPORTED, INCONCLUSIVE). Changes are in
the text (and, for the next step, HTML) rendering only; `--format json`
is byte-for-byte unchanged, every existing line stays as it was except the
two below, and no line asserted by an existing test changed.

* **UNSUPPORTED**: an `Unsupported` list repeated the same construct once
  per location (four copies of "row locking is only supported with
  `.limit(1)` …" on `eval-app`); identical messages are now listed once
  with all their locations in order. Distinct messages render exactly as
  before.
* **INCONCLUSIVE**: the Phase 14 next step asks to declare "an
  `initialState` for the entry", a contract field that does not exist
  (`EntryContract` has `deny_unknown_fields` and no such field; initial
  rows are always synthesized). At integration the lead corrected the
  JSON `nextStep` itself to name the fields that exist (a deliberate JSON
  text change; discovery JSON is not part of the byte-identity gates), so
  text, HTML and JSON agree; a `Declare inputs:` line shows the exact
  `"entries": {"<selector>": {"values": {...}}}` skeleton for the
  candidate's entries.
* **BLOCKED**: a `Declare a property:` line gives a contract skeleton with
  the candidate's entry selectors and table (or channel) filled in and
  placeholders for what only the developer knows, names every template,
  and says to state only a rule the application requires. Evidence: the
  skeleton for `CR-SYNTH-E1AA0A` (`invites` write skew), filled with
  `"key": ["email"]` in a copy of the fixture, makes the candidate
  model-checked (`NO_VIOLATION_WITHIN_BOUNDS` against the `USER_DECLARED`
  property) instead of `BLOCKED`.
* **MODEL_COUNTEREXAMPLE** with `--emit-scenarios DIR`: the deciding
  scenario's `commitrift model-check DIR/<file>` command, the reproduction
  artifact path (when the counterexample was validated, which is when the
  pipeline writes it) and, when the scenario supports original-code
  verification, the `--verify-source --allow-execution` command, labeled
  as executing application code. The test checks that every named file
  exists and that two runs print identical text.

## 6. Clippy on Linux

`needless_borrows_for_generic_args` at
`crates/commitrift-model/src/engine.rs` is already fixed at 1efa383
(`u.filter.as_ref().map(close)`, line 1181, commit 07f6559). The only
toolchain here is 1.98.1 (`rustup run stable` and Homebrew `cargo` are
the same version, clippy 0.1.98); no newer toolchain is installed, so the
Linux lint could not be re-run with a newer clippy. `cargo clippy
--workspace --all-targets -- -D warnings` is clean with 1.98.1.

## 7. For the lead

* (Resolved at integration.) The JSON `nextStep` of INCONCLUSIVE
  candidates (`report.rs::next_step`) named the non-existent contract
  field `initialState`; it now names `entries` / `values` / `arguments`
  and the text-only override was removed.
* CI (`.github/workflows/ci.yml`, not owned by this track) could validate
  a fresh discovery report with the real `jsonschema` package in its
  schema step, e.g. `./target/release/commitrift discover
  fixtures/phase15/worlds-app --model-check --format json --output
  discovery.json` added to the `validate-schemas.py` invocation.

## 8. Evidence (macOS, rustc 1.98.1, Python 3.14.7, Node 22.23.2)

* `python3 scripts/validate-schemas.py --builtin reports/phase13/evidence/*.json`:
  exit 1 at 1efa383, exit 0 now (11 files, 0 errors, corrupted copies
  rejected).
* `COMMITRIFT_REQUIRE_SCHEMA=1 cargo test -p commitrift-cli --test phase16_schema`:
  7 passed.
* `COMMITRIFT_REQUIRE_REPLAY=1 COMMITRIFT_REQUIRE_SCHEMA=1 cargo test
  --no-fail-fast -p commitrift-cli -p commitrift-discover -p
  commitrift-analyzer` (fixture `node_modules`/`dist` copied from the main
  checkout, since a fresh worktree has none): 408 passed, 1 failed,
  1 ignored. The failure is
  `replay.rs::the_node_runtime_protocol_client_passes_its_own_tests`
  (`node --test runtime/node/test/runtime.test.js`, subtest 5 "unknown
  scenarios and prototype keys are refused": `TypeError: Cannot read
  properties of undefined`), a JavaScript test of `runtime/node`, which
  this track does not touch.
* `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings`: clean.
* `python3 scripts/check-doc-links.py`: 0 broken links.

## 9. Limitations

* (Resolved at integration.) The `jsonschema`-package path was not run
  by this track. At integration the real `jsonschema` package validated
  the Phase 16 fixture reports, the alpha run and the midday and
  school-ledger reports: 0 errors, corrupted copies rejected
  (§ 10).
* Text fields are typed only as strings; the debug-rendered invariant
  formula and the `--labels` label are not structured.
* The Phase 14 shape is accepted by construction and by a derived report,
  not by output of a Phase 14 binary (none is committed).
* Enum vocabularies of `String` fields were read from the current code;
  a producer that adds a value must extend the schema (the fresh-report
  test fails until it does).

## 10. Final gate results

Both platforms ran on the same source: digest
`93520c3f8efd7faeec4a68f064bbbce4262629ece9f41bd74c9961235d34a27a`
(`scripts/phase8-digest.py`, 1 038 files). Raw output:
`reports/phase16/final/` (macOS) and `reports/phase16/linux/` (Linux,
`scripts/phase16-linux-docker.sh`).

| gate | macOS arm64 | Linux aarch64 |
| --- | --- | --- |
| environment | macOS 27.0.1, rustc 1.98.1, Node 22.23.2, PostgreSQL 17.11 | Debian 12 (Docker, linuxkit 7.0.12), rustc **1.99.1**, Node 22.23.4, PostgreSQL 15.19, bubblewrap 0.8.0 |
| `cargo fmt --all -- --check` | exit 0 | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 | **exit 0** (1.99.1, the toolchain whose lint failed at the Phase 15 freeze; no `allow` added) |
| `COMMITRIFT_REQUIRE_REPLAY=1 COMMITRIFT_REQUIRE_SCHEMA=1 COMMITRIFT_REQUIRE_ZOD=1 cargo test --workspace --no-fail-fast` | **843 passed, 0 failed, 3 ignored** (83 targets) | **843 passed, 0 failed, 3 ignored** (83 targets) |
| `cargo build --release` | exit 0, sha256 `0d21c255…` | exit 0, sha256 `0e192a83…` (different platform) |
| Phase 16 evidence (`scripts/phase16-evidence.py --repeat 3`) | 197/197 POR agreement; 49/54 confirmed, 264 outcomes, 0 disagreements; reproductions 3/3 | identical on PostgreSQL 15.19 |
| Phase 12–15 evidence, Phase 13 lock matrix | — (Phase 15 record) | exit 0 each |
| report schemas | Phase 13 evidence: exit 0; discovery reports (built-in and `jsonschema`): 0 errors | Phase 12/13 evidence and a fresh discovery report: exit 0 |
| `scripts/check-doc-links.py` | 0 broken links | — |

The 3 ignored tests are tools (`regenerate_fixtures`, `dump_seed`,
`dump_ir`). The required-runtime switches turn every would-be skip into a
failure, so 0 failed means that the PostgreSQL replays, the original-code
runs, the zod differential and the schema tests all ran. On Linux, the
lockstep matrices for NULLS NOT DISTINCT and multi-row locks therefore
also ran against PostgreSQL 15.19 (on macOS against 17.11).

### Linux

* The source digest in the container equals the macOS digest.
* The container installs the pinned zod 3.25.76 and 4.6.5 with npm and
  points `COMMITRIFT_ZOD3_DIR`/`COMMITRIFT_ZOD4_DIR` at them.
* No platform-specific failures remain.

During integration, one test race (two tests writing one per-process temp
file in `phase16_genid.rs`) failed once on macOS. It was fixed by naming
the file per test, and did not recur in three re-runs or in either final
run.

The Node runtime test failure noted in § 8, which was seen only in this
track's worktree, does not occur in the integrated tree on either
platform.

The release sha256 of the gate build (`cargo build --release`, the whole
workspace) differs from the `-p commitrift-cli` build the walkthrough and
evidence scripts used (`9b97bb94…`). Both come from the same digest; the
difference is feature unification across the workspace.

