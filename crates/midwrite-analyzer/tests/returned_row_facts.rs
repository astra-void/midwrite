//! Returned-row lineage must remove contradictions without hiding valid
//! old-state transitions, undefined assignments, escapes or mutations.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use midwrite_analyzer::{Analyzer, ProjectAnalysis};
use midwrite_core::config::Config;

struct Project(PathBuf);

impl Project {
    fn new(entry: &str, extra: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "midwrite-row-facts-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&path).unwrap();
        let p = Self(path);
        p.write(
            "entry.ts",
            &format!("import * as tables from './schema'; const {{ records }} = tables;\n{entry}"),
        );
        p.write("schema.ts", "import { pgTable, text } from 'drizzle-orm/pg-core'; export const records = pgTable('records', { phase: text(), ready: text() });");
        p.write(
            "capture.ts",
            r#"
            import { ok as success, err as failure } from 'neverthrow';
            export async function capture<T>(query: PromiseLike<T>) {
                try { return success(await query); }
                catch (error) { return failure(error); }
            }
        "#,
        );
        p.write(
            "modes.ts",
            "export const Modes = { Running: 'running', Paused: 'paused' } as const;",
        );
        for (file, source) in extra {
            p.write(file, source);
        }
        p
    }

    fn write(&self, file: &str, source: &str) {
        std::fs::write(self.0.join(file), source).unwrap();
    }
    fn analyze(&self) -> ProjectAnalysis {
        let report = Analyzer::new(Config::default())
            .analyze_project(std::slice::from_ref(&self.0))
            .unwrap();
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        report
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

fn source(before_guard: &str, mapping: &str, guard: &str) -> String {
    format!(
        r#"
        import {{ capture }} from './capture';
        import {{ Modes }} from './modes';
        export async function edit(requested: string) {{
            const outcome = await db.transaction(async tx => {{
                const saved = await capture(tx.update(records).set({{ phase: requested }}).returning());
                return saved.map(([updated]) => {{
                    if (!updated) throw new Error('missing');
                    return {{ entity: {mapping} }};
                }});
            }});
            if (outcome.isErr()) return;
            const {{ entity }} = outcome.value;
            {before_guard}
            if ({guard}) {{ await queue.publish('transition', {{}}); }}
            await queue.publish('saved', {{}});
        }}
    "#
    )
}

fn hazards(report: &ProjectAnalysis) -> Vec<&'static str> {
    report
        .files
        .iter()
        .flat_map(|f| &f.diagnostics)
        .filter(|d| d.function == "edit" && d.code.as_str().starts_with("MW1"))
        .map(|d| d.code.as_str())
        .collect()
}

const CONTRADICTION: &str = "requested === Modes.Running && entity.phase === Modes.Paused";

#[test]
fn mapped_new_row_is_not_a_snapshot_of_the_previous_state() {
    let p = Project::new(&source("", "updated", CONTRADICTION), &[]);
    assert_eq!(hazards(&p.analyze()), ["MW1001"]); // the real post-commit publication remains
}

#[test]
fn formatting_and_comments_do_not_select_the_proof() {
    let text = source("", "updated", CONTRADICTION)
        .replace(".returning()", ".returning /* query result */\n ()");
    let p = Project::new(&text, &[]);
    assert_eq!(hazards(&p.analyze()), ["MW1001"]);
}

#[test]
fn undefined_update_can_preserve_the_previous_column() {
    let p = Project::new(
        &source(
            "",
            "updated",
            "requested === undefined && entity.phase === Modes.Paused",
        ),
        &[],
    );
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn database_coercions_and_unknown_codecs_are_not_identity_functions() {
    // PostgreSQL text accepts a numeric parameter as its string form.
    let p = Project::new(
        &source("", "updated", "requested === 3 && entity.phase === '3'"),
        &[],
    );
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
    let schema = r#"
        import { pgTable, customType } from 'drizzle-orm/pg-core';
        const changed = customType({ dataType: () => 'text', fromDriver: () => 'paused', toDriver: v => v });
        export const records = pgTable('records', { phase: changed() });
    "#;
    let p = Project::new(
        &source("", "updated", CONTRADICTION),
        &[("schema.ts", schema)],
    );
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn an_unresolved_table_schema_does_not_supply_a_column_postcondition() {
    let p = Project::new(
        &source("", "updated", CONTRADICTION),
        &[("schema.ts", "export const records = unknownSchema();")],
    );
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn unrelated_previous_row_and_unknown_mapper_keep_the_transition() {
    for mapping in ["previous", "replace(updated)"] {
        let text = source("", mapping, CONTRADICTION).replace(
            "const saved =",
            "const [previous] = await tx.select().from(records); const saved =",
        );
        let p = Project::new(&text, &[]);
        assert!(hazards(&p.analyze()).contains(&"MW1005"), "{mapping}");
    }
}

#[test]
fn mutation_or_escape_of_the_returned_row_cancels_the_proof() {
    for operation in [
        "entity.phase = Modes.Paused;",
        "change(entity);",
        "const holder = { get current() { entity.phase = Modes.Paused; return entity; } }; holder.current;",
        "const flags = { mutate: false }; changeFlags(flags); if (flags.mutate) entity.phase = Modes.Paused;",
        "const flags = []; flags.push(true); if (flags[0]) entity.phase = Modes.Paused;",
        "const flags = [true]; if (flags.length) entity.phase = Modes.Paused;",
        "const flags = { __proto__: { mutate: true } }; if (flags.mutate) entity.phase = Modes.Paused;",
        "const flags = { mutate: false }; new Unknown(flags); if (flags.mutate) entity.phase = Modes.Paused;",
    ] {
        let p = Project::new(&source(operation, "updated", CONTRADICTION), &[]);
        assert!(hazards(&p.analyze()).contains(&"MW1005"), "{operation}");
    }
}

#[test]
fn an_imported_generator_is_not_evaluated_as_an_eager_helper() {
    let generator = "import { Modes } from './modes'; export function* deferred(entity: any) { entity.phase = Modes.Paused; }";
    let text = format!(
        "import {{ deferred }} from './deferred';\n{}",
        source(
            "const worker = deferred(entity); worker.next();",
            "updated",
            CONTRADICTION
        )
    );
    let p = Project::new(&text, &[("deferred.ts", generator)]);
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn rereading_a_mutable_input_object_does_not_reuse_an_old_property_value() {
    let text = source(
        "change(body);",
        "updated",
        "body.requested === Modes.Running && entity.phase === Modes.Paused",
    )
    .replace("edit(requested: string)", "edit(body: any)")
    .replace("phase: requested", "phase: body.requested");
    let p = Project::new(&text, &[]);
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn repeated_helper_invocations_have_distinct_property_read_snapshots() {
    let p = Project::new(
        r#"
        function readPhase(tx: any, body: any) { return body.phase; }
        export async function edit(body: any) {
            await db.transaction(async tx => {
                const initial = readPhase(tx, body);
                const [entity] = await tx.update(records).set({ phase: initial }).returning();
                if (!entity) return;
                change(body);
                const current = readPhase(tx, body);
                if (current === 'running' && entity.phase === 'paused') {
                    await queue.publish('possible', {});
                }
            });
        }
    "#,
        &[],
    );
    assert!(hazards(&p.analyze()).contains(&"MW1003"));
}

#[test]
fn callback_cannot_silently_change_a_captured_input() {
    let text = source(
        "",
        "updated",
        "proposed === Modes.Paused && entity.phase === Modes.Running",
    )
    .replace(
        "const outcome =",
        "let proposed = requested; const outcome =",
    )
    .replace("phase: requested", "phase: proposed")
    .replace(
        "return { entity:",
        "proposed = Modes.Paused; return { entity:",
    );
    let p = Project::new(&text, &[]);
    assert!(hazards(&p.analyze()).contains(&"MW1005"));
}

#[test]
fn an_async_helpers_promise_is_not_its_resolved_result() {
    let p = Project::new(
        r#"
        import { capture } from './capture';
        export async function edit() {
            await db.update(records).set({ ready: true }).returning();
            const pending: any = capture(db.update(records).set({ ready: true }).returning());
            if (!pending.value) { await queue.publish('possible', {}); }
        }
    "#,
        &[],
    );
    assert_eq!(hazards(&p.analyze()), ["MW1001"]);
}

#[test]
fn changed_or_escaped_enum_objects_are_not_constants() {
    for change in [
        "Other.Paused = 'running';",
        "Object.defineProperty(Other, 'Paused', { value: 'running' });",
        "const alias = Other; alias.Paused = 'running';",
        "unknown(...[Other]);",
        "new Unknown(Other);",
        "export const expose = () => Other;",
    ] {
        let mutator = format!("import {{ Modes as Other }} from './modes'; {change}");
        let p = Project::new(
            &source("", "updated", CONTRADICTION),
            &[("mutator.ts", &mutator)],
        );
        assert!(hazards(&p.analyze()).contains(&"MW1005"), "{change}");
    }
}

#[test]
fn readonly_typebox_construction_does_not_escape_the_enum() {
    for method in ["Enum", "Const"] {
        let schema = format!(
            "import {{ Type }} from '@sinclair/typebox'; import {{ Modes }} from './modes'; export const schema = Type.{method}(Modes);"
        );
        let p = Project::new(
            &source("", "updated", CONTRADICTION),
            &[("validation.ts", &schema)],
        );
        assert_eq!(hazards(&p.analyze()), ["MW1001"], "{method}");
    }
}

#[test]
fn lookalike_typebox_or_an_escaped_options_value_is_not_trusted() {
    for schema in [
        "import { Type } from 'other-package'; import { Modes } from './modes'; export const s = Type.Enum(Modes);",
        "import { Type } from '@sinclair/typebox'; import { Modes } from './modes'; export const s = Type.Enum(Modes, { extra: Modes });",
    ] {
        let p = Project::new(
            &source("", "updated", CONTRADICTION),
            &[("validation.ts", schema)],
        );
        assert!(hazards(&p.analyze()).contains(&"MW1005"));
    }
}
