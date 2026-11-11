# Architecture

> **Current overview:** [`architecture/overview.md`](architecture/overview.md)
> (all crates, including `midwrite-model` and `midwrite-discover`). This
> page remains the detailed design of the static analyzer (`check`,
> `trace`, `ir`). Companion pages:
> [value analysis](architecture/value-analysis.md) (abstract values,
> inertness, dispatch, callbacks),
> [failure trace format](reference/trace-format.md) and
> [single-request replay](architecture/replay-protocol.md).

Midwrite is a pipeline with one job: for every function, find the points at
which state becomes durable or externally visible, and report where a failure
between two of them leaves partial state.

```text
 per file (parallel):  .ts/.tsx ──► Oxc parser ──► Oxc semantic ──► provenance ──► recognition ──► lowering + value analysis ──► (functions, calls, facts, classes, exports)
                                                                                                                                           │
 project:              module + export resolution ──► class table ──► one function table ──► summary fixpoint (inertness, parameter use) ──► call summaries (memoized per callback context; handles and values bound) ┘
                                                                                     │
 per function (parallel by file):  diagnostics ◄── rendering ◄── hazard detection ◄── path enumeration (feasibility, merging)
```

## Crates

| Crate | Responsibility | Depends on |
| --- | --- | --- |
| `midwrite-core` | Effect IR, failure-path analysis, diagnostics and code catalog, configuration, report model, line/column mapping. Language independent. | `serde`, `thiserror` |
| `midwrite-analyzer` | TypeScript frontend: file discovery, Oxc parsing and semantic analysis, identifier provenance, effect recognition, branch facts, lowering to IR, export collection, module resolution (`resolve.rs`), project analysis (`project.rs`), and the `Analyzer` driver. | `midwrite-core`, Oxc, `serde_json` (tsconfig/package.json) |
| `midwrite-replay` | Opt-in runtime execution: manifest validation, disposable PostgreSQL clusters, the checkpoint-protocol supervisor, observations and invariants, OS isolation backends (`isolation.rs`: macOS sandbox-exec, Linux bubblewrap), runtime preflight (Node.js/Bun), and the original-code coordinator and fault proxy used by `model-check --verify-source`. Never used by `check`. | `midwrite-core`, `libc` |
| `midwrite-cli` | Argument parsing, configuration discovery (`midwrite.json`), text/JSON/SARIF/HTML rendering, baselines, exit codes, the `trace`, `report` and (feature `replay`) `replay` and `init-replay` commands. Builds the `midwrite` executable. | the above, `clap` |
| `midwrite-corpus` | Developer tool, not published: corpus runs, run comparison, reviewed precision, per-stage memory profile, trace coverage/validation (`traces`), blind review packets (`review-packet`, `review-check`). | core, analyzer, `clap` |

Further modules of `midwrite-core` used by the static side:
`trace` (Failure Trace IR, crash simulation, validation —
[format](reference/trace-format.md)), `verification` (verification levels,
report types, regression comparison `compare_with_baseline`), and
`recovery` (function-level recovery contracts, applied after detection;
fingerprints and traces unchanged —
[configuration](reference/configuration.md#recovery-declarations)). The
JavaScript replay client and driver adapters live in `runtime/node`, the
harness generator in `midwrite-cli/src/init_replay.rs`
([replay protocol](architecture/replay-protocol.md)). Other crates
(`midwrite-model`, `midwrite-discover`) are described in
[architecture/overview.md](architecture/overview.md).

The core never sees an AST. A future frontend (another ORM, another language)
only has to produce `FunctionIr`.

## Frontend (`midwrite-analyzer`)

**Parsing.** `oxc_parser` with the source type derived from the file
extension (`.tsx` enables JSX). Any parser error makes the file a
`FileFailure` of kind `parse`; a partially recovered AST is never analyzed.

**Semantic analysis.** `oxc_semantic` builds scopes and symbols. Midwrite
uses it only to resolve identifier references to their declarations, so that
shadowing, renamed imports and aliases are handled correctly.

**Provenance** (`bindings.rs`). An identifier is resolved to an `Origin`:

| Origin | Example | Effect on matching |
| --- | --- | --- |
| `Unresolved` | a global or ambient `db` | name-based match |
| `Import` | `import { db as database } from "@/db"` | matched by the *imported* name; `importedFrom` can require the module |
| `DrizzleInstance` | `const pg = drizzle(pool)` with `drizzle` from `drizzle-orm*` | a database handle whatever its name |
| `TransactionHandle` | the `tx` in `db.transaction(async (tx) => ...)` | a handle bound to that transaction |
| `PendingBuilder` | `const q = db.update(t).set(v)` | `await q` executes the stored write |
| `Local` | a parameter, `const db = await connect()` | name-based match |
| `CallResult` | `const db = getDb()` (a `const`, a call without arguments) | name-based match, or a configured call path `getDb()` |
| `Contrary` | `const db = new Map()`, literals, functions, classes | never matched |

`const` aliases are followed up to a fixed depth; `let`/`var` are not. The
symbol table also records which bindings name a function declared in the file
(function declarations that are never reassigned, and `const` bindings of an
arrow function or function expression), for call summaries, how each import
binding was imported (named, default, namespace; value or type-only), and
`const` bindings of primitive literals (for constant conditions).

**Handles returned by calls.** A `databaseReceivers` entry ending in `()`
(`getDb()`, `services.db()`) matches a chain whose root and properties are
followed by a call without arguments; the call is part of the receiver
(`getDb().insert(...)`), and `importedFrom` checks the function's import. A
`const` initialized by such a call (`const db = getDb()`) has the origin
`CallResult` and matches the same entry; a call used as an argument
(`save(getDb())`) passes that root handle.

**Object services.** A `const` bound to an object literal whose properties
are inline functions or names of functions of the file records those
properties; an assignment or `delete` of one of its properties anywhere in
the file removes it. `obj.m(...)` on such an object is a call of that
function; on an imported binding it is linked when the binding's export
resolves to such an object (`ExportEntry::Object`); any other receiver is
left alone (not counted as an unresolved import).

**Transaction results.** When an inline transaction callback is the awaited
initializer of a never-reassigned binding (`const result = await
db.transaction(cb)`), each `return` of the callback carries facts about that
binding (`Exit::facts`): a literal gives `result === literal`, an object,
array or function literal gives `Fact::Object`, a bare `return` gives
`undefined`. The walker records them (`Event::Returned`) and checks later
decisions against them.

**Parameters.** The plain identifier parameters of the function being lowered
that are never reassigned are recorded with their position. A Drizzle-shaped
read or write on such a parameter is lowered with
`DbHandle::Parameter { index, name, configured }` instead of being dropped:
`configured` says whether the name matches a configured receiver. The
function analyzed on its own treats a configured parameter as that root
handle and an operation on any other parameter as an
unrecognized call; a caller that passes a handle decides what it really is
(see call summaries). Only reads and writes are lowered this way; other
calls on an unconfigured parameter are classified as before.

**Recognition** (`recognize.rs`). A call is *unwound* into a root (an
identifier or `this`) and a list of property and call segments (`db` ·
`.update` · `(orders)` · `.set` · `({...})`), looking through parentheses,
TypeScript assertions and optional chaining. The root's origin and the
configured receiver paths decide whether the chain is a database chain, a
configured external effect, `Promise.all`, or nothing:

* Provenance first: a transaction handle or a `drizzle(...)` instance is a
  database handle whatever the configuration says.
* Configured receivers are *paths*: `db`, `ctx.db`, `this.db`,
  `stripe.charges`. The root must match the path's first name (`this` matches
  only `this`; an identifier is resolved as above, and `importedFrom` checks
  its import), and the following properties must match the rest. The longest
  matching `databaseReceivers` path wins. The recognition basis is
  `ConfiguredImport` with `importedFrom`, `ConfiguredMemberPath` for a path
  with properties (only the names are checked), and `ConfiguredName` for a
  bare name.

* Database chains: the first method decides the operation (`insert`,
  `update`, `delete` → write; `select*`, `query.<t>.*`, `$count` → read;
  configured transaction methods → transaction; `rollback` on a transaction
  handle → exit; `execute` → raw SQL; anything else → unmodeled method). The
  remaining methods decide whether the chain executes by itself
  (`.execute()`, `.then()`, `.catch()`, `.finally()`), never executes
  (`.prepare()`, `.toSQL()`, `.getSQL()`), or is a lazy builder.
* External effects: the root plus leading properties must equal the configured
  receiver path, followed by the method call and optionally promise
  continuations.
* Lookalikes: a chain in which a configured receiver path (without a leading
  `this`) appears after some other expression, followed by a method call
  (`this.repo.db.update(...)` for `db`, `deps.queue.publish(...)` for
  `queue`), becomes an unknown effect that names the path to configure, so
  that it is reported rather than silently ignored or guessed.

**Lowering** (`lower.rs`). Statements are lowered by explicit recursion to
keep control-flow shape; expressions use Oxc's `Visit` so every expression
form is traversed in source (evaluation) order, with overrides for `await`,
calls, `?:` and logical operators. How a value is *consumed* decides whether a
lazy Drizzle builder executes:

| Consumer | Lazy builder executes | Unknown call recorded |
| --- | --- | --- |
| `await expr` | yes (`awaited`) | yes |
| `return expr` in an `async` function | yes (`returned`) | no |
| `return expr` in a non-async function | no | no |
| element of `await Promise.all([...])` | yes (`concurrent`) | yes |
| statement / unconsumed | no (unless explicitly executed) | no |

Configured external calls execute when called, so an unawaited one is a
`fireAndForget` effect. A call `f(...)` (or `ns.f(...)` on a namespace
import) of a function declared in the same file or imported (value imports
only) that is awaited, or returned from an `async` function, is lowered as an
unknown effect and recorded as a *call site* with its target (a local
function, or a module specifier and exported name) and, for each argument,
the database handle it passes, if any (a transaction handle, a configured
receiver, a `drizzle(...)` instance, or a parameter of the caller). For
`return f()` the effect is only a placeholder, since such returns are not
otherwise unknown effects. The summary pass below replaces it.

**Branch facts.** Each arm of an `if`, `?:`, `&&`/`||`/`??` and each `switch`
case records what taking it implies about *variables*: identifiers that
resolve to a binding of the file that is never written after its declaration
and is not an import. Supported forms: `v`, `!e`, `a && b` (true arm), `a ||
b` (false arm), `v === lit`, `v !== lit`, `v == null`, `v == true`, `switch
(v) { case lit: }` (and `default`: none of the literals), with `lit` a
boolean, string, finite number, `null` or the global `undefined`; a `const`
bound to a literal is evaluated. Everything else implies nothing. Variable
names are made unique in the file (`name#symbol`). Nested functions are not lowered into their parent;
the function extractor (`functions.rs`) lowers each one separately, with names
inferred from declarations, variables, methods (`Class.method`), object
properties and `export default`, or `parent::<anonymous@line:col>`. The one
exception is an inline callback passed to a recognized transaction method: it
runs immediately, so it is lowered in place as the transaction body and
skipped as a separate unit.

## Effect IR (`midwrite-core::ir`)

The IR is a tree that mirrors the source's structured control flow:

```text
FunctionIr { name, location, is_async, transactions: [TransactionInfo], body: [Node] }

Node = Effect { id, kind, label, location, execution, recognition?, summary? }
     | Transaction { transaction: TransactionId, execution, body: [Node] }
     | Call { call: CallId, callee, label, location, callee_location, execution, body: [Node] }
     | Branch { kind: if|conditional|logical, condition, arms: [Arm] }
     | Switch { discriminant, cases: [Arm], has_default }
     | Loop { kind, header, runs_at_least_once, body }
     | Try { block, handler?, finalizer? }
     | Exit { kind: return|throw|rollback|break|continue|unsupported }

EffectKind = DatabaseRead { handle, table? }
           | DatabaseWrite { operation, table?, handle }
           | External { category }
           | Unknown { reason, detail }

DbHandle   = Root { name } | Transaction { id }
Execution  = awaited | returned | returnedLazy | fireAndForget | concurrent   // returnedLazy: a builder a non-async function returns (DEV-148)
SummaryOrigin = { calls: [CallSite { call, callee, label, location, callee_location }], on_every_path }
Arm        = { label, body, facts: [Fact] }        // Fact = Never | Truthy | Equals | NotEquals on a variable
DbHandle  += Parameter { index, name, configured } // bound by callers
Location   = { line, column, end_line, end_column, file? }   // file: index into FileIr.sources
```

Design choices:

* **A write is not a commit.** `DatabaseWrite` records *which handle* issued
  it. Whether it commits immediately or with a transaction is decided per path
  by the analysis, from the transaction scopes that are open on that path.
* **Transaction scopes are explicit nodes** with begin (entering the node) and
  successful commit (leaving it normally) boundaries. `TransactionInfo`
  records the parent of a nested transaction and the handle parameter name.
* **Unknowns are first-class.** Anything not understood is an `Unknown`
  effect or an `Unsupported` exit with a human-readable reason; it is never
  dropped and never upgraded to a known kind.
* **Evidence travels with the IR.** Every recognized effect carries a
  `Recognition` (basis + resolution detail) that ends up in diagnostics.
  Effects and transactions inlined from a summary also carry a
  `SummaryOrigin`: the chain of calls they are reached through and whether
  the outermost callee performs them on every path.

`midwrite ir` prints the tree; `--format json` serializes it.

## Project analysis (`midwrite-analyzer::project`, `resolve`)

`Analyzer::analyze_project` (used by the CLI) lowers every discovered file on
its own (in parallel; results are kept in file order), then:

1. **Resolves modules** for every import call site (`resolve.rs`): relative
   specifiers; the nearest `tsconfig.json`'s `paths`/`baseUrl`, following
   relative `extends`; workspace packages (the nearest `package.json` above
   analyzed files, by `name`, through `exports` or `main`); with candidates
   `.ts`, `.tsx`, `/index.ts`, `/index.tsx` and `.js`→`.ts` mapping. Only
   analyzed files are candidates. Several candidates are reported as
   ambiguous, never chosen (TypeScript would prefer `X.ts`; Midwrite does
   not guess). Bare specifiers that match nothing are package imports and
   are not followed. Not supported: `node_modules`, `typesVersions`,
   project references, `paths` with several `*`, non-relative `extends`,
   `imports` (`#x`) maps, dynamic `import()`, CommonJS `require`.
2. **Resolves exports**: `export function`, `export const f = () => ...`,
   `export default function`, `export { a as b }`, `export { a } from`,
   `export *` (not for `default`), re-exports of imports. A name exported
   twice, or provided by several `export *` modules, is ambiguous; re-export
   chains are bounded (16) and memoized, which also makes cycles terminate.
   A `const` object literal of functions (`export const svc = { async m()
   {} }`) is an object service whose methods are linked; type-only exports
   and imports, classes (linked separately, see
   [value analysis](architecture/value-analysis.md#class-and-method-resolution)),
   enums, `export * as ns` and `export const x = wrap(...)` are not
   summarizable functions.
3. Places **all functions in one table** (file order, then source order).
   Function identity is the table index, never an AST symbol id. A call is
   linked to a callee only through a resolved import and export, never by
   name. Unlinked calls keep their unknown effect, with the reason in its
   detail.
4. Runs the **call summaries** below over the whole table, with each
   function's file, so that inlined locations keep their file
   (`Location::file`).
5. Splits the table back per file, renumbers file ids to a per-file table
   (`FileIr::sources`), and analyzes every function (in parallel by file).

`Analyzer::analyze_source` and `lower_source` keep the single-file behavior:
imported calls are not followed.

## Call summaries (`midwrite-core::analysis::summary`)

The analysis stays intraprocedural except for calls the frontend linked to a
function of the table (same file, or imported in project analysis).
`inline_calls` (and `inline_local_calls` for a single file) receives, per
function, the call sites (the unknown effect standing for each call, the
callee's index, and the argument handles) and rewrites every function that
makes such calls:

1. The callee is *expanded*: its own lowered IR with its local calls replaced
   in the same way, one level less deep. Expansion stops at
   `--max-call-depth` levels (default 3) and never re-enters a function
   already being expanded (recursion).
2. The expanded callee is used only if it contains a recognized operation and
   no unsupported control flow. Otherwise the call stays an unknown effect
   whose detail says why (no recognized operation, recursive, too deep,
   unsupported control flow), and a placeholder is removed.
3. *Certainty*: the callee's paths are enumerated (bounded by `--max-paths`);
   an effect is performed on every path when every path contains it, and a
   transaction when it commits a write on every path. Truncated enumeration
   means nothing is certain.
4. The callee's body is copied into a `Call` node at the call site with fresh
   effect, transaction and call identifiers, and each copied effect and
   transaction gets a `SummaryOrigin` (this call prepended to the chain it
   already had in the callee; `on_every_path` from step 3). While copying:
   * an operation on a callee parameter takes the handle the call passes for
     it (the caller's transaction, a root handle, or a parameter of the
     caller, which a later caller binds in turn); its evidence says so. When
     the call passes no recognized handle, the operation becomes an unknown
     effect (`unresolvedHandle`). If no recognized operation remains, the
     call stays one unknown effect;
   * a transaction started on a parameter becomes a savepoint of the
     caller's transaction when the caller passes its transaction handle;
   * branch facts are renamed with the call (`call#3:flag#12`), so that two
     calls of one callee, or the callee and the caller, never share
     variables;
   * locations without a file get the callee's file when it differs from
     the caller's.
5. **Memoization.** The expansion of a callee at a given remaining depth
   (with its certainty) does not depend on the caller, because arguments are
   substituted while copying; it is computed once and reused, unless a
   recursive call was cut during the expansion (the result then depends on
   the chain being expanded).

Arguments are classified at each call:

| argument | handle passed |
| --- | --- |
| a transaction callback parameter (or a `const` alias of it) | `Transaction { id }` of the caller |
| a `drizzle(...)` instance, or an identifier / member path matching `databaseReceivers` | `Root { name }` |
| a call matching a configured call path (`getDb()`) | `Root { name: "getDb()" }` |
| a parameter of the calling function | `Parameter { … }`, bound later by the caller's own callers |
| anything else (an unconfigured `getDb()`, `tx ?? db`, literals, spreads) | none |

An operation on a parameter is never treated as a write on an unrelated
root handle. The callee's effects keep their own locations and recognition
(or, for bound parameters, the argument's). Because the inlined body is ordinary IR,
ordering, transactions (including a helper that writes on `db` inside the
caller's transaction) and branches inside the callee are handled by the same
path enumeration and replay as the caller's own code.

## Value layer (`midwrite-core::absint`, analyzer `values`, `classes`, `link`, `builtins`)

A bounded abstract interpreter runs alongside lowering; its domains,
fixpoints, dispatch rules and soundness assumptions are in
[value analysis](architecture/value-analysis.md). In outline:

```text
lowering (per function)                     project / single file
  ValueState = Env × Heap (copy-on-write)     link.rs: ClassTable (classes of all modules,
  eval / bind_pattern / provenance              superclasses linked, subclasses indexed)
  call targets: functions, imports,           link_sites: LocalCall {callee | parameter,
    IIFEs, callbacks (Param), methods,          values, receiver, result, binding,
    constructors, super                         resolution, argument placeholders}
  summary facts per body (BodyFacts,          link_facts → absint::summary::FunctionFacts
    interned per module)                      solve (Tarjan SCCs): inertness (gfp),
  return value, $ret facts, effect              parameter use (lfp)
    provenance                                inline_calls_with(…, solved):
                                                inert calls → empty CallScope,
                                                callbacks per context, concurrent copies,
                                                provenance/return substitution, $ret renaming
```

* `absint::value` — `AbsValue`/`Atom` (bounded, canonical, inline for 0–1
  atoms), `Provenance`; `absint::heap` — allocation-site objects, strong/weak
  updates, escape, export to records; `absint::env` — binding environments;
  `absint::summary` — the inertness/parameter-use solver; `absint::relation`
  — `valueRelation` of findings.
* IR additions (all optional, absent when not computed): `Effect.provenance`,
  `FunctionIr.returns`, `CallScope.{resolution, inert}`,
  `CallSite.resolution`, `LoopKind::Callback`; `Exit.facts` may mention
  `$ret`.
* Path walker: facts of a callee's returns about variables live after the
  call are part of the trace-merge key.
* Detection: effects reached through an assumed method dispatch cap the
  finding at medium confidence; findings get `valueRelation`.

## Failure-path analysis (`midwrite-core::analysis`)

**1. Path enumeration** (`paths.rs`). Traces are enumerated forward. Each IR
node maps the set of traces reaching it to the traces leaving it normally,
plus traces leaving via `return`, `throw`, `break` and `continue`.
Inlined calls are walked through *path summaries* (described after this
list); per node:

* `Branch`: one copy of the incoming traces per arm, each tagged with a
  `Decision`. A copy whose arm facts contradict the facts of an earlier
  decision on the same trace is discarded (counted as `prunedPaths`). A
  branch whose arms contain no effects other than reads and no exits does
  not fork; reads never appear in traces.
* `Switch`: one entry per case, falling through subsequent cases until a
  `break` or exit; a missing `default` adds a "no case matched" path.
* `Loop`: "body skipped" and "body executed once" (do-while: once). `break`
  and `continue` rejoin after the loop.
* `Transaction`: pushes `TxBegin`; normal completion or `return` of the
  callback pushes `TxEnd(committed)`; a `throw`/`rollback` pushes
  `TxEnd(rolled back)` and propagates.
* `Try`: the block's traces continue normally; the handler is entered by the
  block's explicit throws and, as an approximation of "some awaited call
  threw", by the traces that entered the `try`. `finally` runs on every way out.
* `Call`: the inlined body runs in place for each incoming trace; a
  `return` inside it resumes the caller, an exception propagates out of the
  call. Of the traces leaving the call from one incoming trace, a trace is
  dropped (`mergedPaths`) when another has the same events after the call
  started and no more branch decisions between any two consecutive events:
  it would produce the same findings with at most the same confidence, and
  the facts that distinguish them concern only the callee's variables, which
  nothing after the call refers to.
* `Exit(unsupported)`: the path stops and is marked incomplete.

The number of traces per function is capped (`--max-paths`, default 512).
When the cap is hit the analysis keeps going on the explored prefix and adds
a `MW9001` diagnostic.

**Path summaries of inlined calls.** Walking every incoming trace through an inlined body multiplies with call
depth, so the walker summarizes calls (`paths.rs`, `PathMode`):

* **Compact summaries.** For one inlined call and the effect context it is
  reached in (innermost loop, `try` guard, handler), the callee is walked
  once from an empty trace; the fragments are grouped by exit (`normal`
  with return facts, `thrown` with rollback events, unsupported) and merged
  by the same dominance rule as the full walk; at the call site every
  fragment is appended to every incoming trace. This is exact when no fact
  variable of the callee is mentioned by a fact outside it (the walker's
  `live` set is empty): pruning depends only on facts sharing a variable,
  so every incoming trace sees the same pruning, merges and truncation.
  Otherwise the call is walked trace by trace. Pruned/merged/truncated
  counts are accounted per incoming trace, so even the report counters are
  unchanged.
* **Shared summaries** (default). Two call scopes of the same callee with a
  structure-preserving correspondence (a bijection of walkable nodes,
  transaction ids and fact variables, verified node by node; a shape hash
  only selects candidates) make the same decisions, so the first's summary
  is mapped onto the second instead of walking it. Anything that does not
  map aborts the instantiation and the call is walked.
* **Depth-canonical expansions.** An IR expansion records the smallest
  remaining depth at which all its call-site decisions are the same and is
  reused for any depth at or above it; expansions whose decisions depended
  on reaching depth 0 stay keyed by depth. The `callsSummarized` counter is
  replayed exactly, so output is byte-identical.

`MIDWRITE_PATHS=legacy|compact|shared` selects the mode; the three modes
must produce identical output, which the corpus identity check and
`MIDWRITE_PATHS_DIFFERENTIAL=1 midwrite-corpus run` (every function
enumerated in each mode and compared event by event) verify.

**2. Replay and points** (`detect.rs`). Each trace is replayed with a stack of
open transactions. The replay produces the trace's *points*, the moments at
which something becomes durable or visible:

* a write on the root handle → a **commit** now (noting the enclosing
  transaction it bypasses, if any);
* a write on a transaction handle → **staged** in its root transaction
  (resolved through `TransactionInfo.parent`, so savepoint writes belong to
  the outermost transaction); a rolled-back savepoint discards its writes;
* the successful end of a root transaction with at least one staged write →
  a **commit** of all of them;
* an external effect → **visible** now, remembering which root transactions
  were open.

**3. Hazards.** A failure between two *adjacent* points leaves the first done
and the second not. Each adjacent pair is classified by kind (commit/visible ×
commit/visible → MW1001, MW1002, MW1004, MW1005). Only adjacent pairs are
reported: `A B C` yields the windows `A|B` and `B|C`, not `A|C`, because a
failure anywhere is already described by the window it falls in. An external
effect that happens while a root transaction with writes is open is MW1003
(the pair "effect, that transaction's commit" is not also reported as MW1002).
Unknown effects become MW9001 when something durable or visible precedes them
on the path, when they run inside a transaction with writes, or when they
certainly involve the database or a configured receiver.

Operations inlined from a summary take part like the caller's own, with two
rules that avoid repeating the callee's findings in every caller: a pair (or
a MW1003 effect and transaction) whose operations both come from the same
inlined call is skipped, because the callee's own analysis reports it; and an
unknown effect inside an inlined call is reported only when the nearest
preceding point lies outside that call, or when it runs inside a transaction
of the caller with writes. Diagnostics about inlined operations are located at
the outermost call in the analyzed function and list the operations' own
locations, the call (role `callSite`) and the summary as evidence.

**4. Selection and confidence.** The same hazard (same code and same pair of
operations) usually appears on many traces. The trace with the highest
confidence, then the fewest branch decisions, then the earliest enumeration
order is kept, which makes output deterministic. Pairs that share their
primary operation (same reported location and label) are then merged into
one diagnostic that lists the other partners. Unknown calls inside one
inlined call are grouped (one MW9001 per call), and under the `recommended`
policy all context-only unknown calls of a function are grouped. The path
certainty starts `straightLine` and confidence `high`; both are lowered by
what the window between the two points contains:

| Evidence | Path certainty | Confidence |
| --- | --- | --- |
| branch decisions between the two points, a loop, an unawaited operation or transaction | `conditional` | at most `medium` |
| an operation taken from the summary of a callee that performs it only on some of its paths | `conditional` | at most `medium` |
| concurrent operations, or a path that enters a catch handler by approximation | `approximated` | `low` |
| MW9001 limitations | `unknown` / `truncated` | always `low` |

Then the weakest recognition evidence of the operations caps the confidence:
a member-path match (`memberPath`) never yields more than `medium`.

Decisions that happen *before* the first point (for example a guard clause)
do not lower confidence: once the first point has happened, the second follows
unconditionally.

**5. Rendering** (`render.rs`). Each finding becomes a `Diagnostic` with a
message, primary location, related effects with roles, a numbered failure
sequence, path conditions, evidence, mitigation and limitations, its
`pathCertainty` and `evidenceStrength`, its severity under the policy, and a
fingerprint (FNV-1a of code, file, function and the related effects' roles
and labels; no positions). Locations in other files are printed with their
path.

## Determinism

Files are discovered with sorted directory walks and de-duplicated; functions
are lowered in source order; summaries are inlined in source order with
identifiers allocated in traversal order; traces are enumerated in a fixed order; findings
are keyed in a `BTreeMap`; the final report is sorted by file, position, code
and related locations. No hash-map iteration order reaches the output.
Files are lowered and analyzed on up to 8 threads
(`MIDWRITE_THREADS` overrides); results are collected by index, so the
output does not depend on scheduling (checked by tests and by comparing
corpus runs).

## Extending

* **A new ORM or effect provider** belongs in the frontend's recognizer: map
  call chains to `EffectKind`s with a `Recognition`. The IR and analysis do not
  change. A provider abstraction will be introduced once a second provider
  exists, rather than designed speculatively now.
* **A new hazard** is a new classification of points or a new replay rule in
  `detect.rs`, plus a catalog entry in `diagnostic.rs` and fixtures.
* **A new language** is a new frontend producing `FunctionIr` (and, for call
  summaries, the call sites per function with their argument handles).

## Source identity and original-code verification

Function identity by source position is shared with the concurrency
extractor (`analyzer::identity`); wrapper invocation analysis
(`analyzer::wrapper`), the original-code coordinator
(`midwrite-replay::source`, `runtime/node/source.js`) and the differential
comparison (`midwrite-model::source_verify`) belong to the concurrency
pipeline: [source extraction](architecture/source-extraction.md),
[runtime verification](architecture/runtime-verification.md).
