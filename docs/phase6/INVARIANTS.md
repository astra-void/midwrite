# Scenarios and invariants

`crates/commitrift-model/src/scenario.rs`, `invariant.rs`. Report schema:
`docs/phase6/schemas/model-check-report.v1.schema.json`.

## Scenario format (version 1)

```json
{
  "schemaVersion": 1,
  "name": "inventory-oversell",
  "description": "Two concurrent purchases of the last unit",
  "extraction": { "root": "../project", "tables": {"products": "products"},
                  "externals": [{"callee": "publishEvent", "channel": "events", "keyArgument": 1}],
                  "maxInlineDepth": 3 },
  "database": {
    "isolation": "read-committed",
    "tables": [ { "name": "products", "columns": [{"name": "id", "notNull": true, "type": "text"}, {"name": "stock", "type": "int"}],
                  "primaryKey": ["id"], "unique": [], "checks": ["@stock >= 0"] } ],
    "initialState": { "products": [ { "id": "product-1", "stock": 1 } ] }
  },
  "channels": { "events": { "dedup": "none" } },
  "app": { "counter": 0 },
  "actors": [
    { "name": "A", "function": "purchase", "file": "src/purchase.ts", "arguments": ["product-1", "order-A"],
      "process": "web-1", "retry": { "trigger": "onCrash", "guaranteed": false }, "description": "one HTTP request" },
    { "name": "B", "function": "purchase", "program": "let p = select products where @id == pid\n…", "arguments": {"pid": "product-1"} }
  ],
  "bounds": { "maxSteps": 40, "maxStates": 100000, "maxCrashes": 0, "maxRetries": 0 },
  "options": { "por": true, "strategy": "dfs", "minimize": true },
  "invariants": [ … ]
}
```

An actor is extracted from TypeScript (`file` + `function`, positional
`arguments` — every parameter must be given), declared (`program` in the
scenario DSL, named `arguments`), or given as pre-lowered `codes`.

### Validation

Rejected with the path of the field (exit status 2): unknown schema
version; unknown isolation level; duplicate tables/columns/actors/invariants;
constraints on unknown columns; CHECK expressions that reference locals or
unknown columns; initial rows of unknown tables, with unknown columns, or
violating a constraint; too many actors; statements on unknown tables or
columns; locals read but never assigned; jump targets out of range;
external effects on undeclared channels; a function that cannot be found or
extracted; missing or extra arguments; invariants referring to unknown
tables, columns or functions; no invariant at all; unknown fields anywhere
(`deny_unknown_fields`). Missing database state is never invented: tables
without `initialState` rows are empty, nullable columns omitted from a row
are `null`.

## Assertion language

An invariant is data: `{ "name", "when": "final" | "always",
"description", "assert": <formula> }`.

Formulas: `{"eq"|"ne"|"lt"|"le"|"gt"|"ge": [term, term]}`, `{"and": [...]}`,
`{"or": [...]}`, `{"not": f}`, `{"implies": [f, g]}`.

Terms:

| Term | Value |
| --- | --- |
| `3`, `"text"`, `null` | literals |
| `{"initial": {"table", "where", "column"}}` | a column of the initial row matching the equality filter |
| `{"row": {"table", "where", "column"}}` | a column of the *committed* row matching the filter (`null` if none) |
| `{"count": {"table", "where"}}` | committed rows matching the filter |
| `{"sum": {"table", "where", "column"}}` | sum of an integer column |
| `{"actors": {"function", "outcome": "ok" \| "failed" \| "crashed"}}` | `ok`: successful completions (a retried operation that succeeds twice counts twice); `failed`/`crashed`: actors whose latest attempt ended so |
| `{"externals": {"channel", "key", "distinctKeys"}}` | performed (not deduplicated) effects, or distinct keys |
| `{"app": "var"}` | an in-process variable (first process) |
| `{"add": [t, t]}`, `{"sub": [t, t]}` | integer arithmetic |

Semantics: the state is the **committed** database (uncommitted versions are
invisible), actor outcomes, the external log and in-process variables.
A comparison involving `null` is false, except `eq`/`ne` against `null`.
`final` invariants are evaluated in quiescent states where every task has
finished; `always` invariants in every reachable state.

Business invariants are never inferred. Examples used in the fixtures:
successful purchases ≤ initial stock; at least one doctor on call; a code
redeemed at most once; an event published at most once; a card charged at
most once; a committed order's event is published or still pending in the
outbox.

## Traceability

Scenario → actor (name, function, file, arguments, process) → operation
(`SourceRef`: file, line, column, function, inlining call chain, source
excerpt, Effect IR effect id, linked existing findings) → transition
(witness step with the events it produced). Declared operations are labeled
`declared` and point at the scenario line.
