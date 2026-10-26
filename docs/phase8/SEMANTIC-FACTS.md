# Shared semantic facts

`crates/commitrift-analyzer/src/semantic.rs`.

## Purpose and scope

Three components read TypeScript: the static analyzer's lowering
(`lower.rs`, `recognize.rs` → Effect IR), the concurrency extractor
(`concurrency.rs` → the model's IR) and, new in Phase 8, the Drizzle schema
extractor (`schema.rs`). The shared layer unifies **recognition** — what a
piece of syntax *is* — and leaves **semantics** to each consumer: the static
analyzer turns a builder chain into a sequential effect with failure paths,
the extractor into a concurrent state transition. Forcing one execution
semantics on both was explicitly not a goal.

## Facts

| fact | API | consumers |
| --- | --- | --- |
| a call/member chain: root (identifier or `this`), property and call segments, looked through parentheses, TypeScript `as`/`!`/`satisfies` and optional chaining — with `optional` recording that an optional chain was crossed | `unwind`, `Chain`, `Segment`, `Root` | `recognize.rs` (all Drizzle and external-effect recognition), `concurrency.rs` (via `flatten`), `schema.rs` |
| the chain as method calls with the member path before each (`db.query.users.findFirst` → path `query.users`, method `findFirst`), and the root's own call (`pgTable(...)`) | `Chain::method_calls`, `Chain::root_call`, `Chain::root_name` | `concurrency.rs`, `schema.rs` |
| a property key's static name | `property_key` | `concurrency.rs` (was `static_key`), `schema.rs` |
| a dotted identifier/static-member path | `member_path` | `concurrency.rs` (was `callee_text`), `schema.rs` |
| a literal string (plain or template without substitutions) | `string_value` | `schema.rs` |

## Migration, category by category

| category (task order) | status | how it was checked |
| --- | --- | --- |
| 1. function and callback identities | already shared since Phase 7 (`identity.rs`, `wrapper.rs`) | — |
| 2. call target resolution — chain decomposition | **migrated**: `recognize::unwind` moved into `semantic.rs` (the static analyzer re-exports it unchanged); `concurrency::flatten` rebuilt on `unwind` + `method_calls`, keeping its stricter acceptance (it refuses optional chains, `this` receivers and a directly called root, as before) | corpus identity (144/144 byte-identical outputs), extraction identity (17/17 scenario reports identical), unit tests in `semantic.rs` |
| 2b. syntax facts (keys, paths) | **migrated** for the concurrency extractor and the schema extractor | same |
| 3. abstract values and returns | not migrated: the static analyzer's abstract domain (`core::absint`) and the extractor's expression lowering (`model::value::Expr`) serve different purposes; no shared code without a behavioral change |
| 4. transaction handles | not migrated (same reason; the extractor's `Binding::Tx`/`Db` and the analyzer's `DbHandle` carry different information) |
| 5. recognized database effects | partially: both now see the same chain facts; the classification of a chain into an operation stays in each consumer |
| 6. async and exceptional completion | not migrated |

## Recorded differences

The migrations were designed to be behavior-preserving, and the
differentials found no difference. One semantic difference is known by
construction and is deliberate: `property_key` returns `None` for a string
key that is not valid UTF-8 text (an unpaired surrogate), where the
extractor's former `static_key` returned an empty name. Such a key is now
"not static" (an unknown property) instead of the property `""`.

## Evidence

* `scripts/phase8-identity.sh`: `check` (json, sarif, text) and `trace`
  of the Phase 8 binary vs the Phase 7 binary on saas-starter, midday,
  school-ledger and unnamed-ai-project at depths 3, 6, 12 in three path
  modes: **144/144 identical** (`reports/phase8/corpus-identity.txt`).
* `scripts/phase8-extraction-identity.sh`: every Phase 6/7 model-checking
  scenario (14 declared/extracted fixtures, the team-app and the original
  saas-starter extraction): **17/17 identical** reports
  (`reports/phase8/extraction-identity.txt`).
* Unit tests: `semantic::tests` (grouping, look-through, optional-chain
  flag, keys, paths, strings).
