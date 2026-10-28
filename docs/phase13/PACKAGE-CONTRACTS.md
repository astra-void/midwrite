# Phase 13: package contracts

`crates/commitrift-analyzer/src/packages.rs`. No package code is read or
executed. Every call into a package falls in exactly one category:

| category | source | lowering |
| --- | --- | --- |
| **verified built-in contract** | an entry of `packages::CONTRACTS` (or the React `cache` higher-order contract), written from the package's documented API at the versions named in the entry | `Op::Unknown` whose detail names the contract: async or not, what it returns, how it fails, its effects, and why its result is not modeled. React `cache`: see below |
| **declared external contract** | `externals` of the project's `commitrift-contracts.json` (callee, channel, idempotency key, dedup) | an external effect (`Op::External`), as since Phase 11; reported as declared |
| **unknown package behavior** | anything else | `Op::Unknown` "call into an unanalyzed package" (unchanged) |

Rules:

* A contract matches an **exact** package specifier and an export
  (`hash` of `bcryptjs`, also `bcrypt.hash` through a default or namespace
  import), or a method of a module-level value **built from that package**
  (`schema.safeParse` where `const schema = z.object(...)` and `z` is
  imported from `zod`). The specifier must resolve as a package (a project
  module behind a path alias does not match). `bcrypt-ts`'s `hash`, a
  `safeParse` of `yup`, or a `cache` of any package other than `react` is
  unknown package behavior (tests
  `package_calls_with_verified_contracts_stay_unknown_and_name_the_contract`,
  `packages::tests`).
* No package is marked pure by name, and **no contract invents a value**:
  a salted hash (fresh randomness), a password comparison (depends on the
  hash) and a validation outcome (validation logic is not run) are not
  representable, so these calls stay unknown operations. What the contract
  adds is the precise reason and the bounded statement of effects; a
  validator's success stays an assumption the user must declare or a
  blocker.

## Verified built-in contracts

| package(s) | call | async | returns | errors | effects |
| --- | --- | --- | --- | --- | --- |
| `bcryptjs`, `bcrypt` | `hash`, `hashSync` | yes / no | a salted hash (fresh value) | rejects/throws on invalid arguments | CPU only |
| `bcryptjs`, `bcrypt` | `compare`, `compareSync` | yes / no | whether the string hashes to the hash | rejects/throws on invalid arguments | CPU only |
| `bcryptjs`, `bcrypt` | `genSalt` | yes | a random salt (fresh value) | rejects on an invalid round count | reads randomness |
| `zod` (`zod/v3`, `zod/v4`) | `schema.parse`, `schema.parseAsync` | no / yes | the parsed output (keys stripped, defaults, coercions, transforms applied) | ZodError when invalid | none, unless the schema's own refinements/transforms have effects (not analyzed) |
| `zod` | `schema.safeParse`, `schema.safeParseAsync` | no / yes | `{ success, data \| error }` | none for invalid input | as above |

### React `cache` (higher-order, verified built-in)

`export const f = cache(fn)` with `cache` imported from `react`: within a
server request, `fn` runs on the first call with given arguments and later
calls with the same arguments return the memoized result. The extractor
inlines `fn` at the actor's **first** call (assumption printed: no earlier
call with the same arguments in the same request) and makes any repeated
call an unknown operation. Invocation evidence is kept: `fn` is not assumed
to run on a repeated call.

## Measured effect

Package calls now named by a contract (`package call with a contract
(result not modeled)`): saas-starter 12 (its `signUp` candidate is still
UNSUPPORTED, now at "`hash` of `bcryptjs` … its result is not
representable in the model"), unnamed-ai-project 62, school-ledger 8,
midday 2. No candidate status changed because of a contract, as expected:
none of these results can be represented without a model feature for
opaque fresh values (bcrypt) or a declared validity assumption (zod).
