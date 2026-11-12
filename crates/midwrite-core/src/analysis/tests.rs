//! Analysis tests on hand-built IR, independent of any frontend.

use super::{
    AnalysisOptions, CallArgument, LocalCall, ProjectFiles, analyze_function, analyze_function_in,
    inline_calls, inline_local_calls,
};
use crate::diagnostic::{
    Confidence, Diagnostic, DiagnosticCode, EffectRole, PathCertainty, Policy, Severity,
};
use crate::ir::*;
use crate::location::Location;

fn loc(line: u32) -> Location {
    Location {
        line,
        column: 3,
        end_line: line,
        end_column: 20,
        file: None,
    }
}

fn recognition(receiver: &str, basis: RecognitionBasis) -> Recognition {
    Recognition {
        basis,
        receiver: receiver.to_owned(),
        detail: "test".to_owned(),
    }
}

#[derive(Default)]
struct Builder {
    next_effect: u32,
    transactions: Vec<TransactionInfo>,
}

impl Builder {
    fn effect(&mut self, line: u32, label: &str, kind: EffectKind, execution: Execution) -> Node {
        let id = EffectId(self.next_effect);
        self.next_effect += 1;
        let recognition = match &kind {
            EffectKind::Unknown { .. } => None,
            EffectKind::DatabaseWrite {
                handle: DbHandle::Transaction { .. },
                ..
            } => Some(recognition("tx", RecognitionBasis::TransactionHandle)),
            EffectKind::External { .. } => {
                Some(recognition("queue", RecognitionBasis::ConfiguredName))
            }
            _ => Some(recognition("db", RecognitionBasis::ConfiguredName)),
        };
        Node::Effect(Effect {
            id,
            kind,
            label: label.into(),
            location: loc(line),
            execution,
            recognition: recognition.map(Box::new),
            summary: None,
            references: Box::default(),
            provenance: None,
        })
    }

    fn write(&mut self, line: u32, table: &str) -> Node {
        self.effect(
            line,
            &format!("db.update({table})"),
            EffectKind::DatabaseWrite {
                operation: WriteOperation::Update,
                table: Some(table.to_owned()),
                handle: DbHandle::Root {
                    name: "db".to_owned(),
                },
            },
            Execution::Awaited,
        )
    }

    fn tx_write(&mut self, line: u32, table: &str, tx: TransactionId) -> Node {
        self.effect(
            line,
            &format!("tx.insert({table})"),
            EffectKind::DatabaseWrite {
                operation: WriteOperation::Insert,
                table: Some(table.to_owned()),
                handle: DbHandle::Transaction { id: tx },
            },
            Execution::Awaited,
        )
    }

    fn publish(&mut self, line: u32) -> Node {
        self.publish_with(line, Execution::Awaited)
    }

    fn publish_with(&mut self, line: u32, execution: Execution) -> Node {
        self.effect(
            line,
            "queue.publish(\"order.completed\")",
            EffectKind::External {
                category: ExternalCategory::MessagePublish,
                best_effort: false,
                recovery: None,
            },
            execution,
        )
    }

    fn unknown(&mut self, line: u32) -> Node {
        self.effect(
            line,
            "unknownService.process(orderId)",
            EffectKind::Unknown {
                reason: UnknownReason::UnrecognizedCall,
                detail: "test".to_owned(),
            },
            Execution::Awaited,
        )
    }

    fn begin(&mut self, line: u32, parent: Option<TransactionId>) -> TransactionId {
        let id = TransactionId(u32::try_from(self.transactions.len()).unwrap());
        self.transactions.push(TransactionInfo {
            id,
            parent,
            label: "db.transaction(…)".to_owned(),
            location: loc(line),
            handle: Some("tx".to_owned()),
            recognition: recognition("db", RecognitionBasis::ConfiguredName),
            summary: None,
            parent_parameter: None,
        });
        id
    }

    fn finish(self, body: Vec<Node>) -> FunctionIr {
        FunctionIr {
            name: "completeOrder".to_owned(),
            location: loc(1),
            is_async: true,
            transactions: self.transactions,
            body,
            returns: None,
            fact_inputs: Default::default(),
        }
    }
}

fn scope(transaction: TransactionId, body: Vec<Node>) -> Node {
    Node::Transaction(TransactionScope {
        transaction,
        execution: Execution::Awaited,
        body,
    })
}

fn branch(line: u32, then: Vec<Node>, otherwise: Vec<Node>) -> Node {
    Node::Branch(Branch {
        kind: BranchKind::If,
        condition: "cond".to_owned(),
        location: loc(line),
        arms: vec![
            Arm::new("then".to_owned(), then),
            Arm::new("else".to_owned(), otherwise),
        ],
    })
}

fn exit(kind: ExitKind, line: u32) -> Node {
    Node::Exit(Exit {
        kind,
        location: loc(line),
        transaction: None,
        detail: None,

        facts: Vec::new(),
    })
}

fn analyze(function: &FunctionIr) -> Vec<Diagnostic> {
    analyze_function(function, "test.ts", &AnalysisOptions::default())
}

fn codes(diagnostics: &[Diagnostic]) -> Vec<&'static str> {
    diagnostics.iter().map(|d| d.code.as_str()).collect()
}

#[test]
fn db_commit_then_publish_is_cp1001() {
    let mut b = Builder::default();
    let body = vec![b.write(2, "orders"), b.publish(3)];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    let d = &diagnostics[0];
    assert_eq!(d.confidence, Confidence::High);
    assert_eq!(d.location.line, 3, "points at the external effect");
    assert!(d.path_conditions.is_empty());
    assert!(!d.reproduced);
    assert_eq!(d.failure_sequence.len(), 4);
}

#[test]
fn publish_then_db_commit_is_cp1002() {
    let mut b = Builder::default();
    let body = vec![b.publish(2), b.write(3, "orders")];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1002"]);
    assert_eq!(diagnostics[0].location.line, 2);
}

#[test]
fn writes_in_one_transaction_are_atomic() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(
        tx,
        vec![b.tx_write(3, "orders", tx), b.tx_write(4, "outbox", tx)],
    )];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn publish_inside_transaction_is_cp1003_only() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(tx, vec![b.tx_write(3, "orders", tx), b.publish(4)])];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1003"]);
    assert_eq!(diagnostics[0].confidence, Confidence::High);
}

#[test]
fn publish_inside_transaction_without_writes_is_not_cp1003() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(tx, vec![b.publish(3)])];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn transaction_commit_then_publish_is_cp1001() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(tx, vec![b.tx_write(3, "orders", tx)]), b.publish(5)];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert!(
        diagnostics[0]
            .message
            .contains("the transaction started at 2:3")
    );
}

#[test]
fn root_handle_inside_transaction_bypasses_it() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(
        tx,
        vec![b.tx_write(3, "orders", tx), b.write(4, "audit")],
    )];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1004"]);
    assert!(
        diagnostics[0]
            .mitigation
            .contains("transaction handle `tx`")
    );
}

#[test]
fn independent_db_commits_are_cp1004() {
    let mut b = Builder::default();
    let body = vec![b.write(2, "orders"), b.write(3, "payments")];
    assert_eq!(codes(&analyze(&b.finish(body))), ["MW1004"]);
}

#[test]
fn only_adjacent_points_are_paired() {
    let mut b = Builder::default();
    let body = vec![b.write(2, "orders"), b.publish(3), b.write(4, "payments")];
    // write→publish and publish→write; not write→write across the publish.
    assert_eq!(codes(&analyze(&b.finish(body))), ["MW1001", "MW1002"]);
}

#[test]
fn nested_transaction_writes_commit_with_the_root() {
    let mut b = Builder::default();
    let outer = b.begin(2, None);
    let inner = b.begin(3, Some(outer));
    let body = vec![scope(
        outer,
        vec![
            scope(inner, vec![b.tx_write(4, "orders", inner)]),
            b.publish(6),
        ],
    )];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1003"]);
}

#[test]
fn unknown_after_commit_is_only_a_limitation() {
    let mut b = Builder::default();
    let body = vec![b.write(2, "orders"), b.unknown(3)];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW9001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::Low);
    assert!(diagnostics[0].message.contains("has committed"));
}

#[test]
fn unknown_before_any_commit_is_silent() {
    let mut b = Builder::default();
    let body = vec![b.unknown(2), b.write(3, "orders")];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn unknown_inside_transaction_with_writes_is_reported() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(tx, vec![b.tx_write(3, "orders", tx), b.unknown(4)])];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW9001"]);
    assert!(diagnostics[0].message.contains("inside the transaction"));
}

#[test]
fn unknown_without_recognized_effects_is_silent() {
    let mut b = Builder::default();
    let body = vec![b.unknown(2)];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn mutually_exclusive_branches_do_not_pair() {
    let mut b = Builder::default();
    let then = vec![b.write(3, "orders")];
    let otherwise = vec![b.publish(5)];
    let body = vec![branch(2, then, otherwise)];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn early_return_between_effects_lowers_confidence() {
    let mut b = Builder::default();
    let first = b.write(2, "orders");
    let then = vec![exit(ExitKind::Return, 4)];
    let last = b.publish(6);
    let body = vec![first, branch(3, then, vec![]), last];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
    assert_eq!(diagnostics[0].path_conditions.len(), 1);
    assert!(
        diagnostics[0].path_conditions[0]
            .description
            .contains("`else`")
    );
}

#[test]
fn return_after_first_effect_prevents_pairing() {
    let mut b = Builder::default();
    let then = vec![b.write(3, "orders"), exit(ExitKind::Return, 4)];
    let last = b.publish(6);
    let body = vec![branch(2, then, vec![]), last];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn condition_before_both_effects_keeps_high_confidence() {
    let mut b = Builder::default();
    let body_then = vec![exit(ExitKind::Return, 3)];
    let body = vec![
        branch(2, body_then, vec![]),
        b.write(5, "orders"),
        b.publish(6),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::High);
}

#[test]
fn rollback_in_transaction_still_reports_cp1003() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(
        tx,
        vec![
            b.tx_write(3, "orders", tx),
            b.publish(4),
            branch(5, vec![exit(ExitKind::Rollback, 6)], vec![]),
        ],
    )];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1003"]);
}

#[test]
fn loops_are_modeled_once_with_medium_confidence() {
    let mut b = Builder::default();
    let body_loop = vec![b.write(3, "items")];
    let body = vec![
        Node::Loop(Loop {
            kind: LoopKind::ForOf,
            header: "item of items".to_owned(),
            location: loc(2),
            runs_at_least_once: false,
            body: body_loop,
        }),
        b.publish(5),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
    assert!(
        diagnostics[0]
            .limitations
            .iter()
            .any(|l| l.contains("loop"))
    );
}

#[test]
fn fire_and_forget_lowers_confidence() {
    let mut b = Builder::default();
    let body = vec![
        b.write(2, "orders"),
        b.publish_with(3, Execution::FireAndForget),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
}

#[test]
fn switch_fall_through_is_followed() {
    let mut b = Builder::default();
    let first = b.write(3, "orders");
    let second = b.publish(5);
    let body = vec![Node::Switch(Switch {
        discriminant: "kind".to_owned(),
        location: loc(2),
        cases: vec![
            Arm::new("case \"a\"".to_owned(), vec![first]),
            Arm::new(
                "case \"b\"".to_owned(),
                vec![second, exit(ExitKind::Break, 6)],
            ),
        ],
        has_default: false,
    })];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(
        codes(&diagnostics),
        ["MW1001"],
        "case a falls through into case b"
    );
}

#[test]
fn catch_handler_effects_are_analyzed_with_low_confidence() {
    let mut b = Builder::default();
    let before = b.write(2, "orders");
    let tried = b.unknown(4);
    let handled = b.publish(6);
    let body = vec![
        before,
        Node::Try(Try {
            location: loc(3),
            block: vec![tried],
            handler: Some(Handler {
                location: loc(5),
                body: vec![handled],
            }),
            finalizer: None,
        }),
    ];
    let diagnostics = analyze(&b.finish(body));
    let cp1001 = diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::DbCommitThenExternal)
        .expect("write before try, publish in catch");
    assert_eq!(cp1001.confidence, Confidence::Low);
}

#[test]
fn path_explosion_is_bounded_and_reported() {
    let mut b = Builder::default();
    let mut body = Vec::new();
    for line in 0..12 {
        let then = vec![b.write(line * 2 + 2, "t")];
        body.push(branch(line * 2 + 1, then, vec![]));
    }
    let function = b.finish(body);
    let diagnostics = analyze_function(
        &function,
        "test.ts",
        &AnalysisOptions {
            max_paths: 64,
            ..AnalysisOptions::default()
        },
    );
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::AnalysisIncomplete && d.message.contains("64"))
    );
}

#[test]
fn unsupported_exit_is_reported_when_effects_exist() {
    let mut b = Builder::default();
    let body = vec![
        b.write(2, "orders"),
        Node::Exit(Exit {
            kind: ExitKind::Unsupported,
            location: loc(3),
            transaction: None,
            detail: Some("labeled break".to_owned()),

            facts: Vec::new(),
        }),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW9001"]);
    assert!(diagnostics[0].message.contains("labeled break"));
}

#[test]
fn output_is_deterministic() {
    let build = || {
        let mut b = Builder::default();
        let body = vec![b.write(2, "a"), b.publish(3), b.write(4, "b"), b.publish(5)];
        b.finish(body)
    };
    let first = serde_json::to_string(&analyze(&build())).unwrap();
    let second = serde_json::to_string(&analyze(&build())).unwrap();
    assert_eq!(first, second);
}

// --- Same-file call summaries ------------------------------------------------

/// A caller `[write, call]` (plus `after`) of `callee`, inlined.
fn inline(callee: FunctionIr, after: impl FnOnce(&mut Builder) -> Vec<Node>) -> FunctionIr {
    let mut b = Builder::default();
    let mut body = vec![b.write(10, "orders"), b.unknown(11)];
    body.extend(after(&mut b));
    let mut caller = b.finish(body);
    caller.name = "caller".to_owned();
    let mut functions = vec![callee, caller];
    let calls = vec![
        Vec::new(),
        vec![LocalCall {
            effect: EffectId(1),
            callee: Some(0),
            placeholder: false,
            arguments: Vec::new(),
            ..LocalCall::new(EffectId(0), None, false)
        }],
    ];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    functions.pop().unwrap()
}

fn notify() -> FunctionIr {
    let mut b = Builder::default();
    let body = vec![b.publish(3)];
    let mut function = b.finish(body);
    function.name = "notify".to_owned();
    function
}

#[test]
fn inlined_effects_pair_with_the_callers_and_report_at_the_call() {
    let caller = inline(notify(), |_| Vec::new());
    assert!(
        matches!(caller.body[1], Node::Call(_)),
        "{}",
        caller.to_text()
    );
    let diagnostics = analyze(&caller);
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::High);
    assert_eq!(diagnostics[0].location.line, 11, "reported at the call");
    assert!(
        diagnostics[0]
            .effects
            .iter()
            .any(|e| e.location.line == 3 && e.role == EffectRole::NotYetPerformed)
    );
}

#[test]
fn a_return_in_the_callee_resumes_the_caller() {
    let mut b = Builder::default();
    let body = vec![
        branch(2, vec![exit(ExitKind::Return, 2)], Vec::new()),
        b.publish(3),
    ];
    let callee = b.finish(body);
    let caller = inline(callee, |b| vec![b.write(12, "audit")]);
    let effects = caller.effects();
    let publish = effects
        .iter()
        .find(|e| e.summary.is_some())
        .expect("inlined publish");
    assert!(!publish.summary.as_ref().unwrap().on_every_path);
    let diagnostics = analyze(&caller);
    // The early return skips the publish; the caller continues to its
    // second write on that path.
    assert_eq!(codes(&diagnostics), ["MW1001", "MW1002", "MW1004"]);
    for d in &diagnostics {
        assert_eq!(d.confidence, Confidence::Medium, "{d:#?}");
    }
}

#[test]
fn recursive_calls_are_kept_as_unknown_effects() {
    let mut b = Builder::default();
    let body = vec![b.publish(2), b.unknown(3)];
    let mut functions = vec![b.finish(body)];
    let calls = vec![vec![LocalCall {
        effect: EffectId(1),
        callee: Some(0),
        placeholder: false,
        arguments: Vec::new(),
        ..LocalCall::new(EffectId(0), None, false)
    }]];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    let effects = functions[0].effects();
    assert_eq!(effects.len(), 2);
    assert!(
        matches!(&effects[1].kind, EffectKind::Unknown { detail, .. } if detail.contains("recursive")),
        "{}",
        functions[0].to_text()
    );
}

#[test]
fn placeholders_disappear_when_the_callee_is_not_summarized() {
    let mut b = Builder::default();
    let body = vec![b.unknown(2)];
    let callee = b.finish(body);
    let mut b = Builder::default();
    let body = vec![b.write(10, "orders"), b.unknown(11)];
    let mut functions = vec![callee, b.finish(body)];
    let calls = vec![
        Vec::new(),
        vec![LocalCall {
            effect: EffectId(1),
            callee: Some(0),
            placeholder: true,
            arguments: Vec::new(),
            ..LocalCall::new(EffectId(0), None, false)
        }],
    ];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    assert_eq!(
        functions[1].effects().len(),
        1,
        "{}",
        functions[1].to_text()
    );
}

#[test]
fn inlined_transactions_are_renumbered() {
    let mut b = Builder::default();
    let tx = b.begin(2, None);
    let body = vec![scope(tx, vec![b.tx_write(3, "orders", tx)])];
    let callee = b.finish(body);
    let mut b = Builder::default();
    let outer = b.begin(9, None);
    let body = vec![scope(
        outer,
        vec![b.tx_write(10, "payments", outer), b.unknown(11)],
    )];
    let mut functions = vec![callee, b.finish(body)];
    let calls = vec![
        Vec::new(),
        vec![LocalCall {
            effect: EffectId(1),
            callee: Some(0),
            placeholder: false,
            arguments: Vec::new(),
            ..LocalCall::new(EffectId(0), None, false)
        }],
    ];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    let caller = &functions[1];
    assert_eq!(caller.transactions.len(), 2);
    assert_eq!(caller.transactions[1].id, TransactionId(1));
    assert!(
        caller.transactions[1]
            .summary
            .as_ref()
            .unwrap()
            .on_every_path
    );
    let inlined = caller.effects();
    assert!(matches!(
        inlined[1].kind,
        EffectKind::DatabaseWrite {
            handle: DbHandle::Transaction {
                id: TransactionId(1)
            },
            ..
        }
    ));
    // The callee's transaction is independent of the caller's.
    let diagnostics = analyze(caller);
    assert_eq!(codes(&diagnostics), ["MW1004"]);
}

// --- Phase 2: path feasibility ----------------------------------------------

fn truthy(variable: &str, value: bool) -> Fact {
    Fact::Truthy {
        variable: variable.to_owned(),
        value,
    }
}

/// `if (<then facts>) { then } else { otherwise }`.
fn guarded(line: u32, facts: [Vec<Fact>; 2], then: Vec<Node>, otherwise: Vec<Node>) -> Node {
    let [then_facts, else_facts] = facts;
    Node::Branch(Branch {
        kind: BranchKind::If,
        condition: "flag".to_owned(),
        location: loc(line),
        arms: vec![
            Arm {
                label: "then".to_owned(),
                body: then,
                facts: then_facts,
            },
            Arm {
                label: "else".to_owned(),
                body: otherwise,
                facts: else_facts,
            },
        ],
    })
}

#[test]
fn contradictory_branches_are_eliminated() {
    // if (flag) write; if (!flag) publish;
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(6);
    let body = vec![
        guarded(
            2,
            [vec![truthy("flag#1", true)], vec![truthy("flag#1", false)]],
            vec![write],
            Vec::new(),
        ),
        guarded(
            5,
            [vec![truthy("flag#1", false)], vec![truthy("flag#1", true)]],
            vec![publish],
            Vec::new(),
        ),
    ];
    let function = b.finish(body);
    let result = analyze_function_in(
        &function,
        crate::location::Sources::single("test.ts"),
        &AnalysisOptions::default(),
    );
    assert!(result.diagnostics.is_empty(), "{:#?}", result.diagnostics);
    assert_eq!(result.paths, 2, "only the two consistent paths remain");
    assert_eq!(result.pruned, 2);
}

#[test]
fn compatible_branches_are_preserved() {
    // if (flag) write; if (flag) publish; -> the path taking both remains.
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(6);
    let body = vec![
        guarded(
            2,
            [vec![truthy("flag#1", true)], vec![truthy("flag#1", false)]],
            vec![write],
            Vec::new(),
        ),
        guarded(
            5,
            [vec![truthy("flag#1", true)], vec![truthy("flag#1", false)]],
            vec![publish],
            Vec::new(),
        ),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
    assert_eq!(diagnostics[0].path_certainty, PathCertainty::Conditional);
}

#[test]
fn conditions_without_facts_stay_feasible() {
    // Two branches on unknown conditions: every combination is kept.
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(6);
    let body = vec![
        branch(2, vec![write], Vec::new()),
        branch(5, vec![publish], Vec::new()),
    ];
    let diagnostics = analyze(&b.finish(body));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
}

#[test]
fn facts_on_different_variables_do_not_interact() {
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(6);
    let body = vec![
        guarded(
            2,
            [vec![truthy("a#1", true)], vec![truthy("a#1", false)]],
            vec![write],
            Vec::new(),
        ),
        guarded(
            5,
            [vec![truthy("b#2", false)], vec![truthy("b#2", true)]],
            vec![publish],
            Vec::new(),
        ),
    ];
    assert_eq!(codes(&analyze(&b.finish(body))), ["MW1001"]);
}

#[test]
fn literal_equalities_are_mutually_exclusive() {
    let equals = |value: &str| Fact::Equals {
        variable: "status#3".to_owned(),
        literal: Literal::String(value.to_owned()),
    };
    let differs = |value: &str| Fact::NotEquals {
        variable: "status#3".to_owned(),
        literal: Literal::String(value.to_owned()),
    };
    assert!(equals("paid").contradicts(&equals("refunded")));
    assert!(!equals("paid").contradicts(&equals("paid")));
    assert!(equals("paid").contradicts(&differs("paid")));
    assert!(!equals("paid").contradicts(&differs("refunded")));
    // `status === ""` implies a falsy value.
    assert!(equals("").contradicts(&truthy("status#3", true)));
    assert!(equals("paid").contradicts(&truthy("status#3", false)));
    assert!(Fact::Never.contradicts(&truthy("x", true)));

    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(6);
    let body = vec![
        guarded(
            2,
            [vec![equals("paid")], vec![differs("paid")]],
            vec![write],
            Vec::new(),
        ),
        guarded(
            5,
            [vec![equals("refunded")], vec![differs("refunded")]],
            vec![publish],
            Vec::new(),
        ),
    ];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn never_arms_are_not_taken() {
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(5);
    let body = vec![
        write,
        guarded(
            4,
            [vec![Fact::Never], Vec::new()],
            vec![publish],
            Vec::new(),
        ),
    ];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn switch_cases_imply_equalities() {
    let case = |value: &str, body: Vec<Node>| Arm {
        label: format!("case {value:?}"),
        body,
        facts: vec![Fact::Equals {
            variable: "kind#1".to_owned(),
            literal: Literal::String(value.to_owned()),
        }],
    };
    let mut b = Builder::default();
    let write = b.write(3, "orders");
    let publish = b.publish(8);
    let body = vec![
        Node::Switch(Switch {
            discriminant: "kind".to_owned(),
            location: loc(2),
            cases: vec![case("a", vec![write, exit(ExitKind::Break, 4)])],
            has_default: false,
        }),
        // if (kind === "b") publish: contradicts entering case "a".
        guarded(
            7,
            [
                vec![Fact::Equals {
                    variable: "kind#1".to_owned(),
                    literal: Literal::String("b".to_owned()),
                }],
                Vec::new(),
            ],
            vec![publish],
            Vec::new(),
        ),
    ];
    assert!(analyze(&b.finish(body)).is_empty());
}

#[test]
fn reads_do_not_fork_paths() {
    let mut b = Builder::default();
    let read = |b: &mut Builder, line| {
        b.effect(
            line,
            "db.select().from(orders)",
            EffectKind::DatabaseRead {
                handle: DbHandle::Root {
                    name: "db".to_owned(),
                },
                table: Some("orders".to_owned()),
            },
            Execution::Awaited,
        )
    };
    let then = read(&mut b, 3);
    let otherwise = read(&mut b, 4);
    let write = b.write(5, "orders");
    let function = b.finish(vec![branch(2, vec![then], vec![otherwise]), write]);
    let result = analyze_function_in(
        &function,
        crate::location::Sources::single("test.ts"),
        &AnalysisOptions::default(),
    );
    assert_eq!(result.paths, 1);
}

// --- Phase 2: parameters, memoization, files --------------------------------

/// `async function save(tx) { await tx.update(orders) }`
fn save_on_parameter(configured: bool) -> FunctionIr {
    let mut b = Builder::default();
    let write = b.effect(
        3,
        "tx.update(orders)",
        EffectKind::DatabaseWrite {
            operation: WriteOperation::Update,
            table: Some("orders".to_owned()),
            handle: DbHandle::Parameter {
                index: 0,
                name: "tx".to_owned(),
                configured,
            },
        },
        Execution::Awaited,
    );
    let mut function = b.finish(vec![write]);
    function.name = "save".to_owned();
    function
}

fn tx_argument(id: TransactionId) -> CallArgument {
    CallArgument {
        handle: DbHandle::Transaction { id },
        recognition: recognition("tx", RecognitionBasis::TransactionHandle),
        label: "tx".to_owned(),
    }
}

fn root_argument() -> CallArgument {
    CallArgument {
        handle: DbHandle::Root {
            name: "db".to_owned(),
        },
        recognition: recognition("db", RecognitionBasis::ConfiguredName),
        label: "db".to_owned(),
    }
}

/// `db.transaction(async (tx) => { await tx.insert(payments); await save(<argument>) })`
fn caller_in_transaction(callee: FunctionIr, argument: Option<CallArgument>) -> FunctionIr {
    let mut b = Builder::default();
    let tx = b.begin(9, None);
    let body = vec![scope(
        tx,
        vec![b.tx_write(10, "payments", tx), b.unknown(11)],
    )];
    let mut functions = vec![callee, b.finish(body)];
    let calls = vec![
        Vec::new(),
        vec![LocalCall {
            effect: EffectId(1),
            callee: Some(0),
            placeholder: false,
            arguments: vec![argument],
            ..LocalCall::new(EffectId(0), None, false)
        }],
    ];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    functions.pop().unwrap()
}

#[test]
fn a_transaction_handle_argument_joins_the_callers_transaction() {
    for configured in [false, true] {
        let caller = caller_in_transaction(
            save_on_parameter(configured),
            Some(tx_argument(TransactionId(0))),
        );
        let effects = caller.effects();
        assert!(
            matches!(
                effects[1].kind,
                EffectKind::DatabaseWrite {
                    handle: DbHandle::Transaction {
                        id: TransactionId(0)
                    },
                    ..
                }
            ),
            "{}",
            caller.to_text()
        );
        assert!(analyze(&caller).is_empty(), "{:#?}", analyze(&caller));
    }
}

#[test]
fn a_root_handle_argument_bypasses_the_callers_transaction() {
    let caller = caller_in_transaction(save_on_parameter(false), Some(root_argument()));
    let diagnostics = analyze(&caller);
    assert_eq!(codes(&diagnostics), ["MW1004"]);
    assert_eq!(diagnostics[0].location.line, 11, "reported at the call");
    assert_eq!(
        diagnostics[0].severity,
        Severity::Warning,
        "a specific bypass"
    );
}

#[test]
fn an_unrecognized_argument_is_never_guessed_to_be_a_handle() {
    // The configured parameter name alone would say "root `tx`", which would
    // claim a bypass; the argument is unknown, so the call stays unknown.
    let caller = caller_in_transaction(save_on_parameter(true), None);
    let effects = caller.effects();
    assert!(
        matches!(&effects[1].kind, EffectKind::Unknown { detail, .. } if detail.contains("passes no recognized database or transaction handle")),
        "{}",
        caller.to_text()
    );
    let diagnostics = analyze(&caller);
    assert_eq!(codes(&diagnostics), ["MW9001"]);
}

#[test]
fn a_function_analyzed_alone_treats_parameters_conservatively() {
    // Unconfigured parameter: an unknown call, no hazard.
    let mut function = save_on_parameter(false);
    let mut b = Builder::default();
    let publish = b.publish(4);
    if let Node::Effect(mut effect) = publish {
        effect.id = EffectId(1);
        function.body.push(Node::Effect(effect));
    }
    assert!(codes(&analyze(&function)).iter().all(|c| *c == "MW9001"));
    // Configured parameter (named like `db`): the root handle, as in Phase 1.
    let mut function = save_on_parameter(true);
    let mut b = Builder::default();
    if let Node::Effect(mut effect) = b.publish(4) {
        effect.id = EffectId(1);
        function.body.push(Node::Effect(effect));
    }
    assert_eq!(codes(&analyze(&function)), ["MW1001"]);
}

#[test]
fn summaries_are_reused_across_callers() {
    let callee = notify();
    let mut functions = vec![callee];
    let mut calls = vec![Vec::new()];
    for _ in 0..3 {
        let mut b = Builder::default();
        functions.push({
            let body = vec![b.write(10, "orders"), b.unknown(11)];
            b.finish(body)
        });
        calls.push(vec![LocalCall::new(EffectId(1), Some(0), false)]);
    }
    let stats = inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    assert_eq!(stats.summarized, 3);
    assert_eq!(stats.expansions, 1);
    assert_eq!(stats.reused, 2);
    let first = serde_json::to_string(&functions[1]).unwrap();
    for function in &functions[2..] {
        assert_eq!(serde_json::to_string(function).unwrap(), first);
    }
}

#[test]
fn callee_early_returns_do_not_multiply_caller_paths() {
    // A callee with three guard clauses (`if (...) return;`) and no effect
    // of its own, called four times: without merging, 4^4 paths.
    let guard = || branch(2, vec![exit(ExitKind::Return, 2)], Vec::new());
    let mut b = Builder::default();
    let publish = b.publish(5);
    let callee = b.finish(vec![guard(), guard(), guard(), publish]);
    let mut b = Builder::default();
    let mut body = vec![b.write(10, "orders")];
    for line in 11..15 {
        body.push(b.unknown(line));
    }
    let mut functions = vec![callee, b.finish(body)];
    let calls = vec![
        Vec::new(),
        (1..5)
            .map(|id| LocalCall::new(EffectId(id), Some(0), false))
            .collect(),
    ];
    inline_local_calls(&mut functions, &calls, &AnalysisOptions::default());
    let result = analyze_function_in(
        &functions[1],
        crate::location::Sources::single("test.ts"),
        &AnalysisOptions::default(),
    );
    assert!(!result.truncated);
    // Each call either publishes or returns early: 2^4 paths, not 4^4.
    assert_eq!(result.paths, 16);
    assert!(result.merged > 0);
}

#[test]
fn operations_inlined_from_another_file_keep_their_file() {
    let mut functions = vec![notify(), {
        let mut b = Builder::default();
        {
            let body = vec![b.write(10, "orders"), b.unknown(11)];
            b.finish(body)
        }
    }];
    let calls = vec![
        Vec::new(),
        vec![LocalCall::new(EffectId(1), Some(0), false)],
    ];
    let names = vec!["lib/notify.ts".to_owned(), "app/route.ts".to_owned()];
    inline_calls(
        &mut functions,
        &calls,
        Some(ProjectFiles {
            ids: &[crate::location::FileId(0), crate::location::FileId(1)],
            names: &names,
        }),
        &AnalysisOptions::default(),
    );
    let caller = &functions[1];
    // The caller's file is 1 and the callee's 0: resolve against that table.
    let others = names.clone();
    let result = analyze_function_in(
        caller,
        crate::location::Sources {
            file: "app/route.ts",
            others: &others,
        },
        &AnalysisOptions::default(),
    );
    let d = &result.diagnostics[0];
    assert_eq!(d.code, DiagnosticCode::DbCommitThenExternal);
    assert_eq!(d.location.file, "app/route.ts", "reported at the call");
    let publish = d
        .effects
        .iter()
        .find(|e| e.role == EffectRole::NotYetPerformed)
        .unwrap();
    assert_eq!(publish.location.file, "lib/notify.ts");
    assert!(
        d.evidence.iter().any(|s| s.contains("(lib/notify.ts:3:3)")),
        "{:#?}",
        d.evidence
    );
    assert!(
        d.effects.iter().any(|e| e.role == EffectRole::CallSite
            && e.description.contains("declared in lib/notify.ts"))
    );
}

// --- Phase 2: policies, confidence, fingerprints ----------------------------

#[test]
fn recommended_policy_reports_sequential_writes_as_review_candidates() {
    let mut b = Builder::default();
    let function = {
        let body = vec![b.write(2, "orders"), b.write(3, "payments")];
        b.finish(body)
    };
    let recommended = analyze(&function);
    assert_eq!(codes(&recommended), ["MW1004"]);
    assert_eq!(recommended[0].severity, Severity::Info);
    let strict = analyze_function(
        &function,
        "test.ts",
        &AnalysisOptions {
            policy: Policy::Strict,
            ..AnalysisOptions::default()
        },
    );
    assert_eq!(strict[0].severity, Severity::Warning);
    assert_eq!(
        strict[0].code, recommended[0].code,
        "the code does not change"
    );
}

#[test]
fn member_path_evidence_caps_confidence() {
    let mut b = Builder::default();
    let write = b.write(2, "orders");
    let mut publish = b.publish(3);
    if let Node::Effect(effect) = &mut publish {
        effect.recognition = Some(Box::new(recognition(
            "this.queue",
            RecognitionBasis::ConfiguredMemberPath,
        )));
    }
    let diagnostics = analyze(&b.finish(vec![write, publish]));
    assert_eq!(diagnostics[0].confidence, Confidence::Medium);
    assert_eq!(diagnostics[0].path_certainty, PathCertainty::StraightLine);
    assert_eq!(
        diagnostics[0].evidence_strength,
        EvidenceStrength::MemberPath
    );
}

#[test]
fn alternative_predecessors_are_reported_once() {
    // if (c) write orders else write payments; publish
    let mut b = Builder::default();
    let a = b.write(3, "orders");
    let c = b.write(5, "payments");
    let publish = b.publish(7);
    let diagnostics = analyze(&b.finish(vec![branch(2, vec![a], vec![c]), publish]));
    assert_eq!(codes(&diagnostics), ["MW1001"]);
    assert!(
        diagnostics[0]
            .limitations
            .iter()
            .any(|l| l.contains("On other paths a different operation precedes it")),
        "{:#?}",
        diagnostics[0].limitations
    );
}

#[test]
fn fingerprints_do_not_depend_on_lines() {
    let build = |offset: u32| {
        let mut b = Builder::default();
        {
            let body = vec![b.write(2 + offset, "orders"), b.publish(3 + offset)];
            b.finish(body)
        }
    };
    let first = analyze(&build(0));
    let moved = analyze(&build(40));
    assert_ne!(first[0].location.line, moved[0].location.line);
    assert_eq!(first[0].fingerprint, moved[0].fingerprint);
    assert_eq!(first[0].fingerprint.len(), 16);
    let mut b = Builder::default();
    let other = analyze(&{
        let body = vec![b.write(2, "payments"), b.publish(3)];
        b.finish(body)
    });
    assert_ne!(first[0].fingerprint, other[0].fingerprint);
}
