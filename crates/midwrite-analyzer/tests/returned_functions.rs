//! Calls through a function returned by an analyzed factory.

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;

fn hazards(source: &str, name: &str) -> Vec<String> {
    Analyzer::new(Config::default())
        .analyze_source("returned-functions.ts", source)
        .expect("source parses")
        .diagnostics
        .into_iter()
        .filter(|d| d.function == name && d.code.as_str() != "MW9001")
        .map(|d| d.code.as_str().to_owned())
        .collect()
}

#[test]
fn a_stored_factory_result_runs_only_when_called() {
    let source = r#"
        function makeWrite() {
          return async () => { await db.insert(orders).values({}); };
        }
        export async function run() {
          const write = makeWrite();
          await write();
          await queue.publish({});
        }
        export async function unused() {
          const write = makeWrite();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
    assert!(hazards(source, "unused").is_empty());
}

#[test]
fn an_inline_factory_is_evaluated_once_before_its_returned_function() {
    let source = r#"
        function makeWrite() {
          return async () => { await db.insert(orders).values({}); };
        }
        export async function run() {
          await makeWrite()();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
}

#[test]
fn returning_a_callback_substitutes_the_actual_function() {
    let source = r#"
        function identity(action: () => Promise<void>) { return action; }
        export async function run() {
          const write = identity(async () => { await db.insert(orders).values({}); });
          await write();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
}

#[test]
fn an_async_factory_must_be_awaited_before_its_result_is_callable() {
    let source = r#"
        async function makeWrite() {
          return async () => { await db.insert(orders).values({}); };
        }
        export async function run() {
          const write = await makeWrite();
          await write();
          await queue.publish({});
        }
        export async function promiseIsNotAFunction() {
          const write = makeWrite();
          await write();
          await queue.publish({});
        }
        export async function inline() {
          await (await makeWrite())();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
    assert_eq!(hazards(source, "inline"), ["MW1001"]);
    assert!(hazards(source, "promiseIsNotAFunction").is_empty());
}

#[test]
fn ambiguous_or_implicit_returns_do_not_choose_a_function() {
    let source = r#"
        function maybeWrite(flag: boolean) {
          if (flag) return async () => { await db.insert(orders).values({}); };
        }
        function choose(flag: boolean) {
          return flag
            ? async () => { await db.insert(orders).values({}); }
            : async () => { await mailer.send({}); };
        }
        export async function implicit(flag: boolean) {
          const write = maybeWrite(flag);
          await write();
          await queue.publish({});
        }
        export async function ambiguous(flag: boolean) {
          const write = choose(flag);
          await write();
          await queue.publish({});
        }
    "#;
    assert!(hazards(source, "implicit").is_empty());
    assert!(hazards(source, "ambiguous").is_empty());
}

#[test]
fn the_returned_functions_own_parameters_are_bound_at_invocation() {
    let source = r#"
        function makeWrite() {
          return async (send: boolean) => {
            if (send) await db.insert(orders).values({});
          };
        }
        export async function enabled() {
          const write = makeWrite();
          await write(true);
          await queue.publish({});
        }
        export async function disabled() {
          const write = makeWrite();
          await write(false);
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "enabled"), ["MW1001"]);
    assert!(hazards(source, "disabled").is_empty());
}

#[test]
fn an_unawaited_effectful_helper_does_not_become_an_awaited_factory() {
    let source = r#"
        function effectful() {
          queue.publish({});
          return async () => { await db.insert(orders).values({}); };
        }
        export async function run() {
          const write = effectful();
          await write();
          await queue.publish({});
        }
    "#;
    assert!(hazards(source, "run").is_empty());
}

#[test]
fn captured_parameters_are_not_rebound_to_the_returned_functions_arguments() {
    let source = r#"
        function wrap(action: () => Promise<void>) {
          return async (other: () => Promise<void>) => { await action(); };
        }
        export async function run() {
          const write = wrap(async () => { await db.insert(orders).values({}); });
          await db.insert(events).values({});
          await write(async () => { await queue.publish({}); });
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1004"]);
    let analysis = Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .unwrap();
    assert!(
        analysis
            .diagnostics
            .iter()
            .all(|d| d.function != "run" || d.code.as_str() != "MW1001")
    );
}

#[test]
fn factories_keep_each_calls_captured_callback_and_flag_separate() {
    let source = r#"
        function wrap(action: () => Promise<void>, enabled: boolean) {
          const send = enabled;
          return async () => { if (send) await action(); };
        }
        export async function enabled() {
          const write = wrap(async () => { await db.insert(orders).values({}); }, true);
          await write();
          await queue.publish({});
        }
        export async function disabled() {
          const write = wrap(async () => { await db.insert(orders).values({}); }, false);
          await write();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "enabled"), ["MW1001"]);
    assert!(hazards(source, "disabled").is_empty());
}

#[test]
fn captures_keep_transaction_handles_inside_the_transaction() {
    let source = r#"
        function makeWrite(database: unknown) {
          return async () => { await database.insert(orders).values({}); };
        }
        export async function run() {
          await db.transaction(async (tx) => {
            const write = makeWrite(tx);
            await write();
            await tx.insert(events).values({});
          });
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
}

#[test]
fn mutable_captures_are_not_creation_time_snapshots() {
    let source = r#"
        function makeWrite() {
          let action = async () => { await db.insert(orders).values({}); };
          const run = async () => { await action(); };
          action = async () => {};
          return run;
        }
        export async function run() {
          const write = makeWrite();
          await write();
          await queue.publish({});
        }
    "#;
    assert!(hazards(source, "run").is_empty());
}

#[test]
fn stable_module_factory_bindings_and_aliases_are_callable() {
    let source = r#"
        function wrap(action: () => Promise<void>) { return async () => { await action(); }; }
        export const write = wrap(async () => { await db.insert(orders).values({}); });
        const alias = write;
        export async function run() { await alias(); await queue.publish({}); }
        export async function asCallback() {
          await execute(write);
          await queue.publish({});
        }
        async function execute(action: () => Promise<void>) { await action(); }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
    assert_eq!(hazards(source, "asCallback"), ["MW1001"]);
    let ir = Analyzer::new(Config::default())
        .lower_source("test.ts", source)
        .unwrap();
    assert!(ir.functions.iter().all(|f| !f.name.starts_with("<module:")));
}

#[test]
fn module_async_factories_require_top_level_await() {
    let source = r#"
        async function makeWrite() { return async () => { await db.insert(orders).values({}); }; }
        const write = await makeWrite();
        const promise = makeWrite();
        export async function awaited() { await write(); await queue.publish({}); }
        export async function unawaited() { await promise(); await queue.publish({}); }
    "#;
    assert_eq!(hazards(source, "awaited"), ["MW1001"]);
    assert!(hazards(source, "unawaited").is_empty());
}

#[test]
fn mutable_module_bindings_and_initializer_cycles_remain_unknown() {
    let source = r#"
        function makeWrite() { return async () => { await db.insert(orders).values({}); }; }
        let write = makeWrite();
        write = async () => {};
        const first = second;
        const second = first;
        const early = later();
        const later = makeWrite;
        export async function mutable() { await write(); await queue.publish({}); }
        export async function cyclic() { await first(); await queue.publish({}); }
        export async function temporalDeadZone() { await early(); await queue.publish({}); }
    "#;
    assert!(hazards(source, "mutable").is_empty());
    assert!(hazards(source, "cyclic").is_empty());
    assert!(hazards(source, "temporalDeadZone").is_empty());
}

#[test]
fn object_captures_do_not_freeze_mutable_properties() {
    let source = r#"
        function wrap(options: { send: boolean }) {
          return async () => { if (!options.send) await db.insert(orders).values({}); };
        }
        export async function run() {
          const options = { send: false };
          const write = wrap(options);
          options.send = true;
          await write();
          await queue.publish({});
        }
    "#;
    let ir = Analyzer::new(Config::default())
        .lower_source("test.ts", source)
        .unwrap();
    let run = ir.functions.iter().find(|f| f.name == "run").unwrap();
    // The analysis can retain both branches, but cannot call the stale
    // `false` property a proven condition after the mutation.
    assert!(run.to_text().contains("some paths"), "{}", run.to_text());
}

#[test]
fn an_exported_factory_result_is_analyzed_as_an_entry_without_a_project_caller() {
    let source = r#"
        function wrap(action: () => Promise<void>) {
          return async () => { await db.insert(orders).values({}); await action(); };
        }
        export const handler = wrap(async () => { await queue.publish({}); });
    "#;
    assert_eq!(hazards(source, "handler"), ["MW1001"]);
}

#[test]
fn a_capture_is_not_initialized_by_a_later_const_declaration() {
    let source = r#"
        export async function run() {
          const invoke = async () => { await write(); };
          await invoke();
          const write = async () => { await db.insert(orders).values({}); };
          await queue.publish({});
        }
    "#;
    assert!(hazards(source, "run").is_empty());
}

#[test]
fn a_captured_callback_is_forwarded_through_another_wrapper() {
    let source = r#"
        async function execute(action: () => Promise<void>) { await action(); }
        async function withCapture(action: () => Promise<void>) {
          await execute(async () => { await action(); });
        }
        export async function run() {
          await db.insert(orders).values({});
          await withCapture(async () => { await queue.publish({}); });
        }
        function make(action: () => Promise<void>) { return async () => { await action(); }; }
        function compose(action: () => Promise<void>) { return make(async () => { await action(); }); }
        export async function composed() {
          const publish = compose(async () => { await queue.publish({}); });
          await db.insert(orders).values({});
          await publish();
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
    assert_eq!(hazards(source, "composed"), ["MW1001"]);
}

#[test]
fn array_callbacks_preserve_their_captured_action() {
    let source = r#"
        async function each(action: () => Promise<void>) {
          await Promise.all([1, 2].map(async () => { await action(); }));
        }
        export async function run() {
          await db.insert(orders).values({});
          await each(async () => { await queue.publish({}); });
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
}

#[test]
fn configured_database_parameters_keep_their_root_provenance_when_captured() {
    let source = r#"
        export async function run(db: unknown) {
          await Promise.all([1, 2].map(async (id) => { await db.insert(orders).values({ id }); }));
          await queue.publish({});
        }
        export async function unknown(database: unknown) {
          await Promise.all([1, 2].map(async (id) => { await database.insert(orders).values({ id }); }));
          await queue.publish({});
        }
        export async function local() {
          const { db } = await getContext();
          await Promise.all([1, 2].map(async (id) => { await db.insert(orders).values({ id }); }));
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
    assert!(hazards(source, "unknown").is_empty());
    assert_eq!(hazards(source, "local"), ["MW1001"]);
}

#[test]
fn a_factory_cannot_ignore_a_temporal_dead_zone_in_its_arguments() {
    let source = r#"
        function make(unused: unknown) { return async () => { await db.insert(orders).values({}); }; }
        const write = make(later);
        const later = write;
        export async function run() { await write(); await queue.publish({}); }
    "#;
    assert!(hazards(source, "run").is_empty());
}

#[test]
fn a_captured_alias_of_a_database_parameter_can_still_be_a_transaction() {
    let source = r#"
        function make(db: unknown) {
          const connection = db;
          return async () => { await connection.insert(orders).values({}); };
        }
        export async function run() {
          await db.transaction(async (tx) => {
            const write = make(tx);
            await write();
            await tx.insert(events).values({});
          });
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "run"), ["MW1001"]);
}

#[test]
fn returned_function_recursion_still_obeys_the_call_bound() {
    let source = r#"
        function makeWrite() {
          return async () => {
            await db.insert(orders).values({});
            await makeWrite()();
          };
        }
        export async function run() {
          await makeWrite()();
          await queue.publish({});
        }
    "#;
    let analysis = Analyzer::new(Config::default())
        .analyze_source("test.ts", source)
        .unwrap();
    assert!(
        analysis.diagnostics.len() < 20,
        "recursive expansion must terminate"
    );
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.code.as_str() == "MW9001")
    );
}

#[test]
fn a_returned_lazy_builder_runs_only_when_the_returned_function_is_awaited() {
    let source = r#"
        function makeWrite() { return () => db.insert(orders).values({}); }
        export async function awaited() {
          const write = makeWrite();
          await write();
          await queue.publish({});
        }
        export async function unawaited() {
          const write = makeWrite();
          write();
          await queue.publish({});
        }
    "#;
    assert_eq!(hazards(source, "awaited"), ["MW1001"]);
    assert!(hazards(source, "unawaited").is_empty());
}
