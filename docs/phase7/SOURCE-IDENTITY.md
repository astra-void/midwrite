# Source-level function identity

`crates/commitrift-analyzer/src/identity.rs`.

## Identity

A function is identified by **its module and the line and column where its
syntax node starts** (its *anchor*): `src/app/actions.ts@43:3`. This is the
position the static analyzer already used to name anonymous functions
(`<anonymous@43:3>` in `commitrift check`), so the concurrency extractor and
the static analyzer refer to the same function by position — never by
text or name. Two anonymous callbacks with identical source text are
different functions (`callbacks_with_identical_text_are_different_functions*`
tests).

The identity is reported in every extraction report as
`entry.anchor` (`file@line:column`), `entry.file`/`entry.endLine` (the
function's region, used to classify runtime statements as inside or outside
the modeled function) and `entry.staticName` (the name `commitrift check`
reports for the same function).

## Selectors

A scenario names its entry function without editing the application:

| Selector | Function |
| --- | --- |
| `src/app/actions.ts#inviteTeamMember/arg[1]` | argument 1 of the call that initializes `inviteTeamMember` |
| `src/factory.ts#makeHandler/return` | the function `makeHandler` returns (its single `return <expr>`) |
| `src/service.ts#OrderService/method[place]` | instance method |
| `src/service.ts#OrderService/static[purge]` | static method |
| `src/handlers.ts#handlers/property[create]` | function-valued object-literal property (incl. method shorthand) |
| `src/handlers.ts#router/property[archive]/arg[0]` | steps chain |
| `src/app/actions.ts@43:3` | the function starting at 43:3 |

Bindings are top-level declarations (exported or not, `export default`).
Every step must resolve to exactly one syntax node; otherwise the selector
is an error with the reason (`the call has no argument 2`, `the function has
2 return statements: select the function by its anchor`, ...). Nothing is
guessed.

Scenario form (`actors[i].entry`, scenario schema extension):

```json
{ "name": "Submit1",
  "entry": { "selector": "src/app/actions.ts#inviteTeamMember/arg[1]",
             "invocation": "awaited",
             "preconditions": ["authenticated-user: ...", "validation-succeeded: ..."] },
  "arguments": [{"email": "new@example.com", "role": "member"}, null, {"id": 1}] }
```

Arguments are the selected function's parameters (here the callback's
`data`, the unused `FormData`, and `user`). Simple object-pattern
parameters (`async ({ input }) => …`) are supported: the argument is bound
whole and each property read once, as JavaScript does.

## What the identity does not do

* It does not track functions created dynamically (`new Function`,
  `eval`), functions stored in maps, or class fields assigned arrow
  functions (select those by anchor).
* The static analyzer and the extractor still lower the function
  separately; they agree on *which* function, not on one shared IR
  (Milestone 2 not done).
