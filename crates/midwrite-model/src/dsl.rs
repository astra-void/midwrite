//! A small text language for *declared* actor programs (fixtures).
//!
//! It exists so that scenarios and tests can state precisely what an actor
//! does without a TypeScript project, and so that fixture-declared programs
//! are clearly separated from programs extracted from source
//! ([`crate::ir::Origin`]). It is not a general language: it lowers directly
//! to [`crate::ir`] and is not executed outside the model.
//!
//! ```text
//! begin isolation repeatable-read retry 2   # open the actor's transaction
//! let p = select products where @id == id for update
//! let n = count doctors where @on_call == true && @id != me
//! if p == null || p.stock <= 0 {
//!     fail "out of stock"
//! }
//! update products where @id == id set @stock = @stock - 1 into changed
//! insert orders set @id = oid, @product = id on conflict do nothing
//! external payment key oid set amount = 10
//! commit
//! ```
//!
//! `@name` is a column of the row a `where`/`set` clause is evaluated
//! against; a bare identifier is an actor-local variable (arguments are
//! locals).
//!
//! Phase 14 values: `[a, b]` (an array), `undefined`, built-in methods
//! `x.includes(y)`, `x.split(",")`, `x.at(0)`, `m.get(k)`, `m.set(k, v)`,
//! ... (the names of [`crate::value::Builtin`]), array callbacks
//! `xs.filter(x => x > 1)`, and the functions `String(x)`, `typeof(x)`,
//! `concat(a, b, ...)`, `length(x)`, `size(x)`, `newMap(entries)`,
//! `newSet(items)`. Statements inside `begin ... commit` run on the transaction
//! (`Conn::Tx`); others run in autocommit mode (`Conn::Pool`). Prefix a
//! statement with `pool` or `tx` to override.
//!
//! Phase 16: `let t = generate "nanoid(21)"` is a fresh value from a package
//! call ([`Op::Generate`]: a generation event of the actor; the string names
//! the call for reports).
//!
//! Phase 16: `let rows = select all t where ... [order by @id] [limit n]
//! [for update]` binds the list of every matching row (the multi-row mode,
//! locked one row at a time in key order); `inArray(@col, [a, b])` is
//! Drizzle's `inArray`.

use crate::ir::{Conn, Isolation, LockMode, OnConflict, Op, SourceRef, Step, Stmt, StmtKind};
use crate::value::{BinOp, Builtin, EachOp, Expr, Value};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct ParseError {
    pub line: u32,
    pub message: String,
}

fn err<T>(line: u32, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError {
        line,
        message: message.into(),
    })
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Col(String),
    Int(i64),
    Str(String),
    Sym(&'static str),
}

fn tokenize(line: u32, text: &str) -> Result<Vec<Tok>, ParseError> {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '#' || (c == '/' && chars.get(i + 1) == Some(&'/')) {
            break;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let s: String = chars[start..i].iter().collect();
            match s.parse() {
                Ok(n) => out.push(Tok::Int(n)),
                Err(_) => return err(line, format!("integer `{s}` is out of range")),
            }
        } else if c.is_alphabetic() || c == '_' || c == '@' {
            let at = c == '@';
            let start = if at { i + 1 } else { i };
            i = start;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            // `@table.column`: a qualified column of a joined query.
            if at
                && chars.get(i) == Some(&'.')
                && chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_alphabetic() || *c == '_')
            {
                i += 1;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            }
            let s: String = chars[start..i].iter().collect();
            if s.is_empty() {
                return err(line, "`@` must be followed by a column name");
            }
            out.push(if at { Tok::Col(s) } else { Tok::Ident(s) });
        } else if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            if i >= chars.len() {
                return err(line, "unterminated string");
            }
            out.push(Tok::Str(chars[start..i].iter().collect()));
            i += 1;
        } else {
            let two: String = chars[i..chars.len().min(i + 2)].iter().collect();
            let sym2: Option<&'static str> = match two.as_str() {
                "==" => Some("=="),
                "!=" => Some("!="),
                "<>" => Some("!="),
                "<=" => Some("<="),
                ">=" => Some(">="),
                "&&" => Some("&&"),
                "||" => Some("||"),
                "=>" => Some("=>"),
                _ => None,
            };
            if let Some(s) = sym2 {
                out.push(Tok::Sym(s));
                i += 2;
                continue;
            }
            let sym1: Option<&'static str> = match c {
                '+' => Some("+"),
                '-' => Some("-"),
                '*' => Some("*"),
                '=' => Some("="),
                '<' => Some("<"),
                '>' => Some(">"),
                '!' => Some("!"),
                '(' => Some("("),
                ')' => Some(")"),
                '.' => Some("."),
                ',' => Some(","),
                '{' => Some("{"),
                '}' => Some("}"),
                '[' => Some("["),
                ']' => Some("]"),
                _ => None,
            };
            match sym1 {
                Some(s) => {
                    out.push(Tok::Sym(s));
                    i += 1;
                }
                None => return err(line, format!("unexpected character `{c}`")),
            }
        }
    }
    Ok(out)
}

struct Cursor<'a> {
    line: u32,
    toks: &'a [Tok],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn at_end(&self) -> bool {
        self.pos >= self.toks.len()
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s == kw)
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn eat_sym(&mut self, sym: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(s)) if *s == sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn expect_sym(&mut self, sym: &str) -> Result<(), ParseError> {
        if self.eat_sym(sym) {
            Ok(())
        } else {
            err(self.line, format!("expected `{sym}`"))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> Result<(), ParseError> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            err(self.line, format!("expected `{kw}`"))
        }
    }
    fn ident(&mut self, what: &str) -> Result<String, ParseError> {
        match self.next() {
            Some(Tok::Ident(s)) => Ok(s),
            _ => err(self.line, format!("expected {what}")),
        }
    }

    // expr := or
    fn expr(&mut self) -> Result<Expr, ParseError> {
        self.or()
    }
    fn or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.and()?;
        while self.eat_sym("||") || self.eat_kw("or") {
            let right = self.and()?;
            left = Expr::bin(BinOp::Or, left, right);
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.cmp()?;
        while self.eat_sym("&&") || self.eat_kw("and") {
            let right = self.cmp()?;
            left = Expr::bin(BinOp::And, left, right);
        }
        Ok(left)
    }
    fn cmp(&mut self) -> Result<Expr, ParseError> {
        let left = self.add()?;
        if self.eat_kw("is") {
            let negated = self.eat_kw("not");
            self.expect_kw("null")?;
            let e = Expr::IsNull(Box::new(left));
            return Ok(if negated { Expr::not(e) } else { e });
        }
        let op = match self.peek() {
            Some(Tok::Sym("==" | "=")) => Some(BinOp::Eq),
            Some(Tok::Sym("!=")) => Some(BinOp::Ne),
            Some(Tok::Sym("<")) => Some(BinOp::Lt),
            Some(Tok::Sym("<=")) => Some(BinOp::Le),
            Some(Tok::Sym(">")) => Some(BinOp::Gt),
            Some(Tok::Sym(">=")) => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let right = self.add()?;
            return Ok(Expr::bin(op, left, right));
        }
        Ok(left)
    }
    fn add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Sym("+")) => BinOp::Add,
                Some(Tok::Sym("-")) => BinOp::Sub,
                _ => break,
            };
            self.pos += 1;
            let right = self.mul()?;
            left = Expr::bin(op, left, right);
        }
        Ok(left)
    }
    fn mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.unary()?;
        while self.eat_sym("*") {
            let right = self.unary()?;
            left = Expr::bin(BinOp::Mul, left, right);
        }
        Ok(left)
    }
    fn unary(&mut self) -> Result<Expr, ParseError> {
        if self.eat_sym("!") || self.eat_kw("not") {
            return Ok(Expr::not(self.unary()?));
        }
        if self.eat_sym("-") {
            let e = self.unary()?;
            return Ok(Expr::bin(BinOp::Sub, Expr::lit(0), e));
        }
        self.postfix()
    }
    fn postfix(&mut self) -> Result<Expr, ParseError> {
        let mut e = self.atom()?;
        while self.eat_sym(".") {
            let name = self.ident("a field name")?;
            if matches!(self.peek(), Some(Tok::Sym("("))) {
                e = self.method(e, &name)?;
            } else {
                e = Expr::Field(Box::new(e), name);
            }
        }
        Ok(e)
    }

    /// Phase 14: `recv.name(args)` of a built-in method, or an array
    /// callback `recv.filter(x => body)`.
    fn method(&mut self, recv: Expr, name: &str) -> Result<Expr, ParseError> {
        self.expect_sym("(")?;
        let each = match name {
            "map" => Some(EachOp::Map),
            "filter" => Some(EachOp::Filter),
            "find" => Some(EachOp::Find),
            "findIndex" => Some(EachOp::FindIndex),
            "some" => Some(EachOp::Some),
            "every" => Some(EachOp::Every),
            _ => None,
        };
        if let Some(op) = each {
            let var = self.ident("a callback parameter")?;
            self.expect_sym("=>")?;
            let body = self.expr()?;
            self.expect_sym(")")?;
            return Ok(Expr::Each {
                op,
                list: Box::new(recv),
                var,
                body: Box::new(body),
            });
        }
        let func = match name {
            "includes" => Builtin::Includes,
            "indexOf" => Builtin::IndexOf,
            "startsWith" => Builtin::StartsWith,
            "endsWith" => Builtin::EndsWith,
            "split" => Builtin::Split,
            "toLowerCase" => Builtin::ToLowerCase,
            "toUpperCase" => Builtin::ToUpperCase,
            "trim" => Builtin::Trim,
            "join" => Builtin::Join,
            "at" => Builtin::At,
            "get" => Builtin::MapGet,
            "set" => Builtin::MapSet,
            "has" => Builtin::Has,
            "add" => Builtin::SetAdd,
            "delete" => Builtin::Delete,
            other => return err(self.line, format!("unknown method `{other}`")),
        };
        let mut args = vec![recv];
        args.extend(self.call_args()?);
        Ok(Expr::Call { func, args })
    }

    /// Arguments after `(`, through `)`.
    fn call_args(&mut self) -> Result<Vec<Expr>, ParseError> {
        let mut args = Vec::new();
        if self.eat_sym(")") {
            return Ok(args);
        }
        loop {
            args.push(self.expr()?);
            if self.eat_sym(")") {
                return Ok(args);
            }
            self.expect_sym(",")?;
        }
    }
    fn atom(&mut self) -> Result<Expr, ParseError> {
        match self.next() {
            Some(Tok::Int(n)) => Ok(Expr::lit(n)),
            Some(Tok::Str(s)) => Ok(Expr::Lit(Value::Str(s))),
            Some(Tok::Col(c)) => Ok(Expr::Col(c)),
            Some(Tok::Ident(s)) => match s.as_str() {
                "true" => Ok(Expr::lit(true)),
                "false" => Ok(Expr::lit(false)),
                "null" => Ok(Expr::Lit(Value::Null)),
                "undefined" => Ok(Expr::undefined()),
                // Phase 14: built-in functions.
                name if matches!(self.peek(), Some(Tok::Sym("(")))
                    && matches!(
                        name,
                        "String"
                            | "typeof"
                            | "concat"
                            | "length"
                            | "size"
                            | "newMap"
                            | "newSet"
                            | "inArray"
                    ) =>
                {
                    let func = match name {
                        "String" => Builtin::ToString,
                        "typeof" => Builtin::TypeOf,
                        "concat" => Builtin::Concat,
                        "length" => Builtin::Length,
                        "size" => Builtin::Size,
                        "newMap" => Builtin::NewMap,
                        // Phase 16: Drizzle's `inArray(column, list)` (SQL).
                        "inArray" => Builtin::InList,
                        _ => Builtin::NewSet,
                    };
                    self.expect_sym("(")?;
                    let args = self.call_args()?;
                    Ok(Expr::Call { func, args })
                }
                _ => Ok(Expr::Local(s)),
            },
            Some(Tok::Sym("[")) => {
                let mut items = Vec::new();
                if !self.eat_sym("]") {
                    loop {
                        items.push(self.expr()?);
                        if self.eat_sym("]") {
                            break;
                        }
                        self.expect_sym(",")?;
                    }
                }
                Ok(Expr::Array(items))
            }
            Some(Tok::Sym("(")) => {
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            _ => err(self.line, "expected an expression"),
        }
    }

    fn assigns(&mut self, columns: bool) -> Result<Vec<(String, Expr)>, ParseError> {
        let mut out = Vec::new();
        loop {
            let name = match self.next() {
                Some(Tok::Col(c)) if columns => c,
                Some(Tok::Ident(s)) if !columns => s,
                _ => {
                    return err(
                        self.line,
                        if columns {
                            "expected `@column = expression`"
                        } else {
                            "expected `name = expression`"
                        },
                    );
                }
            };
            self.expect_sym("=")?;
            let e = self.expr()?;
            out.push((name, e));
            if !self.eat_sym(",") {
                break;
            }
        }
        Ok(out)
    }
}

/// Parse a single expression (used for declared CHECK constraints).
pub fn parse_expr(text: &str) -> Result<Expr, ParseError> {
    let toks = tokenize(1, text)?;
    let mut c = Cursor {
        line: 1,
        toks: &toks,
        pos: 0,
    };
    let e = c.expr()?;
    if !c.at_end() {
        return err(1, "unexpected tokens after the expression");
    }
    Ok(e)
}

struct Line {
    no: u32,
    text: String,
    toks: Vec<Tok>,
}

struct Builder {
    codes: Vec<Vec<Step>>,
    in_tx: bool,
}

/// Parse a declared program into code vectors (index 0 is the main task).
pub fn parse(source: &str) -> Result<Vec<Vec<Step>>, ParseError> {
    let mut lines = Vec::new();
    for (i, raw) in source.lines().enumerate() {
        let no = i as u32 + 1;
        let toks = tokenize(no, raw)?;
        if !toks.is_empty() {
            lines.push(Line {
                no,
                text: raw.trim().to_owned(),
                toks,
            });
        }
    }
    let mut b = Builder {
        codes: vec![Vec::new()],
        in_tx: false,
    };
    let mut pos = 0;
    parse_block(&mut b, &lines, &mut pos, 0, false)?;
    if pos < lines.len() {
        return err(lines[pos].no, "unexpected `}`");
    }
    let main = &mut b.codes[0];
    main.push(Step {
        op: Op::Return,
        src: SourceRef::declared(0, "(end)"),
    });
    for code in b.codes.iter_mut().skip(1) {
        code.push(Step {
            op: Op::Return,
            src: SourceRef::declared(0, "(end)"),
        });
    }
    Ok(b.codes)
}

fn push(b: &mut Builder, code: usize, line: &Line, op: Op) -> usize {
    b.codes[code].push(Step {
        op,
        src: SourceRef::declared(line.no, &line.text),
    });
    b.codes[code].len() - 1
}

/// Parse statements until a line starting with `}` (when `nested`) or the
/// end of input.
fn parse_block(
    b: &mut Builder,
    lines: &[Line],
    pos: &mut usize,
    code: usize,
    nested: bool,
) -> Result<(), ParseError> {
    while *pos < lines.len() {
        let line = &lines[*pos];
        if matches!(line.toks.first(), Some(Tok::Sym("}"))) {
            if nested {
                return Ok(());
            }
            return err(line.no, "unexpected `}`");
        }
        *pos += 1;
        parse_stmt(b, lines, pos, code, line)?;
    }
    if nested {
        let last = lines.last().map_or(0, |l| l.no);
        return err(last, "missing `}`");
    }
    Ok(())
}

fn parse_stmt(
    b: &mut Builder,
    lines: &[Line],
    pos: &mut usize,
    code: usize,
    line: &Line,
) -> Result<(), ParseError> {
    let mut c = Cursor {
        line: line.no,
        toks: &line.toks,
        pos: 0,
    };
    let mut conn = if b.in_tx { Conn::Tx } else { Conn::Pool };
    if c.eat_kw("pool") {
        conn = Conn::Pool;
    } else if c.eat_kw("tx") {
        conn = Conn::Tx;
    } else if c.eat_kw("replica") {
        // Phase 13: a read routed to the read replica.
        conn = Conn::Replica;
    }
    let head = c.ident("a statement")?;
    match head.as_str() {
        "begin" => {
            let mut isolation = None;
            let mut retry = 0;
            while !c.at_end() {
                if c.eat_kw("isolation") {
                    let mut name = c.ident("an isolation level")?;
                    while c.eat_sym("-") {
                        name.push('-');
                        name.push_str(&c.ident("an isolation level")?);
                    }
                    isolation = Some(Isolation::parse(&name).ok_or_else(|| ParseError {
                        line: line.no,
                        message: format!("unknown isolation level `{name}`"),
                    })?);
                } else if c.eat_kw("retry") {
                    match c.next() {
                        Some(Tok::Int(n)) if (0..=16).contains(&n) => retry = n as u32,
                        _ => return err(line.no, "`retry` needs a number between 0 and 16"),
                    }
                } else {
                    return err(line.no, "expected `isolation` or `retry`");
                }
            }
            b.in_tx = true;
            push(b, code, line, Op::Begin { isolation, retry });
        }
        "commit" => {
            b.in_tx = false;
            push(b, code, line, Op::Commit);
        }
        "rollback" => {
            b.in_tx = false;
            push(b, code, line, Op::Rollback);
        }
        "let" => {
            let name = c.ident("a variable name")?;
            c.expect_sym("=")?;
            // Phase 13: `let x = replica select ...` / `replica count ...`.
            if c.is_kw("replica") {
                c.eat_kw("replica");
                conn = Conn::Replica;
            }
            if c.eat_kw("select") {
                // Phase 16: `select all t where ... [limit n] [for ...]`, the
                // multi-row mode (a list of rows).
                let all = c.eat_kw("all");
                let table = c.ident("a table")?;
                let mut joins = Vec::new();
                loop {
                    // `left join`, `inner join` or plain `join` (inner).
                    let kind = if c.eat_kw("left") {
                        crate::ir::JoinKind::Left
                    } else if c.eat_kw("inner") || c.is_kw("join") {
                        crate::ir::JoinKind::Inner
                    } else {
                        break;
                    };
                    c.expect_kw("join")?;
                    let joined = c.ident("a table")?;
                    c.expect_kw("on")?;
                    let (Some(Tok::Col(left)), Some(Tok::Sym("=="))) = (c.next(), c.next()) else {
                        return err(c.line, "a join condition is `@t.a == @u.b`");
                    };
                    let Some(Tok::Col(right)) = c.next() else {
                        return err(c.line, "a join condition is `@t.a == @u.b`");
                    };
                    joins.push(crate::ir::Join {
                        kind,
                        table: joined,
                        left,
                        right,
                    });
                }
                c.expect_kw("where")?;
                let pred = c.expr()?;
                let mut many = all.then(crate::ir::ManyRows::default);
                if let Some(many) = many.as_mut() {
                    if c.eat_kw("order") {
                        c.expect_kw("by")?;
                        let Some(Tok::Col(column)) = c.next() else {
                            return err(c.line, "`order by` expects a column `@name`");
                        };
                        many.order_by = Some(column);
                    }
                    if c.eat_kw("limit") {
                        match c.next() {
                            Some(Tok::Int(n)) if (0..=i64::from(u32::MAX)).contains(&n) => {
                                many.limit = Some(n as u32);
                            }
                            _ => return err(c.line, "`limit` expects a non-negative integer"),
                        }
                    }
                }
                let lock = parse_lock(&mut c)?;
                done(&c)?;
                if !joins.is_empty() && lock != LockMode::None {
                    return err(c.line, "row locks on joins are not modeled");
                }
                if !joins.is_empty() && all {
                    return err(c.line, "a multi-row select with joins is not modeled");
                }
                let kind = StmtKind::Select {
                    table,
                    pred,
                    lock,
                    into: name,
                    joins,
                    projection: None,
                    many,
                };
                push(
                    b,
                    code,
                    line,
                    Op::Db {
                        stmt: Stmt { conn, kind },
                    },
                );
            } else if c.eat_kw("count") {
                let table = c.ident("a table")?;
                c.expect_kw("where")?;
                let pred = c.expr()?;
                done(&c)?;
                let kind = StmtKind::SelectCount {
                    table,
                    pred,
                    into: name,
                };
                push(
                    b,
                    code,
                    line,
                    Op::Db {
                        stmt: Stmt { conn, kind },
                    },
                );
            } else if c.is_kw("generate")
                && matches!(c.toks.get(c.pos + 1), Some(Tok::Str(_)))
                && c.toks.len() == c.pos + 2
            {
                // Phase 16: `let t = generate "nanoid(21)"`: a fresh value
                // from a package call (a generation event of the actor).
                c.eat_kw("generate");
                let Some(Tok::Str(source)) = c.next() else {
                    return err(c.line, "`generate` needs the call's text as a string");
                };
                push(
                    b,
                    code,
                    line,
                    Op::Generate {
                        local: name,
                        source,
                    },
                );
            } else {
                let expr = c.expr()?;
                done(&c)?;
                push(b, code, line, Op::Let { local: name, expr });
            }
        }
        "update" => {
            let table = c.ident("a table")?;
            c.expect_kw("where")?;
            let pred = c.expr_until(&["set"])?;
            c.expect_kw("set")?;
            let set = c.assigns_until_into(true)?;
            let affected = parse_into(&mut c)?;
            done(&c)?;
            let kind = StmtKind::Update {
                table,
                pred,
                set,
                affected,
            };
            push(
                b,
                code,
                line,
                Op::Db {
                    stmt: Stmt { conn, kind },
                },
            );
        }
        "insert" => {
            let table = c.ident("a table")?;
            let values = if c.eat_kw("default") {
                Vec::new()
            } else {
                c.expect_kw("set")?;
                c.assigns_until_into(true)?
            };
            let mut on_conflict = OnConflict::Error;
            let mut upsert = None;
            if c.eat_kw("on") {
                c.expect_kw("conflict")?;
                // Phase 13: `on conflict (@a, @b) do update set @c = e,
                // ... [where e]`; `excluded.c` is the proposed row's column.
                let mut target = Vec::new();
                if c.eat_sym("(") {
                    loop {
                        match c.next() {
                            Some(Tok::Col(col)) => target.push(col),
                            _ => return err(line.no, "expected `@column` in the conflict target"),
                        }
                        if !c.eat_sym(",") {
                            break;
                        }
                    }
                    c.expect_sym(")")?;
                }
                c.expect_kw("do")?;
                if c.eat_kw("update") {
                    if target.is_empty() {
                        return err(
                            line.no,
                            "`do update` needs a conflict target `(@column, ...)`",
                        );
                    }
                    c.expect_kw("set")?;
                    let set: Vec<(String, Expr)> = c
                        .assigns(true)?
                        .into_iter()
                        .map(|(col, e)| (col, excluded_columns(e)))
                        .collect();
                    let filter = if c.eat_kw("where") {
                        Some(excluded_columns(c.expr()?))
                    } else {
                        None
                    };
                    upsert = Some(crate::ir::Upsert {
                        target,
                        set,
                        filter,
                    });
                } else {
                    c.expect_kw("nothing")?;
                    on_conflict = OnConflict::DoNothing;
                }
            }
            let affected = parse_into(&mut c)?;
            let returning = if c.eat_kw("returning") {
                Some(c.ident("a local")?)
            } else {
                None
            };
            done(&c)?;
            let kind = StmtKind::Insert {
                table,
                values,
                on_conflict,
                affected,
                returning,
                returning_columns: None,
                upsert,
                more_rows: Vec::new(),
            };
            push(
                b,
                code,
                line,
                Op::Db {
                    stmt: Stmt { conn, kind },
                },
            );
        }
        "delete" => {
            let table = c.ident("a table")?;
            c.expect_kw("where")?;
            let pred = c.expr_until(&["into"])?;
            let affected = parse_into(&mut c)?;
            done(&c)?;
            let kind = StmtKind::Delete {
                table,
                pred,
                affected,
            };
            push(
                b,
                code,
                line,
                Op::Db {
                    stmt: Stmt { conn, kind },
                },
            );
        }
        "if" => {
            let cond = c.expr_until_brace()?;
            c.expect_sym("{")?;
            done(&c)?;
            let jump_else = push(
                b,
                code,
                line,
                Op::JumpIf {
                    cond: Expr::not(cond),
                    target: usize::MAX,
                },
            );
            let tx_before = b.in_tx;
            parse_block(b, lines, pos, code, true)?;
            // closing line: `}` or `} else {`
            let close = &lines[*pos];
            *pos += 1;
            let mut cc = Cursor {
                line: close.no,
                toks: &close.toks,
                pos: 1,
            };
            if cc.eat_kw("else") {
                cc.expect_sym("{")?;
                done(&cc)?;
                let jump_end = push(b, code, close, Op::Jump { target: usize::MAX });
                let else_start = b.codes[code].len();
                patch(&mut b.codes[code][jump_else], else_start);
                let tx_then = b.in_tx;
                b.in_tx = tx_before;
                parse_block(b, lines, pos, code, true)?;
                *pos += 1; // closing `}`
                let end = b.codes[code].len();
                patch(&mut b.codes[code][jump_end], end);
                b.in_tx = tx_then;
            } else {
                done(&cc)?;
                let end = b.codes[code].len();
                patch(&mut b.codes[code][jump_else], end);
            }
        }
        "fail" => {
            let message = match c.next() {
                Some(Tok::Str(s)) => s,
                _ => return err(line.no, "`fail` needs a quoted message"),
            };
            done(&c)?;
            push(b, code, line, Op::Throw { message });
        }
        "return" => {
            // Phase 13: `return <expr>` states the value the task returns.
            if !c.at_end() {
                let expr = c.expr_until(&[])?;
                done(&c)?;
                push(b, code, line, Op::ReturnValue { expr });
            } else {
                done(&c)?;
            }
            push(b, code, line, Op::Return);
        }
        "external" => {
            let channel = c.ident("a channel name")?;
            let mut key = None;
            if c.eat_kw("key") {
                key = Some(c.expr_until(&["set"])?);
            }
            let mut payload = Vec::new();
            if c.eat_kw("set") {
                payload = c.assigns(false)?;
            }
            done(&c)?;
            push(
                b,
                code,
                line,
                Op::External {
                    channel,
                    payload,
                    key,
                },
            );
        }
        "read_app" => {
            let var = c.ident("a variable")?;
            c.expect_kw("into")?;
            let into = c.ident("a local")?;
            done(&c)?;
            push(b, code, line, Op::AppRead { var, into });
        }
        "write_app" => {
            let var = c.ident("a variable")?;
            c.expect_sym("=")?;
            let expr = c.expr()?;
            done(&c)?;
            push(b, code, line, Op::AppWrite { var, expr });
        }
        "spawn" => {
            let detached = c.eat_kw("detached");
            c.expect_sym("{")?;
            done(&c)?;
            let mut children = Vec::new();
            loop {
                let child = b.codes.len();
                b.codes.push(Vec::new());
                children.push(child);
                let tx_before = b.in_tx;
                parse_block(b, lines, pos, child, true)?;
                b.in_tx = tx_before;
                b.codes[child].push(Step {
                    op: Op::Return,
                    src: SourceRef::declared(0, "(end of task)"),
                });
                let close = &lines[*pos];
                *pos += 1;
                let mut cc = Cursor {
                    line: close.no,
                    toks: &close.toks,
                    pos: 1,
                };
                if cc.eat_sym("{") {
                    done(&cc)?;
                    continue;
                }
                done(&cc)?;
                break;
            }
            push(b, code, line, Op::Spawn { children, detached });
        }
        "join" => {
            done(&c)?;
            push(b, code, line, Op::Join);
        }
        "unknown" => {
            let detail = match c.next() {
                Some(Tok::Str(s)) => s,
                _ => return err(line.no, "`unknown` needs a quoted description"),
            };
            done(&c)?;
            push(
                b,
                code,
                line,
                Op::Unknown {
                    detail,
                    defines: Vec::new(),
                },
            );
        }
        other => return err(line.no, format!("unknown statement `{other}`")),
    }
    Ok(())
}

fn patch(step: &mut Step, target: usize) {
    match &mut step.op {
        Op::JumpIf { target: t, .. } | Op::Jump { target: t } => *t = target,
        _ => unreachable!("patching a non-jump"),
    }
}

fn done(c: &Cursor<'_>) -> Result<(), ParseError> {
    if c.at_end() {
        Ok(())
    } else {
        err(c.line, "unexpected tokens at the end of the statement")
    }
}

/// `excluded.c` (a property of the local `excluded`) as the column
/// `excluded.c` of an upsert's evaluation row.
fn excluded_columns(e: Expr) -> Expr {
    match e {
        Expr::Field(base, c) if matches!(&*base, Expr::Local(l) if l == "excluded") => {
            Expr::Col(format!("excluded.{c}"))
        }
        Expr::Field(b, c) => Expr::Field(Box::new(excluded_columns(*b)), c),
        Expr::Not(x) => Expr::Not(Box::new(excluded_columns(*x))),
        Expr::IsNull(x) => Expr::IsNull(Box::new(excluded_columns(*x))),
        Expr::Bin(op, a, b) => Expr::Bin(
            op,
            Box::new(excluded_columns(*a)),
            Box::new(excluded_columns(*b)),
        ),
        Expr::Object(f) => Expr::Object(
            f.into_iter()
                .map(|(k, v)| (k, excluded_columns(v)))
                .collect(),
        ),
        other => other,
    }
}

fn parse_lock(c: &mut Cursor<'_>) -> Result<LockMode, ParseError> {
    if !c.eat_kw("for") {
        return Ok(LockMode::None);
    }
    // Phase 13: `for share`, `for key share`, `for no key update`.
    if c.eat_kw("share") {
        return Ok(LockMode::ForShare);
    }
    if c.eat_kw("key") {
        c.expect_kw("share")?;
        return Ok(LockMode::ForKeyShare);
    }
    if c.eat_kw("no") {
        c.expect_kw("key")?;
        c.expect_kw("update")?;
        return Ok(LockMode::ForNoKeyUpdate);
    }
    c.expect_kw("update")?;
    if c.eat_kw("skip") {
        c.expect_kw("locked")?;
        Ok(LockMode::SkipLocked)
    } else if c.eat_kw("nowait") {
        Ok(LockMode::NoWait)
    } else {
        Ok(LockMode::ForUpdate)
    }
}

fn parse_into(c: &mut Cursor<'_>) -> Result<Option<String>, ParseError> {
    if c.eat_kw("into") {
        Ok(Some(c.ident("a local")?))
    } else {
        Ok(None)
    }
}

impl Cursor<'_> {
    /// Parse an expression that must stop before one of the keywords.
    /// (Expressions never contain these keywords, so a plain parse stops
    /// there; the argument documents the contract and checks the stop.)
    fn expr_until(&mut self, stops: &[&str]) -> Result<Expr, ParseError> {
        let e = self.expr()?;
        if let Some(Tok::Ident(s)) = self.peek()
            && !stops.contains(&s.as_str())
        {
            return err(self.line, format!("unexpected `{s}`"));
        }
        Ok(e)
    }
    fn expr_until_brace(&mut self) -> Result<Expr, ParseError> {
        self.expr()
    }
    fn assigns_until_into(&mut self, columns: bool) -> Result<Vec<(String, Expr)>, ParseError> {
        self.assigns(columns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_transaction_with_a_branch() {
        let codes = parse(
            "begin isolation serializable\n\
             let p = select products where @id == id for update\n\
             if p == null || p.stock <= 0 {\n\
               fail \"out of stock\"\n\
             }\n\
             update products where @id == id set @stock = @stock - 1 into n\n\
             commit\n",
        )
        .unwrap();
        let main = &codes[0];
        assert!(matches!(main[0].op, Op::Begin { retry: 0, .. }));
        let Op::Db { stmt } = &main[1].op else {
            panic!("expected select")
        };
        assert_eq!(stmt.conn, Conn::Tx);
        assert!(matches!(
            &stmt.kind,
            StmtKind::Select {
                lock: LockMode::ForUpdate,
                ..
            }
        ));
        // JumpIf skips the throw.
        let Op::JumpIf { target, .. } = &main[2].op else {
            panic!("expected jump")
        };
        assert!(matches!(main[*target].op, Op::Db { .. }));
        assert!(matches!(main.last().unwrap().op, Op::Return));
    }

    #[test]
    fn statements_outside_a_transaction_are_autocommit() {
        let codes = parse("let c = count t where @a == 1\n").unwrap();
        let Op::Db { stmt } = &codes[0][0].op else {
            panic!()
        };
        assert_eq!(stmt.conn, Conn::Pool);
    }

    #[test]
    fn spawn_creates_child_codes() {
        let codes =
            parse("spawn {\n  insert t set @a = 1\n} {\n  insert t set @a = 2\n}\njoin\n").unwrap();
        assert_eq!(codes.len(), 3);
        assert!(
            matches!(&codes[0][0].op, Op::Spawn { children, detached: false } if children == &[1, 2])
        );
        assert!(matches!(codes[0][1].op, Op::Join));
    }

    #[test]
    fn errors_name_the_line() {
        let e = parse("let x = 1\nfrobnicate\n").unwrap_err();
        assert_eq!(e.line, 2);
        assert!(parse("if true {\nreturn\n").is_err());
        assert!(parse("update t where @a = 1 set stock = 1\n").is_err());
    }
}
