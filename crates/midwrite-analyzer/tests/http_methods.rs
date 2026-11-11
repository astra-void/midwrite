use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;

#[test]
fn method_filters_respect_overrides_and_keep_unknown_options_unknown() {
    let config = Config::from_json(
        r#"{"externalEffects":[{"function":"fetch","category":"http-request","httpMethods":["POST"]}]}"#,
        "test",
    )
    .unwrap();
    let analyzer = Analyzer::new(config);
    for (options, expected) in [
        ("{ method: 'POST' }", "MW1001"),
        ("{ method: 'post' }", "MW1001"),
        ("{ ...options, method: 'POST' }", "MW1001"),
        ("{ method: 'GET', method: 'POST' }", "MW1001"),
        ("{ method: 'POST', headers: {} }", "MW1001"),
        ("{ method: 'POST', ...options }", "MW9001"),
        ("{ method: 'POST', [key]: 'GET' }", "MW9001"),
        ("{ method: 'POST', method: 'GET' }", "MW9001"),
        ("{ get method() { return 'POST'; } }", "MW9001"),
        ("{ method() { return 'POST'; } }", "MW9001"),
        ("{ method: verb }", "MW9001"),
        ("{ headers: {} }", "MW9001"),
        ("options", "MW9001"),
    ] {
        let source = format!(
            "export async function notify(options, key, verb) {{ await db.update(orders).set({{paid:true}}); await fetch('/events', {options}); }}"
        );
        let result = analyzer.analyze_source("test.ts", &source).unwrap();
        let codes: Vec<_> = result.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, [expected], "{options}");
    }
    let spread = analyzer.analyze_source(
        "spread.ts",
        "export async function f(args) { await db.update(orders).set({paid:true}); await fetch(...args, {method:'POST'}); }",
    ).unwrap();
    assert_eq!(spread.diagnostics[0].code.as_str(), "MW9001");
}

#[test]
fn broad_http_rules_keep_their_existing_behavior() {
    let config = Config::from_json(
        r#"{"externalEffects":[{"function":"fetch","category":"http-request"}]}"#,
        "test",
    )
    .unwrap();
    let result = Analyzer::new(config)
        .analyze_source(
            "test.ts",
            "export async function f() { await db.update(rows).set({x:1}); await fetch('/metadata'); }",
        )
        .unwrap();
    assert_eq!(result.diagnostics[0].code.as_str(), "MW1001");
}
