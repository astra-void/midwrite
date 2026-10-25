//! Phase 2 behavior, end to end: branch-condition correlation, parameter
//! handles, the transaction model, and project analysis.

use std::path::{Path, PathBuf};

use commitrift_analyzer::Analyzer;
use commitrift_core::analysis::AnalysisOptions;
use commitrift_core::config::Config;
use commitrift_core::diagnostic::{Confidence, Diagnostic, Policy, Severity};
use commitrift_core::ir::{DbHandle, EffectKind, Fact};

fn analyze(source: &str) -> Vec<Diagnostic> {
    Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

fn hazards(source: &str) -> Vec<&'static str> {
    codes(&analyze(source))
        .into_iter()
        .filter(|code| *code != "CP9001")
        .collect()
}

// --- Branch condition correlation (2.1) --------------------------------------

#[test]
fn contradictory_boolean_branches_are_eliminated() {
    let source = r#"
        export async function settle(id: string, flag: boolean) {
          if (flag) {
            await db.update(orders).set({ status: "paid" });
          }
          if (!flag) {
            await queue.publish("order.paid", { id });
          }
        }
    "#;
    assert!(analyze(source).is_empty(), "{:#?}", analyze(source));
}

#[test]
fn compatible_branches_are_preserved() {
    let source = r#"
        export async function settle(id: string, flag: boolean) {
          if (flag) {
            await db.update(orders).set({ status: "paid" });
          }
          if (flag === true) {
            await queue.publish("order.paid", { id });
          }
        }
    "#;
    let diagnostics = analyze(source);
    assert_eq!(codes(&diagnostics), ["CP1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
}

#[test]
fn reassignment_invalidates_knowledge() {
    let source = r#"
        export async function settle(id: string, input: boolean) {
          let flag = input;
          if (flag) {
            await db.update(orders).set({ status: "paid" });
          }
          flag = !flag;
          if (!flag) {
            await queue.publish("order.paid", { id });
          }
        }
    "#;
    assert_eq!(hazards(source), ["CP1001"]);
}

#[test]
fn members_calls_and_complex_conditions_stay_unknown() {
    for (first, second) in [
        ("input.offline", "!input.offline"),
        ("isOffline()", "!isOffline()"),
        ("a > 1", "a <= 1"),
        ("typeof a === \"string\"", "typeof a !== \"string\""),
    ] {
        let source = format!(
            r#"
            export async function settle(id: string, input: any, a: any) {{
              if ({first}) {{ await db.update(orders).set({{ status: "paid" }}); }}
              if ({second}) {{ await queue.publish("order.paid", {{ id }}); }}
            }}
            "#
        );
        assert_eq!(hazards(&source), ["CP1001"], "{first} / {second}");
    }
}

#[test]
fn strict_equalities_with_different_literals_are_exclusive() {
    let source = r#"
        export async function settle(id: string, status: string) {
          if (status === "paid") {
            await db.update(orders).set({ status });
          }
          if (status === "refunded") {
            await queue.publish("order.refunded", { id });
          }
        }
    "#;
    assert!(hazards(source).is_empty());
    // Loose equality with strings is not evaluated.
    let loose = source.replace("===", "==");
    assert_eq!(hazards(&loose), ["CP1001"]);
}

#[test]
fn switch_cases_and_ternaries_correlate_with_later_tests() {
    let source = r#"
        export async function route(id: string, kind: string, flag: boolean) {
          switch (kind) {
            case "a":
              await db.update(orders).set({ id });
              break;
          }
          if (kind === "b") {
            await queue.publish("b", { id });
          }
          await (flag ? db.insert(audit).values({ id }) : null);
          await (flag ? null : queue.publish("not-flag", { id }));
        }
    "#;
    let diagnostics = analyze(source);
    assert!(
        diagnostics
            .iter()
            .all(|d| d.message.contains("`queue.publish(\"b\"")
                || !d.message.contains("not-flag")
                || !d.message.contains("db.insert(audit)")),
        "{diagnostics:#?}"
    );
    for d in &diagnostics {
        let pair = (
            d.message.contains("db.update(orders)"),
            d.message.contains("queue.publish(\"b\""),
        );
        assert!(
            pair != (true, true),
            "case \"a\" and kind === \"b\" exclude each other: {}",
            d.message
        );
        let pair = (
            d.message.contains("db.insert(audit)"),
            d.message.contains("not-flag"),
        );
        assert!(
            pair != (true, true),
            "the two ternaries exclude each other: {}",
            d.message
        );
    }
}

#[test]
fn constant_conditions_remove_arms() {
    let source = r#"
        const ENABLED = false;
        export async function pay(id: string) {
          await db.update(orders).set({ status: "paid" });
          if (ENABLED) {
            await queue.publish("order.paid", { id });
          }
          if (false) {
            await queue.publish("never", { id });
          }
        }
    "#;
    assert!(analyze(source).is_empty(), "{:#?}", analyze(source));
}

#[test]
fn conditions_are_recorded_as_facts_in_the_ir() {
    let ir = Analyzer::new(Config::default())
        .lower_source(
            "test.ts",
            r#"
            export async function f(flag: boolean, x: string | null) {
              if (flag && x == null) { await db.insert(t).values({}); }
            }
            "#,
        )
        .unwrap();
    let commitrift_core::ir::Node::Branch(branch) = &ir.functions[0].body[0] else {
        panic!("{}", ir.to_text());
    };
    assert_eq!(branch.arms[0].facts.len(), 2, "{}", ir.to_text());
    assert!(
        matches!(&branch.arms[0].facts[0], Fact::Truthy { variable, value: true } if variable.starts_with("flag#"))
    );
    assert!(
        matches!(&branch.arms[0].facts[1], Fact::Truthy { variable, value: false } if variable.starts_with("x#"))
    );
    assert!(
        branch.arms[1].facts.is_empty(),
        "`!(a && b)` implies nothing"
    );
}

// --- Parameters (3.4), single file -------------------------------------------

#[test]
fn a_helper_receiving_the_transaction_handle_joins_the_transaction() {
    // Phase 1 reported CP1004 (high) here: the helper's parameter `db` was
    // analyzed as the root handle although the caller passes `tx`.
    let source = r#"
        import { db } from "@/db";
        async function saveOrder(db: Database, id: string) {
          await db.update(orders).set({ status: "paid" });
        }
        export async function checkout(id: string) {
          await db.transaction(async (tx) => {
            await tx.insert(payments).values({ id });
            await saveOrder(tx, id);
          });
        }
    "#;
    let analysis = Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .unwrap();
    assert!(
        analysis.diagnostics.is_empty(),
        "{:#?}",
        analysis.diagnostics
    );
    let checkout = &analysis.ir.functions[1];
    let write = checkout
        .effects()
        .into_iter()
        .find(|e| e.label == "db.update(orders)")
        .unwrap();
    assert!(matches!(
        write.kind,
        EffectKind::DatabaseWrite {
            handle: DbHandle::Transaction { .. },
            ..
        }
    ));
}

#[test]
fn a_helper_receiving_the_root_handle_inside_a_transaction_bypasses_it() {
    let source = r#"
        import { db } from "@/db";
        async function saveOrder(executor: Executor, id: string) {
          await executor.update(orders).set({ status: "paid" });
        }
        export async function checkout(id: string) {
          await db.transaction(async (tx) => {
            await saveOrder(db, id);
            await tx.insert(payments).values({ id });
          });
        }
    "#;
    let diagnostics = analyze(source);
    assert_eq!(codes(&diagnostics), ["CP1004"]);
    assert_eq!(diagnostics[0].severity, Severity::Warning);
    assert!(
        diagnostics[0]
            .mitigation
            .contains("Pass the transaction handle")
    );
}

// --- Transaction model audit (2.4) -------------------------------------------

#[test]
fn transaction_model_audit() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "writes on the handle are atomic",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 await tx.insert(outbox).values({ s: 1 });
               });"#,
            &[],
        ),
        (
            "the root handle inside the callback commits on its own",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 await db.insert(audit).values({ s: 1 });
               });"#,
            &["CP1004"],
        ),
        (
            "an external effect before commit is not made atomic",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 await queue.publish("x", {});
               });"#,
            &["CP1003"],
        ),
        (
            "an awaited write inside a transaction is not durable before commit",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 throw new Error("abort");
               });
               await queue.publish("x", {});"#,
            &[],
        ),
        (
            "an early return commits",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 return;
               });
               await queue.publish("x", {});"#,
            &["CP1001"],
        ),
        (
            "a rolled-back savepoint discards only its own writes",
            r#"await db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
                 try {
                   await tx.transaction(async (sp) => {
                     await sp.insert(audit).values({ s: 1 });
                     sp.rollback();
                   });
                 } catch {}
               });
               await queue.publish("x", {});"#,
            &["CP1001"],
        ),
        (
            "an opaque transaction is a limitation, not silence",
            r#"await db.update(orders).set({ s: 1 });
               await db.transaction(handler);"#,
            &["CP9001"],
        ),
        (
            "an unawaited transaction is unordered",
            r#"db.transaction(async (tx) => {
                 await tx.update(orders).set({ s: 1 });
               });
               await queue.publish("x", {});"#,
            &["CP1001"],
        ),
    ];
    for (name, body, expected) in cases {
        let source = format!("export async function f() {{\n{body}\n}}\n");
        let diagnostics = analyze(&source);
        assert_eq!(codes(&diagnostics), *expected, "{name}: {diagnostics:#?}");
    }
    // The unawaited transaction's ordering is approximated.
    let unawaited = analyze(&format!(
        "export async function f() {{\n{}\n}}\n",
        cases[7].1
    ));
    assert_eq!(unawaited[0].confidence, Confidence::Medium);
}

// --- Single-file API stays single-file (3.6) ---------------------------------

#[test]
fn analyze_source_does_not_follow_imports() {
    let source = r#"
        import { notify } from "./notify";
        export async function f(id: string) {
          await db.update(orders).set({ id });
          await notify(id);
        }
    "#;
    let diagnostics = analyze(source);
    assert_eq!(codes(&diagnostics), ["CP9001"]);
    assert!(
        diagnostics[0].evidence[0].contains("no configuration entry or supported API matches it")
    );
}

// --- Project analysis --------------------------------------------------------

fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("phase2")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in files {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn project_codes(root: &Path, config: Config) -> Vec<(String, String, u32)> {
    let analysis = Analyzer::new(config)
        .with_options(AnalysisOptions {
            policy: Policy::Strict,
            ..AnalysisOptions::default()
        })
        .analyze_project(&[root.to_path_buf()])
        .unwrap();
    assert!(analysis.failures.is_empty(), "{:?}", analysis.failures);
    let mut out: Vec<(String, String, u32)> = analysis
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .map(|d| {
            let file = Path::new(&d.location.file)
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| d.location.file.clone());
            (d.code.as_str().to_owned(), file, d.location.line)
        })
        .collect();
    out.sort();
    out
}

const MARK_PAID: &str = "import { db } from \"@/db\";\nexport async function markPaid(id: string) {\n  await db.update(orders).set({ id });\n}\n";
const CHECKOUT: &str = "export async function checkout(id: string) {\n  await markPaid(id);\n  await queue.publish(\"paid\", { id });\n}\n";

#[test]
fn workspace_packages_resolve_through_exports() {
    let root = project(
        "workspace",
        &[
            (
                "packages/db/package.json",
                r#"{ "name": "@acme/db", "exports": { "./queries": "./src/queries/index.ts" } }"#,
            ),
            (
                "packages/db/src/queries/index.ts",
                "export * from \"./orders\";\n",
            ),
            ("packages/db/src/queries/orders.ts", MARK_PAID),
            ("apps/api/package.json", r#"{ "name": "@acme/api" }"#),
            (
                "apps/api/src/route.ts",
                &format!("import {{ markPaid }} from \"@acme/db/queries\";\n{CHECKOUT}"),
            ),
        ],
    );
    assert_eq!(
        project_codes(&root, Config::default()),
        [("CP1001".to_owned(), "apps/api/src/route.ts".to_owned(), 4)]
    );
}

#[test]
fn tsconfig_extends_chains_provide_paths() {
    let root = project(
        "extends",
        &[
            (
                "tsconfig.base.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["src/*"] } } }"#,
            ),
            (
                "app/tsconfig.json",
                r#"{ "extends": "../tsconfig.base.json" }"#,
            ),
            ("src/orders.ts", MARK_PAID),
            (
                "app/route.ts",
                &format!("import {{ markPaid }} from \"~/orders\";\n{CHECKOUT}"),
            ),
        ],
    );
    assert_eq!(
        project_codes(&root, Config::default()),
        [("CP1001".to_owned(), "app/route.ts".to_owned(), 4)]
    );
}

#[test]
fn ambiguous_module_candidates_are_not_chosen() {
    let root = project(
        "ambiguous_files",
        &[
            ("orders.ts", MARK_PAID),
            ("orders.tsx", MARK_PAID),
            (
                "route.ts",
                &format!("import {{ markPaid }} from \"./orders\";\n{CHECKOUT}"),
            ),
        ],
    );
    let analysis = Analyzer::new(Config::default())
        .analyze_project(std::slice::from_ref(&root))
        .unwrap();
    assert_eq!(analysis.counts.cross_file_calls_unresolved, 1);
    assert_eq!(analysis.counts.cross_file_calls_resolved, 0);
    assert!(
        project_codes(&root, Config::default())
            .iter()
            .all(|(code, ..)| code == "CP9001")
    );
}

#[test]
fn excluded_files_are_not_analyzed() {
    let root = project(
        "exclude",
        &[
            ("src/orders.ts", MARK_PAID),
            (
                "src/route.ts",
                &format!("import {{ markPaid }} from \"./orders\";\n{CHECKOUT}"),
            ),
            (
                "tests/route.test.ts",
                &format!("import {{ markPaid }} from \"../src/orders\";\n{CHECKOUT}"),
            ),
        ],
    );
    assert_eq!(project_codes(&root, Config::default()).len(), 2);
    let config = Config::from_json(r#"{ "exclude": ["tests/**"] }"#, "test").unwrap();
    assert_eq!(project_codes(&root, config).len(), 1);
}

#[test]
fn project_analysis_is_deterministic() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let config = Config::load(&root.join("commitrift.json")).unwrap();
    let run = || {
        let report = Analyzer::new(config.clone())
            .analyze_paths(std::slice::from_ref(&root), None)
            .unwrap();
        serde_json::to_string(&report).unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn project_analysis_keeps_single_file_results_for_the_fixtures() {
    // Fixtures do not import each other, so project analysis must report
    // exactly what per-file analysis reports.
    let root =
        std::fs::canonicalize(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures"))
            .unwrap();
    let config = Config::load(&root.join("commitrift.json")).unwrap();
    let analyzer = Analyzer::new(config);
    let dirs: Vec<PathBuf> = ["vulnerable", "safe", "unsupported"]
        .iter()
        .map(|d| root.join(d))
        .collect();
    let project = analyzer.analyze_project(&dirs).unwrap();
    for file in &project.files {
        let single = analyzer
            .analyze_file(Path::new(&file.ir.path))
            .expect("parses");
        let key = |d: &Diagnostic| (d.code, d.location.line, d.function.clone());
        let mut a: Vec<_> = file.diagnostics.iter().map(key).collect();
        let mut b: Vec<_> = single.diagnostics.iter().map(key).collect();
        a.sort();
        b.sort();
        assert_eq!(a, b, "{}", file.ir.path);
    }
}

// --- Handles returned by calls -----------------------------------------------

fn analyze_with(json: &str, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(Config::from_json(json, "test").unwrap())
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

#[test]
fn configured_call_paths_denote_the_returned_handle() {
    let config = r#"{ "databaseReceivers": ["getDb()"] }"#;
    let direct = r#"
        export async function f(id: string) {
          await getDb().update(orders).set({ id });
          await queue.publish("x", { id });
        }
    "#;
    assert_eq!(codes(&analyze_with(config, direct)), ["CP1001"]);
    // Without the `()` entry the chain is not a database chain.
    assert!(analyze(direct).is_empty());
    // A `const` alias of the call is the same handle; `let` is not followed.
    let alias = r#"
        export async function f(id: string) {
          const conn = getDb();
          await conn.update(orders).set({ id });
          await queue.publish("x", { id });
        }
    "#;
    assert_eq!(codes(&analyze_with(config, alias)), ["CP1001"]);
    assert!(analyze_with(config, &alias.replace("const conn", "let conn")).is_empty());
    // Calls with arguments are a different call.
    let with_arguments = direct.replace("getDb()", "getDb(\"replica\")");
    assert!(analyze_with(config, &with_arguments).is_empty());
}

#[test]
fn call_paths_respect_imported_from_and_bind_as_arguments() {
    let config =
        r#"{ "databaseReceivers": [{ "name": "getDb()", "importedFrom": "@/server/db" }] }"#;
    let source = r#"
        import { getDb } from "@/server/db";
        import { getDb as legacy } from "@/legacy";
        async function save(executor: Executor, id: string) {
          await executor.update(orders).set({ id });
        }
        export async function f(id: string) {
          await getDb().transaction(async (tx) => {
            await tx.insert(audit).values({ id });
            await save(getDb(), id);
          });
        }
        export async function g(id: string) {
          await legacy().update(orders).set({ id });
          await queue.publish("x", { id });
        }
    "#;
    let diagnostics = analyze_with(config, source);
    assert_eq!(codes(&diagnostics), ["CP1004"], "{diagnostics:#?}");
    assert_eq!(diagnostics[0].function, "f");
    assert!(
        diagnostics[0]
            .evidence
            .iter()
            .any(|e| e.contains("returns is trusted"))
    );
}

// --- Best-effort external effects --------------------------------------------

const BEST_EFFORT_CONFIG: &str = r#"{
  "externalEffects": [
    { "function": "triggerJob", "firstArgument": "notification", "category": "message-publish", "bestEffort": true },
    { "function": "triggerJob", "category": "message-publish" }
  ]
}"#;

fn analyze_policy(json: &str, policy: Policy, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(Config::from_json(json, "test").unwrap())
        .with_options(AnalysisOptions {
            policy,
            ..AnalysisOptions::default()
        })
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

#[test]
fn best_effort_effects_after_a_commit_are_informational() {
    let source = r#"
        export async function pay(id: string) {
          await db.update(invoices).set({ status: "paid" });
          await triggerJob("notification", { id });
        }
        export async function send(id: string) {
          await db.update(invoices).set({ status: "unpaid" });
          await triggerJob(`generate-invoice`, { id });
        }
        export async function remind(id: string) {
          await triggerJob(`notification`, { id });
          await db.update(invoices).set({ remindedAt: new Date() });
        }
    "#;
    let recommended = analyze_policy(BEST_EFFORT_CONFIG, Policy::Recommended, source);
    let find = |function: &str| {
        recommended
            .iter()
            .find(|d| d.function == function)
            .unwrap_or_else(|| panic!("{function}: {recommended:#?}"))
    };
    let pay = find("pay");
    assert_eq!(pay.code.as_str(), "CP1001");
    assert_eq!(pay.severity, Severity::Info);
    assert!(pay.limitations.iter().any(|l| l.contains("best-effort")));
    assert!(
        pay.evidence
            .iter()
            .any(|e| e.contains("triggerJob(\"notification\", …)"))
    );
    // A different first argument matches the general rule: still a warning.
    assert_eq!(find("send").severity, Severity::Warning);
    // The best-effort effect happened first: the claim is about the commit,
    // which best-effort does not cover.
    let remind = find("remind");
    assert_eq!(remind.code.as_str(), "CP1002");
    assert_eq!(remind.severity, Severity::Warning);
    // `strict` keeps catalog severities.
    let strict = analyze_policy(BEST_EFFORT_CONFIG, Policy::Strict, source);
    assert!(strict.iter().all(|d| d.severity == Severity::Warning));
}

#[test]
fn transaction_results_correlate_with_later_tests() {
    let source = r#"
        export async function withdraw(id: string) {
          const result = await db.transaction(async (tx) => {
            if (!(await tx.select().from(t))) return "forbidden" as const;
            await tx.update(t).set({ s: 1 });
            return { ok: true };
          });
          if (result === "forbidden") {
            await db.insert(audit).values({});
          }
        }
    "#;
    assert!(analyze(source).is_empty(), "{:#?}", analyze(source));
    // A literal returned after the write is compatible with the test.
    let compatible = source.replace("return { ok: true };", "return \"forbidden\";");
    assert_eq!(codes(&analyze(&compatible)), ["CP1004"]);
    // A result variable that is reassigned is not a variable any more.
    let reassigned = source.replace("const result", "let result").replace(
        "if (result ===",
        "result = await refresh();\n          if (result ===",
    );
    assert_eq!(hazards(&reassigned), ["CP1004"]);
}
