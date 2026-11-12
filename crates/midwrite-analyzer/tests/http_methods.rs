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

/// Without a `method`, a request to a URL string is a GET; a target that may
/// be a `Request` carries its own method, so its absent method stays unknown.
#[test]
fn absent_methods_are_get_only_for_url_string_targets() {
    let config = Config::from_json(
        r#"{"externalEffects":[{"function":"fetch","category":"http-request","httpMethods":["GET"]}]}"#,
        "test",
    )
    .unwrap();
    let analyzer = Analyzer::new(config);
    for (request, expected) in [
        ("fetch('/metadata')", "MW1001"),
        ("fetch(`/metadata/${id}`, { headers: {} })", "MW1001"),
        ("fetch(url)", "MW1001"),
        ("fetch(new URL('/metadata', base))", "MW1001"),
        ("fetch(target)", "MW9001"),
        ("fetch(target, { headers: {} })", "MW9001"),
        ("fetch(target, { method: 'GET' })", "MW1001"),
    ] {
        let source = format!(
            "export async function f(url: string, target, id, base) {{ await db.update(orders).set({{paid:true}}); await {request}; }}"
        );
        let result = analyzer.analyze_source("test.ts", &source).unwrap();
        let codes: Vec<_> = result.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, [expected], "{request}");
    }
}

/// A conditional between literal methods matches when every one is listed.
#[test]
fn conditional_methods_match_only_when_every_method_is_listed() {
    let config = Config::from_json(
        r#"{"externalEffects":[{"function":"fetch","category":"http-request","httpMethods":["POST","PUT"]}]}"#,
        "test",
    )
    .unwrap();
    let analyzer = Analyzer::new(config);
    for (method, expected) in [
        ("exists ? 'POST' : 'PUT'", "MW1001"),
        ("exists ? 'POST' : 'GET'", "MW9001"),
        ("exists ? 'POST' : verb", "MW9001"),
    ] {
        let source = format!(
            "export async function f(exists, verb) {{ await db.update(orders).set({{paid:true}}); await fetch('/events', {{ method: {method} }}); }}"
        );
        let result = analyzer.analyze_source("test.ts", &source).unwrap();
        let codes: Vec<_> = result.diagnostics.iter().map(|d| d.code.as_str()).collect();
        assert_eq!(codes, [expected], "{method}");
    }
}
