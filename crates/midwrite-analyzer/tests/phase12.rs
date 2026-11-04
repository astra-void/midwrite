//! Phase 12: default generators, schema modules, re-exports, database
//! factories, repository objects and classes, identity-based table binding.

use std::path::{Path, PathBuf};

use midwrite_analyzer::TsActorProvider;
use midwrite_analyzer::schema::{GeneratorClass, extract_schema_source};
use midwrite_model::ir::{Op, StmtKind};
use midwrite_model::provider::{ActorProvider, ExtractRequest, Extracted, TableBinding};

struct Project(PathBuf);

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("midwrite-a12-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        Self(dir)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn binding(table: &str, columns: &[&str], source: Option<(&str, &str)>) -> TableBinding {
    TableBinding {
        identifier: table.to_owned(),
        table: table.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: source.map(|(f, b)| (f.to_owned(), b.to_owned())),
    }
}

fn extract(root: &Path, selector: &str, tables: Vec<TableBinding>) -> Extracted {
    TsActorProvider::new(root)
        .extract(&ExtractRequest {
            root: root.to_path_buf(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables,
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some(selector.to_owned()),
            contract: Some(Default::default()),
            replicas: false,
            replica_factories: Vec::new(),
            return_values: false,
            js_values: false,
        })
        .unwrap_or_else(|e| panic!("{selector}: {e}"))
}

fn ops(x: &Extracted) -> Vec<&Op> {
    x.codes.iter().flatten().map(|s| &s.op).collect()
}

fn unknowns(x: &Extracted) -> Vec<String> {
    ops(x)
        .into_iter()
        .filter_map(|o| match o {
            Op::Unknown { detail, .. } => Some(detail.clone()),
            _ => None,
        })
        .collect()
}

fn tables_written(x: &Extracted) -> Vec<String> {
    ops(x)
        .into_iter()
        .filter_map(|o| match o {
            Op::Db { stmt } => Some(stmt.kind.table().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn defaults_are_classified_by_mechanism_and_only_known_generators_are_fresh() {
    let source = r#"
import { integer, pgTable, serial, text, timestamp, uuid } from "drizzle-orm/pg-core";
import { sql } from "drizzle-orm";
import { createId } from "@paralleldrive/cuid2";
import { v4 as uuidv4 } from "uuid";
import { customIdService } from "./ids";

function uuid4() { counter += 1; return "x"; }
let counter = 0;

export const t = pgTable("t", {
  a: uuid("a").defaultRandom().primaryKey(),
  b: text("b").$defaultFn(() => crypto.randomUUID()),
  c: text("c").$defaultFn(createId),
  d: text("d").$defaultFn(() => uuidv4()),
  e: text("e").$defaultFn(() => customIdService.generate()),
  f: text("f").$defaultFn(() => process.env.CUSTOM_ID),
  g: text("g").$defaultFn(uuid4),
  h: text("h").$defaultFn(crypto.randomUUID),
  i: serial("i"),
  j: integer("j").generatedByDefaultAsIdentity(),
  k: text("k").default("new"),
  l: timestamp("l").defaultNow(),
  m: uuid("m").default(sql`gen_random_uuid()`),
  n: text("n").default(sql`md5(random()::text)`),
  o: uuid("o").defaultRandom().$defaultFn(() => crypto.randomUUID()),
});
"#;
    let s = extract_schema_source("schema.ts", source).unwrap();
    let t = &s.tables[0];
    let class = |p: &str| {
        let g = t.column(p).unwrap().generation.clone().unwrap();
        (g.class, g.generator)
    };
    use GeneratorClass as G;
    assert_eq!(
        class("a"),
        (G::DatabaseGenerated, Some("gen_random_uuid()".into()))
    );
    assert_eq!(
        class("b"),
        (G::ApplicationGenerated, Some("crypto.randomUUID()".into()))
    );
    assert_eq!(
        class("c"),
        (
            G::ApplicationGenerated,
            Some("@paralleldrive/cuid2 createId()".into())
        )
    );
    assert_eq!(
        class("d"),
        (G::ApplicationGenerated, Some("uuid v4()".into()))
    );
    // Not recognized: an unknown service, an environment value, a local
    // function that merely has a generator-like name (with a side effect),
    // a method passed without its receiver.
    for p in ["e", "f", "g", "h"] {
        assert_eq!(class(p).0, G::UnknownDefault, "column {p}");
        assert_eq!(class(p).1, None, "column {p}");
    }
    assert_eq!(class("i").0, G::SequenceGenerated);
    assert_eq!(class("j").0, G::IdentityGenerated);
    assert_eq!(class("k").0, G::ConstantDefault);
    assert_eq!(class("l"), (G::DatabaseGenerated, None));
    assert_eq!(
        class("m"),
        (G::DatabaseGenerated, Some("gen_random_uuid()".into()))
    );
    assert_eq!(class("n").0, G::UnknownDefault);
    // Drizzle uses $defaultFn when the INSERT omits the column, whatever
    // database default exists.
    assert_eq!(class("o").0, G::ApplicationGenerated);
    // A local `crypto` shadows the global.
    let shadowed = r#"
import { pgTable, text } from "drizzle-orm/pg-core";
const crypto = { randomUUID: () => "fixed" };
export const u = pgTable("u", { id: text("id").$defaultFn(() => crypto.randomUUID()).primaryKey() });
"#;
    let s = extract_schema_source("s.ts", shadowed).unwrap();
    let g = s.tables[0].columns[0].generation.clone().unwrap();
    assert_eq!(g.class, G::UnknownDefault, "{g:?}");
}

#[test]
fn a_callback_imported_from_a_project_module_is_classified_by_its_definition() {
    let p = Project::new(
        "callback",
        &[
            (
                "src/lib/ids.ts",
                "export const createId = () => crypto.randomUUID();\nexport function badId() { return Math.random().toString(); }\n",
            ),
            (
                "src/schema.ts",
                "import { pgTable, text } from \"drizzle-orm/pg-core\";\nimport { createId, badId } from \"./lib/ids\";\nexport const a = pgTable(\"a\", { id: text(\"id\").$defaultFn(createId).primaryKey() });\nexport const b = pgTable(\"b\", { id: text(\"id\").$defaultFn(badId).primaryKey() });\n",
            ),
        ],
    );
    let s = TsActorProvider::new(&p.0)
        .extract_schema(&p.0, "src/schema.ts")
        .unwrap();
    let a = s.table("a").unwrap().columns[0].generation.clone().unwrap();
    assert_eq!(a.class, GeneratorClass::ApplicationGenerated, "{a:?}");
    assert_eq!(a.generator.as_deref(), Some("crypto.randomUUID()"));
    let b = s.table("b").unwrap().columns[0].generation.clone().unwrap();
    assert_eq!(b.class, GeneratorClass::UnknownDefault, "{b:?}");
}

const SCHEMA_A: &str = "import { pgTable, text } from \"drizzle-orm/pg-core\";\nexport const users = pgTable(\"app_users\", { id: text(\"id\").primaryKey(), email: text(\"email\") });\n";
const SCHEMA_B: &str = "import { pgTable, text } from \"drizzle-orm/pg-core\";\nexport const users = pgTable(\"audit_users\", { id: text(\"id\").primaryKey(), email: text(\"email\") });\n";

#[test]
fn tables_are_bound_by_declaration_not_by_name() {
    let p = Project::new(
        "identity",
        &[
            ("src/db/app.ts", SCHEMA_A),
            ("src/db/audit.ts", SCHEMA_B),
            ("src/db/index.ts", "export * from \"./app\";\n"),
            (
                "src/service.ts",
                "import { users as accounts } from \"./db\";\nimport { db } from \"./client\";\nexport async function add(email: string) {\n  await db.insert(accounts).values({ id: \"u1\", email });\n}\n",
            ),
            ("src/client.ts", "export const db = {} as any;\n"),
        ],
    );
    let tables = vec![
        binding(
            "app_users",
            &["id", "email"],
            Some(("src/db/app.ts", "users")),
        ),
        binding(
            "audit_users",
            &["id", "email"],
            Some(("src/db/audit.ts", "users")),
        ),
    ];
    // `accounts` is `users` of app.ts through an alias and a barrel.
    let x = extract(&p.0, "src/service.ts#add", tables);
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    assert_eq!(tables_written(&x), ["app_users"]);
}

#[test]
fn ambiguous_and_cyclic_re_exports_are_not_followed() {
    let p = Project::new(
        "reexports",
        &[
            ("src/a.ts", "export async function save() {}\n"),
            ("src/b.ts", "export async function save() {}\n"),
            (
                "src/both.ts",
                "export * from \"./a\";\nexport * from \"./b\";\n",
            ),
            ("src/c1.ts", "export * from \"./c2\";\n"),
            ("src/c2.ts", "export * from \"./c1\";\n"),
            (
                "src/use.ts",
                "import { save } from \"./both\";\nimport { save as again } from \"./c1\";\nexport async function one() { await save(); }\nexport async function two() { await again(); }\n",
            ),
        ],
    );
    let one = unknowns(&extract(&p.0, "src/use.ts#one", Vec::new()));
    assert!(one.iter().any(|u| u.contains("ambiguous")), "{one:?}");
    let two = unknowns(&extract(&p.0, "src/use.ts#two", Vec::new()));
    assert!(
        two.iter()
            .any(|u| u.contains("no export") || u.contains("cycle")),
        "{two:?}"
    );
}

const CLIENT: &str = "import { drizzle, type NodePgDatabase } from \"drizzle-orm/node-postgres\";\nexport type Database = NodePgDatabase<Record<string, never>>;\nexport const pool = {} as any;\nexport function getDb(): Database { return drizzle(pool); }\nexport function getThing(): Thing { return {} as Thing; }\ntype Thing = { insert(): void };\n";
const ITEMS: &str = "import { pgTable, text } from \"drizzle-orm/pg-core\";\nexport const items = pgTable(\"items\", { id: text(\"id\").primaryKey(), name: text(\"name\") });\n";

#[test]
fn a_function_declared_to_return_a_drizzle_database_is_a_root_handle() {
    let p = Project::new(
        "factory",
        &[
            ("src/client.ts", CLIENT),
            ("src/schema.ts", ITEMS),
            (
                "src/use.ts",
                "import { getDb, getThing, type Database } from \"./client\";\nimport { items } from \"./schema\";\nexport async function viaFactory() { await getDb().insert(items).values({ id: \"i\", name: \"n\" }); }\nexport async function viaLocal() { const db = getDb(); await db.insert(items).values({ id: \"i\", name: \"n\" }); }\nexport async function viaParam(database: Database) { await database.insert(items).values({ id: \"i\", name: \"n\" }); }\nexport async function notAFactory() { await getThing().insert(items); }\n",
            ),
        ],
    );
    let tables = || vec![binding("items", &["id", "name"], None)];
    for entry in ["viaFactory", "viaLocal", "viaParam"] {
        let x = extract(&p.0, &format!("src/use.ts#{entry}"), tables());
        assert!(unknowns(&x).is_empty(), "{entry}: {:?}", unknowns(&x));
        assert_eq!(tables_written(&x), ["items"], "{entry}");
    }
    let x = extract(&p.0, "src/use.ts#notAFactory", tables());
    assert!(tables_written(&x).is_empty());
    assert!(!unknowns(&x).is_empty());
}

#[test]
fn repository_objects_and_classes_are_inlined_with_their_receiver() {
    let p = Project::new(
        "repositories",
        &[
            ("src/client.ts", CLIENT),
            ("src/schema.ts", ITEMS),
            (
                "src/repo.ts",
                "import { getDb, type Database } from \"./client\";\nimport { items } from \"./schema\";\nconst db = getDb();\nexport const itemsRepository = {\n  async add(id: string) { await db.insert(items).values({ id, name: \"n\" }); },\n  async addTwice(id: string) { await this.add(id); },\n};\nexport class ItemStore {\n  constructor(private readonly database: Database) {}\n  async add(id: string) { await this.database.insert(items).values({ id, name: \"c\" }); }\n}\nexport const store = new ItemStore(getDb());\nexport const mutable = { async add(id: string) { await db.insert(items).values({ id, name: \"m\" }); } };\nmutable.add = async () => {};\nexport class Derived extends ItemStore {}\n",
            ),
            (
                "src/index.ts",
                "export { itemsRepository, store, mutable } from \"./repo\";\nexport * from \"./repo\";\n",
            ),
            (
                "src/use.ts",
                "import { itemsRepository, store, mutable, ItemStore, Derived } from \"./index\";\nimport { getDb } from \"./client\";\nexport async function object() { await itemsRepository.add(\"a\"); }\nexport async function viaThis() { await itemsRepository.addTwice(\"a\"); }\nexport async function instance() { await store.add(\"b\"); }\nexport async function local() { const s = new ItemStore(getDb()); await s.add(\"c\"); }\nexport async function mutated() { await mutable.add(\"d\"); }\nexport async function derived() { const d = new Derived(getDb()); await d.add(\"e\"); }\n",
            ),
        ],
    );
    let tables = || vec![binding("items", &["id", "name"], None)];
    for entry in ["object", "viaThis", "instance", "local"] {
        let x = extract(&p.0, &format!("src/use.ts#{entry}"), tables());
        assert!(unknowns(&x).is_empty(), "{entry}: {:?}", unknowns(&x));
        assert_eq!(tables_written(&x), ["items"], "{entry}");
    }
    // A modified object and an inherited method are not guessed.
    for entry in ["mutated", "derived"] {
        let x = extract(&p.0, &format!("src/use.ts#{entry}"), tables());
        assert!(tables_written(&x).is_empty(), "{entry}");
        assert!(!unknowns(&x).is_empty(), "{entry}");
    }
}

#[test]
fn a_transaction_handle_passed_to_a_repository_stays_in_the_transaction() {
    let p = Project::new(
        "tx",
        &[
            ("src/client.ts", CLIENT),
            ("src/schema.ts", ITEMS),
            (
                "src/repo.ts",
                "import { items } from \"./schema\";\nexport const repo = { async add(tx: any, id: string) { await tx.insert(items).values({ id, name: \"t\" }); } };\n",
            ),
            (
                "src/use.ts",
                "import { getDb } from \"./client\";\nimport { repo } from \"./repo\";\nimport { items } from \"./schema\";\nexport async function inTx() {\n  const db = getDb();\n  await db.transaction(async (tx) => { await repo.add(tx, \"a\"); });\n  await db.insert(items).values({ id: \"b\", name: \"pool\" });\n}\n",
            ),
        ],
    );
    let x = extract(
        &p.0,
        "src/use.ts#inTx",
        vec![binding("items", &["id", "name"], None)],
    );
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let conns: Vec<_> = ops(&x)
        .into_iter()
        .filter_map(|o| match o {
            Op::Db { stmt } => Some(stmt.conn),
            _ => None,
        })
        .collect();
    use midwrite_model::ir::Conn;
    assert_eq!(conns, [Conn::Tx, Conn::Pool]);
}

#[test]
fn a_projected_returning_carries_the_generated_key_and_its_columns() {
    let p = Project::new(
        "returning",
        &[
            ("src/client.ts", CLIENT),
            ("src/schema.ts", ITEMS),
            (
                "src/use.ts",
                "import { getDb } from \"./client\";\nimport { items } from \"./schema\";\nexport async function f() {\n  const [row] = await getDb().insert(items).values({ name: \"n\" }).returning({ key: items.id });\n  await getDb().insert(items).values({ id: row.key, name: \"copy\" });\n}\n",
            ),
        ],
    );
    let x = extract(
        &p.0,
        "src/use.ts#f",
        vec![binding("items", &["id", "name"], None)],
    );
    assert!(unknowns(&x).is_empty(), "{:?}", unknowns(&x));
    let first = ops(&x)
        .into_iter()
        .find_map(|o| match o {
            Op::Db { stmt } => Some(stmt.kind.clone()),
            _ => None,
        })
        .unwrap();
    let StmtKind::Insert {
        returning_columns, ..
    } = first
    else {
        panic!("not an insert");
    };
    assert_eq!(returning_columns, Some(vec!["id".to_owned()]));
}

#[test]
fn a_helpers_return_slot_is_attributed_to_the_call_site() {
    let p = Project::new(
        "provenance",
        &[
            (
                "src/helper.ts",
                "\n\n\n\n\n\n\n\n\n\nexport async function helper() { return 1; }\n",
            ),
            (
                "src/use.ts",
                "import { helper } from \"./helper\";\nexport async function f() {\n  const v = await helper();\n  return v;\n}\n",
            ),
        ],
    );
    let x = extract(&p.0, "src/use.ts#f", Vec::new());
    let slot = x
        .codes
        .iter()
        .flatten()
        .find(|s| matches!(&s.op, Op::Let { local, .. } if local.ends_with("$ret")))
        .expect("a return slot");
    assert_eq!(slot.src.file, "src/use.ts");
    assert_eq!(slot.src.line, 3);
}

#[test]
fn a_scenario_composes_schema_modules_and_refuses_ambiguity() {
    let p = Project::new(
        "compose",
        &[("src/db/app.ts", SCHEMA_A), ("src/db/audit.ts", SCHEMA_B)],
    );
    let load = |schema: serde_json::Value, tables: serde_json::Value| {
        let doc = serde_json::json!({
            "schemaVersion": 1,
            "name": "compose",
            "extraction": { "root": ".", "schema": schema },
            "database": { "tables": tables },
            "actors": [{ "name": "a", "function": "f", "program": "return\n", "arguments": {} }],
            "invariants": [{ "name": "true", "assert": { "eq": [0, 0] } }],
        });
        midwrite_model::scenario::load_str(&doc.to_string(), &p.0, &TsActorProvider::new(&p.0))
    };
    let both = serde_json::json!(["src/db/app.ts", "src/db/audit.ts"]);
    // `users` is declared by both modules: it must be qualified.
    let err = load(both.clone(), serde_json::json!([{ "from": "users" }])).unwrap_err();
    assert!(err.message.contains("several schema modules"), "{err}");
    // Qualified, the two declarations are two tables with their own names.
    let (model, report) = load(
        both.clone(),
        serde_json::json!([{ "from": "src/db/app.ts#users" }, { "from": "src/db/audit.ts#users" }]),
    )
    .unwrap();
    let names: Vec<&str> = model
        .schema
        .tables
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(names, ["app_users", "audit_users"]);
    assert_eq!(report.schema[0].module.as_deref(), Some("src/db/app.ts"));
    // One declaration read twice is one table, not two.
    let err = load(
        both,
        serde_json::json!([{ "from": "src/db/app.ts#users" }, { "from": "src/db/app.ts#users" }]),
    )
    .unwrap_err();
    assert!(err.message.contains("read twice"), "{err}");
    // A single module keeps the Phase 8 form.
    assert!(
        load(
            serde_json::json!("src/db/app.ts"),
            serde_json::json!([{ "from": "users" }])
        )
        .is_ok()
    );
}
