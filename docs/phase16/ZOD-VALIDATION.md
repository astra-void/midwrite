# Track A: zod validation and structured object semantics

Code: `crates/commitrift-analyzer/src/zod.rs` (schema reader, version
lookup), `crates/commitrift-analyzer/src/concurrency.rs` (`zod_schema`,
`zod_parse`, `zod_validate`, `zod_static_facts`, `zod_parse_checked`,
`zod_output`, `const_string_array`), `crates/commitrift-model/src/zodfn.rs`
(schemas as values, `safeParse` semantics, valid-input synthesis),
`crates/commitrift-model/src/jsregex.rs` (`RegExp#test` subset), new
built-ins in `crates/commitrift-model/src/value.rs` / `jsfn.rs`,
`crates/commitrift-discover/src/{domains,synth,worlds}.rs` (input domains,
synthesis, worlds) and one resolver rule in
`crates/commitrift-discover/src/effects.rs`.

## Facts that motivated the change

* Phase 14 modeled a zod 3 subset and refused `z.strictObject`, `.strict()`,
  `.passthrough()`, `.optional()`, `.nullable()`, `.default()`, `.uuid()`,
  `.email()`, `.regex()` and `z.array`. In the private corpus project
  (zod 4.6.5) 7 of 9 candidates stopped at `z.strictObject`; Phase 14's own
  evaluation entries E10/E11 (zod 3) stopped at `.uuid()`.
* The zod major version decides semantics that matter to the model. Read
  from the package sources and confirmed on the runtimes (below), zod 3.25.76
  and zod 4.6.5 differ on, at least: the UUID pattern (zod 3: any
  8-4-4-4-12 hex string; zod 4: RFC 9562 versions 1–8 and variant `10`, plus
  the nil and max UUIDs), the email pattern, `.default()` (zod 3 parses the
  default through the inner schema, zod 4 returns it as is), `.optional()`
  over a defaulted schema (zod 3 omits an absent key, zod 4 substitutes the
  default), `.passthrough()` (zod 3 drops unknown keys whose value is
  `undefined`, zod 4 keeps them), repeated array length checks (zod 3 keeps
  the last, zod 4 all), a Map or Set as object input (zod 3 rejects it, zod 4
  accepts it as an object), `z.looseObject`/`z.uuid()`/`z.email()`/`z.int()`
  (zod 4 only) and `.int()` (zod 4: safe integers only).
* zod 3.25 also ships `zod/v4`, an early zod 4 implementation that differs
  from zod 4.x (its UUID pattern rejects the max UUID); zod 4.x ships
  `zod/v3`, which behaves as zod 3.25 on the whole matrix.

## The zod version (static)

`zod::resolve_version(file, root, specifier)`; nothing is executed:

| import | package | API modeled |
| --- | --- | --- |
| `zod/v3` | any | zod 3 |
| `zod` | 3.x | zod 3 |
| `zod`, `zod/v4` | 4.x | zod 4 |
| `zod` | unknown | *either*: see below |
| `zod/v4` | 3.x or unknown | refused (the early implementation) |

The package's major version comes from the installed
`node_modules/zod/package.json` found by Node.js resolution from the
importing file's directory up to the project root, else from the nearest
`package.json` declaring `zod` in `dependencies`, `devDependencies`,
`peerDependencies` or `optionalDependencies`. A `catalog:` / `catalog:name`
reference is followed to `catalog`/`catalogs` of a `package.json` (also under
`workspaces`) or `catalog:` of `pnpm-workspace.yaml`. A range fixes a major
version only when it names one (`4.6.5`, `^4.3.6`, `~3.22.0`, `3.x`,
`npm:zod@^4`); `>=3`, `*`, `latest`, `^3 || ^4`, `workspace:*` do not. The
namespace must be `import { z }`, `import * as z` or `import z` from a zod
specifier; a schema mixing specifiers is refused.

With an **unknown** version: constructs whose semantics differ (`.uuid()`,
`.email()`, `.passthrough()`, repeated array length checks, zod-4-only
constructors) are refused at extraction with the difference named; static
facts that differ (what an absent defaulted key yields, what
`.optional()`/`.default()` give for `undefined`) are refused; everything
else is evaluated with both semantics and an input on which they disagree
(a Map input, ...) is an unsupported evaluation. Nothing is assumed.

## Supported subset

Constructors: `z.string()`, `z.number()`, `z.boolean()` (an error-message
argument is accepted: `"msg"`, `{ message | error | required_error |
invalid_type_error | description }`), `z.literal(string | number |
boolean)`, `z.enum([...])` (members are string literals, a module-level
`const` array of string literals in the module or imported, or spreads of
such arrays; a message second argument), `z.object`, `z.strictObject`,
`z.looseObject` (zod 4), `z.array(item)`, `z.optional(s)`, `z.nullable(s)`,
`z.uuid()`, `z.email()`, `z.int()` (zod 4).

Methods: string `min`/`max`/`length`/`nonempty`/`trim`/`toLowerCase`/
`toUpperCase`/`startsWith`/`endsWith`/`includes` (Phase 14), `uuid`, `email`,
`regex(/literal/flags)`; number `min`/`gte`/`gt`/`max`/`lte`/`lt`/`positive`/
`nonnegative`/`negative`/`nonpositive`/`int`/`finite`; object `strict`,
`strip`, `passthrough` (the last call wins); array `min`/`max`/`length`/
`nonempty`, `.array()`; on every schema `optional`, `nullable`, `nullish`,
`default(literal)` (string, number, boolean or `null`). Integer arguments
may be a module-level `const` initialized with an integer literal; a
template-literal message is accepted when its interpolations are names or
literals.

Refused, with the construct named: refinements (`refine`, `superRefine`),
`transform`, `preprocess`, `coerce`, `pipe`, unions, discriminated unions,
intersections, records, tuples, `extend`/`pick`/`omit`/`partial`/`merge`,
other string formats (`url`, `cuid`, ...), `.uuid({ version })`, a
`.default()` that is not a literal, methods on an optional/nullable/defaulted
schema, shape spreads and computed keys, shape keys that are
`Object.prototype` properties (`toString`, `constructor`, ...: zod reads them
through the prototype), and object inputs with an own `__proto__` key.

### `.regex()`

`jsregex::Regex` implements `RegExp.prototype.test` in ECMAScript's
non-Unicode mode over UTF-16 code units, by backtracking with ECMAScript's
rules (ordered alternatives, greedy/lazy quantifiers, an empty iteration ends
a loop, atomic lookaheads), bounded to 2,000,000 steps (beyond: refused).
Supported: literals, `.`, `^`, `$`, `\b`, `\B`, alternation, capturing,
non-capturing and named groups, `(?=…)`, `(?!…)`, `* + ? {n} {n,} {n,m}`
(greedy and lazy), classes with ranges and negation, `\d \D \w \W \s \S \t
\n \v \f \r \0 \xHH \uHHHH \cX`, identity escapes of non-alphanumeric
characters; flags `i` (only for ASCII patterns, where non-Unicode
`Canonicalize` is ASCII case folding exactly), `m`, `s`, `g` and `y` (zod
resets `lastIndex` to 0 before each test: `g` has no effect, `y` anchors at
0), `d`. Refused: backreferences, lookbehind, `u`/`v`, octal escapes and
Annex B forms (a literal `{`, `}` or `]`, an identity escape of a letter).
zod's own UUID and email patterns are evaluated by the same engine, copied
verbatim from the package sources (`zodfn::V3_UUID`, `V3_EMAIL`, `V4_UUID`,
`V4_EMAIL`).

## Semantics (`commitrift_model::zodfn::parse`)

Read from `zod/v3/types.js` and `zod/v3/helpers/parseUtil.js` (3.25.76) and
`zod/v4/core/schemas.js`, `checks.js`, `regexes.js`, `util.js` (4.6.5):

* strings: checks in declaration order, transforms rewrite the value for
  later checks and the output (Phase 14); non-ASCII case mapping refused;
* numbers: the model's integers; inputs outside ±(2^53 − 1) are refused
  (where `.int()` differs between the versions);
* objects: zod 3 accepts plain objects only; zod 4 also Maps and Sets
  (refused). Missing key vs explicit `undefined` vs `null` are distinct: zod 3
  `mergeObjectSync` sets a key when its parsed value is not `undefined` or
  the key is in the input; zod 4 `handlePropertyResult` follows the
  schema's `optin`/`optout` (an absent optional key is skipped, a defaulted
  one is set). Strict objects reject any extra own key, also one whose value
  is `undefined`; strip drops extra keys; passthrough copies them (zod 3
  without the `undefined` ones). The output's key order is not modeled (the
  model's objects are unordered, as everywhere else).
* arrays: `Array.isArray`, length checks (zod 3: the last of each kind),
  every element parsed;
* `optional`: `undefined` → `undefined` (zod 4: a defaulted inner
  substitutes its default); `nullable`: `null` → `null`; `default(d)`:
  `undefined` → zod 3 parses `d` through the inner schema, zod 4 returns `d`;
* `parse` throws when the input is rejected; `safeParse` yields
  `{ success, data }` with `data` `undefined` on failure. `error` is not
  modeled (reading it is refused).

## Lowering

The Phase 14 subset of a schema whose version is not zod 4 keeps its
explicit steps (zod 3 semantics, verified identical in zod 4.x and in the
zod 3 of zod 4.x by the matrix), with one correction found by the
differential matrix: Phase 14 tested an object input with `typeof x ===
"object" && x !== null` and *assumed* it was not an array; arrays were then
refused and Maps and Sets accepted. The test is now the exact
`ZodCheck(z.object({}))` of the version, and the assumption is gone.

Every other schema, and every schema built with zod 4:

1. `zin = <input>`; `zok = ZodCheck(<descriptor>, zin)` — the whole
   validation, with the version's semantics, as one step;
2. on failure: `parse` throws `ZodError`, `safeParse` sets `success = false`
   (`data` stays `undefined`);
3. on success the parsed value is built from the input as explicit steps, so
   the values the code goes on to use stay traceable to the input's paths:
   string transforms as `Trim`/`ToLowerCase`/`ToUpperCase` calls; a wrapper
   whose output differs from its input branches on `=== undefined` / `===
   null` to the statically computed value; an object is an object literal of
   its always-present fields, plus `ObjectAssign` of each optional field
   under `Object.hasOwn(zin, key)`, plus (passthrough) `OmitKeys(zin, shape)`
   (zod 3: through `DefinedEntries`); an array whose elements are rebuilt
   (objects, transforms, defaults) is `ZodData(<descriptor>, zin)`, an array
   of identity elements is the input itself. The static facts used (what an
   absent key or `undefined` yields) are computed by the same `zodfn::parse`
   before anything is emitted, and refused if not modeled.

### IR and serialization

New `Builtin` variants, appended before `// SQL.` (serialized only when
used, so historical scenarios and reports are byte-identical): `HasOwn`
(`Object.hasOwn`), `OmitKeys` (object rest), `ObjectAssign` (`{...a,
...b}`), `ZodCheck` (`safeParse(x).success`), `DefinedEntries` (entries whose
value is not `undefined`), `ZodData` (`parse(x)` of an accepted input). A
schema is a literal descriptor (`zodfn::to_value`: `{ "zod": "3" | "4" |
"3|4", "schema": {...} }`, shapes as `[key, schema]` pairs). `ZodCheck` and
`ZodData` render as the zod chain they stand for
(`z.strictObject({ ... }).safeParse(x).success`). Discovery: new
`ValueClass::Absent` and `ValueClass::Present { value }`, and
`ArgOverride.absent` (`#[serde(default, skip_serializing_if = "Not::not")]`).

## Input domains and synthesis

* `domains::zod_inputs` finds every `ZodCheck` whose input is a parameter
  path (through the local it is bound to). The schema is the source contract
  of the path and of every field below it, read by the code or not
  (`zod_domains`): kind (`uuid`, `enum`, `string`, `integer`, `boolean`,
  `array`, `object`), `null` and `undefined` permitted exactly when the
  schema accepts them, and the classes `null`, `undefined`, `absent` (the
  key may be missing) and `present` (an optional key the base omits, with a
  valid value). Declared contract values are never varied.
* `synth::arguments` makes validated arguments valid
  (`zodfn::conform`: a valid current value is kept; an object keeps its
  valid fields, gets every required field, loses keys a strict schema
  rejects; strings satisfy length/affix checks, UUIDs are version-4 variant-10
  values hashed from the field name, emails `<name>@example.test`, `.regex()`
  values are the pattern's shortest example; every result is checked with
  `zodfn::parse`). zod enums are literal domains (the base takes the first
  member, distinct-key variants and actor worlds another one); a varied
  value the schema rejects is replaced. Every validated argument is stated in
  the scenario's synthesis notes (`SYNTHESIZED input of …: … satisfies its
  zod 4 schema …`), never presented as observed.
* Worlds: `absent:<path>` removes the key, `present:<path>` adds it,
  `null:`/`undefined:` set it, each where the schema admits it; actor
  worlds skip values the schema rejects.
* Authorization: nothing here grants privileges. A guard the synthesizer
  satisfies is still the Phase 15 explicit `SYNTHESIZED precondition` note;
  an enum input's first member is an input value, stated in the notes.

### Outside the owned files (minimal)

`effects.rs` (`Ctx::object_property`): a property of an object literal is
that property's value (`({ k: e }).k` is `e`), through locals whose every
definition agrees, and through `ObjectAssign(earlier, { … })`. Without it the
key a check-then-act reads and writes from a parsed zod object resolved to a
computation over the whole input and the candidate was rejected ("share no
argument-dependent key column"). `worlds.rs`: the two new classes, the
`absent` override, zod enums and validity in actor worlds. `model/src/lib.rs`:
the two new modules. `analyzer/tests/fixtures.rs`: the shared `phase16`
exclusion line.

## Differential verification against the real zod

`crates/commitrift-analyzer/tests/phase16_zod.rs` with
`fixtures/phase16/zod/matrix.json` (58 schemas, inputs in the model's JSON
value form) and `fixtures/phase16/zod/run.mjs`. For each runtime the test
generates a TypeScript module (`safe_<id>` returns `{ success, data }` of
`safeParse`, `parse_<id>` returns `parse`), extracts both functions with
CommitRift, runs the extracted steps with the model's evaluator, and compares
the decision, the parsed value (exact `Value` equality: missing key ≠
`undefined` ≠ `null`) and whether `parse` threw, with Node.js running the
pinned package from its directory. A second test checks that the input
synthesis builds for every schema (`zodfn::conform`) is accepted by the real
zod. Skipped with a message when Node.js or the zod directories are missing,
unless `COMMITRIFT_REQUIRE_ZOD=1` (directories: `COMMITRIFT_ZOD3_DIR`,
`COMMITRIFT_ZOD4_DIR`).

```
COMMITRIFT_REQUIRE_ZOD=1 cargo test -p commitrift-analyzer --test phase16_zod -- --nocapture
phase16 zod differential:
  zod4: 250 comparisons, 247 agreements
  zod3: 240 comparisons, 239 agreements
  zod4-v3: 240 comparisons, 239 agreements
  unknown: 240 comparisons, 172 agreements
  zod3-v4: 250 comparisons, 46 agreements
  total: 1220 comparisons, 943 agreements, 277 refusals, 0 mismatches
phase16 zod: 95 synthesized inputs accepted by the pinned runtimes
```

* `zod4` = zod 4.6.5 as `zod`; `zod3` = zod 3.25.76 as `zod`; `zod4-v3` =
  zod 4.6.5 as `zod/v3`. Their refusals are exactly the expected ones: the
  prototype-key schema (all three) and the Map and Set inputs (zod 4).
* `unknown`: a project without a zod version, both runtimes as oracle; every
  input on which they differ is refused (required), the 68 refusals also
  cover the version-dependent constructs refused at extraction.
* `zod3-v4`: zod 3.25's `zod/v4`; only the Phase 14 subset is modeled (46
  agreements), everything else is refused.

Mismatches found by the matrix and fixed in the model (the cases stay in the
matrix as regressions): zod 3 passthrough drops unknown keys whose value is
`undefined` (the first implementation kept them); the Phase 14 object test
accepted a Map and a Set and refused an array (`z.object({})` of `new Map()`
succeeded in the model, fails in zod 3).

Unit tests: `zodfn` (descriptors, version differences, strict objects,
UUIDs, `conform`), `jsregex` (ECMAScript cases, refusals), `zod.rs` (reading,
versions, enum constants, ranges). Discovery:
`crates/commitrift-discover/tests/phase16_zod.rs` on
`fixtures/phase16/zod/app` (zod 4.6.5): the strict-object check-then-insert
is a `MODEL_COUNTEREXAMPLE` with synthesized inputs that pass validation and
no implicit precondition; with `--all-worlds` the worlds `absent:`,
`undefined:`, `null:input.note` and `absent:`, `undefined:input.role` are
explored and none stops at the validation; the unique-key counterpart is
`NO_VIOLATION_WITHIN_BOUNDS`.

Phase 14 evaluation entries (zod 3.24.2, `.uuid()`), run before (Phase 15
build of commit 1efa383) and after on `fixtures/phase14/eval-app`: both were
rejected before ("share no argument-dependent key column"); now E10
`createTag` is a `MODEL_COUNTEREXAMPLE` (labeled positive) and E11
`createCategory` a `MODEL_COUNTEREXAMPLE` found in the actor world where the
two names differ, against the heuristic hypothesis "at most one row per
account" (labeled negative for the normalized-name property; the report
says the hypothesis is not established). The evaluation scripts were not
re-run.

## school-ledger (private corpus; aggregate only)

`commitrift discover <project> --model-check --format json`, Phase 15 build
(commit 1efa383, exported with `git archive` and built separately) versus
this change; the project pins zod 4.6.5 (installed), so the zod 4 API is
modeled.

| | before | after |
| --- | --- | --- |
| candidates | 9 (all check-then-act) | 9 |
| status | 9 UNSUPPORTED | 9 UNSUPPORTED |
| model-checkable | 0 | 0 |
| first blocker | 7: `z.strictObject` not modeled; 2: an unresolved method call in test-support code | 7: a zod refinement callback (`.superRefine` on the input array) — refused by design; 2: unchanged |
| strictObject modeled | no | yes (and the nested strict objects, the UUID fields, enums of `const` arrays and spreads, a `.max` bound given by a module constant with a template-literal message, arrays of objects) |
| authorization preconditions (synthesized, stated) | 28 notes over 18 scenarios | 28 notes over 18 scenarios (unchanged; a staff kind and a school-administrator role on the actor argument) |
| synthesized zod inputs | none | none reached (the schema is refused at `superRefine` before a `ZodCheck` exists) |

Across all extracted entries, the zod refusal sites before were dominated by
`z.strictObject`, `z.uuid`, `z.email`, `z.array`, enums of `const` arrays and
spreads in shapes; after, the remaining ones are refinements (`refine`,
`superRefine`), `transform`, `preprocess`, discriminated unions, `z.url`,
shape spreads, a bound given by an imported constant, and schemas not
written as a builder chain. The 7 candidates now stop at another honest
UNSUPPORTED construct: a refinement whose callback (business validation of
the batch) the model does not run. Supporting it requires modeling the
callback body (closures over issue reporting), out of this track's scope.

## Test runs

* `COMMITRIFT_REQUIRE_REPLAY=1 cargo test --no-fail-fast -p commitrift-model`:
  every target passes (unit 27, pg_matrix 13, postgres 1, ...).
* `COMMITRIFT_REQUIRE_ZOD=1 cargo test --no-fail-fast -p commitrift-analyzer
  -p commitrift-discover`: every target passes.
* `cargo test --no-fail-fast -p commitrift-cli` in the isolated worktree:
  14 failures, all of them tests that build fixture harnesses (`dist/` or
  `node_modules/` absent: "`dist/verify/harness.js` does not exist", "building
  fixtures/… failed"); these untracked build outputs exist in the main
  checkout. Every other CLI target passes.
* `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --
  -D warnings`: clean.

## Limitations

* The subset above; refinements, transforms, preprocess, coerce, unions and
  discriminated unions remain refused with the construct named.
* `ZodData` computes a rebuilt array's output whole: its elements' values
  are not traceable to input paths (synthesis treats such arrays as
  opaque values).
* Output key order is not modeled (model objects are unordered).
* The version lookup reads files under the project root only; Yarn PnP and
  `paths`-aliased zod copies are not followed (the version is then unknown,
  with the refusals above).
* `conform` finds values for the documented checks; a `.regex()` whose
  shortest example violates another check of the same string leaves the
  argument invalid (the run stops at the validation and the world reports
  the `ZodError`).
* School-ledger remains blocked (above); Midday was not re-run by this
  track.
