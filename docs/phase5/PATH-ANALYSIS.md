# Path-sensitive analysis

Code: `crates/commitrift-core/src/analysis/paths.rs`,
`crates/commitrift-analyzer/src/lower.rs` (facts, loop fixpoint).

The Phase 1–4 design — a bounded enumeration of paths through the
structured IR, with per-path facts that discard contradictory paths — is
kept. It already is a path-sensitive abstract interpretation over a small
domain (sets of literal facts per path). Phase 5 extends the facts with
information from the value layer and adds a value fixpoint for loops; it
does not replace the walker.

## Abstract execution state

| Component | Where |
| --- | --- |
| variable environment, heap | value layer, while lowering (per program point; joined at merges) |
| path constraints | facts on each trace (`Decision`, `Returned` events) |
| transaction context, pending/completed effects | the trace replay in `detect.rs` (open transactions, staged writes, points) |
| exceptional state | flow buckets (`normal/returned/thrown/broke/continued`) |
| unknown semantics | `Unknown` effects and `Incomplete` exits on the trace |
| approximation flags | effect context (loop, catch entry), execution modes, assumptions |

## New facts

* **Member facts**: `x.p` in a condition is a fact variable `x#id.p` when
  `x` is never reassigned, every reference of `x` is a property read (no
  `x.p = …`, no `x.m()`, no passing `x` anywhere — nothing visible can
  change it), and `x`'s value has no accessors: an object literal of this
  function (not open), a record, a database row, or the result of an
  analyzed call. Objects from parameters or unknown calls are excluded
  (a getter could return different values on each read).
* **Return facts**: see [`INTERPROCEDURAL-SUMMARIES.md`](INTERPROCEDURAL-SUMMARIES.md):
  `if (result.ok)` after `const result = await saveOrder()` is checked
  against the facts of the return `saveOrder` took on that path.

Facts remain literal (in)equality, truthiness and object-ness, compared
pairwise; no constraint solving.

## Merging traces at call exits

When leaving an inlined call, traces that differ only in branch decisions
of the callee are merged (Phase 2 dominance rule). Phase 5 refines what may
be ignored: facts of `return` exits are part of the merge key when their
variable is **live after the call** — mentioned by a fact outside the call's
body. This was found on the corpus: keying by the `call#N:` prefix was wrong
once a callee is itself inlined (its caller's binding gets an outer prefix);
merging lost the path where a transaction committed and a CP1004 with it.
Regression test: `tests/phase5.rs::return_facts_of_nested_calls_never_merge_away_feasible_paths`.

## Loops

Effects are still modeled as zero or one iteration (`runs_at_least_once`
loops: exactly once), with the existing "only zero or one iteration is
modeled" note and medium confidence. The *value state* is iterated:

1. an optimistic pass lowers the body from the entry state; if the state
   after it is below the entry on every binding and object that existed at
   entry (bindings and objects created inside the body are fresh each
   iteration), the entry is a fixpoint and the IR is kept;
2. otherwise up to `MAX_LOOP_ITERATIONS` (3) value-only passes (IR
   discarded) compute `head := head ⊔ (entry ⊔ body(head))`, widening changed
   bindings and objects to `Top` from the second iteration;
3. a final check; if still not stable, the head state is havocked (every
   binding `Top`, every object escaped);
4. the body is lowered once from the fixpoint head; the state after the loop
   is head ⊔ body exit.

A per-function budget of 48 dry runs bounds nested loops; past it, loops are
havocked. Example (`tests/phase5.rs::loop_carried_values_are_joined_not_guessed`):

```ts
let action = first;
for (const item of items) { action = second; }
await action();   // {first, second}: two targets, not resolved
```

## State explosion controls and counters (corpus, final binary)

| | saas-starter | midday | school-ledger | unnamed |
| --- | --- | --- | --- | --- |
| abstract states (forks) | 304 | 39 005 | 13 287 | 1 438 |
| joins | 183 | 27 510 | 7 397 | 829 |
| bindings widened (joins + loops) | 1 | 1 086 | 153 | 5 |
| loop iterations / dry runs | 0 / 0 | 1 307 / 563 | 450 / 116 | 37 / 14 |
| loops widened / havocked | 0 / 0 | 0 / 0 | 1 / 0 | 0 / 0 |
| paths pruned (Phase 4 → 5) | 0 → 18 | 112 → 683 | 907 → 1 901 | 0 → 0 |
| paths merged (Phase 4 → 5) | 62 → 69 | 5 771 → 12 232 | 34 473 → 38 438 | 4 → 4 |
| functions over `--max-paths` (Phase 4 → 5) | 0 → 0 | 10 → 12 | 12 → 16 | 0 → 0 |

Truncation is reported (CP9001, partial coverage) as before; it rose by 6
functions because more calls are summarized. The additional pruning removed
no CP1001–CP1005 finding of Phase 4 on the corpus (every removed finding is
a CP9001; see [`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md)) — evidence,
not a proof, that no feasible hazard was hidden.
