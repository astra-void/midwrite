//! What a module exports, for cross-module call resolution.
//!
//! Only the module-level `export` forms are read, from the program body. Type
//! exports (`export type`, `export interface`, `export { type X }`) are
//! skipped: they have no runtime value and can never be called.

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, ModuleExportName,
    Program, Statement,
};
use oxc_semantic::Scoping;
use oxc_syntax::symbol::SymbolId;

use crate::bindings::resolve_reference;

/// What an exported name refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExportTarget {
    /// A binding of this module (a declaration or an import).
    Local(SymbolId),
    /// A function without a binding of its own, by start offset
    /// (`export default async function () {}`).
    Function(u32),
    /// `export { name } from "module"`; `name` is `default` for the default export.
    ReExport { module: String, name: String },
    /// A class declaration, by start offset.
    Class(u32),
    /// Something that is certainly not a function CommitRift can summarize
    /// (a class, a namespace object, an enum, ...).
    Other,
}

#[derive(Debug, Default)]
pub(crate) struct RawExports {
    /// Exported names in source order; `default` for the default export.
    pub named: Vec<(String, ExportTarget)>,
    /// `export * from "module"` specifiers, in source order.
    pub stars: Vec<String>,
}

pub(crate) fn collect(program: &Program<'_>, scoping: &Scoping) -> RawExports {
    let mut exports = RawExports::default();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::FunctionDeclaration(function) => {
                    if let Some(id) = &function.id
                        && let Some(symbol) = id.symbol_id.get()
                    {
                        exports
                            .named
                            .push((id.name.as_str().to_owned(), ExportTarget::Local(symbol)));
                    }
                }
                Declaration::VariableDeclaration(declaration) => {
                    for declarator in &declaration.declarations {
                        if let BindingPattern::BindingIdentifier(id) = &declarator.id
                            && let Some(symbol) = id.symbol_id.get()
                        {
                            exports
                                .named
                                .push((id.name.as_str().to_owned(), ExportTarget::Local(symbol)));
                        }
                    }
                }
                Declaration::ClassDeclaration(class) => {
                    if let Some(id) = &class.id {
                        exports.named.push((
                            id.name.as_str().to_owned(),
                            ExportTarget::Class(class.span.start),
                        ));
                    }
                }
                Declaration::TSEnumDeclaration(declaration) => exports
                    .named
                    .push((declaration.id.name.as_str().to_owned(), ExportTarget::Other)),
                // Types, ambient modules and namespaces.
                _ => {}
            },
            Statement::ExportNamedDeclaration(export) if export.export_kind.is_value() => {
                for specifier in &export.specifiers {
                    if !specifier.export_kind.is_value() {
                        continue;
                    }
                    let target = match &specifier.local {
                        ModuleExportName::IdentifierReference(ident) => {
                            resolve_reference(scoping, ident)
                                .map_or(ExportTarget::Other, ExportTarget::Local)
                        }
                        _ => ExportTarget::Other,
                    };
                    exports
                        .named
                        .push((specifier.exported.name().as_str().to_owned(), target));
                }
            }
            Statement::ExportFromDeclaration(export) if export.export_kind.is_value() => {
                let module = export.source.value.as_str().unwrap_or_default().to_owned();
                for specifier in &export.specifiers {
                    if !specifier.export_kind.is_value() {
                        continue;
                    }
                    exports.named.push((
                        specifier.exported.name().as_str().to_owned(),
                        ExportTarget::ReExport {
                            module: module.clone(),
                            name: specifier.local.name().as_str().to_owned(),
                        },
                    ));
                }
            }
            Statement::ExportAllDeclaration(export) if export.export_kind.is_value() => {
                let module = export.source.value.as_str().unwrap_or_default().to_owned();
                match &export.exported {
                    // `export * as ns from "m"` exports a namespace object.
                    Some(name) => exports
                        .named
                        .push((name.name().as_str().to_owned(), ExportTarget::Other)),
                    None => exports.stars.push(module),
                }
            }
            Statement::ExportDefaultDeclaration(export) => {
                let target = match &export.declaration {
                    ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                        if function.body.is_some() {
                            ExportTarget::Function(function.span.start)
                        } else {
                            ExportTarget::Other
                        }
                    }
                    ExportDefaultDeclarationKind::TSInterfaceDeclaration(_) => continue,
                    ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                        ExportTarget::Class(class.span.start)
                    }
                    declaration => match declaration
                        .as_expression()
                        .map(Expression::get_inner_expression)
                    {
                        Some(Expression::Identifier(ident)) => resolve_reference(scoping, ident)
                            .map_or(ExportTarget::Other, ExportTarget::Local),
                        Some(Expression::ArrowFunctionExpression(function)) => {
                            ExportTarget::Function(function.span.start)
                        }
                        Some(Expression::FunctionExpression(function))
                            if function.body.is_some() =>
                        {
                            ExportTarget::Function(function.span.start)
                        }
                        _ => ExportTarget::Other,
                    },
                };
                exports.named.push(("default".to_owned(), target));
            }
            _ => {}
        }
    }
    exports
}
