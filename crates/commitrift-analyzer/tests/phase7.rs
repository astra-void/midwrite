//! Phase 7: source selectors, wrapper invocation facts and declared
//! contracts (fixtures/phase7/wrappers), and the extraction of the
//! first-party Server Action application (fixtures/phase7/team-app).

use std::path::PathBuf;

use commitrift_analyzer::TsActorProvider;
use commitrift_model::ir::{Op, StmtKind};
use commitrift_model::provider::{
    ActorProvider, ExtractRequest, Extracted, InvocationContract, TableBinding,
};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase7")
}

fn table(identifier: &str, name: &str, columns: &[&str]) -> TableBinding {
    TableBinding {
        identifier: identifier.to_owned(),
        table: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: None,
    }
}

fn wrappers(selector: &str, contract: Option<InvocationContract>) -> Result<Extracted, String> {
    let root = fixtures().join("wrappers");
    TsActorProvider::new(&root).extract(&ExtractRequest {
        root: root.clone(),
        file: String::new(),
        function: String::new(),
        arguments: Vec::new(),
        tables: vec![
            table("orders", "orders", &["id", "status"]),
            table("audit", "audit", &["id", "orderId"]),
        ],
        externals: Vec::new(),
        transaction_retries: Vec::new(),
        max_inline_depth: 3,
        selector: Some(selector.to_owned()),
        contract,
        replicas: false,
        replica_factories: Vec::new(),
        return_values: false,
        js_values: false,
    })
}

fn contract(invocation: Option<&str>, preconditions: &[&str]) -> Option<InvocationContract> {
    Some(InvocationContract {
        invocation: invocation.map(str::to_owned),
        preconditions: preconditions.iter().map(|p| (*p).to_owned()).collect(),
    })
}

/// `kind table` of every database statement, `unknown: detail` of every
/// unknown operation, in code order (main task).
fn ops(e: &Extracted) -> Vec<String> {
    e.codes[0]
        .iter()
        .filter_map(|s| match &s.op {
            Op::Db { stmt } => Some(match &stmt.kind {
                StmtKind::Select { table, .. } => format!("select {table}"),
                StmtKind::SelectCount { table, .. } => format!("count {table}"),
                StmtKind::Insert { table, .. } => format!("insert {table}"),
                StmtKind::Update { table, .. } => format!("update {table}"),
                StmtKind::Delete { table, .. } => format!("delete {table}"),
            }),
            Op::Unknown { detail, .. } => Some(format!("unknown: {detail}")),
            _ => None,
        })
        .collect()
}

#[test]
fn callbacks_with_identical_text_are_different_functions_with_the_same_model() {
    let a = wrappers("src/handlers.ts#cancelA/arg[0]", None).unwrap();
    let b = wrappers("src/handlers.ts#cancelB/arg[0]", None).unwrap();
    let (ea, eb) = (a.entry.clone().unwrap(), b.entry.clone().unwrap());
    assert_ne!(ea.anchor, eb.anchor);
    assert_eq!(ea.anchor, "src/handlers.ts@8:31");
    // The name `commitrift check` gives the same function (by position).
    assert_eq!(ea.static_name, "<anonymous@8:31>");
    assert_eq!(ops(&a), vec!["update orders"]);
    assert_eq!(ops(&a), ops(&b));
    let w = ea.wrapper.unwrap();
    assert_eq!(w.wrapper, "action");
    assert_eq!(w.invocation, "returned-function-returns-promise");
    assert!(!w.conditional, "{w:?}");
    assert_eq!(ea.contract_status, "absent");
    // The anchor selects the same function.
    let by_anchor = wrappers("src/handlers.ts@8:31", None).unwrap();
    assert_eq!(by_anchor.entry.unwrap().anchor, ea.anchor);
}

#[test]
fn a_guarded_wrapper_needs_declared_preconditions() {
    let selector = "src/handlers.ts#shipOrder/arg[0]/arg[0]";
    let refused = wrappers(selector, None).unwrap_err();
    assert!(refused.contains("only when its guards pass"), "{refused}");
    assert!(refused.contains("if (!user)"), "{refused}");
    let e = wrappers(selector, contract(None, &["authenticated-user"])).unwrap();
    let entry = e.entry.clone().unwrap();
    let w = entry.wrapper.unwrap();
    assert_eq!(w.wrapper, "authed");
    assert_eq!(w.invocation, "returned-function-awaits");
    assert!(w.conditional);
    assert!(
        w.guards.iter().any(|g| g.contains("currentUser")),
        "{:?}",
        w.guards
    );
    assert_eq!(w.arguments, vec!["input", "user"]);
    assert_eq!(entry.contract_status, "consistent");
    assert_eq!(ops(&e), vec!["update orders", "insert audit"]);
    // Declared facts are recorded as assumptions, never as source facts.
    assert!(e
        .assumptions
        .iter()
        .any(|a| a.contains("declared precondition (scenario, not source): authenticated-user")));
}

#[test]
fn invocation_shapes_are_distinguished() {
    let boot = wrappers("src/handlers.ts#bootTask/arg[0]", None).unwrap();
    assert_eq!(
        boot.entry.unwrap().wrapper.unwrap().invocation,
        "invoked-during-wrap"
    );

    let background_selector = "src/handlers.ts#background/arg[0]";
    let refused = wrappers(background_selector, contract(Some("awaited"), &[])).unwrap_err();
    assert!(
        refused.contains("contradicts the wrapper's source"),
        "{refused}"
    );
    let background = wrappers(background_selector, contract(Some("fire-and-forget"), &[])).unwrap();
    assert_eq!(
        background.entry.unwrap().wrapper.unwrap().invocation,
        "returned-function-fire-and-forget"
    );

    let never = wrappers(
        "src/handlers.ts#neverRuns/arg[0]",
        contract(Some("awaited"), &[]),
    );
    assert!(never.unwrap_err().contains("never invokes"));
}

#[test]
fn stored_or_unresolvable_invocations_need_a_declared_contract() {
    let stored = wrappers("src/handlers.ts#stored/arg[0]", None).unwrap_err();
    assert!(stored.contains("is unknown"), "{stored}");
    let declared = wrappers(
        "src/handlers.ts#stored/arg[0]",
        contract(Some("awaited"), &[]),
    )
    .unwrap();
    let entry = declared.entry.unwrap();
    assert_eq!(entry.wrapper.unwrap().invocation, "stored");
    assert_eq!(entry.contract_status, "required");

    // tRPC-style: the builder is a package API (its call of the resolver is
    // not visible), and the resolver destructures its options.
    let selector = "src/handlers.ts#router/property[archive]/arg[0]";
    assert!(wrappers(selector, None).unwrap_err().contains("is unknown"));
    let trpc = wrappers(selector, contract(Some("awaited"), &["input validated"])).unwrap();
    assert_eq!(trpc.entry.as_ref().unwrap().contract_status, "required");
    assert_eq!(ops(&trpc), vec!["update orders"]);
    assert_eq!(trpc.params, vec!["_param0"]);
}

#[test]
fn factories_methods_and_object_properties_are_selectable() {
    // The returned function closes over the factory's parameter, which the
    // actor does not bind: the value is unknown, never guessed.
    let factory = wrappers("src/handlers.ts#makeTransition/return", None).unwrap();
    assert_eq!(
        factory.entry.as_ref().unwrap().contract_status,
        "not-needed"
    );
    let factory_ops = ops(&factory);
    assert_eq!(factory_ops.len(), 1);
    assert!(
        factory_ops[0].contains("unresolved identifier `next`"),
        "{factory_ops:?}"
    );

    let method = wrappers("src/handlers.ts#OrderService/method[place]", None).unwrap();
    assert_eq!(
        method.entry.as_ref().unwrap().static_name,
        "OrderService.place"
    );
    assert_eq!(ops(&method), vec!["insert orders"]);
    let purge = wrappers("src/handlers.ts#OrderService/static[purge]", None).unwrap();
    assert_eq!(ops(&purge), vec!["delete orders"]);
    let reopen = wrappers("src/handlers.ts#objectHandlers/property[reopen]", None).unwrap();
    assert_eq!(ops(&reopen), vec!["update orders"]);
}

#[test]
fn the_team_app_action_callback_is_extracted_without_unknowns() {
    let root = fixtures().join("team-app");
    let e = TsActorProvider::new(&root)
        .extract(&ExtractRequest {
            root: root.clone(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables: vec![
                table("users", "users", &["id", "email", "passwordHash"]),
                table("teams", "teams", &["id", "name"]),
                table(
                    "teamMembers",
                    "team_members",
                    &["id", "userId", "teamId", "role"],
                ),
                table(
                    "invitations",
                    "invitations",
                    &["id", "teamId", "email", "role", "invitedBy", "status"],
                ),
                table(
                    "activityLogs",
                    "activity_logs",
                    &["id", "teamId", "userId", "action", "ipAddress"],
                ),
            ],
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some("src/app/actions.ts#inviteTeamMember/arg[1]".to_owned()),
            contract: contract(Some("awaited"), &["authenticated-user"]),
            replicas: false,
            replica_factories: Vec::new(),
            return_values: false,
            js_values: false,
        })
        .unwrap();
    assert_eq!(
        ops(&e),
        vec![
            "select users",
            "select users",
            "select invitations",
            "insert invitations",
            "insert activity_logs"
        ]
    );
    assert_eq!(e.params, vec!["data", "_", "user"]);
    // The membership check is a LEFT JOIN with the same equality key as
    // the source.
    let joins: Vec<String> = e.codes[0]
        .iter()
        .filter_map(|s| match &s.op {
            Op::Db { stmt } => match &stmt.kind {
                StmtKind::Select { joins, .. } if !joins.is_empty() => Some(format!(
                    "{:?} {} {}={}",
                    joins[0].kind, joins[0].table, joins[0].left, joins[0].right
                )),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert_eq!(
        joins,
        vec![
            "Left team_members users.id=team_members.userId",
            "Left team_members users.id=team_members.userId"
        ]
    );
    let entry = e.entry.unwrap();
    assert_eq!(entry.static_name, "<anonymous@43:3>");
    let w = entry.wrapper.unwrap();
    assert_eq!(w.wrapper, "validatedActionWithUser");
    assert_eq!(
        w.defined_at.as_deref(),
        Some("src/lib/auth/middleware.ts:26:8")
    );
}
