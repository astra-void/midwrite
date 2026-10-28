//! Phase 13, Track D: source resolution (function aliases, React `cache`,
//! template literals) and package contracts.

use std::path::{Path, PathBuf};

use commitrift_analyzer::TsActorProvider;
use commitrift_model::ir::{Op, StmtKind};
use commitrift_model::provider::{ActorProvider, ExtractRequest, Extracted, TableBinding};

struct Project(PathBuf);

impl Project {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("commitrift-a13r-{}-{name}", std::process::id()));
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

const SCHEMA: &str = r#"
import { integer, pgTable, text } from "drizzle-orm/pg-core";
export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey(),
  email: text("email").notNull(),
  balance: integer("balance").notNull(),
});
"#;

const DB: &str = r#"
import { drizzle } from "drizzle-orm/node-postgres";
export const db = drizzle(process.env.URL!);
"#;

fn extract(root: &Path, selector: &str) -> Extracted {
    TsActorProvider::new(root)
        .extract(&ExtractRequest {
            root: root.to_path_buf(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables: vec![TableBinding {
                identifier: "accounts".to_owned(),
                table: "accounts".to_owned(),
                columns: vec!["id".into(), "email".into(), "balance".into()],
                source: None,
            }],
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

/// Database statements (`Pool SELECT accounts`) and unknown operations
/// (`? <detail>`), in code order.
fn summary(x: &Extracted) -> Vec<String> {
    x.codes
        .iter()
        .flatten()
        .filter_map(|s| match &s.op {
            Op::Db { stmt } => Some(format!(
                "{:?} {} {}",
                stmt.conn,
                match stmt.kind {
                    StmtKind::Select { .. } => "SELECT",
                    StmtKind::SelectCount { .. } => "COUNT",
                    StmtKind::Update { .. } => "UPDATE",
                    StmtKind::Insert { .. } => "INSERT",
                    StmtKind::Delete { .. } => "DELETE",
                },
                stmt.kind.table()
            )),
            Op::Unknown { detail, .. } => Some(format!("? {detail}")),
            _ => None,
        })
        .collect()
}

fn unknowns(x: &Extracted) -> Vec<String> {
    summary(x)
        .into_iter()
        .filter_map(|l| l.strip_prefix("? ").map(str::to_owned))
        .collect()
}

#[test]
fn function_aliases_resolve_within_and_across_modules() {
    let project = Project::new(
        "aliases",
        &[
            ("src/schema.ts", SCHEMA),
            ("src/db.ts", DB),
            (
                "src/policies.ts",
                r#"
export function isAdmin(role: string): boolean {
  return role === "admin";
}
export const canManage = isAdmin;
export const canManageToo = canManage;
"#,
            ),
            (
                "src/entry.ts",
                r#"
import { eq } from "drizzle-orm";
import { db } from "./db";
import { accounts } from "./schema";
import { canManage, canManageToo } from "./policies";
const localAlias = canManage;
export async function remove(role: string, id: number) {
  if (!canManage(role)) throw new Error("denied");
  if (!canManageToo(role)) throw new Error("denied");
  if (!localAlias(role)) throw new Error("denied");
  await db.delete(accounts).where(eq(accounts.id, id));
}
"#,
            ),
        ],
    );
    let x = extract(&project.0, "src/entry.ts#remove");
    assert_eq!(
        summary(&x),
        vec!["Pool DELETE accounts"],
        "{:#?}",
        x.assumptions
    );
    assert!(
        x.assumptions
            .iter()
            .any(|a| a.contains("is an alias of the function `isAdmin`")),
        "{:#?}",
        x.assumptions
    );
}

#[test]
fn a_react_cache_function_runs_on_the_first_call_and_a_repeat_is_unknown() {
    let project = Project::new(
        "react-cache",
        &[
            ("src/schema.ts", SCHEMA),
            ("src/db.ts", DB),
            (
                "src/current.ts",
                r#"
import { eq } from "drizzle-orm";
import { cache } from "react";
import { db } from "./db";
import { accounts } from "./schema";
export const currentAccount = cache(async (id: number) => {
  const [row] = await db.select().from(accounts).where(eq(accounts.id, id)).limit(1);
  return row;
});
"#,
            ),
            (
                "src/lookalike.ts",
                r#"
import { cache } from "some-cache-lib";
export const notReact = cache(async () => 1);
"#,
            ),
            (
                "src/entry.ts",
                r#"
import { currentAccount } from "./current";
import { notReact } from "./lookalike";
export async function once(id: number) {
  const row = await currentAccount(id);
  return row;
}
export async function twice(id: number) {
  const a = await currentAccount(id);
  const b = await currentAccount(id);
  return b;
}
export async function other() {
  return notReact();
}
"#,
            ),
        ],
    );
    let x = extract(&project.0, "src/entry.ts#once");
    assert_eq!(summary(&x), vec!["Pool SELECT accounts"]);
    assert!(
        x.assumptions
            .iter()
            .any(|a| a.contains("is React `cache(fn)`")),
        "{:#?}",
        x.assumptions
    );
    let x = extract(&project.0, "src/entry.ts#twice");
    let got = summary(&x);
    assert_eq!(got[0], "Pool SELECT accounts");
    assert!(
        got[1].contains("a repeated call of `currentAccount` (React `cache`)"),
        "{got:#?}"
    );
    // `cache` of another package is not React's contract.
    let x = extract(&project.0, "src/entry.ts#other");
    assert!(
        unknowns(&x)
            .iter()
            .any(|u| u.contains("`notReact` in `src/lookalike.ts` is not a function declaration")),
        "{:#?}",
        summary(&x)
    );
}

#[test]
fn template_literals_without_interpolation_are_strings_and_others_stay_unknown() {
    let project = Project::new(
        "templates",
        &[
            ("src/schema.ts", SCHEMA),
            ("src/db.ts", DB),
            (
                "src/entry.ts",
                r#"
import { db } from "./db";
import { accounts } from "./schema";
export async function plain(id: number) {
  await db.insert(accounts).values({ id, email: `fixed@example.com`, balance: 0 });
}
export async function interpolated(id: number, name: string) {
  await db.insert(accounts).values({ id, email: `${name}@example.com`, balance: 0 });
}
"#,
            ),
        ],
    );
    let x = extract(&project.0, "src/entry.ts#plain");
    assert_eq!(summary(&x), vec!["Pool INSERT accounts"]);
    let x = extract(&project.0, "src/entry.ts#interpolated");
    assert!(
        unknowns(&x)
            .iter()
            .any(|u| u.contains("string concatenation is not modeled")),
        "{:#?}",
        summary(&x)
    );
}

#[test]
fn package_calls_with_verified_contracts_stay_unknown_and_name_the_contract() {
    let project = Project::new(
        "packages",
        &[
            ("src/schema.ts", SCHEMA),
            (
                "src/validation.ts",
                r#"
import { z } from "zod";
export const signUpSchema = z.object({ email: z.string().email() });
"#,
            ),
            (
                "src/entry.ts",
                r#"
import bcrypt from "bcryptjs";
import { compare } from "bcryptjs";
import { hash as hashTs } from "bcrypt-ts";
import { signUpSchema } from "./validation";
export async function viaDefault(password: string) {
  const digest = await bcrypt.hash(password, 10);
  return digest;
}
export async function viaNamed(password: string, digest: string) {
  const ok = await compare(password, digest);
  return ok;
}
export async function lookalike(password: string) {
  const digest = await hashTs(password, 10);
  return digest;
}
export async function validate(input: unknown) {
  const result = signUpSchema.safeParse(input);
  return result;
}
"#,
            ),
        ],
    );
    let u = unknowns(&extract(&project.0, "src/entry.ts#viaDefault"));
    assert!(
        u.iter().any(|d| d.starts_with(
            "package call with a verified built-in contract: `bcrypt.hash` of `bcryptjs` (async;"
        ) && d.ends_with("its result is not representable in the model")),
        "{u:#?}"
    );
    let u = unknowns(&extract(&project.0, "src/entry.ts#viaNamed"));
    assert!(
        u.iter().any(|d| d.starts_with(
            "package call with a verified built-in contract: `compare` of `bcryptjs`"
        )),
        "{u:#?}"
    );
    let u = unknowns(&extract(&project.0, "src/entry.ts#validate"));
    assert!(
        u.iter().any(|d| d.starts_with(
            "package call with a verified built-in contract: `signUpSchema.safeParse` of `zod` (synchronous;"
        )),
        "{u:#?}"
    );
    // Not by name: another package's `hash` is unknown package behavior.
    let u = unknowns(&extract(&project.0, "src/entry.ts#lookalike"));
    assert!(
        u.iter()
            .any(|d| d.contains("imported from the package `bcrypt-ts`, which is not analyzed")),
        "{u:#?}"
    );
}
