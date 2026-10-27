# Phase 12 architecture

Consolidates the plan's `ARCHITECTURE`, `SCHEMA-COMPOSITION`,
`CROSS-MODULE-RESOLUTION`, `SHARED-SEMANTIC-INDEX` and
`INCREMENTAL-INVALIDATION` documents. Generated values: see
`GENERATED-VALUE-SEMANTICS.md`. Nothing here replaces an existing component:
the model checker, the scenario loader and discovery are extended in place.

```text
TypeScript project
  └─ ProjectContext (analyzer/context.rs) ── one per project root, shared by every extraction
       files (one discovery pass) · Resolver (tsconfig paths, workspaces) · sources (read once)
       ModuleSummary per file: imports · exports · `export *` · declarations
       export resolution through re-exports · import closures · memoized type facts
  ├─ schema extraction (schema.rs + generators.rs): tables, keys, FKs, generator classes,
  │     imports; `$defaultFn(helper)` resolved through the context
  ├─ entry extraction (concurrency.rs): tables bound by declaration, repositories, factories
  └─ discovery (commitrift-discover): index → extraction (+ disk cache) → effects → candidates
        → synthesized multi-module scenarios → model checking → original-code replay (authorized)
```

## 1. Shared project context (Milestone 6)

`commitrift_analyzer::context::ProjectContext` holds what every extraction of
a project needs and used to rebuild:

| field | built | before Phase 12 |
| --- | --- | --- |
| `files`, `Resolver` | once per provider and root | per extraction: a full `discover` walk + `Resolver::new` (workspace `package.json` scan) |
| `sources` | once per file (`Arc<str>`) | per inlined call |
| `ModuleSummary` | once per file | — (re-exports were not followed) |
| export resolution, import closures, `db-factory`/`db-type` facts | memoized | — |

`TsActorProvider` keeps `Arc<Mutex<HashMap<root, Arc<ProjectContext>>>>`;
clones share it (`MemoProvider` wraps one provider for a whole discovery
run). The context holds **compact facts, not ASTs**: an Oxc AST lives in a
per-use arena and is dropped after the function that needed it (parsing is
still per use; caching parsed ASTs across entries would require
self-referential arenas and was not needed — see `PERFORMANCE.md`). Every
fact is a function of file contents; a context is never reused across runs.
Interior mutability is `Mutex` (the provider is `Clone + Send`), with no lock
held across parsing of another file except the resolver's short critical
sections.

There is one source of semantic truth: the context only resolves *where*
names are declared; what a declaration means is still decided by the same
lowering code as before.

## 2. Cross-module resolution (Milestone 5)

| pattern | resolution |
| --- | --- |
| `export { x } from`, `export { a as x } from`, `import { a } …; export { a as x }`, `export * from`, `export * as ns from` | `ProjectContext::resolve_export` follows them to the declaring module; a name provided by two `export *` modules is **ambiguous** (as in TypeScript) and not followed; cycles are reported |
| `obj.m(...)` where `obj` is (imported from) a module-level `const obj = { ... }` | the method (`m() {}`, `m: async () => {}`, `m: function`, `m: f` of a module function) is inlined; `this.other()` inside resolves to the same object. Refused when the module writes `obj.p = …`, `obj[…] = …`, `delete obj.p` or `Object.assign(obj, …)` (text scan, conservative) or the literal has a spread |
| `inst.m(...)` where `inst` is `const inst = new C(args)` (module level) or a local `const r = new C(tx)` | the class method is inlined with a `this` binding: constructor parameter properties (`private readonly db: Database`) and `this.f = param` statements map fields to the constructor arguments (handles stay handles: `db`, `tx`, `getDb()`); `this.f.insert(...)` uses the field's handle; `this.m()` resolves within the class. Refused: `extends` (inherited or overridden methods are not guessed), a constructor doing anything else, field initializers with values |
| `getDb().insert(...)`, `const db = getDb()` | a zero-argument call of a function whose **declared return type** is a `drizzle-orm` `…Database` type (directly, `Promise<…>`, or through a type alias of the module or of an imported project module) is a root database handle. Bodies are not interpreted |
| `fn(db: Database)` parameter of an entry | a root database handle when `Database` resolves as above; otherwise every Drizzle call on it is the unknown category **database handle of an unresolved type** (midday's `Awaited<ReturnType<typeof connectDb>>`, a `withReplicas(...)` database; school-ledger's `DbExecutor = Database \| Transaction`) |
| `db.transaction(async (tx) => repo.save(tx, …))` | Phase 7 handle passing; the handle survives object/class methods (`ArgVal::Handle`, `this.f` handles), so an operation on `tx` stays in the transaction and one on `db` stays autocommit (test `a_transaction_handle_passed_to_a_repository_stays_in_the_transaction`) |
| a call into a package | unchanged: unknown (`call into an unanalyzed package`); no package is executed or assumed pure |

## 3. Schema composition and table identity (Milestone 3)

* **Scenario format** (additive): `extraction.schema` is a module or a list
  of modules; a table's `from` is `binding` (unique across the listed
  modules) or `module#binding`. Errors: a binding declared by several listed
  modules and not qualified, a module listed twice, a declaration read twice
  ("two references to one declaration are one table"), two used declarations
  with one SQL name.
* **Identity**: with a list, each table carries its declaration
  (`TableBinding.source = (module, binding)`, `TableFacts.module`); an
  identifier in source denotes a table only if it **resolves** — through
  imports, aliases and barrels — to that declaration. A local variable or
  another declaration with the same name is not the table; an alias
  (`import { users as accounts }`) is. `db.query.<key>` matches the binding
  name, restricted to declarations the querying module reaches. Phase 8–11
  scenarios (one module) keep binding by name, unchanged.
* **Foreign keys** are resolved by declaration too
  (`ProjectIndex.references`: the referenced name as written in the schema
  module → its declaration), so `references(() => accounts.id)` with an
  import alias reaches `users`.
* **Discovery**: a project with several schema modules is indexed in
  identity mode; a second declaration of an SQL name is refused with the
  reason (not merged); the table closure of a candidate (the tables its
  actors touch plus foreign-key parents) may span modules and the emitted
  scenario lists exactly the closure's modules (a slice, not every table of
  the project). Single-module projects produce Phase 11 scenarios unchanged.

## 4. Incremental invalidation (Milestone 6.4)

The Phase 11 disk cache keys each entry's extraction by the digests of its
import closure (re-export sources included), the project configuration, the
contract, and **all** table bindings — so any schema change invalidated every
entry. In identity mode the key covers only the bindings whose declaring
module is in the entry's import closure: an entry can bind no other table
(identifiers must resolve to the declaration; relational-query keys are
restricted to reachable declarations). Single-module projects keep the
Phase 11 key. `CACHE_VERSION` is `commitrift-discover-cache-2` (extraction
output changed). Test
`a_change_to_one_schema_module_invalidates_only_the_entries_that_reach_it`:
editing one schema module makes its readers miss, unrelated entries hit, and
the cached result equals an uncached run.

## 5. Discovery changes

* A new candidate family **`resubmitted-write`**: an entry with a declared
  crash retry whose INSERT can commit before the crash; a candidate only
  with a declared invariant on the table or a unique constraint covering the
  written columns (schema-enforced), otherwise a recorded rejection.
* Controls: a "distinct key" variant now varies the parameters a *read* key
  depends on (`owner.id` of the user found by email); before, such a control
  silently repeated the actual scenario.
* Synthesis never seeds a row whose key the execution generates or inserts
  itself; arguments flowing into `uuid` columns are valid UUIDs.
* Ambiguous-commit candidates: the handler's repeated write is not required
  of a fault-free solo run; declared invariants see every written value.
* `index.generatedColumns` lists modeled generated-key columns.

## 6. Provenance (Milestone 7)

A helper's return slot (`h$ret = null`) is now attributed to the **call
site in the caller's module** (before: the caller's span resolved in the
callee's file — an unrelated line). Every other source reference keeps its
meaning: an operation's `src` is its definition site, and the call chain
(`callChain`, Phase 7) names the callers. This is an intentional correction
of a local operation's location; local operations are not scheduling points,
and the model-checking identity comparison (`FINAL-REPORT.md`) shows whether
any report changed.
