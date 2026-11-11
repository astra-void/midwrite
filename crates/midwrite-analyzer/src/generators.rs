//! Recognition of column-default generators (Milestone 1).
//!
//! A Drizzle column can take its default from PostgreSQL (`defaultRandom()`,
//! `default(sql\`...\`)`) or from a callback Drizzle runs in the application
//! (`$defaultFn(...)`). Callbacks are **never executed**: a callback is
//! recognized only when it calls, with no other effect, one of a short list
//! of identifier generators whose binding is established by its import
//! (or, for `crypto`, by the absence of any local binding: the WebCrypto
//! global). A function merely *named* `uuid` or `generate` is not
//! recognized; neither is a reference that would lose its receiver
//! (`$defaultFn(crypto.randomUUID)` throws "Illegal invocation" for the
//! global). Everything else is reported as an unknown default with the
//! reason.

use std::collections::BTreeMap;

use oxc_ast::ast::{
    Argument, BindingPattern, Declaration, Expression, ImportDeclarationSpecifier, Program,
    Statement,
};

use midwrite_model::schema_facts::{Generation, GeneratorClass};

/// Imports of a module: local name → (specifier, imported name), with
/// `default` and `*` for default and namespace imports.
pub type Imports = BTreeMap<String, (String, String)>;

pub fn imports(program: &Program<'_>) -> Imports {
    let mut out = Imports::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        if import.import_kind.is_type() {
            continue;
        }
        let from = import.source.value.as_str().unwrap_or_default().to_owned();
        for specifier in import.specifiers.iter().flatten() {
            let (local, imported) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => {
                    if s.import_kind.is_type() {
                        continue;
                    }
                    (s.local.name.as_str(), s.imported.name().as_str().to_owned())
                }
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (s.local.name.as_str(), "default".to_owned())
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    (s.local.name.as_str(), "*".to_owned())
                }
            };
            out.insert(local.to_owned(), (from.clone(), imported));
        }
    }
    out
}

/// The generator a package export is, when called with no arguments.
fn package_generator(specifier: &str, imported: &str) -> Option<&'static str> {
    Some(match (specifier, imported) {
        ("crypto" | "node:crypto", "randomUUID") => "crypto.randomUUID()",
        ("@paralleldrive/cuid2", "createId") => "@paralleldrive/cuid2 createId()",
        ("nanoid", "nanoid") => "nanoid()",
        ("uuid", "v4") => "uuid v4()",
        ("uuid", "v7") => "uuid v7()",
        ("ulid" | "ulidx", "ulid") => "ulid()",
        ("uuidv7", "uuidv7") => "uuidv7()",
        _ => return None,
    })
}

fn is_crypto_module(specifier: &str) -> bool {
    matches!(specifier, "crypto" | "node:crypto")
}

/// Whether a specifier names a project module (relative, or a path alias
/// such as `@/lib/ids` or `~/ids`) rather than a package.
pub fn is_project_specifier(specifier: &str) -> bool {
    specifier.starts_with('.')
        || specifier.starts_with("@/")
        || specifier.starts_with("~/")
        || specifier.starts_with('#')
}

/// The recognizer for one module: its imports and module-level bindings
/// (a local binding named `crypto` shadows the global).
pub struct Recognizer<'p, 'a> {
    pub imports: Imports,
    locals: BTreeMap<String, &'p Expression<'a>>,
    functions: BTreeMap<String, &'p oxc_ast::ast::Function<'a>>,
    declared: Vec<String>,
}

impl<'p, 'a> Recognizer<'p, 'a> {
    pub fn new(program: &'p Program<'a>) -> Self {
        let mut locals = BTreeMap::new();
        let mut functions = BTreeMap::new();
        let mut declared = Vec::new();
        for statement in &program.body {
            let declaration = match statement {
                Statement::ExportDeclaration(e) => Some(&e.declaration),
                other => other.as_declaration(),
            };
            match declaration {
                Some(Declaration::VariableDeclaration(d)) => {
                    for declarator in &d.declarations {
                        if let BindingPattern::BindingIdentifier(id) = &declarator.id {
                            declared.push(id.name.as_str().to_owned());
                            if let Some(init) = &declarator.init
                                && d.kind.is_const()
                            {
                                locals.insert(id.name.as_str().to_owned(), init);
                            }
                        }
                    }
                }
                Some(Declaration::FunctionDeclaration(f)) => {
                    if let Some(id) = &f.id {
                        declared.push(id.name.as_str().to_owned());
                        let function: &'p oxc_ast::ast::Function<'a> = f;
                        functions.insert(id.name.as_str().to_owned(), function);
                    }
                }
                Some(Declaration::ClassDeclaration(c)) => {
                    if let Some(id) = &c.id {
                        declared.push(id.name.as_str().to_owned());
                    }
                }
                _ => {}
            }
        }
        Self {
            imports: imports(program),
            locals,
            functions,
            declared,
        }
    }

    fn bound(&self, name: &str) -> bool {
        self.imports.contains_key(name) || self.declared.iter().any(|d| d == name)
    }

    /// Classify the argument of `$defaultFn(...)` / `$default(...)`.
    pub fn callback(&self, argument: Option<&Expression<'_>>, written: &str) -> Generation {
        let unknown = |detail: String| Generation {
            class: GeneratorClass::UnknownDefault,
            generator: None,
            detail,
            callback_import: None,
        };
        let Some(argument) = argument else {
            return unknown("`$defaultFn()` without a callback".to_owned());
        };
        match self.callback_value(argument, 0) {
            Ok(generator) => Generation {
                class: GeneratorClass::ApplicationGenerated,
                generator: Some(generator.to_owned()),
                detail: written.to_owned(),
                callback_import: None,
            },
            Err(Pending::Import(specifier, name)) => Generation {
                class: GeneratorClass::UnknownDefault,
                generator: None,
                detail: format!(
                    "`{written}`: the callback `{name}` is defined in `{specifier}` (resolved when schema modules are composed)"
                ),
                callback_import: Some((specifier, name)),
            },
            Err(Pending::Unknown(reason)) => unknown(format!(
                "`{written}`: {reason}; the callback is not executed and its value is unknown"
            )),
        }
    }

    /// A callback value: a function whose body only returns a recognized
    /// generator call, or a reference to a recognized generator function.
    fn callback_value(&self, e: &Expression<'_>, depth: u8) -> Result<&'static str, Pending> {
        match e.get_inner_expression() {
            Expression::ArrowFunctionExpression(arrow) => {
                if !arrow.params.items.is_empty() || arrow.params.rest.is_some() {
                    return Err(Pending::unknown("the callback takes parameters"));
                }
                if arrow.r#async {
                    return Err(Pending::unknown(
                        "the callback is async (Drizzle would store a promise)",
                    ));
                }
                if let Some(body) = arrow.get_expression() {
                    return self.generator_call(body, depth);
                }
                match &arrow.body {
                    oxc_ast::ast::ArrowFunctionBody::FunctionBody(body) => {
                        self.single_return(&body.statements, depth)
                    }
                    _ => Err(Pending::unknown("the callback body is not understood")),
                }
            }
            Expression::FunctionExpression(f) => self.function(f, depth),
            Expression::Identifier(id) => self.function_reference(id.name.as_str(), depth),
            Expression::StaticMemberExpression(m) => Err(Pending::Unknown(format!(
                "`{}.{}` is passed without its receiver (called unbound)",
                crate::semantic::member_path(&m.object).unwrap_or_default(),
                m.property.name
            ))),
            other => Err(Pending::Unknown(format!(
                "the callback is not a function (kind: {})",
                crate::concurrency_kind(other)
            ))),
        }
    }

    fn function(&self, f: &oxc_ast::ast::Function<'_>, depth: u8) -> Result<&'static str, Pending> {
        if !f.params.items.is_empty() || f.params.rest.is_some() {
            return Err(Pending::unknown("the callback takes parameters"));
        }
        if f.r#async || f.generator {
            return Err(Pending::unknown("the callback is async or a generator"));
        }
        let Some(body) = &f.body else {
            return Err(Pending::unknown("the callback has no body"));
        };
        self.single_return(&body.statements, depth)
    }

    fn single_return(
        &self,
        statements: &[Statement<'_>],
        depth: u8,
    ) -> Result<&'static str, Pending> {
        match statements {
            [Statement::ReturnStatement(r)] => match &r.argument {
                Some(e) => self.generator_call(e, depth),
                None => Err(Pending::unknown("the callback returns undefined")),
            },
            _ => Err(Pending::unknown(
                "the callback body is not a single `return` of a generator call",
            )),
        }
    }

    /// `name` used as the callback itself (`$defaultFn(createId)`).
    fn function_reference(&self, name: &str, depth: u8) -> Result<&'static str, Pending> {
        if let Some((specifier, imported)) = self.imports.get(name) {
            if let Some(g) = package_generator(specifier, imported) {
                return Ok(g);
            }
            if is_project_specifier(specifier) {
                return Err(Pending::Import(specifier.clone(), imported.clone()));
            }
            return Err(Pending::Unknown(format!(
                "`{name}` is `{imported}` of the package `{specifier}`, which is not a recognized identifier generator"
            )));
        }
        self.local_function(name, depth)
    }

    fn local_function(&self, name: &str, depth: u8) -> Result<&'static str, Pending> {
        if depth > 0 {
            return Err(Pending::Unknown(format!(
                "`{name}` is a helper of a helper (one level is followed)"
            )));
        }
        if let Some(f) = self.functions.get(name) {
            return self.function(f, depth + 1);
        }
        if let Some(init) = self.locals.get(name) {
            return match init.get_inner_expression() {
                e
                @ (Expression::ArrowFunctionExpression(_) | Expression::FunctionExpression(_)) => {
                    self.callback_value(e, depth + 1)
                }
                _ => Err(Pending::Unknown(format!(
                    "`{name}` is not a function declared in this module"
                ))),
            };
        }
        Err(Pending::Unknown(format!(
            "`{name}` is not bound to a known function"
        )))
    }

    /// A call of a recognized generator with no arguments (`nanoid` may
    /// take a numeric length).
    fn generator_call(&self, e: &Expression<'_>, depth: u8) -> Result<&'static str, Pending> {
        let Expression::CallExpression(call) = e.get_inner_expression() else {
            return Err(Pending::Unknown(format!(
                "the callback does not return a generator call (kind: {})",
                crate::concurrency_kind(e)
            )));
        };
        if call.optional {
            return Err(Pending::unknown("optional call"));
        }
        let no_arguments = call.arguments.is_empty();
        match call.callee.get_inner_expression() {
            Expression::StaticMemberExpression(m) if m.property.name == "randomUUID" => {
                let Expression::Identifier(object) = m.object.get_inner_expression() else {
                    return Err(Pending::unknown("`randomUUID` of an unknown object"));
                };
                let object = object.name.as_str();
                let crypto = match self.imports.get(object) {
                    Some((specifier, imported)) => {
                        is_crypto_module(specifier) && matches!(imported.as_str(), "default" | "*")
                    }
                    // The WebCrypto global, unless something local shadows it.
                    None => object == "crypto" && !self.bound("crypto"),
                };
                if !crypto {
                    return Err(Pending::Unknown(format!(
                        "`{object}.randomUUID()`: `{object}` is not the `crypto` module or global"
                    )));
                }
                if !no_arguments {
                    return Err(Pending::unknown("`randomUUID` called with arguments"));
                }
                Ok("crypto.randomUUID()")
            }
            Expression::Identifier(id) => {
                let name = id.name.as_str();
                if let Some((specifier, imported)) = self.imports.get(name) {
                    let Some(g) = package_generator(specifier, imported) else {
                        if is_project_specifier(specifier) {
                            if no_arguments {
                                return Err(Pending::Import(specifier.clone(), imported.clone()));
                            }
                            return Err(Pending::Unknown(format!(
                                "`{name}(...)` of `{specifier}` is called with arguments"
                            )));
                        }
                        return Err(Pending::Unknown(format!(
                            "`{name}` is `{imported}` of the package `{specifier}`, which is not a recognized identifier generator"
                        )));
                    };
                    let numeric_length = g == "nanoid()"
                        && matches!(call.arguments.as_slice(), [Argument::NumericLiteral(n)] if n.value >= 1.0);
                    if no_arguments || numeric_length {
                        return Ok(g);
                    }
                    return Err(Pending::Unknown(format!(
                        "`{name}(...)` is called with arguments"
                    )));
                }
                if !no_arguments {
                    return Err(Pending::Unknown(format!(
                        "`{name}(...)` is called with arguments"
                    )));
                }
                self.local_function(name, depth)
            }
            other => Err(Pending::Unknown(format!(
                "the callee is not a recognized function (kind: {})",
                crate::concurrency_kind(other)
            ))),
        }
    }
}

/// Why a callback is not (yet) recognized.
pub enum Pending {
    /// Defined in another project module: resolved by composition.
    Import(String, String),
    Unknown(String),
}

impl Pending {
    fn unknown(text: &str) -> Self {
        Self::Unknown(text.to_owned())
    }
}

/// Classify a callback defined in another module (`export const createId =
/// () => crypto.randomUUID()`, `export function newId() { ... }`) by its
/// exported name: used when schema modules are composed.
pub fn exported_callback(program: &Program<'_>, name: &str) -> Result<&'static str, String> {
    let recognizer = Recognizer::new(program);
    // Only a binding the module declares (re-exports are followed by the
    // composer, not here).
    if !recognizer.declared.iter().any(|d| d == name) {
        return Err(format!("`{name}` is not declared in the module"));
    }
    match recognizer.local_function(name, 0) {
        Ok(g) => Ok(g),
        Err(Pending::Import(specifier, imported)) => Err(format!(
            "`{name}` calls `{imported}` of `{specifier}` (one level of helpers is followed)"
        )),
        Err(Pending::Unknown(reason)) => Err(reason),
    }
}
