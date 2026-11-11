# `midwrite model-check`: bounded concurrency and failure checking

`model-check` explores bounded schedules of the actors a scenario declares,
against a model of PostgreSQL (isolation level, constraints, row locks,
foreign keys when enabled), with optional crashes, retries, lost
acknowledgments and connection pools, and reports a counterexample to a
declared invariant. `midwrite check` never runs it.

Semantics: [concurrency model](../concepts/concurrency-model.md),
[model semantics](../concepts/model-semantics.md),
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

A scenario is JSON (`schemaVersion` 1); every field added over time is
optional, so older scenarios load and serialize unchanged. Unknown fields
anywhere are refused (`deny_unknown_fields`).

| field | content |
| --- | --- |
| `extraction` | `root`, table mapping, `externals` (callee, channel, `keyArgument`), `maxInlineDepth` (default 3); `schema` module(s) to read tables from Drizzle source (several modules, tables named `module#binding`); `replicaFactories`; `transactionRetries` |
| `database` | `isolation` (`read-committed`, `repeatable-read`, `serializable`), declared `tables` (columns with `type` and `notNull`, primary key, `unique`, `checks`, `foreignKeys`), `initialState` |
| `features` | `foreign-keys` (enforce the source schema's foreign keys), `replicas` (route replica reads), `js-values`, `timestamps`, `json` ([value semantics](../concepts/value-semantics.md)); unknown names are refused |
| `actors[]` | `function` + `file` (positional `arguments`, every parameter given), a source selector `entry.selector` (callbacks, methods, `module#name/arg[1]`), a declared `program` (named `arguments`), or pre-lowered `codes`; `process`; `retry` (`trigger`: `onFailure`, `onCrash`, `onFailureOrCrash`, `atLeastOnce`; `guaranteed`); `response: true` (insert the response step) |
| `channels` | external-effect channels and their declared deduplication (`none`, `byKey`) |
| `app` | in-process variables and their initial values |
| `pools` | bounded connection pools (process → capacity) |
| `bounds` | `maxActors`, `maxSteps`, `maxStates`, `maxCrashes`, `maxRetries`, `maxContextSwitches`, `maxTransitions`, `maxWallMs`, `maxMemoryBytes`, `maxInFlight`, `crashPolicy` |
| `options` | `por`, `strategy`, `minimize`, `dpor`, `sourceDpor`, `termination` (a stuck state violates the implicit invariant `termination`) |
| `invariants[]` | `name`, `when` (`final` / `always`), `description`, `assert` formula |
| `sourceVerification` | harness manifest and per-actor input for `--verify-source` |

Validation refuses, with the path of the field (exit 2): unknown schema
versions and isolation levels; duplicate tables, columns, actors or
invariants; constraints on unknown columns; CHECK expressions that
reference locals; initial rows of unknown tables, with unknown columns,
violating a constraint, or containing generated values; too many actors;
statements on unknown tables or columns; locals read but never assigned;
external effects on undeclared channels; functions that cannot be found or
extracted; missing or extra arguments; invariants referring to unknown
tables, columns or functions; no invariant at all; replica statements
without the `replicas` feature; foreign-key actions the model refuses
([PostgreSQL model](../architecture/postgres-model.md#foreign-keys)).
Missing database state is never invented: tables without rows are empty
and omitted nullable columns are `null`. Assumptions made while loading
(primary-key order of `LIMIT 1` reads, generated values distinct, lock
order of multi-row locks, the replica model) are listed in the report
(`-v`).

**Declared programs** (`dsl.rs`) describe an actor without source; their
operations are labeled `declared` everywhere (report, oracle SQL). One
statement per line, `@column` for columns: `begin [isolation …] [retry n]
… commit`, `let x = select T where … [for update [skip locked | nowait]]`,
`let rows = select all T where … [order by @id] [limit n] [for …]`,
`replica select`, `update`, `insert … [on conflict do nothing] [into n]`,
`delete`, `if … { } else { }`, `fail`, `return`, `external ch [key e] [set
k = e]`, `spawn [detached] { } { }`, `join`, `read_app`/`write_app`,
`generate "…"`, `unknown "…"`.

Example: [`fixtures/concurrency/scenarios/inventory-oversell.json`](../../fixtures/concurrency/scenarios/inventory-oversell.json);
with return values: [`fixtures/apps/request-app/scenarios/claim-race.json`](../../fixtures/apps/request-app/scenarios/claim-race.json);
with a replica: [`fixtures/model/replicas/replica-read-your-writes.json`](../../fixtures/model/replicas/replica-read-your-writes.json).
`midwrite discover --emit-scenarios DIR` writes scenarios in this format.

Invariants are data, never inferred by `model-check`
([invariants](../concepts/invariants.md)).

## Search options

| option | effect |
| --- | --- |
| default | depth-first search with sleep-set partial-order reduction and full state deduplication |
| `--no-por` | explore every interleaving (`--strategy bfs\|dfs`) |
| `--dpor` | classic stateless DPOR; refused with an `always` invariant or a context-switch bound |
| `--source-dpor [stateful-sleep\|stateful\|stateless-sleep]` | source-set DPOR, experimental, opt-in; recommended for large models with only `final` invariants |
| `--max-steps`, `--max-states`, `--max-wall-ms`, `--max-in-flight` | override the scenario's bounds |
| `--fault-mode any\|in-flight` | which crash points are explored |
| `--no-minimize` | report the first counterexample unminimized |
| `--profile-storage` | memory of the visited-state structures |

The reductions are evidence-checked against the unreduced search, not
proved ([partial-order reduction](../architecture/partial-order-reduction.md)).
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
