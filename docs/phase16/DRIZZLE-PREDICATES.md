# Phase 16: Drizzle predicates held in locals

Track C. Before Phase 16 a `.where(...)` predicate had to be written in
place: `const inFolder = folderId === null ? isNull(..) : eq(..)` followed
by `.where(and(.., inFolder, ..))` was "predicate is not a Drizzle operator
call" (Phase 15 set E06). This note says what is now resolved, with which
semantics, and what stays unknown.

## 1. Semantics source

* Drizzle 0.45.4 builds an `SQL` object when the operator is called:
  `eq(column, value)` binds `value` as a parameter at that moment, so a
  predicate stored in a local and used later carries the operand values of
  the call, not of the use. The pinned version's `toSQL()` of
  `const p = eq(t.id, 3); db.select().from(t).where(and(p, isNull(t.balance)))`
  is `... where ("accounts"."id" = $1 and "accounts"."balance" is null)` with
  params `[3]`.
* `and(...)` / `or(...)` drop `undefined` operands (`c !== undefined`; `null`
  is not dropped); `where(undefined)` selects every row. Phase 14 already
  lowers this for operators written in place (`pred_in`'s `ident`).

## 2. What is resolved (`crates/commitrift-analyzer/src/concurrency.rs`)

A declaration `const p = <init>` (or a `let` the module never assigns again)
whose initializer is *predicate-shaped* binds `p` to a held predicate
(`Binding::Pred`) instead of a value:

* a call of `eq ne gt gte lt lte and or not isNull isNotNull inArray
  notInArray` imported from `drizzle-orm` (and not shadowed by a local);
* a local already holding a predicate (an alias: the same predicate);
* `test ? predA : predB`, and `test ? pred : undefined` / `test ? undefined :
  pred`;
* a call of a project function — declared in the module or imported from an
  analyzed module, not `async` — whose body is `return <predicate-shaped>`
  (or an arrow with that expression body).

The held predicate is lowered **where it is declared**
(`Lowerer::held_predicate`):

* Its JavaScript operands (everything that does not read a column) are
  evaluated into fresh locals right there (`hoist_operands` + `Let`), so a
  later assignment to a variable it used does not change it (Drizzle's
  capture). Columns stay columns.
* `test ? predA : predB` uses the Phase 14 lowering: `which = !!test`
  (`Let`), the operands of the chosen branch only are evaluated under a
  `JumpIf`, and the predicate is `(which AND a) OR (NOT which AND b)`.
  `which` is always a boolean (never NULL, never a non-SQL value), and the
  unchosen branch's operands are `null`; under SQL three-valued logic
  `(true AND a) OR (false AND b)` is `a` (NULL included) and
  `(false AND a) OR (true AND b)` is `b` — exactly the chosen predicate.
* `test ? pred : undefined` records `defined` (`which`, or `NOT which`) next
  to the predicate. Where it is used, an `undefined` operand stands for what
  it stands for there (`true` in `and`/`where`, `false` in `or`), and the
  use is `(defined AND pred) OR (NOT defined AND <that>)`, exact for the same
  reason. Where `undefined` has no meaning (`not(p)`), the use is refused:
  "the predicate `p` may be `undefined` here, which is not modeled".
* A helper call evaluates its arguments at the call (in the caller), binds
  the helper's parameters to them, and lowers the returned predicate in the
  helper's module (its tables and imports). Recursion and the inline depth
  are bounded as for other helpers.
* Uses: `.where(p)`, any operand of `and`/`or`/`not` (`and(p, eq(..))`,
  `and(alias, ..)`), `findFirst({ where: p })`. Used as a value (`if (p)`)
  it is truthy; any other value use is unknown.
* Provenance: the operand assignments carry the declaration's source
  location; every use adds the assumption "the Drizzle predicate `p`
  (`file:line:column`, or "returned by `f` at ...") is built where it is
  declared: its operands keep the values they had there".

## 3. What stays unknown (never approximated as `true`)

An unknown held predicate makes the query that uses it an `Op::Unknown`
whose detail names the cause (the query does not run):

* a `let` holding a predicate that the module's text assigns again
  (`let where = eq(..); if (x) where = and(where, ..)`): "the predicate `p`
  is held in a variable that is assigned again"; the check is textual and
  conservative (any same-named assignment in the module counts);
* a clock reading in a held predicate (`eq(t.at, new Date())`,
  ``sql`now()` ``): a statement's clock placeholder is bound per execution,
  not where the predicate was built;
* operators outside the list (`like`, `between`, `exists`, raw `sql`),
  `inArray` over a computed list or a subquery, JSON operands — as for
  predicates written in place;
* a nested conditional inside a conditional predicate, a helper call inside
  a conditional branch, a helper whose body is more than `return <pred>`,
  `async` helpers, destructured or rest parameters of a helper;
* a predicate-shaped initializer whose operator is not imported from
  `drizzle-orm` (re-exported through a project module, renamed on import)
  is not recognized as a predicate; its use stays the Phase 15 unknown
  ("`p` is not a Drizzle predicate the model resolves").

## 4. Evidence

```
cargo test -p commitrift-analyzer --test phase16_locks
cargo test -p commitrift-discover --test phase16_locks
```

* Extraction and execution in the model (10 tests): const predicates and
  aliases; operand capture (`let target = id; const byId = eq(.., target);
  target = 999; where(byId)` finds the row with the declaration-time value;
  the same predicate written in place reads 999 and fails); the two
  conditional forms against rows with a NULL and a non-NULL folder (root /
  folder 7 / folder 8, defined and undefined title); local and imported
  helpers (`function inSpace(id) { return eq(..) }`, an arrow
  `titled = (t) => eq(..)`, an imported conditional helper); the unknown
  cases above, each with its reason and with no query emitted.
* Discovery: `fixtures/phase16/locks/src/services/documents.ts#createDocument`
  (E06-like: `const inFolder = folderId === null ? isNull(..) : eq(..)`,
  `const sameTitle = and(.., inFolder, ..)`, `.where(sameTitle)`) is
  MODEL_COUNTEREXAMPLE in the world `null:folderId` (two root documents with
  one title; the NULLS DISTINCT unique key does not stop them) and safe in
  the base world; `createDocumentLocked` (the same with a predicate helper,
  serialized by `FOR UPDATE` on the space row) is NO_VIOLATION_WITHIN_BOUNDS
  in every explored world, the null-folder world included.
* Phase 15 set: E06 (`fixtures/phase15/eval-app`
  `documents.ts#createDocument`) is now extracted and reported as
  MODEL_COUNTEREXAMPLE in `null:folderId` (against the synthesized
  at-most-one hypothesis; E06's property is not declared there). E07
  (`createPage`) is extracted too and stays INCONCLUSIVE: its NULLS NOT
  DISTINCT key is not enforced by this track's model.
