# State-space and analysis optimization

Every change below was chosen from a profile (`sample`, 1 ms interval) and
validated for exactness before it was kept. None changes a result: states,
transitions and verdicts of every model, and every byte of `check`/`trace`
output, are identical to Phase 7.

## Model checker

Profile of the exhaustive 6-actor team-app workload (hypothetical
`UNIQUE(team_id, email)`, ≈ 97 000 states), before:

| share | where |
| --- | --- |
| largest | `engine::apply` cloning the whole `State` per transition: `String` clones and `BTreeMap<String, Value>` subtree clones — every actor's **locals** and every row's **version chain** were deep-copied, although a transition writes the locals of one actor and a few rows |
| next | the IR `Step` of the executing operation cloned per step (`Step::clone`, `Expr::clone`) |
| ≈ 15–19 % | SipHash over canonical state encodings in the visited set |
| rest | canonical encoding, frame drop |

Changes:

1. **Copy-on-write actor locals** — `ActorState.locals` and
   `BeginInfo.locals` are `Arc<Locals>`; writes go through
   `Arc::make_mut`. Equality and the canonical encoding see the contents,
   so state identity is unchanged.
2. **Copy-on-write row chains** — `Store.rows` holds
   `Arc<Vec<Version>>`; commit, abort and write paths use `make_mut`.
3. **No IR clone per step** — the executing `Step` is borrowed from the
   model (it never needed to be owned).
4. **Fast hasher for the visited set** — an Fx-style hasher; the keys are
   canonical encodings produced by the explorer, iteration order never
   reaches output.

Results ([`PERFORMANCE.md`](PERFORMANCE.md)): exhaustive exploration
2.0–2.4× faster from 4 to 6 actors with identical states and transitions;
peak memory unchanged (dominated by the visited set). Not attempted:
state interning beyond the visited set, symbolic resource identities,
relevant-state slicing — the profile did not point at them.

## Partial-order reduction under crashes and retries

Crash and retry transitions keep a `Global` footprint: they are dependent
with every transition, so the sleep-set reduction never commutes a crash or
retry with an effect, a commit, a lock wait or a new execution. New Phase 8
check (`reduction_reaches_every_state_of_the_full_search_with_crashes_retries_and_effects`):
for every Phase 8 model (crashes, retries, at-least-once redelivery,
guaranteed restarts, effects under `none`/`byKey` deduplication, lock
waits, generated ids, 40001 retries, two drivers), the reduced search
reaches exactly the states and violating states of the full search, with no
more transitions, and the independence audit finds no violation. The
Phase 6/7 POR suites (targeted scenarios, randomized scenarios with stored
seeds, the independence audit on the workloads) pass unchanged.

## Static analysis

Profile of school-ledger at depth 8 (single thread, repeated):

| share | where |
| --- | --- |
| ≈ 23 % | path enumeration, of which `merge_equivalent` ≈ half (SipHash of `Vec` keys) |
| ≈ 12 % | the IR inliner, dominated by instantiation copies (`Remap::node`), especially `Remap::origin` re-cloning the chain of `CallSite`s (two strings and a boxed resolution each) for every effect at every level |
| ≈ 11 % | detection |
| — | `call_shape` of the first shared-summary version (removed by gating, see below) |

Changes:

1. **Depth-canonical inliner memo** with exact counter replay
   ([`PARAMETERIZED-SUMMARIES.md`](PARAMETERIZED-SUMMARIES.md)).
2. **Shared path summaries** across corresponding call instances.
3. **Gated, cached shape hashing** — only callees called at least twice in
   a function are hashed; hashes are cached per call scope and nested
   bodies are hashed once (the first version, hashing every call,
   cost as much as it saved).
4. **Shared IR strings** — `CallSite.callee/label/resolution`,
   `CallScope.callee/label` and `Effect.label` are `Arc`: an instantiation
   copies a pointer instead of a string (serialization unchanged, serde
   `rc`).
5. **Per-summary facts computed once** — the effect-id order and the
   number of call scopes of an expansion, recomputed at every
   instantiation before, are stored in the memoized summary.
6. **Fast hasher** for the path walker's internal maps.
