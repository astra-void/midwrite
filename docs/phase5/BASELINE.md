# Phase 5 baseline (frozen before any analysis change)

Recorded on 2026-10-25 before modifying analysis code. Raw logs:
`reports/phase5/baseline/`, `reports/phase5/corpus/*phase4*`,
`reports/phase5/perf/*phase4*`.

## Source revision

* Git: branch `main`, **no commits**, every file untracked (as at the end
  of Phase 4). There is no revision hash; a tarball of the tree (without
  `target/`, `.corpora/`, `node_modules/`) was saved outside the repository
  before any change.
* The Phase 4 release binaries were preserved as
  `.corpora/bin/commitrift-phase4` and `.corpora/bin/commitrift-corpus-phase4`
  (`cargo build --release` was a no-op on the untouched tree, so they are
  the Phase 4 code). All Phase 4 → Phase 5 comparisons use them.

## Environment

| | |
| --- | --- |
| Machine | Apple Silicon Mac, macOS (same machine as Phase 4) |
| Rust | rustc 1.98.1, cargo 1.98.1 (MSRV 1.97) |
| Linux | not re-run at baseline (Phase 4 recorded 245/245 in a Debian 12 container) |

## Quality gates

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --workspace` | **245 passed, 0 failed, 0 ignored** (replay tests executed, none skipped) |
| `cargo build --release` | ok |

## Corpus

`commitrift-corpus run` with the Phase 4 binary on the pinned corpus
(saas-starter `6e33e58b`, midday `51587319`, school-ledger `4b97f34`,
unnamed-ai-project `39fcfc7`) produced a report **byte-identical** to
`reports/phase4/corpus/corpus-phase4.json`.

| Project | Files | Functions | Diagnostics | Traces | Complete | Approximated | Incomplete |
| --- | --- | --- | --- | --- | --- | --- | --- |
| saas-starter | 41 | 156 | 21 | 15 | 14 | 1 | 0 |
| midday | 2022 | 14827 | 269 | 180 | 126 | 7 | 47 |
| school-ledger (private) | 462 | 6391 | 317 | 123 | 30 | 0 | 93 |
| unnamed-ai-project (private) | 104 | 648 | 17 | 8 | 8 | 0 | 0 |

Every incomplete trace (140) is incomplete because of at least one unknown
call inside its failure window; none because of unsupported control flow.

## Unknown calls in failure windows (Milestone 9.1)

`commitrift-corpus traces --unknowns <file>` (added for this phase; the TSV
stays under `.corpora/private/` because it contains source labels of the
private projects) lists each unknown step inside the window of an
incomplete trace with the unknown effect's reason and detail, and assigns
an automatic, purely descriptive category.

Automatic categories (occurrences; one call can sit in several traces):

| Category | midday | school-ledger |
| --- | --- | --- |
| resolved function without recognized effects | 24 | 166 |
| resolved function beyond the depth bound (3) | 7 | 155 |
| method call on a value | 21 | 1 |
| plain call of an unresolved binding | 13 | 23 |
| method chain on a call result | 0 | 23 |
| call through an expression | 10 | 0 |
| method call on `this` | 5 | 0 |
| third-party package (no source) | 1 | 25 |
| resolved function, other reason | 1 | 1 |
| **total** | **82** | **394** |

Manual classification of the 82 midday occurrences into the Milestone 9.1
categories, from reading the pinned midday source (public):

| Milestone 9.1 category | Occurrences | Calls |
| --- | --- | --- |
| Resolvable local function: immediately-invoked async arrows inside `Promise.all` | 10 | 2 IIFEs in `process-attachment.ts` |
| Resolvable imported/local function, called as a `Promise.all` element (Phase 4 never summarizes concurrent calls) | 13 | `getTeamCalibration`, `getTeamById`, `getTeamOwnerContact`, `getInboxById`, `getTransactionById` |
| Resolvable method call | 10 | `this.updateProgress` (inherited from a base class in another module) 5; methods of `new Embed()`, `new DocumentClient()`, a factory result, an object service delegating to a class instance 5 |
| Higher-order wrapper | 8 | `withTimeout(promise, ms)` |
| Callback-dependent / closure | 7 | `removeProcessingReaction` (local closure calling the Slack SDK) 5; `config.afterConnect` (callback property of a parameter: dynamic) 2 |
| Known external effect with missing configuration (SDK calls) | 14 | Supabase storage/query 9, Slack 2, Trigger.dev 3 |
| Third-party package without source / runtime semantics | 6 | `render` (returns `@react-email/render`'s result) 4, `generateText` (`ai`) 1, `Blob.arrayBuffer` 1 |
| Resolved function, other effects unknown | 6 | helpers whose bodies call SDKs or return package calls |
| Resolved function beyond the depth bound | 7 | `getAppConfig`, `shouldSendNotification`, two query helpers |
| Database handle argument not recognized | 1 | `createShortLink` |

The school-ledger categories were reviewed locally; only counts are
published. Its two largest groups are effectful audit helpers reached more
than three calls deep (the depth bound, not a missing technique) and
test-support helpers without recognized operations.

**What this means for Phase 5:** no single technique applies. Concurrent
call summaries and IIFE resolution address 23 midday occurrences; method
resolution 10; higher-order/param-usage summaries 8; an
effect-freedom summary is needed before any "resolved function without
recognized effects" can be removed — and several of those (e.g. `render`,
`removeProcessingReaction`) are *not* effect-free and must stay unknown or
move the uncertainty to the real SDK call. 21 occurrences are SDK or
package calls that only configuration could make known.

## Where source-level information is lost (Phase 4)

| Loss | Location | Consequence |
| --- | --- | --- |
| No values: only database handles and `const` aliases are tracked | `bindings.rs`, `recognize.rs` | callbacks, wrappers, receivers, return values unknown |
| Calls are linked only by `Identifier` / `obj.method` syntax | `lower.rs::call_target` | IIFEs, `this.m()`, `new C().m()`, callback parameters unknown |
| `Consumer::Concurrent` never summarizes | `lower.rs::summarizes_local_calls` | every resolved call inside `Promise.all` stays unknown |
| A callee without recognized operations is never used | `summary.rs::call_site` | inert helpers stay unknown, *but* an async `return pkg()` in a callee is not recorded at all, so "no unknown effects" does not prove inertness |
| Arguments: only handles are bound | `CallArgument` | no provenance across calls; lazy builders passed to wrappers are not executed |
| Callee return values are dropped | inliner | no correlation through helper results |
| Effects record argument *names* only | `Effect::references` | relationships are name-based and not comparable across calls |
| Loops: 0/1 iteration, no loop-carried state | `paths.rs` | — (no value state existed) |

## Performance (Phase 4 binary, this session)

`scripts/bench.py`, 7 runs after a warm-up, median; peak RSS of the child
(`reports/phase5/perf/bench-phase4-{8t,1t}.txt`):

| Project | Wall 8 threads | RSS 8 threads | Wall 1 thread | RSS 1 thread |
| --- | --- | --- | --- | --- |
| saas-starter | 0.0086 s | 7.1 MiB | 0.0100 s | 6.4 MiB |
| midday | 0.1628 s | 66.9 MiB | 0.3768 s | 54.2 MiB |
| school-ledger | 0.1213 s | 73.2 MiB | 0.2523 s | 60.7 MiB |
| unnamed-ai-project | 0.0108 s | 8.4 MiB | 0.0157 s | 7.4 MiB |

Live heap after each phase, 1 thread (`memory-phases-phase4.txt`): midday
11.8 / 14.2 / 17.6 / 20.7 MiB (lowered / resolved / summarized /
analyzed), peak 27.4 MiB, 27 278 retained IR nodes; school-ledger 6.9 / 8.7
/ 20.1 / 22.8, peak 26.9 MiB, 40 358 nodes. `size_of::<Node>()` = 176 bytes.
