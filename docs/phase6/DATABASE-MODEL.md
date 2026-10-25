# The abstract database

`crates/commitrift-model/src/db.rs` (store, schema, constraints) and
`exec.rs` (statement semantics). It is a deliberately small multi-version
store, not a SQL engine.

## Schema and constraints

Declared in the scenario, never inferred from application code:

```json
{ "name": "products",
  "columns": [{"name": "id", "notNull": true, "type": "text"}, {"name": "stock", "type": "int"}],
  "primaryKey": ["id"], "unique": [["sku"]], "checks": ["@stock >= 0"] }
```

| Constraint | Model | SQLSTATE |
| --- | --- | --- |
| Primary key | unique + NOT NULL | 23505 / 23502 |
| UNIQUE (single or multi-column; `null` keys never conflict) | checked against the *newest* version of every row, regardless of snapshot | 23505 |
| NOT NULL | checked on every new image | 23502 |
| CHECK (expression over `@columns`; fails only when it evaluates to false) | checked on every new image | 23514 |
| Foreign keys | **not supported** (a scenario cannot declare them) | — |

`type` (`int`, `text`, `bool`) matters only to the PostgreSQL oracle's DDL;
the model is untyped. Initial rows are validated against every constraint
and must not name unknown columns; omitted nullable columns are `null`.

## Rows and versions

A row id is `(table, inserting actor, per-actor counter)` (initial rows:
`(table, initial, index)`). Each row has a chain of versions
`(image | deleted, creator, commit sequence | uncommitted)` in commit order;
at most one version is uncommitted (its creator holds the row lock). A
snapshot `(sequence, own transaction)` sees the newest version that is its
own or committed at or before the sequence.

## Statements

All reads go through *candidate rows*: rows whose visible image matches the
predicate, in primary-key order.

| Statement | Behavior |
| --- | --- |
| `SELECT … LIMIT 1` | first candidate; no locks |
| `SELECT count(*)` | number of candidates |
| `SELECT … FOR UPDATE` | for each candidate in order: wait if another active transaction holds its lock (`SKIP LOCKED`: skip it; `NOWAIT`: 55P03); if a newer committed version exists: READ COMMITTED re-checks the predicate on it (skip or return it), others raise 40001; lock and return the first row that survives |
| `UPDATE … SET … WHERE` | as `FOR UPDATE` for every candidate, then compute the new image from the target version (`SET stock = stock - 1` reads the *target*, which after a re-check is the newest version; `SET stock = <local>` writes the value the application computed), check NOT NULL / CHECK / unique keys (waiting on an in-progress writer of the same key), write a new uncommitted version, lock |
| `DELETE … WHERE` | as `UPDATE`, writing a deletion |
| `INSERT` | NOT NULL / CHECK, then unique keys in declaration order (primary key first): a key written by an in-progress transaction (inserted, or changed/deleted from a row that had it) waits for it; a committed live row with the key raises 23505, or with `ON CONFLICT DO NOTHING` inserts nothing — except under REPEATABLE READ / SERIALIZABLE when that row is not visible to the snapshot, which raises 40001 |

Commit stamps the transaction's versions with a new sequence (only if it
wrote) and releases its locks; abort removes its uncommitted versions and
releases its locks. Statement errors abort the whole transaction.

## Uncertainty is preserved

* An expression the extractor did not understand is `Expr::Unknown`;
  evaluating it is an error, never a value.
* An operation the extractor did not understand (raw SQL, an unsupported
  Drizzle method, triggers, database functions, dynamic SQL) is
  `Op::Unknown`; reaching it makes the result `UNSUPPORTED`.
* Predicates are evaluated exactly; an evaluation error inside a statement
  is a statement error (and, in footprints, a conflict).
* Query plans and their locking side effects are not modeled beyond the
  rules above (see the SSI granularity difference in
  [`POSTGRES-VALIDATION.md`](POSTGRES-VALIDATION.md)).
