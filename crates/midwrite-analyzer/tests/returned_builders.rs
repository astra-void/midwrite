//! Lazy Drizzle builders returned from non-async functions (DEV-148).
//!
//! `withDb((db) => db.update(t)...)` and `function save() { return
//! db.insert(t)... }` return a builder that runs when the caller awaits the
//! call. Found missing on a dev project (Zero, `decideAction`): the write
//! after an email was not seen, because only `async` functions' returns
//! counted.

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;
use midwrite_core::diagnostic::Diagnostic;

fn analyze(source: &str) -> Vec<Diagnostic> {
    Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

/// Hazard codes reported for `function`.
fn hazards(diagnostics: &[Diagnostic], function: &str) -> Vec<&'static str> {
    diagnostics
        .iter()
        .filter(|d| d.function == function && d.code.as_str().starts_with("MW1"))
        .map(|d| d.code.as_str())
        .collect()
}

const PRELUDE: &str = r#"
    async function withDb<T>(fn: (db: any) => Promise<T>): Promise<T> { return fn(db); }
    function saveReturn(id: string) { return db.update(orders).set({ id }); }
    const saveArrow = (id: string) => db.update(orders).set({ id });
"#;

fn with_prelude(body: &str) -> String {
    format!("{PRELUDE}\n{body}")
}

#[test]
fn a_returned_builder_runs_when_the_caller_awaits_the_call() {
    let diagnostics = analyze(&with_prelude(
        r#"
        async function viaFunction(id: string) {
          await queue.publish("sent", { id });
          await saveReturn(id);
        }
        async function viaArrow(id: string) {
          await queue.publish("sent", { id });
          await saveArrow(id);
        }
        async function viaWrapperCallback(id: string) {
          await queue.publish("sent", { id });
          await withDb((db) => db.update(orders).set({ id }));
        }
        async function viaWrapperBlock(id: string) {
          await queue.publish("sent", { id });
          await withDb((db) => { return db.update(orders).set({ id }); });
        }
        "#,
    ));
    for function in [
        "viaFunction",
        "viaArrow",
        "viaWrapperCallback",
        "viaWrapperBlock",
    ] {
        assert_eq!(
            hazards(&diagnostics, function),
            ["MW1002"],
            "{function}: {diagnostics:#?}"
        );
    }
}

#[test]
fn a_returned_builder_that_is_never_awaited_does_not_run() {
    let diagnostics = analyze(&with_prelude(
        r#"
        async function builtOnly(id: string) {
          await queue.publish("sent", { id });
          const query = saveReturn(id);
          return query.toSQL();
        }
        async function discarded(id: string) {
          await queue.publish("sent", { id });
          saveReturn(id);
        }
        async function unawaitedMap(ids: string[]) {
          await queue.publish("sent", { ids });
          ids.map((id) => db.insert(orders).values({ id }));
        }
        "#,
    ));
    for function in ["builtOnly", "discarded", "unawaitedMap"] {
        assert!(
            hazards(&diagnostics, function).is_empty(),
            "{function}: {diagnostics:#?}"
        );
    }
}

#[test]
fn builders_returned_by_awaited_array_callbacks_run() {
    let diagnostics = analyze(
        r#"
        async function insertAll(ids: string[]) {
          await queue.publish("sent", { ids });
          await Promise.all(ids.map((id) => db.insert(orders).values({ id })));
        }
        "#,
    );
    assert_eq!(
        hazards(&diagnostics, "insertAll"),
        ["MW1002"],
        "{diagnostics:#?}"
    );
}

#[test]
fn an_unawaited_callback_still_performs_the_effects_it_starts() {
    // Not a lazy builder: the publish starts when the callback runs, whether
    // or not anyone awaits the promise it returns.
    let diagnostics = analyze(
        r#"
        async function notifyAll(ids: string[]) {
          await db.update(orders).set({ notified: true });
          const pending = ids.map((id) => queue.publish("sent", { id }));
          await Promise.all(pending);
        }
        "#,
    );
    assert_eq!(
        hazards(&diagnostics, "notifyAll"),
        ["MW1001"],
        "{diagnostics:#?}"
    );
}
