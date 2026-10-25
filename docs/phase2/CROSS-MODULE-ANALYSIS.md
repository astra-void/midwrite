# Cross-module analysis (Milestone 3)

Phase 1 summarized only functions declared in the same file. Phase 2 links
calls of imported functions to their declarations in other analyzed files and
reuses the same summary mechanism for them, so that

```text
action → service → repository → database write
```

is analyzed in the action, with every operation at its own location and file.

## Design in one paragraph

Each file is still lowered on its own; the frontend additionally records, per
call site, *what* is called (a same-file function, or a module specifier and
an exported name) and which database handles are passed as arguments, and,
per file, what it exports. A project step resolves module specifiers and
exports to function declarations, puts every function of the project in one
table, and runs the existing core inliner (`inline_calls`) over that table.
Nothing in `crashproof-core` knows about modules or parsers: it sees a
function table, call sites with callee indices and argument handles, and a
file id per function.

## Project index (`crashproof-analyzer`)

| What | Where |
| --- | --- |
| Source files, discovery order | `discover_excluding` (sorted, de-duplicated, `exclude` patterns) |
| Module identity | absolute, lexically normalized path of the file (`resolve::absolute`) |
| Imports | `bindings::ImportBinding` (named / default / namespace, value or type-only) |
| Exports | `exports::collect` → `ModuleExports` (name → function index, re-export, or not a function; `export *` list) |
| Function identity | position in the project table (file order, then source order); never an Oxc `SymbolId`, which is only unique within a file |
| Call sites | `lower::LocalCallSite` (target, placeholder, argument handles) |
| Local binding provenance | unchanged (`bindings::resolve`), plus parameters (`LocalBindings::params`) |
| Effect summaries | `crashproof-core::analysis::summary` (memoized) |
| Unresolved references | the call keeps its unknown effect; the detail says why; counted in `summary.analysis` |

## Module resolution (`resolve.rs`)

A bounded, documented subset of TypeScript's resolution, applied only to
files being analyzed:

1. **Relative specifiers** (`./x`, `../x`), normalized lexically.
2. **`tsconfig.json`** nearest to the importing file (searched upwards):
   `compilerOptions.paths` (patterns with at most one `*`, longest prefix
   wins, targets tried in order) and `compilerOptions.baseUrl`; `extends`
   is followed when it is a relative path (up to 8 levels). The file is
   parsed as JSONC (comments, trailing commas).
3. **Workspace packages**: the nearest `package.json` above each analyzed
   file, by `name`. A bare specifier naming such a package (or a subpath)
   resolves through `exports` (string targets, single-`*` patterns,
   conditions `types`, `import`, `module`, `default`, `require`, `node`, in
   that order) or, without `exports`, through `main`/`module`/`types` (root)
   or the subpath itself. Two packages with the same name are both ignored.
4. **Candidates**: a TypeScript extension as written, `.js`/`.jsx`/`.mjs`/
   `.cjs` mapped to `.ts`/`.tsx`/`.mts`/`.cts`, or `X.ts`, `X.tsx`,
   `X/index.ts`, `X/index.tsx`. Only analyzed files count. **More than one
   existing candidate is reported as ambiguous and not chosen** (TypeScript
   would prefer `X.ts`; CrashProof does not guess).
5. Anything else is a **package import**: not followed, counted as
   `packageCalls`.

Not supported: `node_modules`, `typesVersions`, project references, `paths`
with several `*`, non-relative `extends`, `imports` (`#x`) maps, dynamic
`import()`, CommonJS `require`.

## Export resolution (`project.rs`)

| Form | Linked to |
| --- | --- |
| `export async function f() {}` | `f` |
| `export const f = async () => {}` (or a function expression) | `f` |
| `export default function () {}`, `export default async () => {}` | the function |
| `export default f` | `f`, if it names a function of the module |
| `export { f, g as h }` | `f`, `g` |
| `import { f } from "./a"; export { f }` | re-export of `./a`'s `f` |
| `export { f as g } from "./a"`, `export { default as f } from "./a"` | re-export |
| `export * from "./a"` | every name of `./a` except `default` |
| `export type`, `export interface`, `export { type T }`, `import type`, `import { type T }` | never a function; a call through a type-only import is not linked |
| `export const svc = { async m() {}, n: fn }` | an object service: `svc.m()` calls are linked to `m` (Phase 2 follow-up) |
| classes, enums, `export * as ns`, `export const x = wrap(...)` | not a function CrashProof can summarize |

A name exported twice, or provided by more than one `export *` module, is
**ambiguous** and not linked. Re-export chains are followed at most 16 deep
and memoized per (module, name), which also stops cycles
(`fixtures/projects/circular_reexports`).

A call is linked only through a resolved import and a resolved export. Two
functions with the same name in different modules are never confused
(`fixtures/projects/same_name`), and a local binding that shadows an import
is not the import (`fixtures/projects/shadowed`).

## Interprocedural summaries (`crashproof-core::analysis::summary`)

The Phase 1 mechanism (depth-bounded inlining of the callee's own IR into a
`Call` node) is unchanged in shape and extended:

* **Call graph and bounds.** Calls form the graph; expansion follows it
  depth-first from each function, at most `--max-call-depth` (default 3)
  levels, never re-entering a function on the current expansion stack. A
  recursive or mutually recursive call is cut and stays an unknown effect
  (`fixtures/projects/recursive_imports`). This stack check is the cycle
  handling; no SCC condensation is needed because nothing is iterated to a
  fixed point.
* **Memoization.** The expansion of a callee at a given remaining depth, and
  the set of operations it performs on every path, are computed once and
  reused for every call site, unless a recursive call was cut during the
  expansion (the result then depends on the stack). Arguments are not part
  of the key because they are substituted while the expansion is copied to
  the call site. Measured reuse: midday 749 expansions, 1181 reuses;
  school-ledger 1001 expansions, 4918 reuses.
* **Provenance and locations.** Copied operations keep their own
  recognition, label and location; locations get the callee's file when it
  differs from the caller's (`Location::file`), and each file's IR carries
  the table of other files it refers to (`FileIr::sources`). The call chain
  (`SummaryOrigin.calls`, now with the callee's declaration location) is
  rendered in the failure sequence and the evidence, with file paths:

  ```text
  1. `db.update(orders)` (named_import/services/order.ts:7:9, in `markPaid`,
     reached through `markPaid(id)` at 7:9) commits immediately ...
  ```

* **No duplicates.** A hazard whose two operations come from the same
  inlined call is reported only in the callee's own analysis
  (`fixtures/projects/internal_hazard`), and unknown calls inside one
  inlined call are reported once per call.

## Argument and transaction-handle binding

A database operation on a parameter of the callee is lowered with
`DbHandle::Parameter { index, name, configured }`. At each call, the frontend
classifies the arguments:

| Argument | Passed handle |
| --- | --- |
| a transaction callback parameter (`tx`, or a `const` alias of it) | `Transaction { id }` of the caller |
| a `drizzle(...)` instance, or an identifier / member path matching `databaseReceivers` | `Root { name }` |
| a parameter of the calling function | `Parameter { … }` of the caller (bound later by the caller's own callers) |
| a call matching a configured call path (`getDb()` with `"getDb()"` in `databaseReceivers`; follow-up) | `Root { name: "getDb()" }` |
| anything else (an unconfigured `getDb()`, `tx ?? db`, literals, spreads) | none |

When the callee is copied to the call site, each `Parameter` handle is
replaced by the passed handle, and the operation's evidence says
"`db` is parameter 1 of `saveOrder`, bound to the argument `tx` passed at
9:11". A transaction the callee starts on a parameter becomes a savepoint of
the caller's transaction when the caller passes its handle. When the call
passes no recognized handle, the operation becomes an unknown effect
(`unresolvedHandle`) and, if nothing recognized remains, the whole call stays
one unknown effect with that reason. **An operation on a parameter is never
treated as a write on an unrelated root handle.**

Analyzed on its own, a function treats a parameter whose name matches a
configured receiver as that root handle (the Phase 1 behavior, so that a
query function `createX(db: Database, ...)` is still analyzed by itself), and
an operation on any other parameter as an unrecognized call.

## Fixtures (`fixtures/projects/`, `crates/crashproof-analyzer/tests/projects.rs`)

| Fixture | Covers |
| --- | --- |
| `named_import` | named import, CP1001 across files |
| `aliases` | `import { f as g }`, default import |
| `barrel` | `export { f as g } from`, `export *` |
| `call_chain` | action → service → repository |
| `recursive_imports` | mutual recursion across modules |
| `circular_reexports` | `export *` cycle, missing name |
| `missing_module` | import of a file that does not exist |
| `type_only` | `import type`, `import { type T }` |
| `shadowed` | parameter shadowing an imported name |
| `tx_handle` | transaction handle passed to helpers (configured and unconfigured parameter names) |
| `root_handle` | root handle passed inside a transaction |
| `unknown_handle` | an unconfigured `getDb()` passed as the handle |
| `object_service` | methods of an imported object-literal service (follow-up) |
| `conditional_callee` | effect on some paths of the callee |
| `pure_helper` | imported function without effects |
| `same_name` | two modules exporting the same name |
| `ambiguous_exports` | name provided by two `export *` modules |
| `tsconfig_alias` | `paths` alias in a JSONC `tsconfig.json` |
| `internal_hazard` | hazard inside the callee reported once |
| `unawaited` | unawaited imported call is not summarized |
| `correlated` | contradictory conditions |

Plus `crates/crashproof-analyzer/tests/phase2.rs`: workspace packages through
`exports`, `tsconfig` `extends` chains, ambiguous module candidates
(`x.ts` and `x.tsx`), `exclude` patterns, determinism, and equality of
project and single-file results on the single-file fixtures.

## API

* `Analyzer::analyze_source(path, source)` and `lower_source`: unchanged
  single-file behavior; imported calls are not followed
  (`phase2.rs: analyze_source_does_not_follow_imports`).
* `Analyzer::analyze_project(inputs) -> ProjectAnalysis`: per-file IR and
  diagnostics, failures, `AnalysisCounts`, phase timings.
* `Analyzer::analyze_paths(inputs, config_label) -> Report`: now project
  analysis; used by the CLI (`check` and `ir`).

## Measured results

See `CORPUS-EVALUATION.md`: 1262 cross-file calls linked in midday (71
unresolved, 70 to packages), 3203 in school-ledger; midday's 40 CP1001 and 9
CP1002 are all reached through cross-module summaries, as are the reviewed
true positives listed there.
