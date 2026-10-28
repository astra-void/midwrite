# Track A: typed input domains

Code: `crates/commitrift-discover/src/domains.rs` (+ the annotation reader
in `crates/commitrift-analyzer/src/concurrency/js_values.rs`).

## What a domain is

For every parameter path an entry reads (`ownerId`, `input.parentId`,
`actor.kind`), `entry_domains` builds a `PathDomain`:

| field | source |
| --- | --- |
| `kind` | one of `null`, `undefined`, `boolean`, `integer`, `decimal`, `string`, `uuid`, `generated-identifier`, `timestamp`, `date`, `json`, `array`, `object`, `enum`, `database-row-reference`, `unknown`; from the Phase 12–14 typing (`Ty`), refined by the column the path is compared with or written to (`uuid`, `timestamp`, `date`, `json(b)`, `numeric`, an enum column, a foreign key) and by a string-literal union annotation (`enum`) |
| `key` | the path is compared with or written to a key column |
| `null`, `undefined` | `permitted`, `excluded` or `unknown` (below) |
| `provenance` | every fact used, in words: the annotation, the column, a contract declaration |
| `classes` | the value classes the code makes relevant (below) |

Values are the model's existing `Value`s; no second value system was
added. Generated identifiers stay `Value::Gen` (Phase 12): a domain never
assigns a concrete text to a generated value; a key a run generates
cannot be required of the initial state (the synthesizer refuses).

## Nullability: only from the source contract

* `null` is **permitted** only when the parameter's annotation is a union
  containing `null` (`parentId: number | null`).
* `undefined` is **permitted** when the union contains `undefined`/`void`,
  or the parameter or member is optional (`x?: T`, a default value).
* An annotation without them **excludes** them. `any`/`unknown` or no
  annotation: **unknown** — no `null` is generated, and the coverage
  section lists the path as unexplored ("whether null is a valid value is
  unknown (no TypeScript annotation)").
* A value declared in `commitrift-contracts.json` (`arguments`, `values`)
  is fixed: its path gets no classes.

`x === null`, `x === undefined`, `x == null`, `x ?? y` are distinguished by
the model's JavaScript value semantics (Phase 14); the domain only decides
which values are tried.

## Value classes

| class | when |
| --- | --- |
| `null` / `undefined` | the domain permits it |
| `equal-to-operand` | the code compares the path with a literal (`kind === "page"`, `qty === 0`) |
| `below-operand` / `above-operand` | the code orders the path against an integer (`qty > 10` → 9, 10, 11) |
| `negated` | a boolean path tested against a literal |

A class whose value equals the synthesized value adds nothing and is
dropped.

## String-literal unions

An annotation that resolves to a union of string literals (`kind:
RouteKind` with `type RouteKind = "page" | "redirect"`, in the same module
or imported through relative imports, `export *` and `export { X } from`
re-exports) is the domain of the path: base arguments take its first
member (previously the placeholder `"kind-1"`, which the signature does not
admit) and "different value" choices take another member. A one-member
union cannot vary (the distinct-key control then keeps the value).

## Correlated values

Domain worlds keep the actors' relationship: a class applies to every
actor that reads the path, and actors with the same synthesized value get
the same new value. Relationships between actors are varied separately:
the existing variants (same key, distinct key, distinct request key,
swapped) and the Phase 15 *actor worlds* (a non-key argument differs
between actors that share the key; only for check-then-act,
read-modify-write and status-transition candidates without a retry
contract — actors that model one request delivered or retried twice keep
identical arguments).

## Tests

`domains.rs` unit tests (union splitting, nullability, class values);
`crates/commitrift-discover/tests/phase15.rs`: E26 (null), the worlds
application (null, boundary), redelivered actors.
