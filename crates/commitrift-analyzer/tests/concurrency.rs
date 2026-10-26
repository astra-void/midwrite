//! The TypeScript -> concurrency IR lowering (Phase 6, stage G).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use commitrift_analyzer::TsActorProvider;
use commitrift_model::ir::{Op, Step, StmtKind};
use commitrift_model::provider::{
    ActorProvider, ExternalBinding, ExtractRequest, Extracted, TableBinding,
};
use commitrift_model::value::Value;

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A fresh project directory with the given files.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!(
            "commitrift-concurrency-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self { root }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn tables() -> Vec<TableBinding> {
    let table = |name: &str, columns: &[&str]| TableBinding {
        identifier: name.to_owned(),
        table: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
    };
    vec![
        table("products", &["id", "stock"]),
        table("orders", &["id", "productId"]),
        table("codes", &["code", "redeemedBy"]),
    ]
}

fn externals() -> Vec<ExternalBinding> {
    vec![ExternalBinding {
        callee: "publishEvent".to_owned(),
        channel: "events".to_owned(),
        key_argument: None,
    }]
}

fn extract(project: &Project, file: &str, function: &str) -> Result<Extracted, String> {
    extract_with_depth(project, file, function, 3)
}

fn extract_with_depth(
    project: &Project,
    file: &str,
    function: &str,
    depth: usize,
) -> Result<Extracted, String> {
    TsActorProvider::new(&project.root).extract(&ExtractRequest {
        root: project.root.clone(),
        file: file.to_owned(),
        function: function.to_owned(),
        arguments: vec![Value::str("product-1")],
        tables: tables(),
        externals: externals(),
        transaction_retries: Vec::new(),
        max_inline_depth: depth,
        selector: None,
        contract: None,
    })
}

/// One readable line per step of a code vector.
fn render(code: &[Step]) -> Vec<String> {
    code.iter().map(|s| render_op(&s.op)).collect()
}

fn render_op(op: &Op) -> String {
    match op {
        Op::Begin { isolation, retry } => format!(
            "begin {}{}",
            isolation.map_or("default", |i| i.name()),
            if *retry > 0 { " retry" } else { "" }
        ),
        Op::Commit => "commit".into(),
        Op::Rollback => "rollback".into(),
        Op::Db { stmt } => {
            let conn = format!("{:?}", stmt.conn).to_lowercase();
            match &stmt.kind {
                StmtKind::Select {
                    table,
                    pred,
                    lock,
                    into,
                    ..
                } => format!(
                    "{conn} select {table} where {} lock={lock:?} -> {into}",
                    pred.render()
                ),
                StmtKind::SelectCount { table, pred, into } => {
                    format!("{conn} count {table} where {} -> {into}", pred.render())
                }
                StmtKind::Update {
                    table,
                    pred,
                    set,
                    affected,
                } => format!(
                    "{conn} update {table} where {} set {} affected={}",
                    pred.render(),
                    set.iter()
                        .map(|(c, e)| format!("{c}={}", e.render()))
                        .collect::<Vec<_>>()
                        .join(","),
                    affected.as_deref().unwrap_or("-")
                ),
                StmtKind::Insert {
                    table,
                    values,
                    on_conflict,
                    affected,
                    ..
                } => format!(
                    "{conn} insert {table} {} on_conflict={on_conflict:?} affected={}",
                    values
                        .iter()
                        .map(|(c, e)| format!("{c}={}", e.render()))
                        .collect::<Vec<_>>()
                        .join(","),
                    affected.as_deref().unwrap_or("-")
                ),
                StmtKind::Delete {
                    table,
                    pred,
                    affected,
                } => format!(
                    "{conn} delete {table} where {} affected={}",
                    pred.render(),
                    affected.as_deref().unwrap_or("-")
                ),
            }
        }
        Op::Let { local, expr } => format!("let {local} = {}", expr.render()),
        Op::JumpIf { cond, target } => format!("jumpif {} -> {target}", cond.render()),
        Op::Jump { target } => format!("jump -> {target}"),
        Op::Throw { message } => format!("throw {message:?}"),
        Op::Return => "return".into(),
        Op::External {
            channel,
            payload,
            key,
        } => format!(
            "external {channel} [{}] key={}",
            payload
                .iter()
                .map(|(k, e)| format!("{k}={}", e.render()))
                .collect::<Vec<_>>()
                .join(","),
            key.as_ref().map_or("-".into(), |k| k.render())
        ),
        Op::AppRead { var, into } => format!("app_read {var} -> {into}"),
        Op::AppWrite { var, expr } => format!("app_write {var} = {}", expr.render()),
        Op::Spawn { children, detached } => format!("spawn {children:?} detached={detached}"),
        Op::Join => "join".into(),
        Op::Try { handler, .. } => format!("try -> {handler}"),
        Op::EndTry => "end try".into(),
        Op::Rethrow { local } => format!("rethrow {local}"),
        Op::Unknown { detail } => format!("UNKNOWN {detail}"),
    }
}

fn lines(code: &[Step]) -> Vec<u32> {
    code.iter().map(|s| s.src.line).collect()
}

fn dump(extracted: &Extracted) -> String {
    extracted
        .codes
        .iter()
        .enumerate()
        .map(|(i, code)| {
            format!(
                "code {i}:\n{}",
                render(code)
                    .iter()
                    .enumerate()
                    .map(|(n, l)| format!("  {n}: {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn _unused(_: &Path) {}

const SCHEMA_IMPORTS: &str = r#"import { and, eq, gt } from "drizzle-orm";
import { db } from "./db";
import { products, orders, codes } from "./schema";
"#;

fn with_imports(body: &str) -> String {
    format!("{SCHEMA_IMPORTS}\n{body}")
}

fn lower(files: &[(&str, &str)], file: &str, function: &str) -> (Project, Extracted) {
    let project = Project::new(files);
    let extracted = extract(&project, file, function)
        .unwrap_or_else(|e| panic!("extraction of `{function}` failed: {e}"));
    (project, extracted)
}

fn assert_ops(extracted: &Extracted, code: usize, expected: &[&str]) {
    let actual = render(&extracted.codes[code]);
    assert_eq!(
        actual,
        expected,
        "lowered code {code}:\n{}",
        dump(extracted)
    );
}

fn unknowns(extracted: &Extracted) -> Vec<(u32, String)> {
    extracted
        .codes
        .iter()
        .flatten()
        .filter_map(|s| match &s.op {
            Op::Unknown { detail } => Some((s.src.line, detail.clone())),
            _ => None,
        })
        .collect()
}

const PURCHASE: &str = r#"export async function purchase(productId: string) {
  const product = await db.query.products.findFirst({
    where: eq(products.id, productId),
  });

  if (!product || product.stock <= 0) {
    throw new Error("Out of stock");
  }

  await db.update(products)
    .set({ stock: product.stock - 1 })
    .where(eq(products.id, productId));

  await db.insert(orders).values({ productId });
}
"#;

#[test]
fn purchase_lowers_to_the_exact_read_check_write_sequence() {
    let (_p, e) = lower(
        &[("purchase.ts", &with_imports(PURCHASE))],
        "purchase.ts",
        "purchase",
    );
    assert_eq!(e.params, ["productId"]);
    assert_ops(
        &e,
        0,
        &[
            "pool select products where (@id = productId) lock=None -> _row1",
            "jumpif !(!_row1 or (_row1.stock <= 0)) -> 3",
            "throw \"Out of stock\"",
            "pool update products where (@id = productId) set stock=(_row1.stock - 1) affected=-",
            "pool insert orders productId=productId on_conflict=Error affected=-",
            "return",
        ],
    );
    // Lines are those of the call/statement in the file (3 imports + blank).
    assert_eq!(lines(&e.codes[0]), [6, 10, 11, 14, 18, 19]);
    let first = &e.codes[0][0].src;
    assert_eq!(first.file, "purchase.ts");
    assert_eq!(first.function, "purchase");
    assert!(first.call_chain.is_empty());
    assert_eq!(first.origin, commitrift_model::ir::Origin::Extracted);
    assert!(first.text.starts_with("db.query.products.findFirst"));
    assert!(e.assumptions.iter().any(|a| a.contains("no orderBy")));
    assert!(unknowns(&e).is_empty());
}

#[test]
fn transaction_wraps_the_body_in_begin_and_commit_with_tx_statements() {
    let body = r#"export async function purchaseTx(productId: string) {
  await db.transaction(async (tx) => {
    const product = await tx.query.products.findFirst({
      where: eq(products.id, productId),
    });
    if (!product || product.stock <= 0) {
      throw new Error("Out of stock");
    }
    await tx.update(products).set({ stock: product.stock - 1 }).where(eq(products.id, productId));
    await tx.insert(orders).values({ productId });
  }, { isolationLevel: "serializable" });
}
"#;
    let (_p, e) = lower(&[("tx.ts", &with_imports(body))], "tx.ts", "purchaseTx");
    let ops = render(&e.codes[0]);
    assert_eq!(ops[0], "begin serializable");
    assert!(ops[1].starts_with("let _tx"), "{ops:?}");
    assert!(ops[2].starts_with("tx select products"), "{ops:?}");
    assert!(ops[5].starts_with("tx update products"), "{ops:?}");
    assert!(ops[6].starts_with("tx insert orders"), "{ops:?}");
    assert_eq!(ops[7], "commit");
    assert_eq!(ops[8], "return");
    assert!(unknowns(&e).is_empty());
    // Without options the isolation level is the database default.
    let plain = body.replace(", { isolationLevel: \"serializable\" }", "");
    let (_p, e) = lower(&[("tx.ts", &with_imports(&plain))], "tx.ts", "purchaseTx");
    assert_eq!(render(&e.codes[0])[0], "begin default");
}

#[test]
fn a_return_inside_a_transaction_callback_jumps_to_the_commit() {
    let body = r#"export async function f(productId: string) {
  const out = await db.transaction(async (tx) => {
    const product = await tx.query.products.findFirst({ where: eq(products.id, productId) });
    if (!product) {
      return 0;
    }
    await tx.delete(products).where(eq(products.id, productId));
    return 1;
  });
  await db.insert(orders).values({ productId: out });
}
"#;
    let (_p, e) = lower(&[("f.ts", &with_imports(body))], "f.ts", "f");
    let ops = render(&e.codes[0]);
    let commit = ops.iter().position(|o| o == "commit").unwrap();
    for (i, step) in e.codes[0].iter().enumerate() {
        if let Op::Jump { target } = &step.op {
            assert!(
                *target <= commit,
                "jump at {i} escapes the transaction: {ops:?}"
            );
        }
    }
    // The early return jumps exactly to the commit.
    assert!(
        e.codes[0]
            .iter()
            .any(|s| matches!(&s.op, Op::Jump { target } if *target == commit)),
        "{ops:?}"
    );
    assert!(
        ops.iter().any(|o| o.starts_with("let out = _tx")),
        "{ops:?}"
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
}

#[test]
fn atomic_conditional_update_with_returning_models_the_affected_count() {
    let body = r#"export async function purchaseAtomic(productId: string) {
  const [updated] = await db
    .update(products)
    .set({ stock: sql`${products.stock} - 1` })
    .where(and(eq(products.id, productId), gt(products.stock, 0)))
    .returning();
  if (!updated) {
    throw new Error("Out of stock");
  }
  await db.insert(orders).values({ productId });
}
"#;
    let (_p, e) = lower(&[("a.ts", &with_imports(body))], "a.ts", "purchaseAtomic");
    assert_ops(
        &e,
        0,
        &[
            "pool update products where ((@id = productId) and (@stock > 0)) set stock=(@stock - 1) affected=_affected1",
            "jumpif !!(_affected1 > 0) -> 3",
            "throw \"Out of stock\"",
            "pool insert orders productId=productId on_conflict=Error affected=-",
            "return",
        ],
    );
}

#[test]
fn locking_reads_counts_and_conflict_clauses() {
    let body = r#"export async function claim(id: string) {
  await db.transaction(async (tx) => {
    const [row] = await tx.select().from(products).where(eq(products.id, id)).limit(1).for("update", { skipLocked: true });
    const [{ n }] = await tx.select({ n: count() }).from(orders).where(eq(orders.productId, id));
    if (n > 3) {
      throw new Error("too many");
    }
    await tx.insert(orders).values({ id: id, productId: id }).onConflictDoNothing();
  });
}
"#;
    let (_p, e) = lower(&[("c.ts", &with_imports(body))], "c.ts", "claim");
    let ops = render(&e.codes[0]);
    assert!(
        ops[2].contains("select products where (@id = id) lock=SkipLocked"),
        "{ops:?}"
    );
    assert!(
        ops[3].starts_with("tx count orders where (@productId = id) -> _count"),
        "{ops:?}"
    );
    assert!(ops[4].starts_with("jumpif !(_count"), "{ops:?}");
    assert!(ops[6].contains("on_conflict=DoNothing"), "{ops:?}");
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    // A multi-row lock is not modeled.
    let multi = body.replace(".limit(1)", "");
    let (_p, e) = lower(&[("c.ts", &with_imports(&multi))], "c.ts", "claim");
    assert!(
        unknowns(&e).iter().any(|(_, d)| d.contains("limit(1)")),
        "{:?}",
        unknowns(&e)
    );
}

#[test]
fn check_then_act_inlines_same_file_and_imported_helpers() {
    let main = with_imports(
        r#"import { markRedeemed } from "./redeem-helpers";

async function alreadyRedeemed(code: string) {
  const row = await db.query.codes.findFirst({ where: eq(codes.code, code) });
  return !!row && row.redeemedBy !== null;
}

export async function redeem(code: string, user: string) {
  if (!(await alreadyRedeemed(code))) {
    await markRedeemed(code, user);
  }
}
"#,
    );
    let helpers = with_imports(
        r#"export async function markRedeemed(code: string, user: string) {
  await db.update(codes).set({ redeemedBy: user }).where(eq(codes.code, code));
}
"#,
    );
    let (_p, e) = lower(
        &[("redeem.ts", &main), ("redeem-helpers.ts", &helpers)],
        "redeem.ts",
        "redeem",
    );
    assert_eq!(e.params, ["code", "user"]);
    let ops = render(&e.codes[0]);
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
    let select = e.codes[0]
        .iter()
        .find(|s| matches!(&s.op, Op::Db { stmt } if matches!(stmt.kind, StmtKind::Select { .. })))
        .unwrap();
    assert_eq!(select.src.function, "alreadyRedeemed");
    assert_eq!(select.src.call_chain, ["redeem"]);
    assert_eq!(select.src.file, "redeem.ts");
    let update = e.codes[0]
        .iter()
        .find(|s| matches!(&s.op, Op::Db { stmt } if matches!(stmt.kind, StmtKind::Update { .. })))
        .unwrap();
    assert_eq!(update.src.function, "markRedeemed");
    assert_eq!(update.src.call_chain, ["redeem"]);
    assert_eq!(update.src.file, "redeem-helpers.ts");
    // The parameters are bound to fresh, prefixed locals; the update sits
    // on the not-redeemed branch.
    assert!(
        ops.iter().any(|o| o.starts_with("let h1$code = code")),
        "{ops:?}"
    );
    assert!(ops.iter().any(|o| o.contains("$user = user")), "{ops:?}");
    let update_index = ops.iter().position(|o| o.contains("update codes")).unwrap();
    let guard = ops
        .iter()
        .position(|o| o.starts_with("jumpif !!"))
        .unwrap_or_else(|| panic!("{ops:?}"));
    assert!(guard < update_index);
    match &e.codes[0][guard].op {
        Op::JumpIf { target, .. } => assert!(*target > update_index, "{ops:?}"),
        _ => unreachable!(),
    }
}

#[test]
fn inline_depth_and_recursion_are_bounded_not_dropped() {
    let src = with_imports(
        r#"async function c(id: string) { await db.delete(products).where(eq(products.id, id)); }
async function b(id: string) { await c(id); }
async function a(id: string) { await b(id); }
async function rec(id: string) { await rec(id); }
export async function top(id: string) { await a(id); }
export async function loops(id: string) { await rec(id); }
"#,
    );
    let project = Project::new(&[("d.ts", &src)]);
    let deep = extract_with_depth(&project, "d.ts", "top", 1).unwrap();
    assert!(
        unknowns(&deep)
            .iter()
            .any(|(_, d)| d.contains("inline depth limit")),
        "{}",
        dump(&deep)
    );
    let ok = extract_with_depth(&project, "d.ts", "top", 3).unwrap();
    assert!(unknowns(&ok).is_empty(), "{}", dump(&ok));
    let rec = extract_with_depth(&project, "d.ts", "loops", 5).unwrap();
    assert!(
        unknowns(&rec).iter().any(|(_, d)| d.contains("recursive")),
        "{}",
        dump(&rec)
    );
}

#[test]
fn promise_all_becomes_spawn_and_join_with_one_child_per_element() {
    let body = r#"export async function finish(productId: string) {
  await Promise.all([
    db.update(products).set({ stock: 0 }).where(eq(products.id, productId)),
    publishEvent({ productId }),
  ]);
}
"#;
    let (_p, e) = lower(&[("p.ts", &with_imports(body))], "p.ts", "finish");
    assert_ops(&e, 0, &["spawn [1, 2] detached=false", "join", "return"]);
    assert_ops(
        &e,
        1,
        &[
            "pool update products where (@id = productId) set stock=0 affected=-",
            "return",
        ],
    );
    assert_ops(
        &e,
        2,
        &["external events [productId=productId] key=-", "return"],
    );
    // Sequential awaits are not concurrent.
    let seq = body
        .replace("await Promise.all([\n    db.", "await db.")
        .replace(
            "),\n    publishEvent({ productId }),\n  ]);",
            ");\n  await publishEvent({ productId });",
        );
    let (_p, e) = lower(&[("p.ts", &with_imports(&seq))], "p.ts", "finish");
    assert_eq!(e.codes.len(), 1);
    assert!(render(&e.codes[0]).iter().all(|o| !o.starts_with("spawn")));
}

#[test]
fn fire_and_forget_effects_are_detached_children() {
    let body = r#"async function notify(id: string) {
  await publishEvent({ id });
}
export async function f(productId: string) {
  await db.delete(products).where(eq(products.id, productId));
  publishEvent({ productId });
  notify(productId);
}
"#;
    let (_p, e) = lower(&[("f.ts", &with_imports(body))], "f.ts", "f");
    let main = render(&e.codes[0]);
    assert_eq!(
        main.iter().filter(|o| o.starts_with("spawn")).count(),
        2,
        "{}",
        dump(&e)
    );
    assert!(main.iter().all(|o| !o.contains("detached=false")));
    assert!(
        e.codes[1]
            .iter()
            .any(|s| matches!(s.op, Op::External { .. }))
    );
    assert!(
        e.codes[2]
            .iter()
            .any(|s| matches!(s.op, Op::External { .. }) && s.src.function == "notify")
    );
}

#[test]
fn optional_chaining_and_ternaries_are_hoisted_exactly() {
    let body = r#"export async function f(id: string) {
  const product = await db.query.products.findFirst({ where: eq(products.id, id) });
  const stock = product?.stock;
  const label = stock > 0 ? 1 : 2;
  await db.insert(orders).values({ id: id, productId: id, label });
}
"#;
    let src = with_imports(body);
    let project = Project::new(&[("o.ts", &src)]);
    let e = extract(&project, "o.ts", "f").unwrap();
    let ops = render(&e.codes[0]);
    // `?.` becomes a null-guarded read; the ternary is a two-armed region.
    assert!(
        ops.iter().any(|o| o.starts_with("jumpif _row1 is null")),
        "{ops:?}"
    );
    assert!(ops.iter().any(|o| o.starts_with("let _opt")), "{ops:?}");
    assert!(ops.iter().any(|o| o.starts_with("let _cond")), "{ops:?}");
    // The `label` shorthand column does not exist in `orders`: unknown.
    assert!(
        unknowns(&e).iter().any(|(_, d)| d.contains("label")),
        "{}",
        dump(&e)
    );
}

#[test]
fn unsupported_constructs_become_unknown_ops_with_the_right_line() {
    let src = with_imports(
        // Phase 7 models `for...of` over an array literal and `try`/`catch`;
        // a loop of unknown length and a `return` through `finally` stay
        // unknown.
        r#"export async function f(id: string, ids: string[]) {
  for (const n of ids) {
    await db.delete(products).where(eq(products.id, id));
  }
  await db.execute(sql`delete from products`);
  await mystery(id);
  try {
    return await db.delete(products).where(eq(products.id, id));
  } finally {
    await mystery(id);
  }
  await db.update(products).set({ stock: id.length }).where(like(products.id, "p%"));
}
"#,
    );
    let (_p, e) = lower(&[("u.ts", &src)], "u.ts", "f");
    let found = unknowns(&e);
    let line_of = |needle: &str| {
        src.lines()
            .position(|l| l.contains(needle))
            .map(|i| i as u32 + 1)
            .unwrap()
    };
    let detail = |line: u32| {
        found
            .iter()
            .find(|(l, _)| *l == line)
            .map(|(_, d)| d.clone())
            .unwrap_or_else(|| panic!("no unknown at line {line}: {found:?}"))
    };
    assert!(detail(line_of("for (const n")).contains("unknown length"));
    assert!(detail(line_of("db.execute")).contains("raw SQL"));
    assert!(detail(line_of("mystery(id)")).contains("mystery"));
    assert!(detail(line_of("try {")).contains("`finally`"));
    // An unsupported predicate operator makes the statement unknown (not a
    // no-op, not an unconditional update).
    assert!(detail(line_of("like(")).contains("like"));
    // The statements inside the loop and the try block are not lowered at all.
    let deletes = render(&e.codes[0])
        .iter()
        .filter(|o| o.contains("delete products"))
        .count();
    assert_eq!(deletes, 0, "{}", dump(&e));
}

#[test]
fn unbound_tables_and_non_column_properties_are_rejected() {
    let src = with_imports(
        r#"export async function f(id: string) {
  await db.delete(users).where(eq(users.id, id));
  await db.update(products).set({ price: 1 }).where(eq(products.id, id));
  await db.delete(products).where(eq(products.sku, id));
}
"#,
    );
    let (_p, e) = lower(&[("t.ts", &src)], "t.ts", "f");
    let found = unknowns(&e);
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("`users`") && d.contains("not bound")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("`price` is not a column")),
        "{found:?}"
    );
    assert!(
        found
            .iter()
            .any(|(_, d)| d.contains("`sku` is not a column")),
        "{found:?}"
    );
}

#[test]
fn handles_are_passed_to_helpers_and_keep_their_connection() {
    let src = with_imports(
        r#"async function decrement(handle: any, id: string) {
  await handle.update(products).set({ stock: sql`${products.stock} - 1` }).where(eq(products.id, id));
}
export async function f(id: string) {
  await db.transaction(async (tx) => {
    await decrement(tx, id);
  });
  await decrement(db, id);
}
"#,
    );
    let (_p, e) = lower(&[("h.ts", &src)], "h.ts", "f");
    let ops = render(&e.codes[0]);
    assert!(
        ops.iter().any(|o| o.starts_with("tx update products")),
        "{ops:?}"
    );
    assert!(
        ops.iter().any(|o| o.starts_with("pool update products")),
        "{ops:?}"
    );
    assert!(unknowns(&e).is_empty(), "{:?}", unknowns(&e));
}

#[test]
fn unawaited_queries_and_unawaited_transactions_are_not_silently_dropped() {
    let src = with_imports(
        r#"export async function f(id: string) {
  db.delete(products).where(eq(products.id, id));
  db.transaction(async (tx) => {
    await tx.delete(products).where(eq(products.id, id));
  });
}
"#,
    );
    let (_p, e) = lower(&[("n.ts", &src)], "n.ts", "f");
    let found = unknowns(&e);
    assert!(
        found.iter().any(|(_, d)| d.contains("not awaited")),
        "{found:?}"
    );
    assert_eq!(found.len(), 2, "{found:?}");
}

#[test]
fn a_missing_function_or_file_is_an_error_not_an_empty_program() {
    let src = with_imports(PURCHASE);
    let project = Project::new(&[("p.ts", &src)]);
    assert!(
        extract(&project, "p.ts", "nope")
            .unwrap_err()
            .contains("not found")
    );
    assert!(extract(&project, "missing.ts", "purchase").is_err());
}

#[test]
fn lowered_purchase_has_the_same_shape_as_the_declared_program() {
    use commitrift_model::dsl;
    let (_p, e) = lower(
        &[("purchase.ts", &with_imports(PURCHASE))],
        "purchase.ts",
        "purchase",
    );
    let declared = dsl::parse(
        "let product = select products where @id == productId\n\
         if product == null || product.stock <= 0 {\n  fail \"Out of stock\"\n}\n\
         update products where @id == productId set @stock = product.stock - 1\n\
         insert orders set @productId = productId\n",
    )
    .unwrap();
    let kinds = |code: &[Step]| -> Vec<String> {
        code.iter()
            .map(|s| match &s.op {
                Op::Db { stmt } => format!(
                    "{:?}:{}:{}",
                    stmt.conn,
                    match &stmt.kind {
                        StmtKind::Select { .. } => "select",
                        StmtKind::SelectCount { .. } => "count",
                        StmtKind::Update { .. } => "update",
                        StmtKind::Insert { .. } => "insert",
                        StmtKind::Delete { .. } => "delete",
                    },
                    stmt.kind.table()
                ),
                other => format!("{:?}", std::mem::discriminant(other)),
            })
            .collect()
    };
    assert_eq!(kinds(&e.codes[0]), kinds(&declared[0]));
    // And the throw carries the same message.
    let message = |code: &[Step]| {
        code.iter().find_map(|s| match &s.op {
            Op::Throw { message } => Some(message.clone()),
            _ => None,
        })
    };
    assert_eq!(message(&e.codes[0]), message(&declared[0]));
}

#[test]
fn external_calls_carry_their_idempotency_key_and_payload() {
    let src = with_imports(
        r#"export async function pay(orderId: string) {
  await charge(orderId, { amount: 10 });
}
"#,
    );
    let project = Project::new(&[("pay.ts", &src)]);
    let e = TsActorProvider::new(&project.root)
        .extract(&ExtractRequest {
            root: project.root.clone(),
            file: "pay.ts".to_owned(),
            function: "pay".to_owned(),
            arguments: vec![],
            tables: tables(),
            externals: vec![ExternalBinding {
                callee: "charge".to_owned(),
                channel: "payment".to_owned(),
                key_argument: Some(0),
            }],
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: None,
            contract: None,
        })
        .unwrap();
    assert_ops(
        &e,
        0,
        &["external payment [amount=10] key=orderId", "return"],
    );
}

#[test]
fn select_array_with_limit_one_models_only_its_length() {
    let project = Project::new(&[(
        "src/check.ts",
        r#"
import { eq } from "drizzle-orm";
export async function once(id: string) {
  const rows = await db.select().from(products).where(eq(products.id, id)).limit(1);
  if (rows.length > 0) {
    return;
  }
  await db.insert(products).values({ id, stock: 1 });
}
export async function unlimited(id: string) {
  const rows = await db.select().from(products).where(eq(products.id, id));
  if (rows.length > 0) {
    return;
  }
}
"#,
    )]);
    let ex = extract(&project, "src/check.ts", "once").unwrap();
    let ops = render(&ex.codes[0]);
    assert!(ops[0].starts_with("pool select products"), "{ops:#?}");
    assert!(!ops.iter().any(|o| o.starts_with("UNKNOWN")), "{ops:#?}");
    // The length is 1 when the row exists, else 0, and drives the branch.
    let ex = extract(&project, "src/check.ts", "unlimited").unwrap();
    let ops = render(&ex.codes[0]);
    assert!(
        ops.iter()
            .any(|o| o.starts_with("UNKNOWN") && o.contains("limit(1)")),
        "{ops:#?}"
    );
}
