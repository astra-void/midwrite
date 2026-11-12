# Facts from returned database rows

`check`, `ir` and `trace` project analysis run a bounded row-lineage pass
after lowering and before call summaries. Implementation:
[`row_flow.rs`](../../crates/midwrite-analyzer/src/row_flow.rs).
It adds `Fact::Never` only when a branch arm is contradictory on all
modeled normal continuations and the proof uses a returned-column fact.
It does not create effects, infer business invariants, or suppress findings
by project name, source line or fingerprint.

The source snapshots are the exact bytes already lowered. Imports,
re-exports, namespace imports and top-level destructuring aliases resolve
through the same analyzed file set. An unrelated edit during analysis
cannot make a second read supply different helper bodies to this pass.

## The supported relation

Given a supported string column, `UPDATE ... SET phase = requested
RETURNING *` returns the **new** `phase`. On a continuation where
`requested` is a compatible defined string, the returned column has that
same string value. It is not a snapshot of the previous row. The pass can
carry that relation through an awaited source helper, a transaction
callback, `neverthrow` `ok`/`err`, `map`, `andThen`, `mapErr` and a successful
`isOk`/`isErr` guard.

For example, the following conjunction cannot be true after the new row
has been returned:

```ts
requested === Mode.Running && updated.phase === Mode.Paused
```

This does **not** apply to a row selected before the write. Nor does it
equate an undefined assignment with the returned column: Drizzle can skip
that assignment, leaving the old/default value in place.

Schema evidence is required. The column must come from an analyzed
Drizzle `pgTable` declaration, and use a supported built-in `text`,
`varchar` or `pgEnum` builder with understood modifiers. A bounded varchar
must accept the string's character length, and an enum must contain the
string. Numeric-to-text coercion, custom codecs, unknown schemas,
projections and other column types do not establish this identity.

## Value and control boundaries

The abstract domain distinguishes ordinary records, arrays, rows, query
builders, promises, result variants, input snapshots and schema identities.
Each invocation and input-property read has a structural identity; repeated
reads of a mutable input or calls of the same helper do not reuse an old
snapshot. Copied local values preserve their identity.

Ordinary record/array state is forgotten across possible mutation or
escape. Row escape, property mutation, captured writes, unsupported control
flow, unknown result transformations and exhausted budgets cancel the
proof. An unawaited promise is not treated as its resolved result.

Literal enum objects are usable only when their identity is neither
mutated nor exposed to unknown code in the analyzed sources, including
through aliases and re-exports. TypeScript type queries do not expose a
runtime object. The narrowly recognized TypeBox `Type.Enum` and `Type.Const`
readers construct schemas from values; their additional options are still
checked for escapes. These operations follow the published
[TypeBox 0.34.37 Enum implementation](https://cdn.jsdelivr.net/npm/@sinclair/typebox@0.34.37/build/cjs/type/enum/enum.js)
and [Const implementation](https://cdn.jsdelivr.net/npm/@sinclair/typebox@0.34.37/build/cjs/type/const/const.js).
Result transformations follow the [neverthrow 6.2.1 implementation](https://github.com/supermacro/neverthrow/blob/v6.2.1/src/result.ts).

The limits per candidate are 20,000 interpretation steps, 128 source-helper
calls, call depth 8, input-property depth 8 and 8 value alternatives. A
failed proof leaves the existing analysis and warnings intact. This is a
project-only refinement over normal source-level executions, not a general
JavaScript interpreter or proof of runtime PostgreSQL behavior. It assumes
standard Drizzle/driver semantics, unmodified library/built-in APIs, and no
database triggers or custom driver type parsers overriding the column's
returned representation.

Regression coverage: [returned-row facts](../../crates/midwrite-analyzer/tests/returned_row_facts.rs),
[project fixtures](../../fixtures/projects/returned_row_results),
and [independent maintenance writes](../../fixtures/vulnerable/independent_maintenance.ts).
