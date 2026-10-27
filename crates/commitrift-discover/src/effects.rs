//! Effect summaries of extracted entries.
//!
//! An [`EffectSummary`] restates the extracted concurrency IR of one entry
//! as the facts candidate discovery needs: every database statement with
//! its table, connection, lock, equality key and written columns, each value
//! traced back to where it comes from (a parameter path, a literal, a column
//! of an earlier read, or a computation over those); every external effect;
//! every branch that depends on a read; which operation can follow which on
//! the control-flow graph; transactions; `try` blocks and their handlers;
//! and every operation the extractor did not understand.
//!
//! Nothing here is new semantics: the summary is a read-only view of the
//! code vectors the model checker executes.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use commitrift_model::ir::{Conn, Isolation, LockMode, OnConflict, Op, SourceRef, Step, StmtKind};
use commitrift_model::provider::Extracted;
use commitrift_model::value::{BinOp, Expr, Value};

/// Where a value comes from.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Val {
    /// A parameter, or a property path below one (`data.email`).
    Param {
        path: String,
    },
    Lit {
        value: Value,
    },
    /// A column of the row an earlier read returned.
    Read {
        effect: usize,
        column: String,
    },
    /// The row (or row list) an earlier read returned, as a whole.
    ReadRow {
        effect: usize,
    },
    /// The row count of an earlier statement (`rows.length`, `count()`,
    /// the rows of `returning()`).
    ReadCount {
        effect: usize,
    },
    /// An expression over the row being written (`stock = stock - $q`): the
    /// database computes it atomically.
    ColumnRelative {
        text: String,
        deps: Vec<Val>,
    },
    /// A computation over other values.
    Computed {
        text: String,
        deps: Vec<Val>,
    },
    Unknown {
        text: String,
    },
}

impl Val {
    /// The leaves this value depends on.
    pub fn leaves(&self, out: &mut Vec<Val>) {
        match self {
            Self::ColumnRelative { deps, .. } | Self::Computed { deps, .. } => {
                for d in deps {
                    d.leaves(out);
                }
            }
            other => out.push(other.clone()),
        }
    }

    pub fn depends_on_read(&self) -> Option<usize> {
        let mut leaves = Vec::new();
        self.leaves(&mut leaves);
        leaves.iter().find_map(|l| match l {
            Self::Read { effect, .. } | Self::ReadRow { effect } | Self::ReadCount { effect } => {
                Some(*effect)
            }
            _ => None,
        })
    }

    pub fn render(&self) -> String {
        match self {
            Self::Param { path } => format!("${path}"),
            Self::Lit { value } => value.render(),
            Self::Read { effect, column } => format!("read#{effect}.{column}"),
            Self::ReadRow { effect } => format!("read#{effect}"),
            Self::ReadCount { effect } => format!("count(read#{effect})"),
            Self::ColumnRelative { text, .. } | Self::Computed { text, .. } => text.clone(),
            Self::Unknown { text } => format!("unknown({text})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Access {
    Read,
    Count,
    Insert,
    Update,
    Delete,
}

impl Access {
    pub fn writes(self) -> bool {
        matches!(self, Self::Insert | Self::Update | Self::Delete)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbEffect {
    pub index: usize,
    /// (code vector, pc).
    pub at: (usize, usize),
    pub access: Access,
    pub table: String,
    pub conn: Conn,
    pub lock: LockMode,
    /// Index of the transaction it runs in (`transactions`), if any.
    pub tx: Option<usize>,
    /// `column = value` conjuncts of the predicate.
    pub eq: Vec<(String, Val)>,
    /// Other conjuncts of the predicate, rendered (`stock >= $q`), with the
    /// columns they mention.
    pub other: Vec<(String, Vec<String>)>,
    /// The predicate could not be split into conjuncts.
    pub opaque_predicate: bool,
    /// Inserted values or `SET` assignments.
    pub writes: Vec<(String, Val)>,
    pub on_conflict: OnConflict,
    pub joins: Vec<String>,
    /// The local receiving the result (row, count or row count).
    pub result: Option<String>,
    /// A branch reads the result (row count of a write, row of a read).
    pub result_checked: bool,
    /// Inside a `try` block, inside a `catch` handler.
    pub in_try: bool,
    pub in_handler: bool,
    pub src: SourceRef,
}

impl DbEffect {
    pub fn eq_val(&self, column: &str) -> Option<&Val> {
        self.eq.iter().find(|(c, _)| c == column).map(|(_, v)| v)
    }
    pub fn write_val(&self, column: &str) -> Option<&Val> {
        self.writes
            .iter()
            .find(|(c, _)| c == column)
            .map(|(_, v)| v)
    }
    pub fn label(&self) -> String {
        let verb = match self.access {
            Access::Read => "SELECT",
            Access::Count => "SELECT count(*)",
            Access::Insert => "INSERT",
            Access::Update => "UPDATE",
            Access::Delete => "DELETE",
        };
        let mut text = format!("{verb} {}", self.table);
        if !self.eq.is_empty() || !self.other.is_empty() {
            let mut parts: Vec<String> = self
                .eq
                .iter()
                .map(|(c, v)| format!("{c} = {}", v.render()))
                .collect();
            parts.extend(self.other.iter().map(|(t, _)| t.clone()));
            text.push_str(&format!(" WHERE {}", parts.join(" AND ")));
        }
        if self.lock != LockMode::None {
            text.push_str(" FOR UPDATE");
        }
        if self.on_conflict == OnConflict::DoNothing {
            text.push_str(" ON CONFLICT DO NOTHING");
        }
        text
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalEffect {
    pub index: usize,
    pub at: (usize, usize),
    pub channel: String,
    pub key: Option<Val>,
    pub tx: Option<usize>,
    pub src: SourceRef,
}

/// A branch whose condition depends on database results.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Guard {
    pub at: (usize, usize),
    pub condition: String,
    /// Database effects whose results the condition reads.
    pub reads: Vec<usize>,
    /// Columns of those results the condition reads.
    pub columns: Vec<String>,
    /// Columns of those results the condition compares (`=`, `!=`) with a
    /// literal: a test of the row's state (`status === "paid"`).
    pub state_tests: Vec<String>,
    /// The branch target and fall-through successors.
    pub targets: (usize, usize),
    pub src: SourceRef,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub begin: (usize, usize),
    pub commit: Option<(usize, usize)>,
    pub isolation: Option<Isolation>,
    pub retries: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnknownOp {
    pub detail: String,
    pub at: (usize, usize),
    pub src: SourceRef,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectSummary {
    pub params: Vec<String>,
    pub db: Vec<DbEffect>,
    pub externals: Vec<ExternalEffect>,
    pub guards: Vec<Guard>,
    pub transactions: Vec<Transaction>,
    pub unknown: Vec<UnknownOp>,
    /// Child tasks (`Promise.all`, fire-and-forget) exist.
    pub spawns: bool,
    /// Shared in-process variables are read or written.
    pub app_state: bool,
    pub assumptions: Vec<String>,
    /// `reach[i]` = the db effects reachable from db effect `i` (main code
    /// vector only; children are not ordered with it).
    #[serde(skip)]
    pub reach: Vec<BTreeSet<usize>>,
    #[serde(skip)]
    pub codes: Vec<Vec<Step>>,
}

impl EffectSummary {
    pub fn writes(&self) -> bool {
        self.db.iter().any(|e| e.access.writes()) || !self.externals.is_empty()
    }

    /// Tables the entry reads or writes, joined tables included.
    pub fn tables(&self) -> BTreeSet<&str> {
        self.db
            .iter()
            .flat_map(|e| {
                std::iter::once(e.table.as_str()).chain(e.joins.iter().map(String::as_str))
            })
            .collect()
    }

    /// Whether db effect `to` can execute after db effect `from`.
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        self.reach.get(from).is_some_and(|r| r.contains(&to))
    }

    /// Whether a pc of code 0 can follow db effect `from`.
    pub fn reaches_pc(&self, from: usize, pc: usize) -> bool {
        let start = self.db[from].at;
        start.0 == 0 && cfg_reach(&self.codes[0], start.1).contains(&pc)
    }

    /// The guard that decides whether `effect` runs after `read`: a branch
    /// reachable from the read that reads its result and from exactly one
    /// of whose successors the effect is reachable.
    pub fn deciding_guard(&self, read: usize, effect_at: (usize, usize)) -> Option<&Guard> {
        if effect_at.0 != 0 || self.db[read].at.0 != 0 {
            return None;
        }
        let code = &self.codes[0];
        let after_read = cfg_reach(code, self.db[read].at.1);
        self.guards.iter().find(|g| {
            g.at.0 == 0 && g.reads.contains(&read) && after_read.contains(&g.at.1) && {
                let a = cfg_reach_from(code, g.targets.0).contains(&effect_at.1);
                let b = cfg_reach_from(code, g.targets.1).contains(&effect_at.1);
                a != b
            }
        })
    }
}

fn successors(code: &[Step], pc: usize, handlers: &BTreeMap<usize, usize>) -> Vec<usize> {
    let mut out = match &code[pc].op {
        Op::Jump { target } => vec![*target],
        Op::JumpIf { target, .. } => vec![pc + 1, *target],
        Op::Return | Op::Throw { .. } | Op::Rethrow { .. } => Vec::new(),
        Op::Try { handler, .. } => vec![pc + 1, *handler],
        _ => vec![pc + 1],
    };
    // Any operation inside a `try` block can fail into its handler.
    if let Some(h) = handlers.get(&pc) {
        out.push(*h);
    }
    out.retain(|t| *t < code.len());
    out
}

/// pc → handler of the innermost enclosing `try` block.
fn try_handlers(code: &[Step]) -> BTreeMap<usize, usize> {
    let mut out = BTreeMap::new();
    let mut stack: Vec<usize> = Vec::new();
    for (pc, step) in code.iter().enumerate() {
        match &step.op {
            Op::Try { handler, .. } => stack.push(*handler),
            Op::EndTry => {
                stack.pop();
            }
            _ => {
                if let Some(h) = stack.last() {
                    out.insert(pc, *h);
                }
            }
        }
    }
    out
}

/// pcs strictly after `pc` on some path.
pub fn cfg_reach(code: &[Step], pc: usize) -> BTreeSet<usize> {
    let handlers = try_handlers(code);
    let mut seen = BTreeSet::new();
    let mut todo = successors(code, pc, &handlers);
    while let Some(p) = todo.pop() {
        if seen.insert(p) {
            todo.extend(successors(code, p, &handlers));
        }
    }
    seen
}

/// pcs reachable from `pc` (inclusive).
pub fn cfg_reach_from(code: &[Step], pc: usize) -> BTreeSet<usize> {
    let mut out = cfg_reach(code, pc);
    if pc < code.len() {
        out.insert(pc);
    }
    out
}

struct Ctx<'a> {
    params: &'a [String],
    /// local → its definitions (`Let`) in all code vectors.
    defs: BTreeMap<String, Vec<&'a Expr>>,
    /// local → (db effect, what the local holds).
    results: BTreeMap<String, (usize, ResultKind)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResultKind {
    Row,
    Count,
}

impl Ctx<'_> {
    fn local(&self, name: &str, depth: u32) -> Val {
        if self.params.iter().any(|p| p == name) {
            return Val::Param {
                path: name.to_owned(),
            };
        }
        if let Some((effect, kind)) = self.results.get(name) {
            return match kind {
                ResultKind::Row => Val::ReadRow { effect: *effect },
                ResultKind::Count => Val::ReadCount { effect: *effect },
            };
        }
        let Some(defs) = self.defs.get(name) else {
            return Val::Unknown {
                text: name.to_owned(),
            };
        };
        // A helper's return slot is first initialized to `null`; the other
        // definition is the value it returns.
        let real: Vec<&&Expr> = defs
            .iter()
            .filter(|e| !matches!(e, Expr::Lit(Value::Null)))
            .collect();
        match real.as_slice() {
            [one] if depth < 16 => self.val(one, depth + 1),
            [] => Val::Lit { value: Value::Null },
            _ => Val::Computed {
                text: format!("{name} (several definitions)"),
                deps: real.iter().map(|e| self.val(e, depth + 1)).collect(),
            },
        }
    }

    fn val(&self, e: &Expr, depth: u32) -> Val {
        if depth > 24 {
            return Val::Unknown { text: e.render() };
        }
        match e {
            Expr::Lit(v) => Val::Lit { value: v.clone() },
            Expr::Local(name) => self.local(name, depth),
            Expr::Field(base, field) => match self.val(base, depth + 1) {
                Val::Param { path } => Val::Param {
                    path: format!("{path}.{field}"),
                },
                Val::ReadRow { effect } if field == "length" => Val::ReadCount { effect },
                Val::ReadRow { effect } => Val::Read {
                    effect,
                    column: field.clone(),
                },
                Val::Read { effect, column } => Val::Read {
                    effect,
                    column: format!("{column}.{field}"),
                },
                other => Val::Computed {
                    text: e.render(),
                    deps: vec![other],
                },
            },
            Expr::Col(c) => Val::ColumnRelative {
                text: c.clone(),
                deps: Vec::new(),
            },
            Expr::Bin(_, a, b) => {
                let deps = vec![self.val(a, depth + 1), self.val(b, depth + 1)];
                if e.mentions_column() {
                    Val::ColumnRelative {
                        text: e.render(),
                        deps,
                    }
                } else {
                    Val::Computed {
                        text: e.render(),
                        deps,
                    }
                }
            }
            Expr::Not(a) | Expr::IsNull(a) => Val::Computed {
                text: e.render(),
                deps: vec![self.val(a, depth + 1)],
            },
            Expr::Object(fields) => Val::Computed {
                text: e.render(),
                deps: fields.iter().map(|(_, v)| self.val(v, depth + 1)).collect(),
            },
            Expr::Unknown(text) => Val::Unknown { text: text.clone() },
        }
    }
}

/// Split a predicate into `column = value` conjuncts and the rest.
fn conjuncts(pred: &Expr, out: &mut Vec<Expr>) {
    match pred {
        Expr::Bin(BinOp::And, a, b) => {
            conjuncts(a, out);
            conjuncts(b, out);
        }
        Expr::Lit(Value::Bool(true)) => {}
        other => out.push(other.clone()),
    }
}

fn column_name(c: &str) -> String {
    // Joined predicates qualify columns (`table.column`).
    c.rsplit('.').next().unwrap_or(c).to_owned()
}

/// Summarize the extracted code of one entry.
pub fn summarize(extracted: &Extracted) -> EffectSummary {
    let codes = &extracted.codes;
    let mut defs: BTreeMap<String, Vec<&Expr>> = BTreeMap::new();
    for code in codes {
        for step in code {
            if let Op::Let { local, expr } = &step.op {
                defs.entry(local.clone()).or_default().push(expr);
            }
        }
    }
    let mut summary = EffectSummary {
        params: extracted.params.clone(),
        assumptions: extracted.assumptions.clone(),
        codes: codes.clone(),
        ..EffectSummary::default()
    };
    // First pass: number db effects and record their result locals.
    let mut results: BTreeMap<String, (usize, ResultKind)> = BTreeMap::new();
    let mut db_at: Vec<(usize, usize)> = Vec::new();
    for (ci, code) in codes.iter().enumerate() {
        for (pc, step) in code.iter().enumerate() {
            if let Op::Db { stmt } = &step.op {
                let n = db_at.len();
                db_at.push((ci, pc));
                match &stmt.kind {
                    StmtKind::Select { into, .. } => {
                        results.insert(into.clone(), (n, ResultKind::Row));
                        // `rows.length` of a `limit(1)` select is lowered as
                        // `len = 1; if (row is null) len = 0` right after it.
                        if let Some(Op::Let {
                            local,
                            expr: Expr::Lit(Value::Int(1)),
                        }) = code.get(pc + 1).map(|s| &s.op)
                            && local.contains("_length")
                        {
                            results.insert(local.clone(), (n, ResultKind::Count));
                        }
                    }
                    StmtKind::SelectCount { into, .. } => {
                        results.insert(into.clone(), (n, ResultKind::Count));
                    }
                    StmtKind::Insert {
                        affected,
                        returning,
                        ..
                    } => {
                        if let Some(a) = affected {
                            results.insert(a.clone(), (n, ResultKind::Count));
                        }
                        if let Some(r) = returning {
                            results.insert(r.clone(), (n, ResultKind::Row));
                        }
                    }
                    StmtKind::Update { affected, .. } | StmtKind::Delete { affected, .. } => {
                        if let Some(a) = affected {
                            results.insert(a.clone(), (n, ResultKind::Count));
                        }
                    }
                }
            }
        }
    }
    let ctx = Ctx {
        params: &extracted.params,
        defs,
        results,
    };
    // Transactions (per code vector, in order).
    let mut tx_of: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for (ci, code) in codes.iter().enumerate() {
        let mut open: Option<usize> = None;
        for (pc, step) in code.iter().enumerate() {
            match &step.op {
                Op::Begin { isolation, retry } => {
                    summary.transactions.push(Transaction {
                        begin: (ci, pc),
                        commit: None,
                        isolation: *isolation,
                        retries: *retry,
                    });
                    open = Some(summary.transactions.len() - 1);
                }
                Op::Commit | Op::Rollback => {
                    if let Some(t) = open.take() {
                        summary.transactions[t].commit = Some((ci, pc));
                    }
                }
                _ => {
                    if let Some(t) = open {
                        tx_of.insert((ci, pc), t);
                    }
                }
            }
        }
    }
    // Try blocks and handler regions (straight-line ranges).
    let mut in_try: BTreeSet<(usize, usize)> = BTreeSet::new();
    let mut in_handler: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (ci, code) in codes.iter().enumerate() {
        for (pc, step) in code.iter().enumerate() {
            if let Op::Try { handler, .. } = &step.op {
                let mut depth = 0;
                let mut end = pc + 1;
                while end < code.len() {
                    match &code[end].op {
                        Op::Try { .. } => depth += 1,
                        Op::EndTry if depth == 0 => break,
                        Op::EndTry => depth -= 1,
                        _ => {}
                    }
                    end += 1;
                }
                for p in pc + 1..end {
                    in_try.insert((ci, p));
                }
                // The handler runs from `handler` to the first jump past it
                // or the end of the vector.
                let mut p = *handler;
                while p < code.len() {
                    in_handler.insert((ci, p));
                    if matches!(
                        code[p].op,
                        Op::Return | Op::Throw { .. } | Op::Rethrow { .. }
                    ) {
                        break;
                    }
                    if let Op::Jump { target } = code[p].op
                        && target > p
                    {
                        break;
                    }
                    p += 1;
                }
            }
        }
    }
    // Second pass: effects.
    let mut checked_locals: BTreeSet<String> = BTreeSet::new();
    for code in codes {
        for step in code {
            if let Op::JumpIf { cond, .. } = &step.op {
                let mut locals = Vec::new();
                cond.locals(&mut locals);
                checked_locals.extend(locals);
            }
        }
    }
    for (n, &(ci, pc)) in db_at.iter().enumerate() {
        let step = &codes[ci][pc];
        let Op::Db { stmt } = &step.op else {
            continue;
        };
        let (access, pred, writes, on_conflict, joins, lock, result) = match &stmt.kind {
            StmtKind::Select {
                pred,
                lock,
                into,
                joins,
                ..
            } => (
                Access::Read,
                Some(pred),
                Vec::new(),
                OnConflict::Error,
                joins.iter().map(|j| j.table.clone()).collect(),
                *lock,
                Some(into.clone()),
            ),
            StmtKind::SelectCount { pred, into, .. } => (
                Access::Count,
                Some(pred),
                Vec::new(),
                OnConflict::Error,
                Vec::new(),
                LockMode::None,
                Some(into.clone()),
            ),
            StmtKind::Update {
                pred,
                set,
                affected,
                ..
            } => (
                Access::Update,
                Some(pred),
                set.iter()
                    .map(|(c, e)| (c.clone(), ctx.val(e, 0)))
                    .collect(),
                OnConflict::Error,
                Vec::new(),
                LockMode::None,
                affected.clone(),
            ),
            StmtKind::Insert {
                values,
                on_conflict,
                affected,
                returning,
                ..
            } => (
                Access::Insert,
                None,
                values
                    .iter()
                    .map(|(c, e)| (c.clone(), ctx.val(e, 0)))
                    .collect(),
                *on_conflict,
                Vec::new(),
                LockMode::None,
                returning.clone().or_else(|| affected.clone()),
            ),
            StmtKind::Delete { pred, affected, .. } => (
                Access::Delete,
                Some(pred),
                Vec::new(),
                OnConflict::Error,
                Vec::new(),
                LockMode::None,
                affected.clone(),
            ),
        };
        let mut eq = Vec::new();
        let mut other = Vec::new();
        let mut opaque = false;
        if let Some(pred) = pred {
            let mut parts = Vec::new();
            conjuncts(pred, &mut parts);
            for part in parts {
                match &part {
                    Expr::Bin(BinOp::Eq, a, b) => match (a.as_ref(), b.as_ref()) {
                        (Expr::Col(c), v) | (v, Expr::Col(c)) if !v.mentions_column() => {
                            eq.push((column_name(c), ctx.val(v, 0)));
                        }
                        _ => {
                            let mut cols = Vec::new();
                            part.columns(&mut cols);
                            other.push((
                                part.render(),
                                cols.iter().map(|c| column_name(c)).collect(),
                            ));
                        }
                    },
                    Expr::Unknown(_) => opaque = true,
                    _ => {
                        let mut cols = Vec::new();
                        part.columns(&mut cols);
                        other.push((part.render(), cols.iter().map(|c| column_name(c)).collect()));
                    }
                }
            }
        }
        let result_checked = result
            .as_ref()
            .is_some_and(|r| local_is_checked(r, &ctx, &checked_locals));
        summary.db.push(DbEffect {
            index: n,
            at: (ci, pc),
            access,
            table: stmt.kind.table().to_owned(),
            conn: stmt.conn,
            lock,
            tx: tx_of.get(&(ci, pc)).copied(),
            eq,
            other,
            opaque_predicate: opaque,
            writes,
            on_conflict,
            joins,
            result,
            result_checked,
            in_try: in_try.contains(&(ci, pc)),
            in_handler: in_handler.contains(&(ci, pc)),
            src: step.src.clone(),
        });
    }
    for (ci, code) in codes.iter().enumerate() {
        for (pc, step) in code.iter().enumerate() {
            match &step.op {
                Op::External { channel, key, .. } => {
                    let index = summary.externals.len();
                    summary.externals.push(ExternalEffect {
                        index,
                        at: (ci, pc),
                        channel: channel.clone(),
                        key: key.as_ref().map(|k| ctx.val(k, 0)),
                        tx: tx_of.get(&(ci, pc)).copied(),
                        src: step.src.clone(),
                    });
                }
                Op::JumpIf { cond, target } => {
                    let mut leaves = Vec::new();
                    ctx.val(cond, 0).leaves(&mut leaves);
                    let mut reads = BTreeSet::new();
                    let mut columns = BTreeSet::new();
                    for leaf in &leaves {
                        match leaf {
                            Val::Read { effect, column } => {
                                reads.insert(*effect);
                                columns.insert(column.clone());
                            }
                            Val::ReadRow { effect } | Val::ReadCount { effect } => {
                                reads.insert(*effect);
                            }
                            _ => {}
                        }
                    }
                    if !reads.is_empty() {
                        let mut state_tests = BTreeSet::new();
                        state_columns(cond, &ctx, &mut state_tests);
                        summary.guards.push(Guard {
                            at: (ci, pc),
                            condition: cond.render(),
                            reads: reads.into_iter().collect(),
                            columns: columns.into_iter().collect(),
                            state_tests: state_tests.into_iter().collect(),
                            targets: (*target, pc + 1),
                            src: step.src.clone(),
                        });
                    }
                }
                Op::Unknown { detail, .. } => summary.unknown.push(UnknownOp {
                    detail: detail.clone(),
                    at: (ci, pc),
                    src: step.src.clone(),
                }),
                Op::Spawn { .. } => summary.spawns = true,
                Op::AppRead { .. } | Op::AppWrite { .. } => summary.app_state = true,
                _ => {}
            }
        }
    }
    // Reachability between db effects of the main code vector.
    let main = codes.first().map(Vec::as_slice).unwrap_or(&[]);
    summary.reach = summary
        .db
        .iter()
        .map(|e| {
            if e.at.0 != 0 {
                return BTreeSet::new();
            }
            let pcs = cfg_reach(main, e.at.1);
            summary
                .db
                .iter()
                .filter(|o| o.at.0 == 0 && pcs.contains(&o.at.1))
                .map(|o| o.index)
                .collect()
        })
        .collect();
    summary
}

/// Columns of read results compared by `=`/`!=` with a literal.
fn state_columns(e: &Expr, ctx: &Ctx<'_>, out: &mut BTreeSet<String>) {
    match e {
        Expr::Bin(BinOp::Eq | BinOp::Ne, a, b) => {
            for (x, y) in [(a, b), (b, a)] {
                if matches!(y.as_ref(), Expr::Lit(v) if !v.is_null())
                    && let Val::Read { column, .. } = ctx.val(x, 0)
                {
                    out.insert(column);
                }
            }
        }
        Expr::Bin(_, a, b) => {
            state_columns(a, ctx, out);
            state_columns(b, ctx, out);
        }
        Expr::Not(a) => state_columns(a, ctx, out),
        _ => {}
    }
}

/// A local is checked when a branch reads it or a local defined from it.
fn local_is_checked(local: &str, ctx: &Ctx<'_>, checked: &BTreeSet<String>) -> bool {
    if checked.contains(local) {
        return true;
    }
    // Locals whose definition reads `local` (one level of aliasing per step).
    let mut frontier = vec![local.to_owned()];
    let mut seen = BTreeSet::new();
    while let Some(l) = frontier.pop() {
        if !seen.insert(l.clone()) {
            continue;
        }
        for (name, defs) in &ctx.defs {
            if defs.iter().any(|e| {
                let mut locals = Vec::new();
                e.locals(&mut locals);
                locals.contains(&l)
            }) {
                if checked.contains(name) {
                    return true;
                }
                frontier.push(name.clone());
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use commitrift_model::ir::{Origin, Stmt};

    fn step(op: Op) -> Step {
        Step {
            op,
            src: SourceRef {
                origin: Origin::Extracted,
                ..SourceRef::declared(1, "")
            },
        }
    }

    fn select(table: &str, pred: Expr, into: &str) -> Op {
        Op::Db {
            stmt: Stmt {
                conn: Conn::Pool,
                kind: StmtKind::Select {
                    table: table.into(),
                    pred,
                    lock: LockMode::None,
                    into: into.into(),
                    joins: Vec::new(),
                    projection: None,
                },
            },
        }
    }

    #[test]
    fn values_are_traced_through_helper_return_slots() {
        // h$ret = null; h$row = SELECT ...; h$ret = h$row; x = h$ret;
        // if (!x) return; INSERT t (email) VALUES (data.email)
        let codes = vec![vec![
            step(Op::Let {
                local: "h$ret".into(),
                expr: Expr::lit(Value::Null),
            }),
            step(select(
                "t",
                Expr::eq(
                    Expr::col("email"),
                    Expr::field(Expr::local("data"), "email"),
                ),
                "h$row",
            )),
            step(Op::Let {
                local: "h$ret".into(),
                expr: Expr::local("h$row"),
            }),
            step(Op::Let {
                local: "x".into(),
                expr: Expr::local("h$ret"),
            }),
            step(Op::JumpIf {
                cond: Expr::not(Expr::local("x")),
                target: 6,
            }),
            step(Op::Return),
            step(Op::Db {
                stmt: Stmt {
                    conn: Conn::Pool,
                    kind: StmtKind::Insert {
                        table: "t".into(),
                        values: vec![("email".into(), Expr::field(Expr::local("data"), "email"))],
                        on_conflict: OnConflict::Error,
                        affected: None,
                        returning: None,
                        returning_columns: None,
                    },
                },
            }),
            step(Op::Return),
        ]];
        let s = summarize(&Extracted {
            params: vec!["data".into()],
            codes,
            ..Extracted::default()
        });
        assert_eq!(s.db.len(), 2);
        assert_eq!(
            s.db[0].eq,
            vec![(
                "email".to_owned(),
                Val::Param {
                    path: "data.email".into()
                }
            )]
        );
        assert_eq!(s.db[1].write_val("email"), s.db[0].eq_val("email"));
        assert!(s.reaches(0, 1));
        assert!(!s.reaches(1, 0));
        assert_eq!(s.guards.len(), 1);
        assert_eq!(s.guards[0].reads, vec![0]);
        assert!(s.deciding_guard(0, s.db[1].at).is_some());
        assert!(s.db[0].result_checked);
    }
}
