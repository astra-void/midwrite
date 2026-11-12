//! Regressions for effect coverage and source-grounded API contracts.

use std::path::{Path, PathBuf};

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;
use midwrite_core::diagnostic::Diagnostic;

fn analyze(config: Config, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(config)
        .analyze_source("case.ts", source)
        .unwrap()
        .diagnostics
}

fn hazards(diagnostics: &[Diagnostic], function: &str) -> Vec<&'static str> {
    diagnostics
        .iter()
        .filter(|d| d.function == function && d.code.as_str().starts_with("MW1"))
        .map(|d| d.code.as_str())
        .collect()
}

#[test]
fn callback_cache_capacity_does_not_erase_later_effects() {
    let mut source =
        "async function run(action: () => Promise<void>) { await action(); }\n".to_owned();
    for i in 0..12 {
        source.push_str(&format!("async function entry{i}(id: string) {{ await db.insert(items).values({{id}}); await run(async () => {{ await queue.publish('sent', {{id}}); }}); }}\n"));
    }
    let diagnostics = analyze(Config::default(), &source);
    for i in 0..12 {
        assert_eq!(
            hazards(&diagnostics, &format!("entry{i}")),
            ["MW1001"],
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn configured_import_alias_and_declared_function_keep_their_effects() {
    let config = Config::from_json(
        r#"{"externalEffects":[
        {"function":"publishAlias","importedFrom":"./events","category":"message-publish"},
        {"function":"sendLocal","category":"email"}
    ]}"#,
        "test",
    )
    .unwrap();
    let diagnostics = analyze(
        config,
        r#"
        import { publish as publishAlias } from './events';
        function sendLocal() { return transport.send(); }
        async function aliases() { await db.insert(items).values({}); await publishAlias(); }
        async function declared() { await db.insert(items).values({}); await sendLocal(); }
    "#,
    );
    assert_eq!(hazards(&diagnostics, "aliases"), ["MW1001"]);
    assert_eq!(hazards(&diagnostics, "declared"), ["MW1001"]);
}

#[test]
fn configured_factory_receiver_requires_a_zero_argument_call() {
    let config = Config::from_json(
        r#"{"externalEffects":[
        {"receiver":"getAnalytics()","method":"insert","category":"other"}
    ]}"#,
        "test",
    )
    .unwrap();
    let diagnostics = analyze(
        config,
        r#"
        import { getAnalytics } from './analytics';
        async function supported() { await db.insert(items).values({}); await getAnalytics().insert({}); }
        async function otherClient(id: string) { await db.insert(items).values({}); await getAnalytics(id).insert({}); }
    "#,
    );
    assert_eq!(hazards(&diagnostics, "supported"), ["MW1001"]);
    assert!(hazards(&diagnostics, "otherClient").is_empty());
}

#[test]
fn resolved_result_prunes_only_the_declared_success_path() {
    let source = r#"
        async function invite() {
            await db.insert(invites).values({});
            const { status } = await mailer.send();
            if (!status) { await db.delete(invites); }
        }
    "#;
    let base = r#"{"externalEffects":[{"receiver":"mailer","method":"send","category":"email"}]}"#;
    let precise = r#"{"externalEffects":[{"receiver":"mailer","method":"send","category":"email","resolvedResult":{"status":true}}]}"#;
    let before = analyze(Config::from_json(base, "test").unwrap(), source);
    assert!(hazards(&before, "invite").contains(&"MW1002"));
    let after = analyze(Config::from_json(precise, "test").unwrap(), source);
    // The real crash window between the insert and email must remain.
    assert_eq!(hazards(&after, "invite"), ["MW1001"]);
}

#[test]
fn unsupported_result_contracts_are_rejected() {
    for value in ["[]", "{\"status\":{\"nested\":true}}"] {
        let json = format!(
            r#"{{"externalEffects":[{{"function":"send","category":"email","resolvedResult":{value}}}]}}"#
        );
        assert!(Config::from_json(&json, "test").is_err());
    }
}

#[test]
fn resolved_numeric_zero_keeps_javascript_falsiness() {
    for zero in ["0.0", "-0.0"] {
        let config = Config::from_json(&format!(r#"{{"externalEffects":[{{"function":"send","category":"email","resolvedResult":{{"status":{zero}}}}}]}}"#), "test").unwrap();
        let diagnostics = analyze(
            config,
            r#"
            async function followup() {
                const { status } = await send();
                if (!status) { await db.insert(audit).values({}); }
            }
        "#,
        );
        assert_eq!(
            hazards(&diagnostics, "followup"),
            ["MW1002"],
            "{zero}: {diagnostics:#?}"
        );
    }
}

#[test]
fn resolved_result_does_not_describe_an_unawaited_promise_object() {
    let config = Config::from_json(r#"{"externalEffects":[{"function":"send","category":"email","resolvedResult":{"status":true}}]}"#, "test").unwrap();
    let diagnostics = analyze(
        config,
        r#"
        async function followup() {
            const { status } = send();
            if (!status) { await db.insert(audit).values({}); }
        }
    "#,
    );
    assert_eq!(hazards(&diagnostics, "followup"), ["MW1002"]);
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/projects")
        .join(name)
}

#[test]
fn closed_world_transaction_context_is_opt_in() {
    let path = fixture("transaction_only_helper");
    for closed_world in [false, true] {
        let config = Config {
            closed_world,
            ..Config::default()
        };
        let report = Analyzer::new(config)
            .analyze_project(std::slice::from_ref(&path))
            .unwrap();
        assert!(report.failures.is_empty());
        let diagnostics: Vec<_> = report
            .files
            .iter()
            .flat_map(|f| f.diagnostics.clone())
            .collect();
        assert_eq!(
            hazards(&diagnostics, "replace").is_empty(),
            closed_world,
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn closed_world_keeps_hazards_when_a_caller_passes_the_root_handle() {
    let config = Config {
        closed_world: true,
        ..Config::default()
    };
    let report = Analyzer::new(config)
        .analyze_project(&[fixture("mixed_transaction_callers")])
        .unwrap();
    assert!(report.failures.is_empty());
    let diagnostics: Vec<_> = report
        .files
        .iter()
        .flat_map(|f| f.diagnostics.clone())
        .collect();
    assert_eq!(hazards(&diagnostics, "replace"), ["MW1004"]);
}

#[test]
fn tracing_callback_preserves_captures_returns_and_following_code() {
    let diagnostics = analyze(
        Config::default(),
        r#"
        import { trace } from '@opentelemetry/api';
        async function wrapped(action: () => Promise<void>) {
            const tracer = trace.getTracer('test');
            await tracer.startActiveSpan('work', async () => { await action(); return; });
            await db.insert(completed).values({});
        }
        async function entry() { await wrapped(async () => { await queue.publish('done', {}); }); }
    "#,
    );
    assert_eq!(
        hazards(&diagnostics, "entry"),
        ["MW1002"],
        "{diagnostics:#?}"
    );
    let lookalike = analyze(
        Config::default(),
        r#"
        async function fake(tracer: any) {
            await db.insert(items).values({});
            await tracer.startActiveSpan('work', async () => { await queue.publish('done', {}); });
        }
    "#,
    );
    assert!(hazards(&lookalike, "fake").is_empty());
}
