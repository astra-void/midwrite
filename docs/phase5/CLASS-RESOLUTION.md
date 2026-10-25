# Class and method resolution

Code: `crates/commitrift-analyzer/src/{classes,link}.rs`, class collection
in `bindings.rs`, receiver evaluation in `lower.rs`.

## What is collected

Per module, every class (declaration, `export default class`,
`const C = class {}`), keyed by its start offset: name, superclass (local
class, imported class, global constructor such as `Error`, or "unknown"
for anything else), instance methods, static methods, instance fields
holding functions (`save = async () => {}`), accessors and non-function
fields (they shadow methods), the explicit constructor, whether a field
initializer evaluates calls, and names changed dynamically
(`this.m = …` inside the class, `C.prototype.m = …`,
`Object.defineProperty/defineProperties/assign/setPrototypeOf` on `this`, the
class or its prototype → `"*"`). Superclasses are linked across modules like
functions (import → module → export). Exports of classes are recorded
(`export class`, `export default class`, `export { C }`).

## Receivers

| Call | Receiver kind | Lookup starts at |
| --- | --- | --- |
| `const x = new C(); x.m()` (or `new C().m()`) | exact: the object's class is `C` | `C` |
| `this.m()` in an instance method or arrow field of `C` (arrows inherit `this`) | `this`: `C` or any subclass | `C` |
| `C.m()`, `Imported.m()` | static | `C` |
| `this.m()` in a static method | static `this`: `C` or a subclass | `C` |
| `super(…)`, `super.m()` | the parent class | parent |

A receiver that is a parameter, a property of anything, or a value from an
unknown call is never resolved — methods are never matched by name.

## Lookup

1. If any class in the chain changes `m` dynamically → not resolved.
2. Instance receivers: an instance **field** `m` anywhere in the known chain
   wins (the most derived), because fields are own properties set by
   constructors; otherwise the first **prototype method** `m` up the chain.
   An accessor or data field named `m` before the method → not resolved.
3. `this` receivers: every analyzed subclass of `C` that declares `m`
   (method, field, accessor) or changes it dynamically is a possible target;
   any → not resolved ("may dispatch to an override in `Loud`; several
   targets are possible").
4. Not found and the chain ends in an unanalyzed superclass → not resolved
   ("may be inherited from `WorkerHost` from the package …").
5. Found, but the chain ends in an unanalyzed superclass (instance methods)
   → resolved under the **assumption** that the unanalyzed superclass does
   not install an instance property `m`; an exact receiver that escaped
   before the call → resolved under the assumption that unanalyzed code did
   not replace the method. Assumptions are recorded on the call site
   (`CallResolution::Method { assumption }`), shown in `commitrift ir`, and
   every finding whose operation is reached through such a call gets
   confidence ≤ medium and a note naming the assumption.

`new C(…)` runs `C`'s explicit constructor, or walks up to the first
explicit one; field initializers with calls or an unanalyzed superclass
constructor make construction non-inert. Constructor parameter properties
(`constructor(private db: Database)`) are not bound to instance fields;
`this.db.update(...)` is recognized through the configured member path as in
Phase 4.

**Closed world.** Subclasses are searched only in the analyzed files.

## Corpus

midday: 104 classes; 263 method calls resolved, 20 under an assumption
(package superclasses such as BullMQ's `WorkerHost`), 18 unresolved.
school-ledger: 18 classes; 93 resolved, 67 assumed.

`this.updateProgress(…)` (the Phase 4 example) now resolves:
`ExportTransactionsProcessor → AccountingProcessorBase (imported) →
BaseProcessor (imported)`, method `BaseProcessor.updateProgress`. It stays an
unknown effect in failure windows because that method calls BullMQ's
`job.updateProgress(...)` — a third-party write — so it is not inert; the
unknown effect's detail now says exactly that. Resolving
`this.initializeProvider(…)` in the same processors revealed the
`db.update(apps)` it performs (three new CP1004 info findings, see
[`CORPUS-EVALUATION.md`](CORPUS-EVALUATION.md)).

## Tests

`fixtures/vulnerable/class_this_dispatch.ts`,
`class_instance_dispatch.ts`, `fixtures/unsupported/class_override_dispatch.ts`,
`class_dynamic_method.ts`, `fixtures/projects/class_hierarchy/` (inherited
across two modules), `fixtures/projects/package_superclass/` (assumed,
medium), `tests/phase5.rs` (`this_dispatch_…`, `an_override_…`,
`methods_are_never_resolved_by_name_alone`,
`instance_fields_holding_functions_shadow_prototype_methods`), and the
metamorphic tests (method of a known instance ≡ helper; parameter receiver
and reassigned method change the result).
