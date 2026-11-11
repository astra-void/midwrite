# zod validation

What the concurrency extractor and discovery understand of
[zod](https://zod.dev) object validation, with which semantics, and what
is refused. Code: `crates/midwrite-analyzer/src/zod.rs` (schema reader,
version lookup), `concurrency.rs` (`zod_*` lowering),
`crates/midwrite-model/src/zodfn.rs` (schemas as values, `safeParse`
semantics, valid-input synthesis), `crates/midwrite-model/src/jsregex.rs`
(`RegExp#test` subset), `crates/midwrite-discover/src/{domains,synth,worlds}.rs`.
Active with JavaScript value semantics (always in `discover`).

The static analyzer (`check`) does not interpret zod; for it a `parse` is
an ordinary call.

## The zod version

zod 3 and zod 4 differ on semantics that matter to the model — the UUID
pattern (zod 3: any 8-4-4-4-12 hex string; zod 4: RFC 9562 versions 1–8
and variant `10`, plus the nil and max UUIDs), the email pattern,
`.default()` (zod 3 parses the default through the inner schema, zod 4
returns it as is), `.optional()` over a defaulted schema, `.passthrough()`
with `undefined` values, repeated array length checks, Maps and Sets as
object input, `.int()` (zod 4: safe integers only) and zod-4-only
constructors. The version is therefore read statically per import:

| import | installed/declared package | API modeled |
| --- | --- | --- |
| `zod/v3` | any | zod 3 |
| `zod` | 3.x | zod 3 |
| `zod`, `zod/v4` | 4.x | zod 4 |
| `zod` | unknown | *either* (below) |
| `zod/v4` | 3.x or unknown | refused (zod 3.25's early zod 4 implementation differs from zod 4.x) |

The major version comes from `node_modules/zod/package.json` found by
Node.js resolution from the importing file up to the project root, else
from the nearest `package.json` declaring `zod` in any dependency field;
`catalog:` references are followed (`package.json` catalogs and
`pnpm-workspace.yaml`). A range fixes a major only when it names one
(`4.6.5`, `^4.3.6`, `~3.22.0`, `3.x`, `npm:zod@^4`; not `>=3`, `*`,
`latest`, `^3 || ^4`, `workspace:*`). The namespace must be `import { z }`,
`import * as z` or `import z` from a zod specifier; a schema mixing
specifiers is refused.

With an **unknown** version, constructs whose semantics differ are refused
at extraction with the difference named; everything else is evaluated with
both semantics, and an input on which they disagree is an unsupported
evaluation.

## Supported subset

* **Constructors:** `z.string()`, `z.number()`, `z.boolean()`,
  `z.literal(string | number | boolean)`, `z.enum([...])` (string
  literals, a module-level `const` array of them in the module or
  imported, or spreads of such arrays), `z.object`, `z.strictObject`,
  `z.looseObject` (zod 4), `z.array(item)`, `z.optional(s)`,
  `z.nullable(s)`, `z.uuid()`, `z.email()`, `z.int()` (zod 4). An
  error-message argument is accepted.
* **String methods:** `min`, `max`, `length`, `nonempty`, `trim`,
  `toLowerCase`, `toUpperCase`, `startsWith`, `endsWith`, `includes`,
  `uuid`, `email`, `regex(/literal/flags)`.
* **Number methods:** `min`/`gte`/`gt`/`max`/`lte`/`lt`, `positive`,
  `nonnegative`, `negative`, `nonpositive`, `int`, `finite`.
* **Object modes:** `strict`, `strip`, `passthrough` (the last call wins).
* **Arrays:** `min`, `max`, `length`, `nonempty`, `.array()`.
* **On every schema:** `optional`, `nullable`, `nullish`,
  `default(literal)` (string, number, boolean or `null`).
* Integer arguments may be a module-level `const` integer; a template
  message is accepted when its interpolations are names or literals.

**Refused, with the construct named:** refinements (`refine`,
`superRefine` — accepting them would bypass validation), `transform`,
`preprocess`, `coerce`, `pipe`, unions, discriminated unions,
intersections, records, tuples, `extend`/`pick`/`omit`/`partial`/`merge`,
other string formats (`url`, `cuid`, …), `.uuid({ version })`, a
non-literal `.default()`, methods on an optional/nullable/defaulted
schema, shape spreads and computed keys, shape keys that are
`Object.prototype` properties, object inputs with an own `__proto__` key.

### `.regex()`

`jsregex` implements `RegExp.prototype.test` in ECMAScript's non-Unicode
mode over UTF-16 code units by backtracking (ordered alternatives,
greedy/lazy quantifiers, atomic lookaheads), bounded to 2,000,000 steps.
Supported: literals, `.`, `^`, `$`, `\b`, `\B`, alternation, capturing,
non-capturing and named groups, `(?=…)`, `(?!…)`, all quantifiers,
classes with ranges and negation, the common escapes; flags `i` (ASCII
patterns only), `m`, `s`, `g`, `y`, `d`. Refused: backreferences,
lookbehind, `u`/`v`, octal escapes and Annex B forms. zod's own UUID and
email patterns, copied verbatim from the package sources, run on the same
engine.

## Semantics

Read from `zod/v3/types.js` (3.25.76) and `zod/v4/core/*` (4.6.5):

* strings: checks in declaration order; `trim`/`toLowerCase`/`toUpperCase`
  rewrite the value for later checks and the output; non-ASCII case
  mapping refused;
* numbers: the model's integers; inputs outside ±(2^53 − 1) refused;
* objects: zod 3 accepts plain objects only (zod 4 also Maps and Sets:
  refused). **A missing key, an explicit `undefined` and `null` are
  distinct inputs.** Strict objects reject any extra own key (also one
  whose value is `undefined`); strip drops extra keys; passthrough copies
  them. Output key order is not modeled;
* arrays: `Array.isArray`, length checks (zod 3 keeps the last of each
  kind), every element parsed;
* `optional`, `nullable`, `default(d)`: as each version defines them;
* `parse` throws `ZodError` when the input is rejected; `safeParse` yields
  `{ success, data }` with `data` `undefined` on failure; `error` is not
  modeled (reading it is refused).

## Lowering

Validation is one step, `zok = ZodCheck(<descriptor>, zin)`, evaluated
with the version's semantics; on failure `parse` throws and `safeParse`
sets `success = false`. On success the parsed value is rebuilt from the
input as explicit steps, so values the code goes on to use stay traceable
to the input's paths: string transforms as calls, wrappers that change
`undefined`/`null` as branches, objects as literals of their
always-present fields plus `ObjectAssign` of optional fields under
`Object.hasOwn`, passthrough as `OmitKeys`. An array whose elements are
rebuilt is `ZodData(<descriptor>, zin)` (its elements are then opaque to
synthesis). The simple zod 3 subset of a non-zod-4 schema keeps explicit
`typeof`/length/check steps. The new built-ins (`HasOwn`, `OmitKeys`,
`ObjectAssign`, `ZodCheck`, `DefinedEntries`, `ZodData`) are serialized
only when used, and render as the zod chain they stand for.

## Input domains and synthesis

A `ZodCheck` whose input is a parameter path makes the schema the source
contract of that path and every field below it: kind (`uuid`, `enum`,
`string`, `integer`, `boolean`, `array`, `object`), whether `null` and
`undefined` are permitted, and the classes `absent` (the key may be
missing) and `present` (an optional key the base input omits). Discovery
makes validated arguments valid (`zodfn::conform`: required fields added,
keys a strict schema rejects removed, length and affix checks met, UUIDs
as version-4 variant-10 values, emails as `<name>@example.test`, `.regex()`
values as the pattern's shortest example; every result checked with
`zodfn::parse`) and states each in the scenario's synthesis notes
(`SYNTHESIZED input of …`), never as observed. Worlds `absent:<path>`,
`present:<path>`, `null:<path>`, `undefined:<path>` are explored where the
schema admits them ([scenario synthesis](../architecture/scenario-synthesis.md)).
Nothing here grants privileges: a guard the synthesizer satisfies is still
a stated precondition.

## Verification against the real zod

`crates/midwrite-analyzer/tests/zod.rs` runs every schema of
[`fixtures/apps/zod/matrix.json`](../../fixtures/apps/zod/matrix.json)
through the extractor and the model's evaluator, and through Node.js with
the pinned zod 3.25.76 and 4.6.5 (as `zod` and as `zod/v3`, and with an
unknown version using both as oracle), comparing the decision, the exact
parsed value (missing key ≠ `undefined` ≠ `null`) and whether `parse`
threw; a second test checks that every synthesized input is accepted by
the real zod. Without Node.js or the zod directories it is skipped with a
message unless `MIDWRITE_REQUIRE_ZOD=1` (directories:
`MIDWRITE_ZOD3_DIR`, `MIDWRITE_ZOD4_DIR`). Discovery:
`crates/midwrite-discover/tests/zod.rs` on
[`fixtures/apps/zod/app`](../../fixtures/apps/zod/app).

## Limitations

* The subset above; refinements, transforms, preprocess, coerce and unions
  stay refused.
* Elements of a rebuilt array (`ZodData`) are not traceable to input
  paths.
* The version lookup reads files under the project root only; Yarn PnP
  and `paths`-aliased zod copies are not followed (the version is then
  unknown).
* `conform` covers the documented checks; a `.regex()` whose shortest
  example violates another check of the same string leaves the argument
  invalid (the world reports the `ZodError`).
