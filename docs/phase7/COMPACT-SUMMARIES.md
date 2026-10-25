# Compact call-scope summaries

`crates/commitrift-core/src/analysis/paths.rs` (`PathMode`, `call_compact`),
`analysis/mod.rs` (`compare_path_modes`, differential hook).

## Problem

The static analyzer inlines callees into the caller's Effect IR (Phase 2/5,
memoized per callee and context). The path walker then sends **each** trace
that reaches an inlined call through the callee's body separately and
merges the results by dominance. With *k* incoming traces, a callee body is
walked *k* times; nested calls multiply. Phase 6 measured that this, not
summary construction, dominates the cost of deeper call chains.

## Representation

For an inlined call scope (one `Node::Call` instance) and the effect
context it is reached in (innermost loop, `try` guard, handler), the
compact summary is the callee's behavior from an *empty* trace, grouped by
exit exactly as the legacy walk groups it after leaving the call:

* `normal` — trace fragments that fall through or `return` (with
  `Returned` facts for bound return values), merged by the same dominance
  rule as the legacy walk;
* `returned` — fragments that reached unsupported control flow
  (`Incomplete`), kept apart;
* `thrown` — fragments leaving by exception (with the transaction
  rollback events of a failing transaction callback), merged;
* the pruned/merged counts and whether the walk was truncated.

A fragment is an ordered list of events: effects (with locations, context,
summary origin), transaction begin/end (committed or rolled back), branch
decisions (path predicates through their facts), returns with facts,
unsupported exits. Exceptional and normal completions are never combined.
At each call site the walker appends every fragment to every incoming
trace (in the legacy order) instead of walking the body again.

This is an exit-sensitive set of trace fragments, i.e. a deduplicated
unfolding of the callee's effect automaton; a DAG-shared encoding was not
needed for the measured gains.

## Why it is exact

Walking a callee depends on the incoming trace in exactly two ways:

1. the walker's effect context (enclosing loop/try) — part of the key;
2. branch feasibility: an arm is dropped if its facts contradict a fact of
   an earlier decision *on the same trace*. Facts contradict only when they
   are about the same variable (`Fact::Never` contradicts everything but
   depends on nothing).

So when no fact variable of the callee is mentioned by a fact outside it
(the walker's `live` set for the call is empty), every incoming trace sees
the same pruning, the same fragments, the same dominance merges (computed
on the suffix only) and the same truncation. The compact summary is used
only then; otherwise the call is walked the legacy way (counted as
`shared_facts`). Statistics (`pruned`, `merged`, `truncated`) are accounted
per incoming trace, as the legacy walk accounts them, so even the analysis
summary in reports is unchanged.

## Evidence

* **Corpus differential** (`COMMITRIFT_PATHS_DIFFERENTIAL=1
  commitrift-corpus run`): every function analysis enumerated in both modes
  and compared event by event (IR identity, arm, context) plus
  truncation/pruned/merged: **22 022 function analyses at each of depths 3,
  6 and 8, 0 differences** on saas-starter, midday, school-ledger and
  unnamed-ai-project. Summaries built / incoming traces spliced / calls
  walked legacy because of shared facts: 6 830 / 23 428 / 156 (depth 3),
  14 561 / 40 808 / 221 (depth 6).
* **Fixture differential**: `compact_summaries_equal_the_legacy_walk_on_every_fixture`
  (all fixture directories, depths 3 and 6, with and without traces).
* **End to end**: `commitrift check` (json, sarif, text) and `commitrift
  trace` byte-identical to the Phase 6 binary on all four projects at depths
  3 and 6, in both path modes (64 comparisons,
  `reports/phase7/corpus-identity.txt`).

Because of this, compact summaries are the default;
`COMMITRIFT_PATHS=legacy` restores the legacy walk.

## Performance

[`PERFORMANCE.md`](PERFORMANCE.md): path analysis −32 % to −55 % on
midday and school-ledger at depths 3–8; school-ledger at depth 4 with
compact summaries (142.8 ms) costs less than the legacy walk at depth 3
(148.9 ms). The default depth stays 3: deeper analysis changes findings
(school-ledger 332 → 593 at depth 6), which still needs the reviewed
policy decision Phase 5 asked for.

## Limits

* Summaries are per inlined call *instance*: two call sites of the same
  callee are summarized separately (their inlined bodies are distinct IR).
* A callee whose facts interact with the caller's facts is walked the
  legacy way (exactness over speed).
* Lowering and resolution (61 % of midday's time) are untouched.
