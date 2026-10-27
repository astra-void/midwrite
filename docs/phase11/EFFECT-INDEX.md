# Entry and effect index, resource identity

## Entry candidates (`commitrift-analyzer/src/entries.rs`)

For every module (the analyzer's file discovery and import resolution) the
index lists functions a caller outside the module can reach, each as a
source selector:

| kind | selector | example |
| --- | --- | --- |
| `server-action` | `file#name`, `file#name/arg[i]` | exports of a `"use server"` module, callbacks of the call that initializes them |
| `route-handler` | `file#GET` … | `GET`/`POST`/`PUT`/`PATCH`/`DELETE`/`HEAD`/`OPTIONS` of a `route.ts` |
| `exported-function` | `file#name` | any other exported function |
| `wrapped-callback` | `file#name/arg[i]` | a callback passed to the call that initializes an export |
| `class-method` | `file#Class/method[m]`, `/static[m]` | methods of an exported class |
| `object-property` | `file#obj/property[p]` | function-valued properties of an exported object |

`export default`, export lists and re-exports are not listed; they are
recorded as skipped with the reason. An entry candidate is a syntactic
fact: "exported" does not mean "externally callable". The candidate's
assumptions say how concurrency is established (`CANDIDATE-DISCOVERY.md`).

The index also records each module's resolved project imports (packages and
unresolved specifiers separately), whether it calls `pgTable(` (a schema
module), and **transaction helpers**: functions that pass a parameter as the
callback of `<receiver>.transaction(...)`, with the options parameter and
whether the body contains a loop.

An exported function that another analyzed entry inlines (a repository
method such as `setStock`) is a *helper* for pairing: it is not used as a
cross-entry partner, because its effects already appear in the entries that
call it.

## Extraction

Entries whose import closure mentions neither a database receiver nor a
declared external callee (a word match over the files' text, a superset of
what the extractor can lower) are not extracted and are listed as
`not-extracted`. The rest are extracted with the request the scenario
loader would make: the table bindings of a probe scenario that reads every
table of the schema module(s) with `from`, the declared externals, the
transaction helpers (source-derived when they have no loop: one attempt;
declared otherwise), inline depth 3, and the declared contract — or, without
one, an empty contract that makes the extractor report the wrapper's facts
instead of refusing.

## Effect summaries (`commitrift-discover/src/effects.rs`)

A summary restates the extracted code vectors; it adds nothing the model
does not execute.

* **Database effects**, in code order: access (`Read`, `Count`, `Insert`,
  `Update`, `Delete`), table, connection (`pool` autocommit or `tx`), row
  lock, the transaction it runs in, the predicate split into `column =
  value` conjuncts and other conjuncts (with their columns), written
  columns, `ON CONFLICT DO NOTHING`, joined tables, the result local and
  whether a branch reads it, and whether it lies in a `try` block or a
  `catch` handler.
* **Value provenance** (`Val`): `Param{path}` (`data.email`), `Lit`,
  `Read{effect, column}`, `ReadRow`, `ReadCount` (`rows.length`, `count()`,
  `returning()` rows), `ColumnRelative` (an expression over the row being
  written — the database computes it atomically), `Computed`, `Unknown`.
  Locals are traced through `Let` chains, including the helper return slots
  inlining creates (`h1$ret = null; …; h1$ret = h1$row`) and the
  `len = 1; if (row is null) len = 0` lowering of `rows.length`.
* **External effects** with channel and key provenance.
* **Guards**: branches whose condition reads a database result, the columns
  read, and the columns compared with a literal (state tests).
* **Control flow**: per code vector, successor sets (jumps, branches, `try`
  handlers), so "effect B can follow effect A" and "this guard decides
  whether B runs" are computed, not assumed.
* **Transactions** with isolation and retry count, **unknown operations**
  with detail and location, child tasks, shared in-process state.

## Resource identity

Candidates name the logical resource their operations share:

| resource | identity | from |
| --- | --- | --- |
| `unique-key` | table + key columns (+ literal filter) | a check's equality conjuncts matched with the insert's written values |
| `row` | table + the read's equality key | status transitions, lock order |
| `row-predicate` | table + predicate | write skew |
| `column` | `table.column` + row key | read-modify-write |
| `external-channel` | channel name | declared externals |

Two values are the *same* logical value when their provenance is equal
after resolving `Read{e, c}` through the read's own equality (`read#0.id`
with `id = $input.productId` is `$input.productId`). Unknown and computed
values are never the same value. The resource index used for pairing maps
`(table, written column)` to the entries that write it; pairing never
enumerates all pairs of functions.

## Caching (`--cache FILE`)

An entry's extraction is cached under a key that covers everything the
extractor can read: the tool version and cache format, the selector, the
contract, the contents of every file in the entry's import closure,
`tsconfig.json`/`package.json`/`commitrift.json` files of the project, the
table bindings (so the schema), the external and transaction-helper
bindings, and the database receivers. Candidates, scenarios and
verification results are never cached. `discovery.rs` tests that a changed
helper invalidates exactly its importers, that a schema change or a changed
external binding invalidates everything, and that a channel's deduplication
(not read by extraction) invalidates nothing.
