# Value semantics in the model

How the model checker represents and evaluates values
(`crates/midwrite-model/src/value.rs`, `jsfn.rs`, `sqlfn.rs`, `exec.rs`).
Which source forms are lowered to these values is described in
[source extraction](../architecture/source-extraction.md) and
[supported patterns](../reference/supported-patterns.md).

Values are **concrete and bounded**: `null`, booleans, 64-bit integers,
strings, rows, generated values, and — with the feature `js-values` —
`undefined`, bounded arrays, `Map` and `Set`. There are no floating-point
values and no decimals; integer overflow is an error, never a wrap-around.
In JSON reports the tagged forms are `{"$generated": g, "actor": a, "n":
k}`, `{"$undefined": true}`, `{"$map": [[k, v], …]}` and `{"$set": […]}`
([counterexample schema](../reference/schemas/counterexample.v1.schema.json)).

## Two evaluation modes

* **SQL mode** inside statements: three-valued logic — a comparison with
  `null` is unknown and does not match. `inArray` of an empty list is
  `false`, `notInArray` of an empty list `true`; `and`/`or` drop
  `undefined` operands, as Drizzle does. A conditional predicate `test ?
  A : B` is `(which AND A) OR (NOT which AND B)` with `which = !!test`
  computed in JavaScript.
* **JavaScript mode** elsewhere: strict equality, truthiness,
  short-circuit evaluation; reading a property of `null`/`undefined` is a
  `TypeError` the application can catch.

An expression or built-in the model cannot decide at run time (an
unextracted expression, a non-ASCII case mapping, the text of a generated
value, an integer beyond 2^53−1, object identity) is `Unknown`: the path
ends as **unsupported**, never as an exception the application's `catch`
could observe.

## Generated values

Application- and database-generated identifiers (`gen_random_uuid()`,
`uuid().defaultRandom()`, `$defaultFn` with a recognized generator,
`nanoid()`/`crypto.randomUUID()` calls) are `Value::Gen(GenId { generator,
actor, n })`.

* **Identity is the generation event**: which generator (a column's
  generated default, or one shared index for every package call with a
  fresh-value contract), which actor, and that actor's generation counter.
  The counter is per actor, shared by all its generators, and never reset —
  not by a rollback, a retried transaction or a retried request — so every
  event has its own identity, independent of how other actors are scheduled
  (generation adds no footprint, unlike a sequence). A call site is not part
  of the identity.
* **Equality is identity** (assumption `generated-values-distinct`, printed
  with every result of a model that has generators): a generated value
  equals itself only and differs from every concrete value. Collisions of
  random identifiers are not explored.
* **No order**: `<`, `<=`, … on a generated value are unknown. Where a
  `LIMIT 1` read needs an order among rows with generated keys, rows are
  ordered by generation event (a stated assumption; PostgreSQL orders random
  UUIDs by value).
* **Execution boundary**: a generated default is assigned when the INSERT
  starts executing — where Drizzle builds the statement (application side)
  and where PostgreSQL evaluates defaults (database side). A statement that
  waits and resumes keeps its values; any new execution generates new ones.
  A failed INSERT consumes the counter and leaves nothing observable.
* **Never initial data**: the loader refuses generated values in
  `initialState`.
* **Propagation**: a generated key returned by `RETURNING` and stored
  elsewhere (`values({ ownerId: created.id })`) is the same event, so
  foreign keys keep identity. Returned row *values* of an `UPDATE` are not
  modeled (only the row count and truthiness).

These rules rest on measured behavior of the pinned drizzle-orm and
PostgreSQL: one evaluation of `$defaultFn` per row and per execution of a
builder, the callback still running for an INSERT that fails, sequences and
generated keys not reused after a rollback. Combinations not probed
(multi-row generation in one statement, a builder awaited twice, an `SQL`
value returned from a callback) are extracted as unknown operations.

At runtime, the lockstep oracle sends a generated value as a deterministic
UUID that encodes the event and reads it back as that event; original-code
verification matches model generated values to the run's concrete texts
**bijectively per run** (one generated value matches one text, never two),
binding values seen at crash checkpoints first, in transition order.

## Clock readings (feature `timestamps`)

`timestamp`, `timestamptz` and `date` columns in their default Drizzle
modes.

* `now()` (`defaultNow()`, `` sql`now()` ``) is the **transaction's start
  time**: one value for every statement of a transaction.
* `new Date()` in a statement's arguments, `$defaultFn(() => new Date())`
  and `$onUpdate(() => new Date())` are one **application clock reading**
  per occurrence and execution; `$onUpdate` columns are added to `UPDATE …
  SET`, an upsert's `DO UPDATE SET`, and inserts that omit the column.
* Clock readings are opaque generated values: equality is identity
  (`clock-readings-distinct`), an order comparison is unsupported, and they
  match none of the generated-identity predicates of
  [invariants](invariants.md).
* Explicit values must be canonical text (`YYYY-MM-DDTHH:MM:SS.sssZ`;
  `YYYY-MM-DD` for `date`) and compare by value.

## JSON (feature `json`)

`json`/`jsonb` columns hold objects, arrays, integers, strings and booleans
nested at most 4 deep: writes, read-back and `isNull` work. Every comparison
of a JSON column is unsupported and `->>` is refused. This is not
PostgreSQL's JSONB semantics.

## JavaScript values (feature `js-values`)

Without the feature, extraction and state encodings are those of scenarios
written before it existed; discovery turns it on for a synthesized scenario
whose actors use one of these forms.

* **`undefined`** is a value distinct from `null`: missing properties,
  out-of-range indexes, missing arguments, `find` and `Map.get` misses,
  `return;`. `x == null` tests nullish; `??` and `?.` test nullish.
  `undefined` never reaches SQL.
* **Strings**: template literals and `+` (concatenation when a side is
  statically a string), `String(x)`, `.length`/`.indexOf` in UTF-16 code
  units, `includes`/`startsWith`/`endsWith`, `split` on a literal
  separator, ASCII `toLowerCase`/`toUpperCase`, `trim` with the ECMAScript
  whitespace set.
* **Arrays** are values: literals, indexing, `.at(k)`,
  `includes`/`indexOf` (SameValueZero), `join`,
  `map`/`filter`/`find`/`findIndex`/`some`/`every` with a single-expression
  arrow, destructuring, `for…of` over a snapshot. Mutation and identity
  (`===` on arrays, maps, sets) are refused.
* **`Map` and `Set`**: construction, `get`/`has`/`size`; `set`/`add`/
  `delete`/`clear` only as a rebinding of a `const` local used through its
  own methods (an alias would observe the mutation, which a value model
  cannot). Object keys and iteration are refused.

New value forms are local state: they add no footprints, and states without
them keep their encodings.

## Approximations kept

`null + x` is `null`; relational comparisons with `null` are `false` (in
invariants and JavaScript mode); mixed-type `===` is an error in the model;
`+` on a value not statically known to be a string is arithmetic; a default
parameter applies only to a missing argument, not an explicit `undefined`;
non-null unique keys compare by model value equality (collations and
type-specific equality are not modeled).
