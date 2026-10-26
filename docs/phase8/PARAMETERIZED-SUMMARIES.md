# Parameterized summaries reused across call sites

`crates/commitrift-core/src/analysis/summary.rs` (IR inliner),
`crates/commitrift-core/src/analysis/paths.rs` (path summaries).

## What already existed, and what was missing

The audit found that the static analyzer already had a parameterized,
cross-call-site summary at the IR level: the inliner expands a callee once
per `(callee, remaining depth, callback context)` and **instantiates** the
expansion at every call site (`Remap`): symbolic parameter handles are
substituted (an operation on parameter `database` takes the caller's `db`
or `tx`, a transaction on a parameter becomes a nested transaction of the
caller's), abstract argument values replace `Param` atoms, effect ids,
call ids and transaction ids are fresh, fact variables are prefixed per
call, the callee's `$ret` facts are renamed to the caller's binding,
locations are remapped to the callee's file. So `save(db, id)` and
`save(tx, id)` already share one expansion with different transaction
semantics (tested since Phase 5).

Two things were per-instance:

1. the inliner's memo key contained the **remaining depth**, a constant
   that is irrelevant for any expansion that never reached the bound, so
   the same callee was expanded again at every depth it was called from;
2. the Phase 7 **path summaries** (compact summaries) were keyed by the
   address of the inlined `CallScope`: two instantiations of the same
   expansion were walked separately.

## 1. Depth-canonical summary keys

Each expansion records `needed` — the smallest remaining depth at which
every call-site decision in it is the same — and `exact`, set when any
decision was made *because* the depth was 0 (a cut call, and also an inert
callee resolved without expansion, which at a larger depth would be
expanded). A non-exact expansion is valid at every depth ≥ `needed`, so it
is stored under `(callee, callback context)` and reused for any such depth.

Exactness of the output was required (the analysis output is
byte-identical). One report field depends on the work done, not on the
result: `summary.analysis.callsSummarized` counts call sites summarized
during *fresh* expansions under the exact-depth memo. To keep its value,
each memoized summary records the call sites it summarized itself and the
memo requests its call sites made; a canonical reuse **replays** that
bookkeeping exactly as the exact-key memo would have done (inserting the
keys it would have inserted). The memo key sets of the two strategies are
identical by construction, and the counter is identical on the corpus.

## 2. Path summaries shared between call instances

A compact path summary is the set of trace fragments a call body produces
from an empty trace (Phase 7: valid when no fact variable of the call is
mentioned outside it). The walker's behavior inside a call depends only
on: node kinds and nesting, whether effects are reads, the facts of arms,
cases and exits (contradiction needs a common variable), exit kinds, loop
and `try` shapes and transaction nesting. The enclosing effect context is
only *recorded* in events, never branched on (audited in `paths.rs`).

So for two call scopes with a **structure-preserving correspondence** — a
bijection of every node the walker can put into a trace, of transaction
ids and of fact variables — walking one makes exactly the decisions walking
the other makes, and the first's summary mapped through the correspondence
*is* the second's summary. `PathMode::Shared` (default) does this:

1. callees with at least two call scopes in the function are candidates;
   their body shape is hashed (variables numbered by first occurrence,
   nested calls hashed once and cached);
2. for a call whose summary is not built, a summarized call with the same
   callee and shape is a candidate; `Corr::nodes` verifies the
   correspondence node by node (`is_read`, fact variants and literals,
   variable and transaction **bijections**, exit kinds, loop and try
   shapes) — the hash only selects candidates;
3. the summary is instantiated by mapping every event; context slots that
   point inside the call map through the correspondence, the context the
   summary was built in maps to the current context; anything that does
   not map aborts the instantiation (the call is walked).

Statistics (pruned/merged/truncated) are copied, as the per-trace
accounting of compact summaries requires.

## Context keys, instantiation, recursion, demand

* **Context keys** are minimal: `(callee, callback context)` at the IR
  level (depth only when the expansion depends on it); the call's
  structure (not its address) at the path level. Constants of arguments
  are not part of the key: they reach the summary only through facts,
  which the correspondence compares exactly.
* **Specialization cost** stays bounded by the existing
  `MAX_SPECIALIZATIONS` (8 callback contexts per callee); exceeding it
  falls back to the context-free expansion, as before.
* **No shared mutable state between instantiations**: expansions are
  immutable (`Rc<Summary>`); every instantiation produces fresh nodes;
  instantiated path summaries are new values.
* **Recursion and cycles**: recursive expansions are cut and never
  memoized (unchanged); inertness is the Phase 5 monotone fixpoint over
  SCCs (`absint::summary::solve`), which terminates on recursion, mutual
  recursion and callback cycles; a recursive call that is not proven inert
  stays an unknown effect, so the uncertainty is visible (CP9001).
* **Demand-driven construction**: expansions are computed only when a call
  site asks for them; path summaries only for calls a walk reaches;
  shapes only for callees called at least twice. Functions are still all
  analyzed by `commitrift check` (that is its contract).

## Exactness evidence

* `COMMITRIFT_PATHS_DIFFERENTIAL=1 commitrift-corpus run`: every function
  analysis enumerated in the legacy walk, the Phase 7 compact mode and the
  Phase 8 shared mode, compared event by event (IR identity, arm, context)
  plus truncation/pruned/merged: **22 022 function analyses at each of
  depths 3, 6, 8 and 12, 0 differences** in both comparisons; in the shared
  mode 860 (depth 3) to 2 409 (depth ≥ 8) call summaries were instantiated
  through a verified correspondence instead of being walked, and no shape
  candidate was ever rejected (`reports/phase8/path-differential.txt`).
* Byte identity of `check` and `trace` with the Phase 7 binary in all three
  path modes and at depths 3, 6, 12 (144/144).
* The fixture differential test (`compact_summaries_equal_the_legacy_walk_on_every_fixture`)
  now compares both modes with the legacy walk.

## Performance

See [`PERFORMANCE.md`](PERFORMANCE.md): expansions −20 % to −36 %,
summary construction −7 % to −25 %, path analysis up to −20 % on the
projects where call chains matter (school-ledger, midday). Profiling showed
that expansions were not the dominant cost — instantiation copies were —
which led to the shared label strings and cached per-summary facts
described in [`STATE-SPACE-OPTIMIZATION.md`](STATE-SPACE-OPTIMIZATION.md).

The default call depth stays 3 (unchanged); deeper analysis still changes
findings and needs a reviewed policy decision.
