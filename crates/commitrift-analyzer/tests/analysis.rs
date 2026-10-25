//! Targeted end-to-end tests: TypeScript source in, IR and diagnostics out.

use std::path::PathBuf;

use commitrift_analyzer::{Analyzer, DiscoveryError, discover};
use commitrift_core::analysis::AnalysisOptions;
use commitrift_core::config::Config;
use commitrift_core::diagnostic::{Confidence, Diagnostic, DiagnosticKind, EffectRole};
use commitrift_core::ir::{DbHandle, EffectKind, Execution, Node, RecognitionBasis};

fn analyze(source: &str) -> Vec<Diagnostic> {
    analyze_with(Config::default(), source)
}

fn analyze_with(config: Config, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(config)
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn config(json: &str) -> Config {
    Config::from_json(json, "test").expect("valid config")
}

// --- The six examples from the Phase 1 specification, verbatim -------------

#[test]
fn example_a_db_commit_before_event_publication() {
    let diagnostics = analyze(
        r#"
        async function completeOrder(id: string) {
          await db.update(orders).set({ status: "paid" });

          await queue.publish("order.completed", { id });
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1001"]);
    let d = &diagnostics[0];
    assert_eq!(d.kind, DiagnosticKind::PotentialHazard);
    assert_eq!(d.confidence, Confidence::High);
    assert_eq!(d.function, "completeOrder");
    assert_eq!((d.location.line, d.location.column), (5, 17));
    let roles: Vec<EffectRole> = d.effects.iter().map(|e| e.role).collect();
    assert_eq!(
        roles,
        [EffectRole::CompletedFirst, EffectRole::NotYetPerformed]
    );
    assert_eq!(d.effects[0].label, "db.update(orders)");
    assert_eq!(d.effects[0].location.line, 3);
    assert!(d.failure_sequence[0].contains("commits immediately"));
    assert!(
        d.evidence
            .iter()
            .any(|e| e.contains("databaseReceivers entry `db`"))
    );
    assert!(
        d.evidence
            .iter()
            .any(|e| e.contains("externalEffects entry `queue.publish`"))
    );
    assert!(d.mitigation.contains("outbox"));
}

#[test]
fn example_b_external_effect_before_database_commit() {
    let diagnostics = analyze(
        r#"
        async function completeOrder(id: string) {
          await queue.publish("order.completed", { id });

          await db.update(orders).set({ status: "paid" });
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1002"]);
    assert_eq!(
        diagnostics[0].location.line, 3,
        "points at the external effect"
    );
}

#[test]
fn example_c_transactional_outbox_has_no_cross_resource_finding() {
    let diagnostics = analyze(
        r#"
        async function completeOrder(id: string) {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({ status: "paid" });

            await tx.insert(outbox).values({
              topic: "order.completed",
              payload: { id },
            });
          });
        }
        "#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn example_d_external_publication_inside_transaction() {
    let diagnostics = analyze(
        r#"
        async function completeOrder(id: string) {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({ status: "paid" });

            await queue.publish("order.completed", { id });
          });
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1003"]);
    let d = &diagnostics[0];
    assert_eq!(d.location.line, 6);
    assert!(
        d.effects
            .iter()
            .any(|e| e.role == EffectRole::StagedWrite && e.label == "tx.update(orders)")
    );
    assert!(d.effects.iter().any(|e| e.role == EffectRole::Transaction));
    assert!(
        d.evidence
            .iter()
            .any(|e| e.contains("transaction callback parameter"))
    );
}

#[test]
fn example_e_safe_local_computation() {
    let diagnostics = analyze(
        r#"
        async function calculatePrice(amount: number) {
          const tax = amount * 0.1;
          return amount + tax;
        }
        "#,
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn example_f_unknown_effect_is_a_limitation_not_a_hazard() {
    let analysis = Analyzer::new(Config::default())
        .analyze_source(
            "test.ts",
            r#"
            async function completeOrder(orderId: string) {
              await db.update(orders).set({ status: "paid" });
              await unknownService.process(orderId);
            }
            "#,
        )
        .unwrap();
    assert_eq!(codes(&analysis.diagnostics), ["CP9001"]);
    let d = &analysis.diagnostics[0];
    assert_eq!(d.kind, DiagnosticKind::AnalysisLimitation);
    assert_eq!(d.confidence, Confidence::Low);
    assert!(
        d.message
            .contains("does not know whether it has side effects")
    );

    let effects = analysis.ir.functions[0].effects();
    assert!(matches!(effects[1].kind, EffectKind::Unknown { .. }));
    assert!(
        effects[1].recognition.is_none(),
        "unknown effects carry no recognition claim"
    );
}

// --- IR shape ------------------------------------------------------------

#[test]
fn unexecuted_builders_produce_no_effects() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            async function f() {
              db.update(orders).set({ status: "draft" });
              const sqlText = db.insert(orders).values({}).toSQL();
              const prepared = db.delete(orders).prepare("p");
            }
            "#,
        )
        .unwrap();
    assert!(ir.functions[0].effects().is_empty(), "{}", ir.to_text());
}

#[test]
fn explicit_execute_without_await_is_fire_and_forget() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            "async function f() { db.update(orders).set({ a: 1 }).execute(); }",
        )
        .unwrap();
    let effects = ir.functions[0].effects();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].execution, Execution::FireAndForget);
}

#[test]
fn async_return_executes_a_builder_but_sync_return_does_not() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            const executes = async (id: string) => db.update(orders).set({ id });
            const builds = (id: string) => db.update(orders).set({ id });
            "#,
        )
        .unwrap();
    assert_eq!(ir.functions[0].name, "executes");
    assert_eq!(ir.functions[0].effects()[0].execution, Execution::Returned);
    assert_eq!(ir.functions[1].name, "builds");
    assert!(ir.functions[1].effects().is_empty());
}

#[test]
fn transaction_callbacks_are_lowered_inline_not_as_separate_functions() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            async function f() {
              await db.transaction(async (tx) => {
                await tx.update(orders).set({});
                await tx.transaction(async (inner) => {
                  await inner.insert(items).values({});
                });
              });
            }
            "#,
        )
        .unwrap();
    assert_eq!(ir.functions.len(), 1, "{}", ir.to_text());
    let function = &ir.functions[0];
    assert_eq!(function.transactions.len(), 2);
    assert_eq!(
        function.transactions[1].parent,
        Some(function.transactions[0].id)
    );
    assert_eq!(function.transactions[0].handle.as_deref(), Some("tx"));
    let text = function.to_text();
    assert!(text.contains("tx#0 begin db.transaction"), "{text}");
    assert!(text.contains("tx#1 begin tx.transaction"), "{text}");
    assert!(text.contains("nested in tx#0"), "{text}");
    assert!(matches!(function.body[0], Node::Transaction(_)));
}

#[test]
fn evaluation_order_puts_awaited_arguments_first() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            async function f() {
              await queue.publish("x", { row: await db.insert(log).values({}) });
            }
            "#,
        )
        .unwrap();
    let labels: Vec<&str> = ir.functions[0]
        .effects()
        .iter()
        .map(|e| e.label.as_str())
        .collect();
    assert_eq!(labels[0], "db.insert(log)");
    assert!(labels[1].starts_with("queue.publish"));
}

#[test]
fn nested_functions_are_separate_units_with_qualified_names() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            export async function outer(items: string[]) {
              const record = async (item: string) => {
                await db.insert(t).values({ item });
              };
              items.forEach(async (item) => {
                await db.insert(t).values({ item });
              });
            }
            "#,
        )
        .unwrap();
    assert_eq!(ir.functions.len(), 3);
    // Defining a callback runs nothing: `record` is never called. Passing
    // one to `forEach` runs it once per element without awaiting it (Phase
    // 5): the insert appears in `outer` as fire-and-forget, inside an
    // "array callback" loop, inlined from the callback's own unit.
    let outer = ir.functions[0].effects();
    assert_eq!(outer.len(), 1, "{}", ir.to_text());
    assert_eq!(
        outer[0].execution,
        commitrift_core::ir::Execution::FireAndForget
    );
    assert!(outer[0].summary.is_some());
    assert!(
        ir.functions[2].name.starts_with("outer::<anonymous@6:"),
        "{}",
        ir.functions[2].name
    );
    assert_eq!(ir.functions[2].effects().len(), 1);
}

// --- Recognition and provenance ---------------------------------------------

#[test]
fn similar_method_names_on_unrelated_objects_are_ignored() {
    let diagnostics = analyze(
        r#"
        async function f(events: any, store: Map<string, string>) {
          store.delete("k");
          await events.publish("topic");
          await cache.update("k", 1);
          await array.insert(0, 1);
        }
        "#,
    );
    assert!(diagnostics.is_empty(), "{diagnostics:#?}");
}

#[test]
fn shadowing_with_an_obviously_different_value_disables_name_matching() {
    let ir = Analyzer::default()
        .lower_source(
            "test.ts",
            r#"
            async function f() {
              const db = new Map();
              await db.delete("k");
              const queue = { publish: async () => {} };
              await queue.publish();
            }
            "#,
        )
        .unwrap();
    assert!(
        ir.functions[0]
            .effects()
            .iter()
            .all(|e| e.recognition.is_none()),
        "{}",
        ir.to_text()
    );
}

#[test]
fn imported_from_requires_the_configured_module() {
    let config = || {
        config(
            r#"{ "databaseReceivers": [{ "name": "db", "importedFrom": "@/db" }],
                 "externalEffects": [{ "function": "sendEmail", "importedFrom": "@/lib/email", "category": "email" }] }"#,
        )
    };
    let matching = analyze_with(
        config(),
        r#"
        import { db } from "@/db";
        import { sendEmail as notify } from "@/lib/email";
        async function f() {
          await db.update(orders).set({});
          await notify("a@example.com");
        }
        "#,
    );
    assert_eq!(codes(&matching), ["CP1001"]);
    assert!(
        matching[0]
            .evidence
            .iter()
            .any(|e| e.contains("import-based match"))
    );

    let other_module = analyze_with(
        config(),
        r#"
        import { db } from "./legacy-db";
        import { sendEmail } from "./other";
        async function f() {
          await db.update(orders).set({});
          await sendEmail("a@example.com");
        }
        "#,
    );
    assert!(other_module.is_empty(), "{other_module:#?}");

    let global = analyze_with(
        config(),
        "async function f() { await db.update(orders).set({}); await db.insert(x).values({}); }",
    );
    assert!(
        global.is_empty(),
        "an undeclared `db` does not satisfy importedFrom"
    );
}

#[test]
fn drizzle_constructor_is_strong_provenance() {
    let analysis = Analyzer::default()
        .analyze_source(
            "test.ts",
            r#"
            import { drizzle } from "drizzle-orm/postgres-js";
            const database = drizzle(client);
            async function f() {
              await database.insert(t).values({});
              await queue.publish("x");
            }
            "#,
        )
        .unwrap();
    assert_eq!(codes(&analysis.diagnostics), ["CP1001"]);
    let recognition = analysis.ir.functions[0].effects()[0]
        .recognition
        .clone()
        .unwrap();
    assert_eq!(recognition.basis, RecognitionBasis::DrizzleConstructor);
}

#[test]
fn transaction_handle_alias_is_followed() {
    let diagnostics = analyze(
        r#"
        async function f() {
          await db.transaction(async (tx) => {
            const t = tx;
            await t.update(orders).set({});
            await queue.publish("x");
          });
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1003"]);
}

#[test]
fn explicit_rollback_path_still_reports_cp1003() {
    let diagnostics = analyze(
        r#"
        async function f(ok: boolean) {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({});
            await queue.publish("x");
            if (!ok) {
              tx.rollback();
            }
          });
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1003"]);
}

#[test]
fn return_inside_transaction_callback_commits_and_continues() {
    // `return` ends the callback (the transaction commits), not the function.
    let diagnostics = analyze(
        r#"
        async function f(skip: boolean) {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({});
            if (skip) return;
            await tx.insert(log).values({});
          });
          await queue.publish("x");
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::High);
}

#[test]
fn transaction_started_on_root_inside_callback_is_independent() {
    let diagnostics = analyze(
        r#"
        async function f() {
          await db.transaction(async (tx) => {
            await tx.update(orders).set({});
            await db.transaction(async (other) => {
              await other.insert(log).values({});
            });
          });
        }
        "#,
    );
    assert_eq!(
        codes(&diagnostics),
        ["CP1004"],
        "two independent root transactions"
    );
}

#[test]
fn known_false_positive_parameter_named_like_receiver() {
    // Name-based matching trusts any binding called `db` unless the file shows
    // otherwise. This documents the behavior; see docs/LIMITATIONS.md.
    let diagnostics = analyze(
        r#"
        async function f(db: Map<string, number>) {
          await db.delete("k");
          await queue.publish("x");
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1001"]);
    assert!(
        diagnostics[0]
            .evidence
            .iter()
            .any(|e| e.contains("name-based match"))
    );
}

// --- Member receivers ---------------------------------------------------------

fn member_config() -> Config {
    config(
        r#"{ "databaseReceivers": ["db", "this.db", "ctx.db"],
             "externalEffects": [
               { "receiver": "this.queue", "method": "publish", "category": "message-publish" },
               { "receiver": "queue", "method": "publish", "category": "message-publish" }
             ] }"#,
    )
}

#[test]
fn configured_member_paths_are_matched_with_a_weaker_basis() {
    let analysis = Analyzer::new(member_config())
        .analyze_source(
            "test.ts",
            r#"
            class OrderService {
              async complete(id: string) {
                await this.db.update(orders).set({ status: "paid" });
                await this.queue.publish("order.completed", { id });
              }
            }
            async function procedure({ ctx }: any) {
              await ctx.db.insert(orders).values({});
              await queue.publish("x");
            }
            "#,
        )
        .unwrap();
    assert_eq!(codes(&analysis.diagnostics), ["CP1001", "CP1001"]);
    let effects = analysis.ir.functions[0].effects();
    let write = effects[0].recognition.as_ref().unwrap();
    assert_eq!(write.basis, RecognitionBasis::ConfiguredMemberPath);
    assert_eq!(write.receiver, "this.db");
    assert!(
        matches!(&effects[0].kind, EffectKind::DatabaseWrite { handle: DbHandle::Root { name }, .. } if name == "this.db")
    );
    assert_eq!(
        effects[1].recognition.as_ref().unwrap().basis,
        RecognitionBasis::ConfiguredMemberPath
    );
    let procedure = analysis.ir.functions[1].effects();
    assert_eq!(
        procedure[0].recognition.as_ref().unwrap().receiver,
        "ctx.db"
    );
    for diagnostic in &analysis.diagnostics {
        assert!(
            diagnostic
                .evidence
                .iter()
                .any(|e| e.contains("member-path match")),
            "{:#?}",
            diagnostic.evidence
        );
    }
    assert!(
        analysis.diagnostics[0]
            .evidence
            .iter()
            .any(|e| e.contains("`this` is not resolved")),
        "{:#?}",
        analysis.diagnostics[0].evidence
    );
}

#[test]
fn member_path_transactions_and_bypasses_are_modeled() {
    let diagnostics = analyze_with(
        member_config(),
        r#"
        class OrderService {
          async complete(id: string) {
            await this.db.transaction(async (tx) => {
              await tx.update(orders).set({ status: "paid" });
              await this.db.insert(audit).values({ id });
            });
          }
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1004"]);
    assert!(
        diagnostics[0]
            .mitigation
            .contains("instead of on `this.db`"),
        "{}",
        diagnostics[0].mitigation
    );
}

#[test]
fn unconfigured_member_paths_stay_unknown_with_a_hint() {
    let diagnostics = analyze_with(
        member_config(),
        r#"
        async function f(deps: any, ctx: any) {
          await deps.db.update(orders).set({});
          await ctx.queue.publish("x");
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP9001", "CP9001"]);
    assert!(
        diagnostics[0]
            .message
            .contains("add `deps.db` to `databaseReceivers`"),
        "{}",
        diagnostics[0].message
    );
    assert!(
        diagnostics[1]
            .message
            .contains("resembles the configured external-effect receiver `this.queue`"),
        "{}",
        diagnostics[1].message
    );
    // With only the defaults, `this.db` is a lookalike of `db`.
    let defaults = analyze("class S { async f() { await this.db.update(orders).set({}); } }");
    assert_eq!(codes(&defaults), ["CP9001"]);
}

#[test]
fn member_paths_respect_contrary_roots_and_imports() {
    let config = config(
        r#"{ "databaseReceivers": ["ctx.db", { "name": "services.db", "importedFrom": "@/services" }] }"#,
    );
    let ir = Analyzer::new(config)
        .lower_source(
            "test.ts",
            r#"
            import { services } from "@/services";
            import { services as other } from "./other";
            async function f() {
              const ctx = { db: new Map() };
              await ctx.db.delete("k");
              await services.db.update(orders).set({});
              await other.db.update(orders).set({});
            }
            "#,
        )
        .unwrap();
    let effects = ir.functions[0].effects();
    let recognized: Vec<(&str, RecognitionBasis)> = effects
        .iter()
        .filter_map(|e| {
            e.recognition
                .as_ref()
                .map(|r| (r.receiver.as_str(), r.basis))
        })
        .collect();
    assert_eq!(
        recognized,
        [("services.db", RecognitionBasis::ConfiguredImport)],
        "{}",
        ir.to_text()
    );
}

// --- Same-file helper summaries -----------------------------------------------

#[test]
fn awaited_local_helpers_are_inlined_from_their_summary() {
    let analysis = Analyzer::default()
        .analyze_source(
            "test.ts",
            r#"
            async function notify(id: string) {
              await queue.publish("order.completed", { id });
            }
            async function completeOrder(id: string) {
              await db.update(orders).set({ status: "paid" });
              await notify(id);
            }
            "#,
        )
        .unwrap();
    assert_eq!(codes(&analysis.diagnostics), ["CP1001"]);
    let d = &analysis.diagnostics[0];
    assert_eq!(d.function, "completeOrder");
    assert_eq!(d.confidence, Confidence::High);
    assert_eq!(
        (d.location.line, d.location.column),
        (7, 21),
        "the call site"
    );
    let publish = d
        .effects
        .iter()
        .find(|e| e.role == EffectRole::NotYetPerformed)
        .unwrap();
    assert_eq!(
        publish.location.line, 3,
        "the publish keeps its own location"
    );
    let call = d
        .effects
        .iter()
        .find(|e| e.role == EffectRole::CallSite)
        .unwrap();
    assert_eq!((call.label.as_str(), call.location.line), ("notify(id)", 7));
    assert!(
        d.evidence.iter().any(
            |e| e.contains("taken from the summary of `notify`") && e.contains("on every path")
        ),
        "{:#?}",
        d.evidence
    );

    let caller = &analysis.ir.functions[1];
    let Some(Node::Call(scope)) = caller.body.get(1) else {
        panic!("expected an inlined call: {}", caller.to_text());
    };
    assert_eq!(scope.callee, "notify");
    let inlined = caller.effects();
    let summary = inlined[1].summary.as_ref().expect("summarized effect");
    assert!(summary.on_every_path);
    assert_eq!(summary.calls[0].callee, "notify");
    assert!(caller.to_text().contains("inlines `notify`"));
}

#[test]
fn summaries_can_be_disabled() {
    let source = r#"
        async function notify(id: string) { await queue.publish("x", { id }); }
        async function completeOrder(id: string) {
          await db.update(orders).set({});
          await notify(id);
          return notify(id);
        }
    "#;
    let analysis = Analyzer::default()
        .with_options(AnalysisOptions {
            max_call_depth: 0,
            ..AnalysisOptions::default()
        })
        .analyze_source("test.ts", source)
        .unwrap();
    assert_eq!(codes(&analysis.diagnostics), ["CP9001"]);
    assert!(
        analysis.diagnostics[0]
            .evidence
            .iter()
            .any(|e| e.contains("call summaries are disabled")),
        "{:#?}",
        analysis.diagnostics[0].evidence
    );
    // The returned call is not an unknown effect of its own.
    assert_eq!(analysis.ir.functions[1].effects().len(), 2);
}

#[test]
fn hazards_inside_one_helper_are_reported_once() {
    let diagnostics = analyze(
        r#"
        const finish = async (id: string) => {
          await db.update(orders).set({});
          await queue.publish("x", { id });
        };
        async function a(id: string) { await finish(id); }
        async function b(id: string) { await finish(id); }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP1001"]);
    assert_eq!(diagnostics[0].function, "finish");
}

#[test]
fn shadowed_and_reassignable_helpers_are_not_summarized() {
    let diagnostics = analyze(
        r#"
        async function notify(id: string) { await queue.publish("x", { id }); }
        let later = async (id: string) => { await queue.publish("y", { id }); };
        async function f(id: string, notify: (id: string) => Promise<void>) {
          await db.update(orders).set({});
          await notify(id);
        }
        async function g(id: string) {
          await db.update(orders).set({});
          await later(id);
        }
        async function replaced(id: string) { await queue.publish("z", { id }); }
        replaced = async () => {};
        async function h(id: string) {
          await db.update(orders).set({});
          await replaced(id);
        }
        "#,
    );
    assert_eq!(codes(&diagnostics), ["CP9001", "CP9001", "CP9001"]);
}

#[test]
fn parse_failures_report_a_location() {
    let failure = Analyzer::default()
        .analyze_source(
            "broken.ts",
            "async function f() {\n  await db.update(;\n}\n",
        )
        .unwrap_err();
    assert_eq!(failure.line, Some(2));
    assert!(failure.column.is_some());
}

#[test]
fn tsx_files_parse_jsx() {
    let analysis = Analyzer::default()
        .analyze_source(
            "page.tsx",
            "export function Page() { return <div className=\"x\">{1}</div>; }",
        )
        .expect("JSX parses in .tsx files");
    assert!(analysis.diagnostics.is_empty());
}

// --- Discovery ---------------------------------------------------------------

#[test]
fn discovery_skips_excluded_directories_and_declarations() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("discovery");
    let _ = std::fs::remove_dir_all(&root);
    for dir in [
        "src/lib",
        "node_modules/pkg",
        "dist",
        ".next/server",
        ".git",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in [
        "src/a.ts",
        "src/lib/b.tsx",
        "src/types.d.ts",
        "src/c.js",
        "node_modules/pkg/index.ts",
        "dist/out.ts",
        ".next/server/page.ts",
        ".git/hook.ts",
    ] {
        std::fs::write(root.join(file), "export {};\n").unwrap();
    }
    let files = discover(std::slice::from_ref(&root)).unwrap();
    let names: Vec<String> = files
        .iter()
        .map(|f| {
            f.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert_eq!(names, ["src/a.ts", "src/lib/b.tsx"]);

    // An explicitly named file is analyzed even inside an excluded directory.
    let explicit = discover(&[root.join("dist/out.ts")]).unwrap();
    assert_eq!(explicit.len(), 1);
    assert!(matches!(
        discover(&[root.join("src/c.js")]),
        Err(DiscoveryError::Unsupported(_))
    ));
    assert!(matches!(
        discover(&[root.join("missing")]),
        Err(DiscoveryError::NotFound(_))
    ));
}
