# Input domains and world search

Part of [`midwrite discover`](scenario-synthesis.md). Synthesis builds one
initial state and one argument vector per scenario variant (the **base
world**). With `--model-check`, each **actual** variant is then checked in
further initial worlds, each built by the same synthesizer and checked by
the same model checker — no second model checker and no SMT solver. This
page defines the input domains, the worlds, the coverage accounting and
what a result over worlds does and does not claim.

```text
candidate ─► variants (actual / control / hypothetical)
              ├─ base world: synth::build ─► invariants ─► model check
              │        │ solo runs record branch outcomes
              │        ▼
              ├─ pending worlds (actual variants only):
              │     domain worlds        typed path domains → value classes
              │     actor worlds         a non-key argument differs between actors
              │     relationship worlds  stored value = / ≠ written value
              │     boundary worlds      stored value at a guard's integer operand
              │     ordered by priority; feedback moves worlds over one-way branches ahead
              └─ loop (≤ --max-worlds attempts per variant):
                   synth::build_world(spec) ─► fingerprint (dedup) ─► invariants ─► model check
                   stop at the first validated counterexample (unless --all-worlds)
              ▼
coverage (four dimensions) ─► status ─► finding text ─► reproduction artifact
```

Code: `crates/midwrite-discover/src/domains.rs` (domains), `worlds.rs`
(world specs, generators, fingerprints), `synth.rs` (`build_world`,
`satisfy_guard`, `argument_guard`), `pipeline.rs` (the world loop,
coverage, status); the annotation reader is in
`crates/midwrite-analyzer/src/concurrency/js_values.rs`. Tests:
`crates/midwrite-discover/tests/input_domains.rs`.

## Typed input domains

For every parameter path an entry reads (`ownerId`, `input.parentId`,
`actor.kind`) discovery builds a `PathDomain`:

| field | source |
| --- | --- |
| `kind` | `null`, `undefined`, `boolean`, `integer`, `decimal`, `string`, `uuid`, `generated-identifier`, `timestamp`, `date`, `json`, `array`, `object`, `enum`, `database-row-reference` or `unknown`: from the extractor's typing, refined by the column the path meets (uuid, time, JSON, numeric, enum, foreign key) and by a string-literal union annotation |
| `key` | the path is compared with or written to a key column |
| `null`, `undefined` | `permitted`, `excluded` or `unknown` (below) |
| `provenance` | every fact used, in words |
| `classes` | the value classes the code makes relevant |

Values are the model's own `Value`s. Generated identifiers stay generated
values: a domain never assigns a concrete text to one, and a key a run
generates cannot be required of the initial state.

**Nullability comes only from the source contract.** `null` is permitted
only when the parameter's annotation is a union containing `null`;
`undefined` when the union contains `undefined`/`void` or the parameter or
member is optional (`x?: T`, a default value). An annotation without them
excludes them. `any`, `unknown` or no annotation: unknown — no `null` is
generated, and coverage lists the path as unexplored. A value declared in
the contracts (`arguments`, `values`) is fixed and gets no classes.

**Value classes:** `null` / `undefined` (when permitted);
`equal-to-operand` (the code compares the path with a literal);
`below-operand` / `above-operand` (ordered against an integer: `qty > 10`
gives 9, 10, 11); `negated` (a boolean tested against a literal). A class
equal to the synthesized value is dropped.

**String-literal unions.** An annotation resolving to a union of string
literals (same module, relative imports, `export *` and `export { X } from`
re-exports, depth-limited) is the domain of the path: base arguments take
its first member, "different value" choices another member; a one-member
union cannot vary.

**Correlated values.** A class applies to every actor that reads the path;
actors with the same synthesized value get the same new value.
Relationships between actors are varied separately, by the variants and by
actor worlds.

## Guards and stated preconditions

When every read on an actor's path found a row and the actor still stopped
before the candidate's operations, a guard stopped it. Two repairs, each
tried once per (row or path, value):

* **stored row**: a branch compares a property of a row the actor read
  (`membership.role !== "admin"`, `org.vacancies > 0`) with a literal or an
  integer; the stored column is set to a passing value. Identity columns
  (primary, unique, foreign keys) are never changed.
* **argument**: a branch compares an argument path — followed through
  inlined callees' locals and condition locals — with a literal, or tests
  membership (`roles.includes("admin")`); the argument is set for every
  actor of that entry. Declared values and paths a world chose are never
  changed.

Each repair is recorded as `SYNTHESIZED precondition of <actor>: … (the
scenario explores callers that satisfy this guard; it is not assumed of
every caller)`. Authentication is never bypassed: the code still runs every
guard; the scenario only chooses a caller and rows for which it passes.
When no repair applies, the world is unsatisfiable and the message names
where the actor stopped and with what failure.

## Worlds

A world (`WorldSpec`) has **argument overrides**, applied before rows are
synthesized (rows then follow the arguments: a `null` parent needs no
parent row), and **cell overrides**, applied to the rows the actors need
after their first solo run. Generators:

| world | rule |
| --- | --- |
| domain | every value class of every parameter domain, applied to all actors that read the path |
| actor | a non-key argument the actors share differs — only for independent requests (check-then-act, read-modify-write, status transition without a retry or delivery contract), and never an argument that takes part in a read predicate |
| relationship | for a column an actor writes from an argument: if the stored value equals the written one, a world where it differs (`stored-distinct:<table>.<column>`), else one where it is equal (`stored-equal:…`); keys never change |
| boundary | a guard compares a read row's column with an integer: stored operand − 1, operand, operand + 1 (`stored-boundary:…`), except values a declared `nonNegative` excludes |

Order (deterministic: priority, then name): `stored-distinct` of a column
the actor also reads back (10), `stored-boundary` (15), `null` (20),
`undefined` (22), actor worlds (26), operands (30–34), other
`stored-distinct` (40), `stored-equal` (50). After each built world the
solo runs' branch outcomes are merged; a domain world over a parameter that
a branch has so far taken one way only moves ahead by 10 (its `feedback`
says so). Code coverage is never a goal by itself, and only solo-run branch
outcomes are observed. Argument combinations are not enumerated as a
Cartesian product: each world changes one relationship. Controls and
hypothetical variants keep one world. `--max-worlds 1` turns the search
off.

`fingerprint` hashes everything the model checker is given (the scenario
without name and description). Equal fingerprints are explored once
(`duplicate`); worlds whose values differ are never merged.

| world outcome | meaning |
| --- | --- |
| `explored` | built, loaded, model-checked |
| `unreached` | valid inputs, but an actor stops before the candidate's operations: a valid state where the candidate does not occur |
| `invalid` | no valid initial state found for this world within the bounds — never a proof that none exists |
| `unsupported` | an actor reaches an operation the model does not support |
| `duplicate` | same fingerprint as an explored world |
| `skipped-budget`, `skipped-after-counterexample` | not attempted |

## Coverage and what a verdict claims

Status codes keep their meaning; every model-checked candidate gets a
**coverage** section (`--explain-coverage` prints every world, gap and loop
bound) in four dimensions kept apart:

* **source semantics**: unknown operations in the actors' entries and
  whether one is on the candidate's path (an unsupported operation the
  actors reach still makes the candidate `UNSUPPORTED`);
* **initial states** and **actor inputs**: the number of distinct row sets
  and argument vectors among the explored worlds of the actual
  configuration, and the relationships they establish;
* **schedules**: per explored world, whether the schedule search
  completed, was truncated or reached unsupported behavior — complete for
  that world only;
* **branch outcomes**: two per branch in the actors' extracted code;
  reached = directions an actor took running alone on an explored world's
  rows. Counted, never turned into a percentage.

`domainExhaustive` is always `false` for synthesized inputs: value domains
are not enumerated, only relationship classes. `classesExhaustive` is true
only when world search was on and every derived world was explored or a
duplicate. A `NO_VIOLATION_WITHIN_BOUNDS` finding always states the number
of worlds, distinct row sets and argument vectors, that schedules completed
within the bounds for those worlds, that the input domain was not
exhaustively explored, and that general application safety is not
established. A counterexample outside the base world is reported as "In the
initial world `<id>` (<relationships>): MODEL_COUNTEREXAMPLE …". With
`--max-worlds 1` the mode is `single-world`.

## Budgets

| budget | flag | reported |
| --- | --- | --- |
| worlds per actual variant (base included) | `--max-worlds N` (default 8) | `budgets.maxWorldsPerVariant`, `worldsSkippedForBudget`, per-world `skipped-budget` |
| states per world | `--max-states` | per scenario `complete`; `schedules.worldsTruncated` |
| states over all scenarios | `--max-total-states` | `scenariosSkippedForBudget` |
| wall time, memory per world | `--max-wall-ms`, `--max-memory-bytes` | as above |
| exploring after a counterexample | `--all-worlds` | `skipped-after-counterexample` otherwise |

## Reproduction artifact

With `--emit-scenarios DIR`, a validated counterexample of a
`MODEL_COUNTEREXAMPLE` candidate gets `<scenario>.counterexample.json`
([schema](../reference/schemas/counterexample.v1.schema.json)): candidate,
class, variant, world and its relationships, property and origin, status
and finding, initial rows, actor arguments, assumptions, bounds, algorithm,
schedule, final terms, a source fingerprint (FNV-64 of the entry and schema
modules, not their contents), the evidence level ("model counterexample
(not an original-code reproduction)") and the commands to replay it in the
model and, with a harness, on the original code. Every value is
synthesized; nothing is read from a database or the environment.
