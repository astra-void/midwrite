# Track E: source and framework support added in Phase 15

Only constructs shown to block verification in the corpus or the
evaluation sets were added.

| construct | before | Phase 15 | where |
| --- | --- | --- | --- |
| a type alias that is a union of string literals (`kind: RouteKind`), same module or imported (relative imports, `export *`, `export { X } from`) | the argument got placeholder text (`"kind-1"`), which the signature does not admit | members are the argument's domain | `js_values.rs` `literal_union`, `imported_literal_union`, `alias_in_module`; `synth.rs` `arguments` |
| optional parameters/members (`x?: T`, defaults) | not recorded | `undefined` is in the domain | `TsActorProvider::param_signature` |
| a guard testing a column of a row read earlier, then a relative UPDATE of it (`if (q.remaining <= 0) throw; … set remaining = remaining - 1`) | no candidate (`rmw_sites` skips atomic writes) | `guarded-update` candidates (check-then-act class), **only with a declared property** over the table | `detect.rs` `guarded_update` |
| a declared `atMostOne` key column the check tests conditionally (`parentId === null ? isNull(...) : eq(...)`) | the key was unknown, "no property could be instantiated" | the value the entry's own INSERT writes to the column | `detect.rs` `declared_properties` |
| guards over arguments and read rows (authorization) | synthesized inputs failed them: INCONCLUSIVE | satisfied with stated preconditions | `synth.rs` (`CONSTRAINT-SYNTHESIS.md`) |
| where a solo run stopped | not reported | `(the actor ended after <location> with: <failure>)` in the unsatisfiable message | `synth.rs` `stopped_at` |

## Not added (and why)

| construct (Phase 15 brief) | status |
| --- | --- |
| `await req.json()` request bodies (E2) | not modeled; Phase 14 set E34 still blocked. Needs a declared input contract format; not started |
| `Response.json(...)` (E3) | not modeled |
| bounded `for`/`while` loops (E4) | not added; Phase 15 set E24 and Phase 14 set E12 (retry helpers written as `for` loops) remain blocked: "`for` loops are not part of the supported concurrency subset" |
| zod `uuid`, `email`, `regex`, `optional`, `nullable`, `default`, arrays, `strictObject` (E5) | not added. `z.strictObject(...)` is the **new first blocker of all 7 school-ledger candidates** that previously failed the authorization guard; modeling it faithfully needs the object's own keys (a new model builtin) |
| `Date#getTime()`, time arithmetic (E7) | not added (Phase 14 set E26 still blocked) |
| a predicate hoisted into a local (`const inFolder = cond ? isNull(..) : eq(..)`; `.where(and(..., inFolder, ...))`) | not modeled ("predicate is not a Drizzle operator call"); Phase 15 set E06 |
| multi-row `FOR UPDATE` without `.limit(1)` | not modeled; Phase 15 set E10 |
| generated identifiers in properties (midday `portalId = nanoid()`) | unchanged: serial outcomes that are generated values cannot be written as invariant terms; the candidate stays INCONCLUSIVE with this reason |
