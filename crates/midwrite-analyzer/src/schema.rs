//! Static extraction of PostgreSQL schema facts from Drizzle definitions
//! (Milestone 3).
//!
//! Reads `pgTable(...)`, `pgEnum(...)` and `pgSchema(...).table(...)`
//! declarations of a module **without executing it**: only literal names,
//! column builders with literal arguments, and the documented modifiers and
//! constraint builders are understood. Everything else is kept as an
//! *unsupported* fact of the table (never generalized): a partial unique
//! index stays a partial index, a default computed by a function stays
//! unknown, a column whose name depends on a casing option is reported.
//!
//! Every fact carries its provenance (file, line, the binding it was read
//! from) so that a scenario, a report or the catalog comparison can say
//! where it came from: `extracted from source` is kept apart from facts a
//! scenario declares and from what a database catalog shows.

use std::collections::BTreeMap;
use std::path::Path;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, Declaration, Expression, ObjectPropertyKind,
    Program, Statement, VariableDeclaration,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType};

pub use midwrite_model::schema_facts::{
    ExtractedSchema, Generation, GeneratorClass, SchemaColumn, SchemaDefault, SchemaForeignKey,
    SchemaLiteral, SchemaTable, SchemaUnique,
};

use crate::generators::Recognizer;

use crate::semantic::{member_path, property_key, string_value, unwind};

/// Read the schema declarations of `file` (relative to `root`).
pub fn extract_schema(root: &Path, file: &str) -> Result<ExtractedSchema, String> {
    let path = root.join(file);
    let source =
        std::fs::read_to_string(&path).map_err(|e| format!("cannot read `{file}`: {e}"))?;
    extract_schema_source(file, &source)
}

/// [`extract_schema`] for source text.
pub fn extract_schema_source(file: &str, source: &str) -> Result<ExtractedSchema, String> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.fatal_error || parsed.diagnostics.errors().next().is_some() {
        return Err(format!("`{file}` does not parse"));
    }
    let lines = LineMap::new(source);
    let recognizer = Recognizer::new(&parsed.program);
    let mut x = Extractor {
        file,
        lines: &lines,
        out: ExtractedSchema {
            imports: recognizer.imports.clone(),
            ..ExtractedSchema::default()
        },
        schemas: BTreeMap::new(),
        recognizer: &recognizer,
    };
    x.program(&parsed.program);
    Ok(x.out)
}

struct LineMap {
    starts: Vec<u32>,
}

impl LineMap {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                starts.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
            }
        }
        Self { starts }
    }

    fn line(&self, offset: u32) -> u32 {
        match self.starts.binary_search(&offset) {
            Ok(i) => i as u32 + 1,
            Err(i) => i as u32,
        }
    }
}

struct Extractor<'s, 'r, 'a> {
    file: &'s str,
    lines: &'s LineMap,
    out: ExtractedSchema,
    /// `const auth = pgSchema("auth")` bindings → schema name.
    schemas: BTreeMap<String, String>,
    /// Recognizes default generators against the module's imports.
    recognizer: &'r Recognizer<'r, 'a>,
}

fn generation(class: GeneratorClass, generator: Option<&str>, detail: &str) -> Generation {
    Generation {
        class,
        generator: generator.map(str::to_owned),
        detail: detail.to_owned(),
        callback_import: None,
    }
}

/// `sql\`gen_random_uuid()\`` with `sql` imported from `drizzle-orm`.
fn sql_generator(recognizer: &Recognizer<'_, '_>, e: &Expression<'_>) -> Option<&'static str> {
    let Expression::TaggedTemplateExpression(t) = e else {
        return None;
    };
    let Expression::Identifier(tag) = t.tag.get_inner_expression() else {
        return None;
    };
    let from_drizzle = recognizer
        .imports
        .get(tag.name.as_str())
        .is_some_and(|(specifier, imported)| specifier == "drizzle-orm" && imported == "sql");
    if !from_drizzle || !t.quasi.expressions.is_empty() {
        return None;
    }
    let text: String = t
        .quasi
        .quasis
        .iter()
        .map(|q| q.value.raw.as_str())
        .collect();
    (text.trim().eq_ignore_ascii_case("gen_random_uuid()")).then_some("gen_random_uuid()")
}

fn string_arg(arguments: &[Argument<'_>], index: usize) -> Option<String> {
    arguments.get(index)?.as_expression().and_then(string_value)
}

fn source_text<'a>(source: &'a str, expression: &Expression<'_>) -> &'a str {
    let span = expression.span();
    source
        .get(span.start as usize..span.end as usize)
        .unwrap_or("<expression>")
}

impl Extractor<'_, '_, '_> {
    fn line(&self, offset: u32) -> u32 {
        self.lines.line(offset)
    }

    fn program(&mut self, program: &Program<'_>) {
        let source = program.source_text;
        // Enums and schemas first: tables refer to them by binding.
        for pass in 0..2 {
            for statement in &program.body {
                let declaration: Option<&VariableDeclaration<'_>> = match statement {
                    Statement::VariableDeclaration(d) => Some(d),
                    Statement::ExportDeclaration(e) => match &e.declaration {
                        Declaration::VariableDeclaration(d) => Some(d),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(d) = declaration {
                    self.declaration(source, d, pass);
                }
            }
        }
    }

    fn declaration(&mut self, source: &str, d: &VariableDeclaration<'_>, pass: u8) {
        for declarator in &d.declarations {
            let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                continue;
            };
            let Some(init) = &declarator.init else {
                continue;
            };
            let binding = id.name.as_str().to_owned();
            let chain = unwind(init);
            let Some(root) = chain.root_name() else {
                continue;
            };
            let calls = chain.method_calls();
            let line = self.line(init.span().start);
            match (pass, root, calls.as_deref()) {
                // pgEnum("role", ["a", "b"])
                (0, "pgEnum", Some([])) => {
                    if let Some(call) = chain.root_call() {
                        self.enumeration(&binding, call.arguments.as_slice(), line);
                    }
                }
                // pgSchema("auth")
                (0, "pgSchema", Some([])) => {
                    if let Some(name) = chain
                        .root_call()
                        .and_then(|c| string_arg(c.arguments.as_slice(), 0))
                    {
                        self.schemas.insert(binding, name);
                    }
                }
                // pgTable("t", {...}, (t) => [...])
                (1, "pgTable", Some([])) => {
                    if let Some(call) = chain.root_call() {
                        self.table(source, &binding, None, call.arguments.as_slice(), line);
                    }
                }
                // auth.table("t", {...})
                (1, schema, Some([only])) if only.method == "table" && only.path.is_empty() => {
                    if let Some(name) = self.schemas.get(schema).cloned() {
                        self.table(source, &binding, Some(name), only.arguments, line);
                    }
                }
                (1, "pgTable", _) => self.out.unsupported.push(format!(
                    "{}:{line}: `{binding}`: `pgTable(...)` followed by method calls is not understood",
                    self.file
                )),
                _ => {}
            }
        }
    }

    fn enumeration(&mut self, binding: &str, arguments: &[Argument<'_>], line: u32) {
        let Some(name) = string_arg(arguments, 0) else {
            self.out.unsupported.push(format!(
                "{}:{line}: `{binding}`: enum name is not a literal",
                self.file
            ));
            return;
        };
        let values = match arguments.get(1).and_then(Argument::as_expression) {
            Some(Expression::ArrayExpression(array)) => {
                let mut values = Vec::new();
                for element in &array.elements {
                    match element.as_expression().and_then(string_value) {
                        Some(value) => values.push(value),
                        None => {
                            self.out.unsupported.push(format!(
                                "{}:{line}: `{binding}`: an enum value is not a string literal",
                                self.file
                            ));
                            return;
                        }
                    }
                }
                values
            }
            _ => {
                self.out.unsupported.push(format!(
                    "{}:{line}: `{binding}`: enum values are not an array literal",
                    self.file
                ));
                return;
            }
        };
        self.out.enums.insert(binding.to_owned(), (name, values));
    }

    fn table(
        &mut self,
        source: &str,
        binding: &str,
        schema: Option<String>,
        arguments: &[Argument<'_>],
        line: u32,
    ) {
        let Some(name) = string_arg(arguments, 0) else {
            self.out.unsupported.push(format!(
                "{}:{line}: `{binding}`: the table name is not a literal (dynamic schema declarations are not extracted)",
                self.file
            ));
            return;
        };
        let mut table = SchemaTable {
            binding: binding.to_owned(),
            name,
            schema,
            columns: Vec::new(),
            primary_key: Vec::new(),
            unique: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            checks: Vec::new(),
            unsupported: Vec::new(),
            file: self.file.to_owned(),
            line,
        };
        match arguments
            .get(1)
            .and_then(Argument::as_expression)
            .map(Expression::get_inner_expression)
        {
            Some(Expression::ObjectExpression(object)) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::ObjectProperty(p) if !p.computed => {
                            match property_key(&p.key) {
                                Some(key) => self.column(source, &mut table, &key, &p.value),
                                None => table.unsupported.push(format!(
                                    "line {}: a column key is not static",
                                    self.line(p.span.start)
                                )),
                            }
                        }
                        other => table.unsupported.push(format!(
                            "line {}: spread or computed column (`{}`) is not extracted",
                            self.line(other.span().start),
                            source
                                .get(other.span().start as usize..other.span().end as usize)
                                .unwrap_or("?")
                        )),
                    }
                }
            }
            _ => {
                table
                    .unsupported
                    .push("the columns are not an object literal".to_owned());
            }
        }
        if let Some(extra) = arguments.get(2).and_then(Argument::as_expression) {
            self.extras(source, &mut table, extra);
        }
        // Column-level primary keys.
        if table.primary_key.is_empty() {
            table.primary_key = table
                .columns
                .iter()
                .filter(|c| c.primary_key)
                .map(|c| c.sql_name.clone())
                .collect();
        }
        self.out.tables.push(table);
    }

    fn column(
        &mut self,
        source: &str,
        table: &mut SchemaTable,
        property: &str,
        value: &Expression<'_>,
    ) {
        let line = self.line(value.span().start);
        let chain = unwind(value);
        let (Some(builder), Some(steps)) = (chain.root_name(), chain.method_calls()) else {
            table.unsupported.push(format!(
                "line {line}: column `{property}` is not a column builder call"
            ));
            return;
        };
        let Some(root_call) = chain.root_call() else {
            table.unsupported.push(format!(
                "line {line}: column `{property}`: `{builder}` is not called"
            ));
            return;
        };
        let root_args = root_call.arguments.as_slice();
        let sql_name = match string_arg(root_args, 0) {
            Some(name) => name,
            None if root_args.is_empty()
                || matches!(
                    root_args
                        .first()
                        .and_then(Argument::as_expression)
                        .map(Expression::get_inner_expression),
                    Some(Expression::ObjectExpression(_))
                ) =>
            {
                // Drizzle uses the property name unless the database was
                // created with a casing option, which this extractor cannot
                // see: recorded.
                table.unsupported.push(format!(
                    "line {line}: column `{property}` has no explicit SQL name: assumed `{property}` (a `casing` option of drizzle() would change it)"
                ));
                property.to_owned()
            }
            None => {
                table.unsupported.push(format!(
                    "line {line}: column `{property}`: the SQL name is not a literal"
                ));
                return;
            }
        };
        let options = |key: &str| -> Option<&Expression<'_>> {
            root_args
                .iter()
                .filter_map(Argument::as_expression)
                .find_map(|e| match e.get_inner_expression() {
                    Expression::ObjectExpression(o) => o.properties.iter().find_map(|p| match p {
                        ObjectPropertyKind::ObjectProperty(p)
                            if property_key(&p.key).as_deref() == Some(key) =>
                        {
                            Some(&p.value)
                        }
                        _ => None,
                    }),
                    _ => None,
                })
        };
        let mut enum_values = Vec::new();
        let sql_type = match builder {
            "serial" | "bigserial" | "smallserial" => builder.to_owned(),
            "integer" | "smallint" | "text" | "boolean" | "date" | "real" | "uuid" | "json"
            | "jsonb" | "doublePrecision" | "time" | "interval" | "inet" | "cidr" => {
                match builder {
                    "doublePrecision" => "double precision".to_owned(),
                    other => other.to_owned(),
                }
            }
            "bigint" => "bigint".to_owned(),
            "timestamp" => match options("withTimezone").map(Expression::get_inner_expression) {
                Some(Expression::BooleanLiteral(b)) if b.value => {
                    "timestamp with time zone".to_owned()
                }
                _ => "timestamp".to_owned(),
            },
            "varchar" | "char" => match options("length").map(Expression::get_inner_expression) {
                Some(Expression::NumericLiteral(n)) => format!("{builder}({})", n.value),
                _ => builder.to_owned(),
            },
            "numeric" | "decimal" => "numeric".to_owned(),
            other => match self.out.enums.get(other) {
                Some((name, values)) => {
                    enum_values.clone_from(values);
                    format!("enum {name}")
                }
                None => {
                    table.unsupported.push(format!(
                        "line {line}: column `{property}`: builder `{other}` is not a known Drizzle column type"
                    ));
                    format!("unknown {other}")
                }
            },
        };
        let mut column = SchemaColumn {
            property: property.to_owned(),
            sql_name: sql_name.clone(),
            builder: builder.to_owned(),
            sql_type,
            not_null: false,
            primary_key: false,
            default: matches!(builder, "serial" | "bigserial" | "smallserial").then(|| {
                SchemaDefault::Sequence {
                    generated: builder.to_owned(),
                }
            }),
            enum_values,
            references: None,
            generation: None,
            // An explicit `mode` of a time or JSON column.
            mode: matches!(builder, "timestamp" | "date" | "json" | "jsonb")
                .then(|| options("mode").and_then(string_value))
                .flatten(),
            on_update: None,
            line,
        };
        if matches!(builder, "serial" | "bigserial" | "smallserial") {
            column.not_null = true;
        }
        // How the default is produced. Drizzle uses a `$defaultFn`
        // callback whenever the INSERT omits the column, whatever database
        // default the column also has, so an application default wins.
        let mut database_side: Option<Generation> =
            matches!(builder, "serial" | "bigserial" | "smallserial")
                .then(|| generation(GeneratorClass::SequenceGenerated, None, builder));
        let mut application_side: Option<Generation> = None;
        for step in steps {
            if !step.path.is_empty() {
                table.unsupported.push(format!(
                    "line {line}: column `{property}`: `{}.{}` is not a column modifier",
                    step.path.join("."),
                    step.method
                ));
                continue;
            }
            let args = step.arguments;
            match step.method {
                "notNull" => column.not_null = true,
                "primaryKey" => {
                    column.primary_key = true;
                    column.not_null = true;
                }
                "unique" => {
                    let nulls = args.iter().filter_map(Argument::as_expression).any(|e| {
                        matches!(e.get_inner_expression(), Expression::ObjectExpression(o) if o.properties.iter().any(|p| matches!(p,
                            ObjectPropertyKind::ObjectProperty(p) if property_key(&p.key).as_deref() == Some("nulls")
                                && string_value(&p.value).as_deref() == Some("not distinct"))))
                    });
                    table.unique.push(SchemaUnique {
                        name: string_arg(args, 0),
                        columns: vec![sql_name.clone()],
                        kind: "constraint".to_owned(),
                        nulls_not_distinct: nulls,
                        predicate: None,
                        line,
                    });
                }
                "default" => {
                    let first = args
                        .first()
                        .and_then(Argument::as_expression)
                        .map(Expression::get_inner_expression);
                    database_side = Some(match first {
                        Some(
                            Expression::StringLiteral(_)
                            | Expression::NumericLiteral(_)
                            | Expression::BooleanLiteral(_)
                            | Expression::NullLiteral(_),
                        ) => generation(
                            GeneratorClass::ConstantDefault,
                            None,
                            first.map_or("", |e| source_text(source, e)),
                        ),
                        Some(e) => match sql_generator(self.recognizer, e) {
                            Some(g) => generation(
                                GeneratorClass::DatabaseGenerated,
                                Some(g),
                                source_text(source, e),
                            ),
                            None if matches!(e, Expression::UnaryExpression(_)) => generation(
                                GeneratorClass::ConstantDefault,
                                None,
                                source_text(source, e),
                            ),
                            None => generation(
                                GeneratorClass::UnknownDefault,
                                None,
                                &format!(
                                    "`{}`: a database expression the model does not represent",
                                    source_text(source, e)
                                ),
                            ),
                        },
                        None => generation(GeneratorClass::UnknownDefault, None, "<missing>"),
                    });
                    column.default = Some(
                        match args
                            .first()
                            .and_then(Argument::as_expression)
                            .map(Expression::get_inner_expression)
                        {
                            Some(e @ Expression::StringLiteral(_)) => match string_value(e) {
                                Some(text) => SchemaDefault::Constant {
                                    value: SchemaLiteral::Text(text),
                                },
                                None => SchemaDefault::Expression {
                                    text: source_text(source, e).to_owned(),
                                },
                            },
                            Some(Expression::NumericLiteral(n))
                                if n.value.fract() == 0.0 && n.value.abs() < 9.0e15 =>
                            {
                                SchemaDefault::Constant {
                                    value: SchemaLiteral::Integer(n.value as i64),
                                }
                            }
                            Some(Expression::UnaryExpression(u))
                                if u.operator.as_str() == "-"
                                    && matches!(u.argument.get_inner_expression(), Expression::NumericLiteral(n) if n.value.fract() == 0.0) =>
                            {
                                let Expression::NumericLiteral(n) =
                                    u.argument.get_inner_expression()
                                else {
                                    unreachable!()
                                };
                                SchemaDefault::Constant {
                                    value: SchemaLiteral::Integer(-(n.value as i64)),
                                }
                            }
                            Some(Expression::BooleanLiteral(b)) => SchemaDefault::Constant {
                                value: SchemaLiteral::Boolean(b.value),
                            },
                            Some(Expression::NullLiteral(_)) => SchemaDefault::Constant {
                                value: SchemaLiteral::Null,
                            },
                            Some(other) => SchemaDefault::Expression {
                                text: source_text(source, other).to_owned(),
                            },
                            None => SchemaDefault::Expression {
                                text: "<missing>".to_owned(),
                            },
                        },
                    );
                }
                "defaultNow" => {
                    database_side =
                        Some(generation(GeneratorClass::DatabaseGenerated, None, "now()"));
                    column.default = Some(SchemaDefault::Expression {
                        text: "now()".to_owned(),
                    });
                }
                "defaultRandom" => {
                    database_side = Some(generation(
                        GeneratorClass::DatabaseGenerated,
                        Some("gen_random_uuid()"),
                        "defaultRandom()",
                    ));
                    column.default = Some(SchemaDefault::Expression {
                        text: "gen_random_uuid()".to_owned(),
                    });
                }
                "$defaultFn" | "$default" => {
                    let written = format!(
                        ".{}({})",
                        step.method,
                        args.first()
                            .and_then(Argument::as_expression)
                            .map(|e| source_text(source, e))
                            .unwrap_or_default()
                    );
                    application_side = Some(
                        self.recognizer
                            .callback(args.first().and_then(Argument::as_expression), &written),
                    );
                    column.default = Some(SchemaDefault::Application {
                        text: args
                            .first()
                            .and_then(Argument::as_expression)
                            .map(|e| source_text(source, e).to_owned())
                            .unwrap_or_default(),
                    });
                }
                "generatedAlwaysAsIdentity" | "generatedByDefaultAsIdentity" => {
                    database_side = Some(generation(
                        GeneratorClass::IdentityGenerated,
                        None,
                        step.method,
                    ));
                    column.not_null = true;
                    column.default = Some(SchemaDefault::Sequence {
                        generated: step.method.to_owned(),
                    });
                }
                "references" => {
                    let target =
                        args.first()
                            .and_then(Argument::as_expression)
                            .and_then(|e| match e.get_inner_expression() {
                                Expression::ArrowFunctionExpression(arrow)
                                    if arrow.is_expression() =>
                                {
                                    arrow.get_expression().and_then(member_path)
                                }
                                _ => None,
                            });
                    // `{ onDelete: "cascade", onUpdate: ... }`.
                    let action = |key: &str| {
                        args.get(1)
                            .and_then(Argument::as_expression)
                            .and_then(|e| match e.get_inner_expression() {
                                Expression::ObjectExpression(o) => Some(o),
                                _ => None,
                            })
                            .and_then(|o| {
                                o.properties.iter().find_map(|p| match p {
                                    ObjectPropertyKind::ObjectProperty(p)
                                        if property_key(&p.key).as_deref() == Some(key) =>
                                    {
                                        match p.value.get_inner_expression() {
                                            Expression::StringLiteral(s) => {
                                                s.value.as_str().map(str::to_owned)
                                            }
                                            other => {
                                                Some(format!("<{}>", source_text(source, other)))
                                            }
                                        }
                                    }
                                    _ => None,
                                })
                            })
                    };
                    match target.as_deref().and_then(|t| t.split_once('.')) {
                        Some((t, c)) if !c.contains('.') => {
                            column.references = Some((t.to_owned(), c.to_owned()));
                            table.foreign_keys.push(SchemaForeignKey {
                                columns: vec![sql_name.clone()],
                                table: t.to_owned(),
                                foreign_columns: vec![c.to_owned()],
                                on_delete: action("onDelete"),
                                on_update: action("onUpdate"),
                                line,
                            });
                        }
                        _ => table.unsupported.push(format!(
                            "line {line}: column `{property}`: the referenced column is not `() => table.column`"
                        )),
                    }
                }
                "$onUpdate" | "$onUpdateFn" => {
                    // The callback, as written; the scenario
                    // loader models `() => new Date()` on a time column.
                    let text = args
                        .first()
                        .and_then(Argument::as_expression)
                        .map(|e| source_text(source, e).to_owned())
                        .unwrap_or_default();
                    let clock: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                    if !matches!(
                        clock.as_str(),
                        "()=>newDate()" | "()=>{returnnewDate();}" | "()=>{returnnewDate()}"
                    ) {
                        table.unsupported.push(format!(
                            "line {line}: column `{property}`: `.{}()` is not modeled",
                            step.method
                        ));
                    }
                    column.on_update = Some(text);
                }
                "$type" | "array" | "$inferSelect" => {
                    if step.method != "$type" {
                        table.unsupported.push(format!(
                            "line {line}: column `{property}`: `.{}()` is not modeled",
                            step.method
                        ));
                    }
                }
                other => table.unsupported.push(format!(
                    "line {line}: column `{property}`: modifier `.{other}()` is not understood"
                )),
            }
        }
        column.generation = application_side.or(database_side);
        table.columns.push(column);
    }

    /// The third argument: `(table) => [ ... ]` or `(table) => ({ ... })`.
    fn extras(&mut self, source: &str, table: &mut SchemaTable, extra: &Expression<'_>) {
        let line = self.line(extra.span().start);
        let Expression::ArrowFunctionExpression(arrow) = extra.get_inner_expression() else {
            table.unsupported.push(format!(
                "line {line}: the table's third argument is not an arrow function"
            ));
            return;
        };
        let parameter = arrow
            .params
            .items
            .first()
            .and_then(|p| match &p.pattern {
                BindingPattern::BindingIdentifier(id) => Some(id.name.as_str().to_owned()),
                _ => None,
            })
            .unwrap_or_default();
        let Some(body) = arrow.get_expression() else {
            table.unsupported.push(format!(
                "line {line}: the table's third argument has a block body"
            ));
            return;
        };
        let items: Vec<&Expression<'_>> = match body.get_inner_expression() {
            Expression::ArrayExpression(array) => array
                .elements
                .iter()
                .filter_map(ArrayExpressionElement::as_expression)
                .collect(),
            Expression::ObjectExpression(object) => object
                .properties
                .iter()
                .filter_map(|p| match p {
                    ObjectPropertyKind::ObjectProperty(p) => Some(&p.value),
                    ObjectPropertyKind::SpreadProperty(_) => None,
                })
                .collect(),
            _ => {
                table.unsupported.push(format!(
                    "line {line}: the table's constraints are not an array or object literal"
                ));
                return;
            }
        };
        for item in items {
            self.constraint(source, table, &parameter, item);
        }
    }

    /// A column reference `table.column` (by property) → SQL name.
    fn column_ref(table: &SchemaTable, parameter: &str, e: &Expression<'_>) -> Option<String> {
        let path = member_path(e)?;
        let (root, property) = path.split_once('.')?;
        if root != parameter || property.contains('.') {
            return None;
        }
        table.column(property).map(|c| c.sql_name.clone())
    }

    fn constraint(
        &mut self,
        source: &str,
        table: &mut SchemaTable,
        parameter: &str,
        item: &Expression<'_>,
    ) {
        let line = self.line(item.span().start);
        let text = source_text(source, item);
        let chain = unwind(item);
        let (Some(root), Some(steps)) = (chain.root_name(), chain.method_calls()) else {
            table
                .unsupported
                .push(format!("line {line}: `{text}` is not understood"));
            return;
        };
        let root_args = chain
            .root_call()
            .map(|c| c.arguments.as_slice())
            .unwrap_or_default();
        let columns_of = |args: &[Argument<'_>]| -> Option<Vec<String>> {
            args.iter()
                .map(|a| {
                    a.as_expression()
                        .and_then(|e| Self::column_ref(table, parameter, e))
                })
                .collect()
        };
        match root {
            "uniqueIndex" | "unique" | "index" => {
                let mut columns = None;
                let mut predicate = None;
                let mut nulls_not_distinct = false;
                let mut ok = true;
                for step in &steps {
                    match step.method {
                        "on" => columns = columns_of(step.arguments),
                        "where" => {
                            predicate = step
                                .arguments
                                .first()
                                .and_then(Argument::as_expression)
                                .map(|e| source_text(source, e).to_owned());
                        }
                        "nullsNotDistinct" if root == "unique" => nulls_not_distinct = true,
                        // Drizzle 0.45's index builders have no
                        // `nullsNotDistinct()` (a TypeError when the schema
                        // module loads); only `unique().on(...)` has it.
                        "nullsNotDistinct" => {
                            table.unsupported.push(format!(
                                "line {line}: `{text}`: `{root}().on(...)` has no `nullsNotDistinct()` in Drizzle (only `unique().on(...)` does)"
                            ));
                            return;
                        }
                        "using" | "concurrently" | "with" | "asc" | "desc" | "nullsFirst"
                        | "nullsLast" => {}
                        _ => ok = false,
                    }
                }
                let Some(columns) = columns.filter(|_| ok) else {
                    table.unsupported.push(format!(
                        "line {line}: `{text}`: columns are not plain `table.column` references"
                    ));
                    return;
                };
                if root == "index" {
                    table
                        .indexes
                        .push(string_arg(root_args, 0).unwrap_or_else(|| columns.join(",")));
                    return;
                }
                table.unique.push(SchemaUnique {
                    name: string_arg(root_args, 0),
                    columns,
                    kind: if root == "unique" {
                        "constraint"
                    } else {
                        "index"
                    }
                    .to_owned(),
                    nulls_not_distinct,
                    predicate,
                    line,
                });
            }
            "primaryKey" => {
                let columns = root_args
                    .first()
                    .and_then(Argument::as_expression)
                    .and_then(|e| {
                        match e.get_inner_expression() {
                            Expression::ObjectExpression(o) => {
                                o.properties.iter().find_map(|p| match p {
                                    ObjectPropertyKind::ObjectProperty(p)
                                        if property_key(&p.key).as_deref() == Some("columns") =>
                                    {
                                        match p.value.get_inner_expression() {
                                            Expression::ArrayExpression(a) => a
                                                .elements
                                                .iter()
                                                .map(|el| {
                                                    el.as_expression().and_then(|e| {
                                                        Self::column_ref(table, parameter, e)
                                                    })
                                                })
                                                .collect::<Option<Vec<_>>>(),
                                            _ => None,
                                        }
                                    }
                                    _ => None,
                                })
                            }
                            // Older form: primaryKey(table.a, table.b)
                            _ => columns_of(root_args),
                        }
                    });
                match columns {
                    Some(c) if !c.is_empty() => table.primary_key = c,
                    _ => table.unsupported.push(format!(
                        "line {line}: `{text}`: primary key columns not understood"
                    )),
                }
            }
            "foreignKey" => {
                let object = root_args
                    .first()
                    .and_then(Argument::as_expression)
                    .and_then(|e| match e.get_inner_expression() {
                        Expression::ObjectExpression(o) => Some(o),
                        _ => None,
                    });
                let field = |key: &str| -> Option<Vec<String>> {
                    object?.properties.iter().find_map(|p| match p {
                        ObjectPropertyKind::ObjectProperty(p)
                            if property_key(&p.key).as_deref() == Some(key) =>
                        {
                            match p.value.get_inner_expression() {
                                Expression::ArrayExpression(a) => a
                                    .elements
                                    .iter()
                                    .map(|el| el.as_expression().and_then(member_path))
                                    .collect(),
                                _ => None,
                            }
                        }
                        _ => None,
                    })
                };
                let local = field("columns").and_then(|paths| {
                    paths
                        .iter()
                        .map(|p| {
                            p.split_once('.').and_then(|(r, c)| {
                                (r == parameter)
                                    .then(|| table.column(c).map(|c| c.sql_name.clone()))
                                    .flatten()
                            })
                        })
                        .collect::<Option<Vec<_>>>()
                });
                let foreign = field("foreignColumns");
                match (local, foreign) {
                    (Some(columns), Some(foreign)) if !foreign.is_empty() => {
                        let target = foreign[0]
                            .split_once('.')
                            .map(|(t, _)| t.to_owned())
                            .unwrap_or_default();
                        // `.onDelete("...")` / `.onUpdate("...")`
                        // chained on the builder, read from its text.
                        let action = |method: &str| {
                            let at = text.find(&format!(".{method}("))?;
                            let rest = &text[at + method.len() + 2..];
                            let rest = rest.trim_start();
                            let quote = rest.chars().next()?;
                            if !matches!(quote, '"' | '\'' | '`') {
                                return Some(format!("<{}>", rest.split(')').next()?.trim()));
                            }
                            let body = &rest[1..];
                            Some(body[..body.find(quote)?].to_owned())
                        };
                        table.foreign_keys.push(SchemaForeignKey {
                            columns,
                            table: target,
                            foreign_columns: foreign
                                .iter()
                                .map(|f| {
                                    f.split_once('.')
                                        .map(|(_, c)| c.to_owned())
                                        .unwrap_or_default()
                                })
                                .collect(),
                            on_delete: action("onDelete"),
                            on_update: action("onUpdate"),
                            line,
                        });
                    }
                    _ => table
                        .unsupported
                        .push(format!("line {line}: `{text}`: foreign key not understood")),
                }
            }
            "check" => table.checks.push(text.to_owned()),
            _ => table.unsupported.push(format!(
                "line {line}: `{text}` is not a known constraint builder"
            )),
        }
    }
}
