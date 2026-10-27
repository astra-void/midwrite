# Reduction architecture (Phase 10)

```text
                   engine.rs: enabled(s), apply(s, t) → (s', footprint)
                                    │
      ┌───────────────┬─────────────┼───────────────┬──────────────────────┐
      ▼               ▼             ▼               ▼                      ▼
 unreduced DFS/BFS  sleep sets   classic DPOR    source-set DPOR        audit.rs
 (explore_with,     (explore_with (explore_dpor; (explore/sdpor.rs;     (independence,
  por=false)         por=true;    Phase 9, every  cache, sleep, source   enabling pairs,
  the ORACLE         DEFAULT)     race since P10) sets, enabling rule)   per state)
      │               │             │               │
      └──── state table: exact canonical keys (keytable.rs, interned fragments) ────┐
                                                    │                              │
                                     per-node reduction metadata, NOT in the key:  │
                                     summary id, steps explored at, sleep set ─────┘
```

## Layers

| layer | file | what it decides |
| --- | --- | --- |
| transition semantics | `engine.rs` | which transitions are enabled, what they do, which resources they touch (footprints) |
| dependence relation | `footprint.rs` | which footprints conflict (unchanged in Phase 10) |
| enabling rule | `explore/sdpor.rs::enabling_event` | the event after which a transition became enabled is dependent with it (not in the classic search) |
| state identity | `state.rs::canonical_key`, `keytable.rs` | exact equality of states; hashes only index |
| equivalence for comparing searches | `state.rs::canonical_observable` | terminal states equal modulo the interleaving of effects on different channels |
| reduction metadata | `explore/sdpor.rs::NodeInfo`, `SummaryPool`, `Descriptors` | what a completed exploration of a state covers |
| property gate | `explore.rs::dpor_inapplicable` | refuses DPORs for `always` invariants and context-switch bounds |
| test oracle | `tests/common/por_diff.rs` | what a reduced search must agree on with the unreduced one |

## Selection

| how | search |
| --- | --- |
| default | sleep sets (unchanged) |
| `--no-por` / `"por": false` | unreduced |
| `--dpor` / `"dpor": true` | classic DPOR (report schema 4, `reduction: "dpor"`) |
| `--source-dpor [stateful-sleep\|stateful\|stateless-sleep]` / `"sourceDpor": "..."` | source-set DPOR (report schema 5, `reduction: "source-dpor"`, `sourceDporMode`, `exploration.reductionMetrics`) |

A DPOR that is refused runs sleep sets, and the report says why
(`bounds.partialOrderReductionDisabledBecause`).

## Why sleep sets stay the default

See `STATE-SPACE-PERFORMANCE.md` §Decision. Source-DPOR (stateful+sleep) wins on
most of the measured workloads, and all correctness evidence agrees. The
evidence base for it is one phase old and found three defects in DPOR
variants on its way; the default stays the search whose soundness argument is
the textbook one and which covers `always` invariants as well.

## Profiling hook

`COMMITRIFT_DPOR_PROFILE=1` with `--dpor` prints, on stderr, the classic
search's profile (`explore_dpor_profiled`): distinct states, trace signatures,
transitions below revisited states, backtrack points by origin. It keeps side
tables of every state and trace, so it is a measurement tool
(`BASELINE.md`), not a mode.
