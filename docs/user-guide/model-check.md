# `midwrite model-check`: bounded concurrency and failure checking

`model-check` explores bounded schedules of the actors a scenario declares,
against a model of PostgreSQL (isolation level, constraints, row locks,
foreign keys when enabled), with optional crashes, retries, lost
acknowledgments and connection pools, and reports a counterexample to a
declared invariant. `midwrite check` never runs it.

Semantics: [concurrency model](../concepts/concurrency-model.md),
[PostgreSQL model](../architecture/postgres-model.md). Flags:
[CLI reference](../reference/cli.md#model-check).

## Typical use

```bash
midwrite model-check scenario.json                         # text report; exit 1 on a counterexample
midwrite model-check scenario.json --format json -o r.json
midwrite model-check scenario.json --format html -o r.html # script-free HTML timeline
midwrite model-check scenario.json -v                      # also every modeling assumption
midwrite model-check scenario.json --no-timing             # byte-reproducible output
midwrite model-check scenario.json --validate-postgres     # replay the model's SQL on a disposable PostgreSQL
midwrite model-check scenario.json --verify-source --allow-execution --source-repeat 3   # original code
```

## Verdicts and exit status

| verdict | exit | meaning |
| --- | --- | --- |
| `COUNTEREXAMPLE_FOUND` | 1 | a schedule within the bounds violates an invariant (`MW7001`) |
| `NO_COUNTEREXAMPLE_WITHIN_BOUNDS` | 0 | no violation in any explored behavior within the declared bounds — not a proof of race freedom |
| `INCONCLUSIVE` | 3 | a resource bound ended the search first (`MW7002`) |
| `UNSUPPORTED` | 3 | a reachable operation is not modeled (`MW7003`) |

Invalid scenarios are refused with the path of the offending field (exit
2). Validation and verification add `MW7101` (PostgreSQL did not
reproduce), `MW7102` (validation unavailable) and `MW7103` (the original
code did not reproduce). `midwrite explain` lists MW7001–MW7102;
`midwrite explain MW7103` also works.

## Scenario files

A scenario is JSON (`schemaVersion` 1). The format is defined in
[Phase 6 § scenarios and invariants](../phase6/INVARIANTS.md); later phases
added optional fields only.

| field | content | since |
| --- | --- | --- |
| `extraction` | `root`, table mapping, `externals`, `maxInlineDepth`; `schema` module(s) to read tables from Drizzle source (several modules, tables named `module#binding`); `transactionRetries` | 6; schema from source 8; several modules 12 |
| `database` | `isolation` (`read-committed`, `repeatable-read`, `serializable`), declared `tables` (columns, primary key, unique, checks, `foreignKeys`), `initialState` | 6; foreign keys 13 |
| `features` | `foreign-keys` (enforce the source schema's foreign keys), `replicas` (route replica reads); unknown names are refused | 13 |
| `actors[]` | `function` + `file`, a source selector `entry.selector` (callbacks, methods, `module#name/arg[1]`), or a declared `program`; `arguments`; `process`; `retry`; `response: true` (insert the response step) | 6; selectors 7; response 13 |
| `channels` | external-effect channels and their declared deduplication | 6 |
| `app` | in-process variables and their initial values | 6 |
| `pools` | bounded connection pools (name → capacity) | 9 |
| `bounds` | `maxActors`, `maxSteps`, `maxStates`, `maxCrashes`, `maxRetries`, `maxContextSwitches`, `maxTransitions`, `maxWallMs`, a memory bound; `maxInFlight` and `crashPolicy` | 6; in-flight 9 |
| `options` | `por`, `strategy`, `minimize`, `dpor`, `sourceDpor` | 6; DPOR 9–10 |
| `invariants[]` | `name`, `when` (`final` / `always`), `assert` formula | 6; `returned` terms 13 |
| `sourceVerification` | harness manifest and per-actor input for `--verify-source` | 7 |

Example: [`fixtures/concurrency/scenarios/inventory-oversell.json`](../../fixtures/concurrency/scenarios/inventory-oversell.json);
with return values: [`fixtures/phase13/request-app/scenarios/claim-race.json`](../../fixtures/phase13/request-app/scenarios/claim-race.json);
with a replica: [`fixtures/phase13/model/replica-read-your-writes.json`](../../fixtures/phase13/model/replica-read-your-writes.json).
`midwrite discover --emit-scenarios DIR` writes scenarios in this format.

Invariants are data, never inferred by `model-check`
([invariants](../concepts/invariants.md)).

## Search options

| option | effect |
| --- | --- |
| default | depth-first search with sleep-set partial-order reduction and full state deduplication |
| `--no-por` | explore every interleaving (`--strategy bfs\|dfs`) |
| `--dpor` | classic stateless DPOR (Phase 9); refused with an `always` invariant or a context-switch bound |
| `--source-dpor [stateful-sleep\|stateful\|stateless-sleep]` | source-set DPOR (Phase 10), experimental, opt-in |
| `--max-steps`, `--max-states`, `--max-wall-ms`, `--max-in-flight` | override the scenario's bounds |
| `--fault-mode any\|in-flight` | which crash points are explored |
| `--no-minimize` | report the first counterexample unminimized |
| `--profile-storage` | memory of the visited-state structures |

The reductions are evidence-checked against the unreduced search, not
proved ([model checker](../architecture/model-checker.md#reductions)).
Sleep sets are the default.

## Reading a counterexample

The text report shows the initial state, the minimized schedule (each step
with its actor, statement, result and source location), the final state,
the invariant evaluated on it, the evidence level, and — for extracted
actors — what was represented, assumed or unsupported. A real excerpt is in
[first verification](../getting-started/first-verification.md#1-model-check-a-declared-scenario).
A counterexample is a possible execution **of the model** under the
scenario's assumptions; [evidence levels](../concepts/evidence-levels.md)
say what each further step adds.
