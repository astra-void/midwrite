//! Built-in recognition of well-known SDK clients (DEV-148).
//!
//! Most external effects in real projects go through a handful of SDKs
//! (Resend, Stripe, Supabase storage, BullMQ, Redis, ...). Without a
//! configuration entry they were unknown calls. A built-in rule applies only
//! when the client provably comes from the SDK package:
//!
//! * the binding is constructed in the same file by the package's class or
//!   factory (`new Resend(key)`, `createClient(url, key)`, `await
//!   createClient()`), or
//! * it is declared with the package's client type (`client:
//!   SupabaseClient`, `queue: Queue`), or
//! * it is a function or instance the package exports (`put` from
//!   `@vercel/blob`, the default export of `@sendgrid/mail`), or
//! * (linking, [`crate::link`]) it is imported from a project module that
//!   constructs it that way (`export const resend = new Resend(key)`), or
//!   returned by a project function that returns such a construction.
//!
//! A name alone never matches. Only calls that change state outside the
//! process are listed: reads (`retrieve`, `list`, `get`) are not effects.
//! Configured rules are tried first; `builtinEffects: false` turns this off.

use midwrite_core::ir::ExternalCategory;

/// A client of a known SDK package.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Client {
    Resend,
    Stripe,
    Supabase,
    BullQueue,
    BullJob,
    BullFlow,
    Redis,
    Polar,
    Mailer,
    S3,
    Twilio,
    SendGrid,
}

impl Client {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Resend => "a Resend client (`resend`)",
            Self::Stripe => "a Stripe client (`stripe`)",
            Self::Supabase => "a Supabase client (`@supabase/supabase-js`)",
            Self::BullQueue => "a BullMQ queue (`bullmq`)",
            Self::BullJob => "a BullMQ job (`bullmq`)",
            Self::BullFlow => "a BullMQ flow producer (`bullmq`)",
            Self::Redis => "a Redis client",
            Self::Polar => "a Polar client (`@polar-sh/sdk`)",
            Self::Mailer => "a nodemailer transport (`nodemailer`)",
            Self::S3 => "an AWS S3 client (`@aws-sdk/client-s3`)",
            Self::Twilio => "a Twilio client (`twilio`)",
            Self::SendGrid => "the SendGrid mail client (`@sendgrid/mail`)",
        }
    }
}

/// What a binding imported from an SDK package is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Imported {
    /// A class: `new C(...)` is a client, and `C` as a type names one.
    Class(Client),
    /// A function returning a client (`createClient`).
    Factory(Client),
    /// A type naming a client (`RedisClientType`).
    Type(Client),
    /// A ready client (the default export of `@sendgrid/mail`).
    Instance(Client),
    /// A module object with a client factory method
    /// (`nodemailer.createTransport`).
    Module {
        method: &'static str,
        client: Client,
    },
    /// A function that performs an effect itself (`put` from `@vercel/blob`).
    Effect(ExternalCategory),
}

/// The SDK binding `module` exports under `name` (`None`: the default export
/// or the namespace).
pub(crate) fn imported(module: &str, name: Option<&str>) -> Option<Imported> {
    use Client::*;
    use Imported::*;
    Some(match (module, name) {
        ("resend", Some("Resend")) => Class(Resend),
        ("stripe", None | Some("Stripe")) => Class(Stripe),
        ("@supabase/supabase-js", Some("createClient")) => Factory(Supabase),
        ("@supabase/supabase-js", Some("SupabaseClient")) => Class(Supabase),
        ("@supabase/ssr", Some("createServerClient" | "createBrowserClient")) => Factory(Supabase),
        ("bullmq", Some("Queue")) => Class(BullQueue),
        ("bullmq", Some("Job")) => Class(BullJob),
        ("bullmq", Some("FlowProducer")) => Class(BullFlow),
        ("ioredis", None | Some("Redis" | "Cluster")) => Class(Redis),
        ("redis", Some("createClient" | "createCluster")) => Factory(Redis),
        ("redis", Some("RedisClientType")) => Type(Redis),
        ("@upstash/redis", Some("Redis")) => Class(Redis),
        ("@polar-sh/sdk", Some("Polar")) => Class(Polar),
        ("nodemailer", None) => Module {
            method: "createTransport",
            client: Mailer,
        },
        ("nodemailer", Some("createTransport")) => Factory(Mailer),
        ("nodemailer", Some("Transporter")) => Type(Mailer),
        ("@aws-sdk/client-s3", Some("S3Client" | "S3")) => Class(S3),
        ("twilio", None) => Factory(Twilio),
        ("twilio", Some("Twilio")) => Class(Twilio),
        ("@sendgrid/mail", None) => Instance(SendGrid),
        ("@vercel/blob", Some("put" | "del" | "copy")) => Effect(ExternalCategory::FileWrite),
        _ => return None,
    })
}

/// One step of a call chain after its root.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step<'s> {
    Property(&'s str),
    /// A call; `command` is the class name of a `new XCommand(...)` first
    /// argument, if any.
    Call {
        command: Option<&'s str>,
    },
}

const STRIPE_WRITES: &[&str] = &[
    "create",
    "update",
    "del",
    "cancel",
    "confirm",
    "capture",
    "pay",
    "finalizeInvoice",
    "voidInvoice",
    "sendInvoice",
    "markUncollectible",
    "attach",
    "detach",
    "resume",
    "release",
    "approve",
    "reject",
    "reverse",
    "createReversal",
    "createBalanceTransaction",
    "createSource",
    "deleteSource",
    "createTaxId",
    "deleteTaxId",
    "expire",
];
const POLAR_WRITES: &[&str] = &[
    "create",
    "update",
    "delete",
    "revoke",
    "updateExternal",
    "deleteExternal",
];
const REDIS_WRITES: &[&str] = &[
    // ioredis / @upstash/redis (lower case) and node-redis v4 (camel case).
    "set",
    "setex",
    "psetex",
    "setnx",
    "getset",
    "getdel",
    "del",
    "unlink",
    "expire",
    "pexpire",
    "expireat",
    "persist",
    "incr",
    "incrby",
    "incrbyfloat",
    "decr",
    "decrby",
    "append",
    "hset",
    "hmset",
    "hsetnx",
    "hdel",
    "hincrby",
    "lpush",
    "rpush",
    "lpop",
    "rpop",
    "lrem",
    "lset",
    "ltrim",
    "sadd",
    "srem",
    "spop",
    "zadd",
    "zrem",
    "zincrby",
    "zremrangebyscore",
    "publish",
    "xadd",
    "xdel",
    "xtrim",
    "mset",
    "rename",
    "setEx",
    "pSetEx",
    "setNX",
    "getDel",
    "hSet",
    "hSetNX",
    "hDel",
    "hIncrBy",
    "incrBy",
    "incrByFloat",
    "decrBy",
    "pExpire",
    "expireAt",
    "lPush",
    "rPush",
    "lPop",
    "rPop",
    "lRem",
    "lSet",
    "lTrim",
    "sAdd",
    "sRem",
    "sPop",
    "zAdd",
    "zRem",
    "zIncrBy",
    "zRemRangeByScore",
    "xAdd",
    "xDel",
    "xTrim",
    "mSet",
];
const QUEUE_WRITES: &[&str] = &[
    "add",
    "addBulk",
    "remove",
    "obliterate",
    "drain",
    "clean",
    "pause",
    "resume",
    "removeRepeatable",
    "removeRepeatableByKey",
    "upsertJobScheduler",
    "removeJobScheduler",
    "retryJobs",
    "promoteJobs",
];
const JOB_WRITES: &[&str] = &[
    "remove",
    "retry",
    "promote",
    "update",
    "updateData",
    "changeDelay",
    "changePriority",
    "moveToFailed",
    "moveToCompleted",
    "moveToDelayed",
    "discard",
];

/// The effect a call on `client` performs: `steps` are the chain after the
/// client up to and including the effect call (promise continuations
/// removed). `None` for reads and anything not listed.
pub(crate) fn effect(client: Client, steps: &[Step<'_>]) -> Option<ExternalCategory> {
    use ExternalCategory::*;
    use Step::{Call, Property as P};
    let (method, before) = match steps {
        [before @ .., P(method), Call { .. }] => (*method, before),
        _ => return None,
    };
    let props: Option<Vec<&str>> = before
        .iter()
        .map(|step| match step {
            P(name) => Some(*name),
            Call { .. } => None,
        })
        .collect();
    match client {
        Client::Resend => match (props?.as_slice(), method) {
            (["emails" | "batch"], "send" | "create") => Some(Email),
            (["broadcasts"], "create" | "send" | "update" | "remove") => Some(Email),
            (["contacts" | "audiences" | "domains"], "create" | "update" | "remove") => Some(Other),
            _ => None,
        },
        Client::Stripe => {
            let props = props?;
            (!props.is_empty()
                && props.len() <= 3
                && props[0] != "webhooks"
                && STRIPE_WRITES.contains(&method))
            .then_some(Payment)
        }
        Client::Polar => {
            let props = props?;
            (!props.is_empty() && props.len() <= 2 && POLAR_WRITES.contains(&method))
                .then_some(Payment)
        }
        Client::Supabase => match (before, method) {
            (
                [P("storage"), P("from"), Call { .. }],
                "upload" | "update" | "move" | "copy" | "remove" | "uploadToSignedUrl",
            ) => Some(FileWrite),
            (
                [P("auth"), P("admin")],
                "createUser" | "deleteUser" | "updateUserById" | "inviteUserByEmail"
                | "generateLink" | "signOut",
            ) => Some(Other),
            _ => None,
        },
        Client::BullQueue => {
            (props?.is_empty() && QUEUE_WRITES.contains(&method)).then_some(MessagePublish)
        }
        Client::BullJob => {
            (props?.is_empty() && JOB_WRITES.contains(&method)).then_some(MessagePublish)
        }
        Client::BullFlow => {
            (props?.is_empty() && matches!(method, "add" | "addBulk")).then_some(MessagePublish)
        }
        Client::Redis => {
            if props.as_ref().is_some_and(Vec::is_empty) && REDIS_WRITES.contains(&method) {
                return Some(Other);
            }
            // `redis.multi().incr(k).expire(k, ttl).exec()`: a transaction or
            // pipeline that queues at least one write.
            match (before, method) {
                ([P("multi" | "pipeline"), Call { .. }, queued @ ..], "exec")
                    if queued
                        .iter()
                        .any(|step| matches!(step, P(m) if REDIS_WRITES.contains(m))) =>
                {
                    Some(Other)
                }
                _ => None,
            }
        }
        Client::Mailer => (props?.is_empty() && method == "sendMail").then_some(Email),
        Client::S3 => {
            if !props?.is_empty() {
                return None;
            }
            match (method, steps.last()) {
                (
                    "send",
                    Some(Call {
                        command: Some(command),
                    }),
                ) => {
                    let read = ["Get", "Head", "List", "Select"]
                        .iter()
                        .any(|prefix| command.starts_with(prefix));
                    (!read && command.ends_with("Command")).then_some(FileWrite)
                }
                (
                    "putObject"
                    | "deleteObject"
                    | "deleteObjects"
                    | "copyObject"
                    | "createMultipartUpload"
                    | "completeMultipartUpload"
                    | "uploadPart",
                    _,
                ) => Some(FileWrite),
                _ => None,
            }
        }
        Client::Twilio => matches!(
            (props?.as_slice(), method),
            (["messages" | "calls"], "create")
        )
        .then_some(Other),
        Client::SendGrid => {
            (props?.is_empty() && matches!(method, "send" | "sendMultiple")).then_some(Email)
        }
    }
}

/// The client a method call on `client` returns, for the few that hand out
/// another client (`queue.getJob(id)`, `queue.add(...)` → a job).
pub(crate) fn returned_client(client: Client, method: &str) -> Option<Client> {
    match (client, method) {
        (Client::BullQueue, "getJob" | "add") => Some(Client::BullJob),
        _ => None,
    }
}

/// DEV-148: what a project module exports that is an SDK client: a
/// module-level construction (`export const resend = new Resend(key)`) or a
/// function returning one (`export function createAdminClient() { return
/// createClient(url, key); }`, or declared `(): Queue`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Export {
    Client(Client),
    Factory(Client),
}

impl Export {
    pub fn imported(self) -> Imported {
        match self {
            Self::Client(client) => Imported::Instance(client),
            Self::Factory(client) => Imported::Factory(client),
        }
    }
}

/// Syntactic SDK facts of one module, for [`crate::project`]'s pre-pass.
#[derive(Debug, Default)]
pub(crate) struct FileFacts {
    /// Exported name (`default` for the default export) → what it is.
    pub exports: HashMap<String, Export>,
    /// `export { imported as exported } from "specifier"`:
    /// `(exported, specifier, imported)`.
    pub reexports: Vec<(String, String, String)>,
    /// `export * from "specifier"`.
    pub stars: Vec<String>,
    /// `(specifier, imported name)` of every named or default import from a
    /// module that is not a known SDK package.
    pub imports: Vec<(String, String)>,
}

/// SDK clients a module imports from project modules:
/// `(specifier, imported name)` → what the target module exports.
pub(crate) type ImportedClients = HashMap<(String, String), Export>;

use std::collections::HashMap;

use oxc_ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, Function,
    ImportDeclarationSpecifier, ModuleExportName, Program, Statement, TSType, TSTypeName,
};
use oxc_ast_visit::Visit;

pub(crate) fn file_facts(program: &Program<'_>) -> FileFacts {
    let mut imports: HashMap<String, (String, Option<String>)> = HashMap::new();
    let mut facts = FileFacts::default();
    for statement in &program.body {
        let Statement::ImportDeclaration(import) = statement else {
            continue;
        };
        let module = import.source.value.as_str().unwrap_or_default().to_owned();
        for specifier in import.specifiers.iter().flatten() {
            let (local, imported_name) = match specifier {
                ImportDeclarationSpecifier::ImportSpecifier(s) => (
                    s.local.name.as_str(),
                    Some(s.imported.name().as_str().to_owned()),
                ),
                ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                    (s.local.name.as_str(), None)
                }
                ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                    imports.insert(s.local.name.as_str().to_owned(), (module.clone(), None));
                    continue;
                }
            };
            if imported(&module, imported_name.as_deref()).is_none() {
                facts.imports.push((
                    module.clone(),
                    imported_name
                        .clone()
                        .unwrap_or_else(|| "default".to_owned()),
                ));
            }
            imports.insert(local.to_owned(), (module.clone(), imported_name));
        }
    }
    let scope = Scope { imports: &imports };
    let mut locals: HashMap<String, Export> = HashMap::new();
    let mut exported: Vec<(String, String)> = Vec::new();
    for statement in &program.body {
        match statement {
            Statement::ExportDeclaration(export) => {
                for (name, value) in scope.declaration(&export.declaration) {
                    exported.push((name.clone(), name.clone()));
                    locals.insert(name, value);
                }
            }
            Statement::ExportNamedDeclaration(export) => {
                for specifier in &export.specifiers {
                    exported.push((
                        module_export_name(&specifier.exported),
                        module_export_name(&specifier.local),
                    ));
                }
            }
            Statement::ExportFromDeclaration(export) => {
                let source = export.source.value.as_str().unwrap_or_default().to_owned();
                for specifier in &export.specifiers {
                    facts.reexports.push((
                        module_export_name(&specifier.exported),
                        source.clone(),
                        module_export_name(&specifier.local),
                    ));
                }
            }
            Statement::ExportDefaultDeclaration(export) => match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                    if let Some(client) = scope.function_returns(function) {
                        facts
                            .exports
                            .insert("default".to_owned(), Export::Factory(client));
                    }
                }
                other => match other.as_expression().map(Expression::get_inner_expression) {
                    Some(Expression::Identifier(ident)) => {
                        exported.push(("default".to_owned(), ident.name.as_str().to_owned()));
                    }
                    Some(expression) => {
                        if let Some(client) = scope.construction(expression) {
                            facts
                                .exports
                                .insert("default".to_owned(), Export::Client(client));
                        }
                    }
                    None => {}
                },
            },
            Statement::ExportAllDeclaration(export) if export.exported.is_none() => {
                facts
                    .stars
                    .push(export.source.value.as_str().unwrap_or_default().to_owned());
            }
            other => {
                if let Some(declaration) = other.as_declaration() {
                    locals.extend(scope.declaration(declaration));
                }
            }
        }
    }
    for (name, local) in exported {
        if let Some(value) = locals.get(&local) {
            facts.exports.insert(name, *value);
        }
    }
    facts
}

fn module_export_name(name: &ModuleExportName<'_>) -> String {
    name.name().as_str().to_owned()
}

struct Scope<'s> {
    /// Local name → (module, imported name; `None` for default/namespace).
    imports: &'s HashMap<String, (String, Option<String>)>,
}

impl Scope<'_> {
    fn sdk(&self, local: &str) -> Option<Imported> {
        let (module, name) = self.imports.get(local)?;
        imported(module, name.as_deref())
    }

    /// The SDK client `expression` constructs (`new C(...)`, `f(...)`,
    /// `m.createTransport(...)`, awaited or not).
    fn construction(&self, expression: &Expression<'_>) -> Option<Client> {
        let mut expression = expression.get_inner_expression();
        if let Expression::AwaitExpression(awaited) = expression {
            expression = awaited.argument.get_inner_expression();
        }
        match expression {
            Expression::NewExpression(new) => match new.callee.get_inner_expression() {
                Expression::Identifier(callee) => match self.sdk(callee.name.as_str())? {
                    Imported::Class(client) => Some(client),
                    _ => None,
                },
                _ => None,
            },
            Expression::CallExpression(call) => match call.callee.get_inner_expression() {
                Expression::Identifier(callee) => match self.sdk(callee.name.as_str())? {
                    Imported::Factory(client) => Some(client),
                    _ => None,
                },
                Expression::StaticMemberExpression(member) => {
                    let Expression::Identifier(object) = member.object.get_inner_expression()
                    else {
                        return None;
                    };
                    match self.sdk(object.name.as_str())? {
                        Imported::Module { method, client } if member.property.name == method => {
                            Some(client)
                        }
                        Imported::Class(Client::Redis) if member.property.name == "fromEnv" => {
                            Some(Client::Redis)
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// The client a declared type names (`Queue`, `Promise<SupabaseClient>`).
    fn type_client(&self, ty: &TSType<'_>) -> Option<Client> {
        let TSType::TSTypeReference(reference) = ty else {
            return None;
        };
        let TSTypeName::IdentifierReference(name) = &reference.type_name else {
            return None;
        };
        if name.name == "Promise" {
            return self.type_client(reference.type_arguments.as_ref()?.params.first()?);
        }
        match self.sdk(name.name.as_str())? {
            Imported::Class(client) | Imported::Type(client) => Some(client),
            _ => None,
        }
    }

    fn function_returns(&self, function: &Function<'_>) -> Option<Client> {
        if let Some(ty) = &function.return_type {
            return self.type_client(&ty.type_annotation);
        }
        self.body_returns(&function.body.as_ref()?.statements)
    }

    /// The client every `return` of a body constructs (all must agree).
    fn body_returns(&self, statements: &[Statement<'_>]) -> Option<Client> {
        struct Returns<'r, 's> {
            scope: &'r Scope<'s>,
            found: Vec<Option<Client>>,
        }
        impl<'a> Visit<'a> for Returns<'_, '_> {
            fn visit_return_statement(&mut self, it: &oxc_ast::ast::ReturnStatement<'a>) {
                self.found.push(
                    it.argument
                        .as_ref()
                        .and_then(|e| self.scope.construction(e)),
                );
            }
            fn visit_function(&mut self, _: &Function<'a>, _: oxc_syntax::scope::ScopeFlags) {}
            fn visit_arrow_function_expression(
                &mut self,
                _: &oxc_ast::ast::ArrowFunctionExpression<'a>,
            ) {
            }
        }
        let mut returns = Returns {
            scope: self,
            found: Vec::new(),
        };
        for statement in statements {
            returns.visit_statement(statement);
        }
        let first = (*returns.found.first()?)?;
        returns
            .found
            .iter()
            .all(|client| *client == Some(first))
            .then_some(first)
    }

    /// Module-level bindings of a declaration that hold or return a client.
    fn declaration(&self, declaration: &Declaration<'_>) -> Vec<(String, Export)> {
        match declaration {
            Declaration::FunctionDeclaration(function) => function
                .id
                .as_ref()
                .zip(self.function_returns(function))
                .map(|(id, client)| (id.name.as_str().to_owned(), Export::Factory(client)))
                .into_iter()
                .collect(),
            Declaration::VariableDeclaration(variables) => variables
                .declarations
                .iter()
                .filter_map(|declarator| {
                    let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                        return None;
                    };
                    let init = declarator.init.as_ref()?;
                    let value = match init.get_inner_expression() {
                        Expression::ArrowFunctionExpression(arrow) => {
                            let client = match (&arrow.return_type, &arrow.body) {
                                (Some(ty), _) => self.type_client(&ty.type_annotation),
                                (None, oxc_ast::ast::ArrowFunctionBody::FunctionBody(body)) => {
                                    self.body_returns(&body.statements)
                                }
                                (None, other) => {
                                    other.as_expression().and_then(|e| self.construction(e))
                                }
                            }?;
                            Export::Factory(client)
                        }
                        Expression::FunctionExpression(function) => {
                            Export::Factory(self.function_returns(function)?)
                        }
                        other => Export::Client(self.construction(other)?),
                    };
                    Some((id.name.as_str().to_owned(), value))
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Step::{Call, Property as P};
    use super::*;

    const CALL: Step<'static> = Call { command: None };

    #[test]
    fn writes_are_effects_and_reads_are_not() {
        assert_eq!(
            effect(Client::Resend, &[P("emails"), P("send"), CALL]),
            Some(ExternalCategory::Email)
        );
        assert_eq!(effect(Client::Resend, &[P("emails"), P("get"), CALL]), None);
        assert_eq!(
            effect(
                Client::Stripe,
                &[P("checkout"), P("sessions"), P("create"), CALL]
            ),
            Some(ExternalCategory::Payment)
        );
        assert_eq!(
            effect(Client::Stripe, &[P("customers"), P("retrieve"), CALL]),
            None
        );
        assert_eq!(
            effect(Client::Stripe, &[P("webhooks"), P("constructEvent"), CALL]),
            None
        );
        assert_eq!(
            effect(
                Client::Supabase,
                &[P("storage"), P("from"), CALL, P("remove"), CALL]
            ),
            Some(ExternalCategory::FileWrite)
        );
        assert_eq!(
            effect(
                Client::Supabase,
                &[P("storage"), P("from"), CALL, P("download"), CALL]
            ),
            None
        );
        // Table writes through Supabase are database operations, not modeled here.
        assert_eq!(
            effect(Client::Supabase, &[P("from"), CALL, P("insert"), CALL]),
            None
        );
        assert_eq!(
            effect(Client::Redis, &[P("del"), CALL]),
            Some(ExternalCategory::Other)
        );
        assert_eq!(effect(Client::Redis, &[P("get"), CALL]), None);
        assert_eq!(
            effect(
                Client::Redis,
                &[
                    P("multi"),
                    CALL,
                    P("incr"),
                    CALL,
                    P("expire"),
                    CALL,
                    P("exec"),
                    CALL
                ]
            ),
            Some(ExternalCategory::Other)
        );
        assert_eq!(
            effect(
                Client::Redis,
                &[P("multi"), CALL, P("get"), CALL, P("exec"), CALL]
            ),
            None
        );
        assert_eq!(
            effect(
                Client::S3,
                &[
                    P("send"),
                    Call {
                        command: Some("PutObjectCommand")
                    }
                ]
            ),
            Some(ExternalCategory::FileWrite)
        );
        assert_eq!(
            effect(
                Client::S3,
                &[
                    P("send"),
                    Call {
                        command: Some("GetObjectCommand")
                    }
                ]
            ),
            None
        );
        assert_eq!(
            effect(Client::BullJob, &[P("remove"), CALL]),
            Some(ExternalCategory::MessagePublish)
        );
        assert_eq!(effect(Client::BullQueue, &[P("getJob"), CALL]), None);
    }
}
