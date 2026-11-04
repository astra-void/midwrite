//! Phase 5 behavior, end to end: the value layer (abstract values, heap,
//! provenance), method dispatch, callbacks, inert summaries, asynchronous
//! semantics, return-value facts, loops, determinism, and metamorphic
//! properties of the analysis.

use std::path::{Path, PathBuf};

use midwrite_analyzer::Analyzer;
use midwrite_core::absint::RelationKind;
use midwrite_core::config::Config;
use midwrite_core::diagnostic::{Confidence, Diagnostic};
use midwrite_core::ir::{CallResolution, EffectKind, Execution, FileIr, FunctionIr, Node};

fn analyzer() -> Analyzer {
    Analyzer::new(Config::default())
}

fn analyze(source: &str) -> Vec<Diagnostic> {
    analyzer()
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

fn lower(source: &str) -> FileIr {
    analyzer().lower_source("test.ts", source).expect("parses")
}

fn function<'a>(ir: &'a FileIr, name: &str) -> &'a FunctionIr {
    ir.functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function {name} in\n{}", ir.to_text()))
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn of<'a>(diagnostics: &'a [Diagnostic], function: &str) -> Vec<&'a Diagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.function == function)
        .collect()
}

/// Every call scope of a function body, recursively.
fn calls(nodes: &[Node]) -> Vec<&midwrite_core::ir::CallScope> {
    let mut out = Vec::new();
    for node in nodes {
        match node {
            Node::Call(call) => {
                out.push(call);
                out.extend(calls(&call.body));
            }
            Node::Transaction(scope) => out.extend(calls(&scope.body)),
            Node::Branch(branch) => branch.arms.iter().for_each(|a| out.extend(calls(&a.body))),
            Node::Switch(switch) => switch.cases.iter().for_each(|c| out.extend(calls(&c.body))),
            Node::Loop(lp) => out.extend(calls(&lp.body)),
            Node::Try(t) => {
                out.extend(calls(&t.block));
                if let Some(h) = &t.handler {
                    out.extend(calls(&h.body));
                }
                if let Some(f) = &t.finalizer {
                    out.extend(calls(f));
                }
            }
            Node::Effect(_) | Node::Exit(_) => {}
        }
    }
    out
}

fn unknown_labels(function: &FunctionIr) -> Vec<String> {
    function
        .effects()
        .iter()
        .filter(|e| matches!(e.kind, EffectKind::Unknown { .. }))
        .map(|e| e.label.to_string())
        .collect()
}

fn unknown_detail(function: &FunctionIr, label: &str) -> String {
    function
        .effects()
        .iter()
        .find_map(|e| match &e.kind {
            EffectKind::Unknown { detail, .. } if e.label.starts_with(label) => {
                Some(detail.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no unknown `{label}` in\n{}", function.to_text()))
}

// --- Method dispatch (Milestone 4) ----------------------------------------------

const SERVICE: &str = r#"
    class OrderService {
      async complete(id: string) {
        await db.update(orders).set({ status: "paid" });
        await this.publish(id);
      }
      async publish(id: string) {
        await queue.publish("order.completed", { id });
      }
    }
    export async function direct(id: string) {
      const service = new OrderService();
      await db.update(orders).set({ status: "paid" });
      await service.publish(id);
    }
"#;

#[test]
fn this_dispatch_is_resolved_without_overrides_and_recorded() {
    let ir = lower(SERVICE);
    let complete = function(&ir, "OrderService.complete");
    let call = calls(&complete.body)[0];
    assert_eq!(&*call.callee, "OrderService.publish");
    assert_eq!(
        call.resolution.as_deref(),
        Some(&CallResolution::Method {
            class: "OrderService".to_owned(),
            exact: false,
            assumption: None
        })
    );
    let direct = function(&ir, "direct");
    match calls(&direct.body)[0].resolution.as_deref() {
        Some(CallResolution::Method { exact, .. }) => assert!(*exact),
        other => panic!("{other:?}"),
    }
    let diagnostics = analyze(SERVICE);
    assert_eq!(
        codes(
            &of(&diagnostics, "OrderService.complete")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["MW1001"]
    );
    assert_eq!(of(&diagnostics, "direct")[0].confidence, Confidence::High);
}

#[test]
fn an_override_in_a_subclass_keeps_this_dispatch_unknown() {
    let source = format!(
        "{SERVICE}\nclass Loud extends OrderService {{ async publish(id: string) {{ await mailer.send(id); }} }}"
    );
    let ir = lower(&source);
    let complete = function(&ir, "OrderService.complete");
    assert!(calls(&complete.body).is_empty(), "{}", complete.to_text());
    assert!(unknown_detail(complete, "this.publish").contains("override in `Loud`"));
    // A receiver created by `new OrderService()` is exactly that class: the
    // override does not matter.
    let direct = function(&ir, "direct");
    assert_eq!(&*calls(&direct.body)[0].callee, "OrderService.publish");
}

#[test]
fn methods_are_never_resolved_by_name_alone() {
    // `service` is a parameter: its class is unknown, even though a class
    // with a `publish` method exists in the file.
    let source = format!(
        "{SERVICE}\nexport async function viaParameter(service: OrderService, id: string) {{\n  await db.update(orders).set({{ status: \"paid\" }});\n  await service.publish(id);\n}}"
    );
    let ir = lower(&source);
    let f = function(&ir, "viaParameter");
    assert!(calls(&f.body).is_empty());
    assert_eq!(unknown_labels(f), ["service.publish(id)"]);
}

#[test]
fn instance_fields_holding_functions_shadow_prototype_methods() {
    let source = r#"
        class Base {
          notify = async (id: string) => { await queue.publish("field", { id }); };
        }
        class Sub extends Base {
          async notify(id: string) { await mailer.send({ id }); }
        }
        export async function run(id: string) {
          const sub = new Sub();
          await db.update(orders).set({ status: "paid" });
          await sub.notify(id);
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    let call = calls(&run.body)[0];
    // The base class's field initializer installs an own property that wins
    // over the subclass's prototype method.
    assert_eq!(&*call.callee, "Base.notify");
}

// --- Inert summaries (Milestones 3, 9) -----------------------------------------

#[test]
fn inert_callees_are_resolved_even_recursive_or_deep() {
    let source = r#"
        function fib(n: number): number { return n < 2 ? n : fib(n - 1) + fib(n - 2); }
        async function l4(id: string) { return `x${id}`; }
        async function l3(id: string) { return await l4(id); }
        async function l2(id: string) { return await l3(id); }
        async function l1(id: string) { return await l2(id); }
        async function loop(n: number): Promise<number> { return n > 0 ? await loop(n - 1) : 0; }
        export async function run(id: string) {
          await db.update(orders).set({ status: "paid" });
          await l1(id);
          await loop(fib(3));
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    let scopes = calls(&run.body);
    assert!(scopes.iter().any(|c| &*c.callee == "l1" && c.inert));
    assert!(scopes.iter().any(|c| &*c.callee == "loop" && c.inert));
    assert!(unknown_labels(run).is_empty(), "{}", run.to_text());
    assert!(analyze(source).is_empty());
}

#[test]
fn effectful_recursion_and_unknown_calls_are_not_inert() {
    let source = r#"
        async function drain(n: number): Promise<void> {
          if (n > 0) { await mailer.send({ n }); await drain(n - 1); }
        }
        async function log(id: string) { await telemetry.track(id); }
        export async function run(id: string) {
          await db.update(orders).set({ status: "paid" });
          await log(id);
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    assert_eq!(unknown_labels(run), ["log(id)"]);
    assert!(unknown_detail(run, "log(id)").contains("not proven inert"));
    let drain = function(&ir, "drain");
    assert!(unknown_detail(drain, "drain(n - 1)").contains("recursive"));
}

#[test]
fn a_returned_package_call_is_not_inert() {
    // Phase 4 recorded nothing for `return pkg()` in an async function; an
    // inertness proof must account for it.
    let source = r#"
        import { render as renderEmail } from "@react-email/render";
        async function render(x: unknown) { return renderEmail(x); }
        export async function run(id: string) {
          await db.update(orders).set({ status: "paid" });
          await render(id);
        }
    "#;
    assert_eq!(codes(&analyze(source)), ["MW9001"]);
}

// --- Callbacks and wrappers (Milestone 5) --------------------------------------

#[test]
fn callbacks_are_linked_per_call_site_context() {
    let source = r#"
        async function execute(action: () => Promise<void>) { await action(); }
        async function publishA() { await queue.publish("a", {}); }
        async function mailB() { await queue.publish("b", {}); }
        export async function run(callback: () => Promise<void>) {
          await db.update(orders).set({ status: "paid" });
          await execute(publishA);
          await execute(mailB);
          await execute(callback);
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    let callbacks: Vec<&str> = calls(&run.body)
        .iter()
        .filter(|c| {
            matches!(
                c.resolution.as_deref(),
                Some(CallResolution::Callback { .. })
            )
        })
        .map(|c| &*c.callee)
        .collect();
    assert_eq!(callbacks, ["publishA", "mailB"]);
    // The third call passes a parameter: which function runs is decided by
    // `run`'s callers; analyzed on its own the call stays unknown, and says
    // why.
    assert_eq!(
        unknown_labels(run),
        ["execute(callback)"],
        "{}",
        run.to_text()
    );
    assert!(unknown_detail(run, "execute(callback)").contains("inert only for some arguments"));
}

#[test]
fn passing_a_callback_does_not_imply_invoking_it() {
    let source = r#"
        async function store(action: () => Promise<void>) { registry.push(action); }
        export async function run() {
          await db.update(orders).set({ status: "paid" });
          await store(async () => { await queue.publish("never-here", {}); });
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    assert!(
        run.effects()
            .iter()
            .all(|e| !matches!(e.kind, EffectKind::External { .. })),
        "{}",
        run.to_text()
    );
}

#[test]
fn a_wrapper_that_awaits_its_argument_moves_the_uncertainty_to_the_argument() {
    let source = r#"
        async function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
          return await Promise.race([promise, new Promise<T>((_, reject) => setTimeout(() => reject(new Error("t")), ms))]);
        }
        function describe(value: unknown, ms: number) { return `${ms}`; }
        export async function run(id: string) {
          await db.update(orders).set({ status: "paid" });
          await withTimeout(fetchStatus(id), 5000);
          await describe(fetchOther(id), 5);
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    // `withTimeout` is inert and awaits its first parameter: the unknown
    // call passed for it becomes the unknown effect, concurrent (it may
    // still be running after a timeout). `describe` never awaits its
    // argument, which therefore stays an unawaited call (not an effect).
    let unknown: Vec<(String, Execution)> = run
        .effects()
        .iter()
        .filter(|e| matches!(e.kind, EffectKind::Unknown { .. }))
        .map(|e| (e.label.to_string(), e.execution))
        .collect();
    assert_eq!(
        unknown,
        [("fetchStatus(id)".to_owned(), Execution::Concurrent)],
        "{}",
        run.to_text()
    );
    assert!(calls(&run.body).iter().all(|c| c.inert));
}

#[test]
fn deferred_and_fire_and_forget_callbacks_are_not_serialized_as_awaited() {
    let source = r#"
        export async function deferred(id: string) {
          await db.update(orders).set({ status: "paid" });
          setTimeout(async () => { await queue.publish("later", { id }); }, 10);
        }
        export async function each(ids: string[]) {
          await db.update(orders).set({ status: "paid" });
          ids.forEach(async (id) => { await queue.publish("each", { id }); });
        }
        export async function all(ids: string[]) {
          await db.update(orders).set({ status: "paid" });
          await Promise.all(ids.map(async (id) => { await queue.publish("all", { id }); }));
        }
    "#;
    let diagnostics = analyze(source);
    assert!(of(&diagnostics, "deferred").is_empty());
    assert_eq!(of(&diagnostics, "each")[0].confidence, Confidence::Medium);
    assert_eq!(of(&diagnostics, "all")[0].confidence, Confidence::Low);
}

#[test]
fn concurrent_and_immediately_invoked_calls_are_summarized() {
    let source = r#"
        async function notify(id: string) { await queue.publish("n", { id }); }
        export async function run(id: string) {
          await db.update(orders).set({ status: "paid" });
          await Promise.all([notify(id), (async () => { await queue.publish("m", { id }); })()]);
        }
    "#;
    let ir = lower(source);
    let run = function(&ir, "run");
    let externals: Vec<Execution> = run
        .effects()
        .iter()
        .filter(|e| matches!(e.kind, EffectKind::External { .. }))
        .map(|e| e.execution)
        .collect();
    assert_eq!(externals, [Execution::Concurrent, Execution::Concurrent]);
    assert!(
        calls(&run.body)
            .iter()
            .any(|c| c.resolution.as_deref() == Some(&CallResolution::Immediate))
    );
}

// --- Return values and path facts (Milestones 3.3, 7.3) -------------------------

#[test]
fn callee_returns_constrain_the_callers_member_tests() {
    let source = r#"
        async function check(id: string) {
          if (id.length > 3) { return { ok: true }; }
          return { ok: false };
        }
        export async function exclusive(id: string) {
          const result = await check(id);
          if (result.ok) { await db.update(orders).set({ status: "checked" }); }
          if (!result.ok) { await queue.publish("rejected", { id }); }
        }
    "#;
    assert!(analyze(source).is_empty(), "{:#?}", analyze(source));
}

#[test]
fn an_escaping_result_is_not_correlated() {
    // `audit(result)` may change `result.ok` (or `result` may have a getter
    // installed by code we do not see): no member facts.
    let source = r#"
        async function check(id: string) {
          if (id.length > 3) { return { ok: true }; }
          return { ok: false };
        }
        export async function exclusive(id: string) {
          const result = await check(id);
          audit(result);
          if (result.ok) { await db.update(orders).set({ status: "checked" }); }
          if (!result.ok) { await queue.publish("rejected", { id }); }
        }
    "#;
    assert_eq!(codes(&analyze(source)), ["MW1001"]);
}

#[test]
fn member_tests_on_unknown_objects_are_not_correlated() {
    // `input` comes from the caller: a getter could return different values.
    let source = r#"
        export async function settle(input: { a: boolean }) {
          if (input.a) { await db.update(orders).set({ status: "paid" }); }
          if (!input.a) { await queue.publish("x", {}); }
        }
    "#;
    assert_eq!(codes(&analyze(source)), ["MW1001"]);
}

// --- Provenance and relationships (Milestones 1.4, 2, 8) -----------------------

fn relation(source: &str, function: &str) -> midwrite_core::absint::ValueRelation {
    let diagnostics = analyze(source);
    of(&diagnostics, function)
        .into_iter()
        .find_map(|d| d.value_relation.clone())
        .unwrap_or_else(|| panic!("no relation for {function}: {diagnostics:#?}"))
}

#[test]
fn provenance_survives_calls_returns_and_aliases() {
    let source = r#"
        async function createOrder(input: { id: string }) {
          return await db.insert(orders).values(input).returning();
        }
        export async function run(input: { id: string }) {
          const created = await createOrder(input);
          const payload = { orderId: created[0].id };
          const request = payload;
          await queue.publish("order.created", request);
        }
    "#;
    let relation = relation(source, "run");
    assert_eq!(relation.status, RelationKind::ProvenShared);
    assert_eq!(relation.shared[0].kind, "derivedFromResult");
    assert!(relation.shared[0].second.contains(".0.id"), "{relation:?}");
}

#[test]
fn shared_inputs_are_the_same_value_and_different_names_are_not_disjoint() {
    let shared = r#"
        export async function run(input: { id: string }) {
          await db.insert(orders).values({ id: input.id });
          await queue.publish("x", { orderId: input.id });
        }
    "#;
    let same = relation(shared, "run");
    assert_eq!(same.status, RelationKind::ProvenShared);
    assert_eq!(same.shared[0].kind, "sameValue");
    let unrelated = r#"
        export async function run(a: string, b: string) {
          await db.insert(orders).values({ id: a });
          await queue.publish("x", { id: b });
        }
    "#;
    // Never `provenDisjoint`: different variables may hold the same value.
    assert_eq!(
        relation(unrelated, "run").status,
        RelationKind::NoRelationEstablished
    );
}

#[test]
fn strong_updates_replace_and_escapes_invalidate() {
    let strong = r#"
        export async function run(a: string, b: string) {
          await db.insert(orders).values({ id: b });
          const payload = { id: a };
          const alias = payload;
          alias.id = b;
          await queue.publish("x", payload);
        }
    "#;
    let replaced = relation(strong, "run");
    assert_eq!(replaced.status, RelationKind::ProvenShared);
    assert!(
        replaced.shared.iter().all(|s| s.second == "param#1"),
        "{replaced:?}"
    );
    let escaped = r#"
        export async function run(a: string) {
          await db.insert(orders).values({ id: a });
          const payload = { id: a };
          await enrich(payload);
          await queue.publish("x", payload);
        }
    "#;
    // `enrich` (unknown) may have changed `payload`.
    assert_eq!(relation(escaped, "run").status, RelationKind::Unknown);
}

#[test]
fn shadowed_bindings_are_distinct() {
    let source = r#"
        export async function run(id: string, other: string) {
          await db.insert(orders).values({ id });
          {
            const id = other;
            await queue.publish("x", { id });
          }
        }
    "#;
    // The inner `id` is `other`: same name, different binding.
    assert_eq!(
        relation(source, "run").status,
        RelationKind::NoRelationEstablished
    );
}

// --- Loops (Milestone 7.4) ------------------------------------------------------

#[test]
fn loop_carried_values_are_joined_not_guessed() {
    let source = r#"
        async function first() { await queue.publish("first", {}); }
        async function second() { await mailer.send({}); }
        export async function run(items: string[]) {
          let action = first;
          for (const item of items) { action = second; }
          await db.update(orders).set({ status: "paid" });
          await action();
        }
        export async function stable(items: string[]) {
          let action = first;
          for (const item of items) { audit.log(item); }
          await db.update(orders).set({ status: "paid" });
          await action();
        }
    "#;
    let ir = lower(source);
    // After the loop `action` is first or second: two targets, unresolved.
    assert_eq!(unknown_labels(function(&ir, "run")), ["action()"]);
    // Unchanged by the loop: still `first`.
    assert_eq!(&*calls(&function(&ir, "stable").body)[0].callee, "first");
}

// --- Determinism (Milestone 10.4) -----------------------------------------------

#[test]
fn analysis_and_ir_are_deterministic() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/vulnerable");
    for name in [
        "class_this_dispatch.ts",
        "callback_wrapper.ts",
        "concurrent_helper.ts",
        "return_value_correlation.ts",
    ] {
        let source = std::fs::read_to_string(fixtures.join(name)).unwrap();
        let a = analyzer().analyze_source(name, &source).unwrap();
        let b = analyzer().analyze_source(name, &source).unwrap();
        assert_eq!(
            serde_json::to_string(&a.diagnostics).unwrap(),
            serde_json::to_string(&b.diagnostics).unwrap()
        );
        assert_eq!(a.ir.to_text(), b.ir.to_text());
    }
}

// --- Metamorphic properties (Milestone 10.1, 10.2) ------------------------------

/// A diagnostic without source positions or names that a behavior-preserving
/// transformation may change: code, confidence, and the kinds of operations.
fn normalized(diagnostics: &[Diagnostic]) -> Vec<(String, Confidence, Vec<String>)> {
    let mut out: Vec<(String, Confidence, Vec<String>)> = diagnostics
        .iter()
        .filter(|d| d.code.as_str() != "MW9001")
        .map(|d| {
            // Operations only (no call sites), without where they are inlined
            // from.
            let mut kinds: Vec<String> = d
                .effects
                .iter()
                .filter(|e| !e.description.starts_with("call of"))
                .map(|e| {
                    e.description
                        .split(", inside")
                        .next()
                        .unwrap_or_default()
                        .to_owned()
                })
                .collect();
            kinds.sort();
            (d.code.as_str().to_owned(), d.confidence, kinds)
        })
        .collect();
    out.sort();
    out
}

fn project(files: &[(&str, &str)]) -> Vec<Diagnostic> {
    let dir = tempdir();
    for (name, source) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, source).unwrap();
    }
    let report = analyzer()
        .analyze_paths(std::slice::from_ref(&dir), None)
        .expect("analysis");
    std::fs::remove_dir_all(&dir).ok();
    report.diagnostics
}

fn tempdir() -> PathBuf {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "midwrite-phase5-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const BASE: &str = r#"
    export async function completeOrder(id: string) {
      await db.update(orders).set({ status: "paid" });
      await queue.publish("order.completed", { id });
    }
"#;

#[test]
fn behavior_preserving_transformations_preserve_findings() {
    let expected = normalized(&analyze(BASE));
    assert_eq!(expected.len(), 1);
    let variants: Vec<(&str, Vec<Diagnostic>)> = vec![
        (
            "renamed locals",
            analyze(
                r#"
                export async function completeOrder(orderId: string) {
                  const key = orderId;
                  await db.update(orders).set({ status: "paid" });
                  await queue.publish("order.completed", { id: key });
                }
            "#,
            ),
        ),
        (
            "extracted helper",
            analyze(
                r#"
                async function announce(id: string) { await queue.publish("order.completed", { id }); }
                export async function completeOrder(id: string) {
                  await db.update(orders).set({ status: "paid" });
                  await announce(id);
                }
            "#,
            ),
        ),
        (
            "pure wrapper around a value",
            analyze(
                r#"
                async function identity<T>(value: T) { return value; }
                export async function completeOrder(id: string) {
                  await db.update(orders).set({ status: "paid" });
                  const key = await identity(id);
                  await queue.publish("order.completed", { id: key });
                }
            "#,
            ),
        ),
        (
            "return through a local variable",
            analyze(
                r#"
                async function announce(id: string) {
                  const result = await queue.publish("order.completed", { id });
                  return result;
                }
                export async function completeOrder(id: string) {
                  await db.update(orders).set({ status: "paid" });
                  await announce(id);
                }
            "#,
            ),
        ),
        (
            "helper as a method of a known instance",
            analyze(
                r#"
                class Announcer { async announce(id: string) { await queue.publish("order.completed", { id }); } }
                export async function completeOrder(id: string) {
                  const announcer = new Announcer();
                  await db.update(orders).set({ status: "paid" });
                  await announcer.announce(id);
                }
            "#,
            ),
        ),
        (
            "helper passed as a callback",
            analyze(
                r#"
                async function run(step: (id: string) => Promise<void>, id: string) { await step(id); }
                async function announce(id: string) { await queue.publish("order.completed", { id }); }
                export async function completeOrder(id: string) {
                  await db.update(orders).set({ status: "paid" });
                  await run(announce, id);
                }
            "#,
            ),
        ),
        (
            "helper moved to another module, imported under an alias",
            project(&[
                (
                    "orders.ts",
                    r#"import { announce as tell } from "./events";
                    export async function completeOrder(id: string) {
                      await db.update(orders).set({ status: "paid" });
                      await tell(id);
                    }"#,
                ),
                (
                    "events.ts",
                    r#"export async function announce(id: string) { await queue.publish("order.completed", { id }); }"#,
                ),
            ]),
        ),
    ];
    for (name, diagnostics) in variants {
        assert_eq!(
            normalized(&diagnostics),
            expected,
            "{name}: {diagnostics:#?}"
        );
    }
}

#[test]
fn behavior_changing_transformations_change_the_evidence() {
    // Root handle → transaction handle: the write now commits with the
    // transaction, and the publication runs inside it (MW1003).
    let in_transaction = analyze(
        r#"
        export async function completeOrder(id: string) {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({ status: "paid" });
            await queue.publish("order.completed", { id });
          });
        }
    "#,
    );
    assert_eq!(codes(&in_transaction), ["MW1003"]);

    // Awaited callback → fire-and-forget callback: confidence drops.
    let fire_and_forget = analyze(
        r#"
        export async function completeOrder(ids: string[]) {
          await db.update(orders).set({ status: "paid" });
          ids.forEach(async (id) => { await queue.publish("order.completed", { id }); });
        }
    "#,
    );
    assert_eq!(fire_and_forget[0].confidence, Confidence::Medium);

    // Known receiver → dynamic receiver: the call is no longer resolved.
    let dynamic = analyze(
        r#"
        class Announcer { async announce(id: string) { await queue.publish("order.completed", { id }); } }
        export async function completeOrder(id: string, announcer: Announcer) {
          await db.update(orders).set({ status: "paid" });
          await announcer.announce(id);
        }
    "#,
    );
    assert_eq!(codes(&dynamic), ["MW9001"]);

    // A method made reassignable: not resolved either.
    let reassigned = analyze(
        r#"
        class Announcer {
          constructor() { this.announce = async () => {}; }
          async announce(id: string) { await queue.publish("order.completed", { id }); }
        }
        export async function completeOrder(id: string) {
          const announcer = new Announcer();
          await db.update(orders).set({ status: "paid" });
          await announcer.announce(id);
        }
    "#,
    );
    assert_eq!(codes(&reassigned), ["MW9001"]);

    // Sequential → concurrent: confidence drops to low.
    let concurrent = analyze(
        r#"
        async function announce(id: string) { await queue.publish("order.completed", { id }); }
        export async function completeOrder(id: string) {
          await Promise.all([db.update(orders).set({ status: "paid" }), announce(id)]);
        }
    "#,
    );
    assert!(
        concurrent.iter().all(|d| d.confidence == Confidence::Low),
        "{concurrent:#?}"
    );

    // An object escaping into unknown code loses its provenance.
    let escaped = analyze(
        r#"
        export async function completeOrder(id: string) {
          await db.update(orders).set({ id });
          const payload = { id };
          register(payload);
          await queue.publish("order.completed", payload);
        }
    "#,
    );
    assert_eq!(
        escaped[0].value_relation.as_ref().map(|r| r.status),
        Some(RelationKind::Unknown)
    );
}

#[test]
fn return_facts_of_nested_calls_never_merge_away_feasible_paths() {
    // Regression (found on the corpus): `load`'s returns state facts about
    // `account`, a binding of `activate`, which is itself inlined into
    // `run`. Leaving `load`, the two returns must not be merged, or the
    // path where `account` is an object (and the transaction commits) is
    // lost together with the MW1004.
    let source = r#"
        async function load(tx: any, id: string) {
          const row = await tx.select().from(lifecycle);
          if (!row) return null;
          return { status: derive(row), identity: { active: row.status === "active" } };
        }
        async function activate(id: string) {
          try {
            return await db.transaction(async (tx) => {
              const account = await load(tx, id);
              if (!account || account.status !== "pending") { throw new Error("unavailable"); }
              await tx.update(accounts).set({ status: "active" });
              return { ok: true };
            });
          } catch (error) {
            throw error;
          }
        }
        async function provision(id: string) {
          await db.transaction(async (tx) => { await tx.insert(accounts).values({ id }); });
          return { id };
        }
        export async function run(id: string) {
          const account = await provision(id);
          await activate(account.id);
        }
    "#;
    assert_eq!(
        codes(
            &of(&analyze(source), "run")
                .into_iter()
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["MW1004"]
    );
}
