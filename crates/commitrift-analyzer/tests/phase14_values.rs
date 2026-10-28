//! Phase 14 Track B: lowering of the JavaScript value domain
//! (`ExtractRequest::js_values`) on the first-party static application
//! `fixtures/phase14/values-app`, end-to-end model checking of scenarios
//! whose verdict depends on string and array operations, and the guarantee
//! that without the flag nothing changes (extraction and state bytes).

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use commitrift_analyzer::TsActorProvider;
use commitrift_model::engine::{Recorder, apply, enabled, initial_state};
use commitrift_model::explore::{Verdict, explore};
use commitrift_model::ir::{Op, StmtKind};
use commitrift_model::provider::{ActorProvider, ExtractRequest, Extracted, TableBinding};
use commitrift_model::value::{Builtin, Expr, Value};

fn app() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/phase14/values-app")
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tables() -> Vec<TableBinding> {
    let t = |name: &str, columns: &[&str]| TableBinding {
        identifier: name.to_owned(),
        table: name.to_owned(),
        columns: columns.iter().map(|c| (*c).to_owned()).collect(),
        source: None,
    };
    vec![
        t("posts", &["id", "tags"]),
        t("members", &["id", "teamId", "email", "role"]),
        t("handles", &["id", "handle"]),
        t("audit", &["id", "note"]),
    ]
}

fn extract(selector: &str, js_values: bool) -> Extracted {
    TsActorProvider::new(app())
        .extract(&ExtractRequest {
            root: app(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables: tables(),
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some(selector.to_owned()),
            contract: Some(Default::default()),
            replicas: false,
            replica_factories: Vec::new(),
            return_values: false,
            js_values,
        })
        .unwrap_or_else(|e| panic!("{selector}: {e}"))
}

fn unknowns(x: &Extracted) -> Vec<String> {
    let mut out = Vec::new();
    for step in x.codes.iter().flatten() {
        match &step.op {
            Op::Unknown { detail, .. } => out.push(detail.clone()),
            Op::Let { expr, .. } | Op::JumpIf { cond: expr, .. } => {
                if let Some(d) = expr.unknown_detail() {
                    out.push(d.to_owned());
                }
            }
            Op::Db { stmt } => {
                let exprs: Vec<&Expr> = match &stmt.kind {
                    StmtKind::Insert { values, .. } => values.iter().map(|(_, e)| e).collect(),
                    StmtKind::Update { pred, set, .. } => std::iter::once(pred)
                        .chain(set.iter().map(|(_, e)| e))
                        .collect(),
                    StmtKind::Select { pred, .. }
                    | StmtKind::SelectCount { pred, .. }
                    | StmtKind::Delete { pred, .. } => vec![pred],
                };
                out.extend(
                    exprs
                        .iter()
                        .filter_map(|e| e.unknown_detail())
                        .map(str::to_owned),
                );
            }
            _ => {}
        }
    }
    out
}

/// Every expression of the extracted code, rendered.
fn rendered(x: &Extracted) -> String {
    let mut out = Vec::new();
    for step in x.codes.iter().flatten() {
        match &step.op {
            Op::Let { local, expr } => out.push(format!("{local} = {}", expr.render())),
            Op::JumpIf { cond, .. } => out.push(format!("if {}", cond.render())),
            Op::Db { stmt } => out.push(format!("{:?}", stmt.kind)),
            Op::Unknown { detail, .. } => out.push(format!("? {detail}")),
            other => out.push(format!("{other:?}")),
        }
    }
    out.join("\n")
}

fn calls(x: &Extracted) -> HashSet<Builtin> {
    fn walk(e: &Expr, out: &mut HashSet<Builtin>) {
        match e {
            Expr::Call { func, args } => {
                out.insert(*func);
                args.iter().for_each(|a| walk(a, out));
            }
            Expr::Each { list, body, .. } => {
                walk(list, out);
                walk(body, out);
            }
            Expr::Field(b, _) | Expr::Not(b) | Expr::IsNull(b) => walk(b, out),
            Expr::Bin(_, a, b) => {
                walk(a, out);
                walk(b, out);
            }
            Expr::Object(f) => f.iter().for_each(|(_, v)| walk(v, out)),
            Expr::Array(items) => items.iter().for_each(|v| walk(v, out)),
            _ => {}
        }
    }
    let mut out = HashSet::new();
    for step in x.codes.iter().flatten() {
        match &step.op {
            Op::Let { expr, .. } | Op::JumpIf { cond: expr, .. } => walk(expr, &mut out),
            Op::Db { stmt } => {
                if let StmtKind::Update { set, .. } = &stmt.kind {
                    set.iter().for_each(|(_, e)| walk(e, &mut out));
                }
                if let StmtKind::Insert { values, .. } = &stmt.kind {
                    values.iter().for_each(|(_, e)| walk(e, &mut out));
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Without the flag: Phase 13 extraction
// ---------------------------------------------------------------------------

#[test]
fn without_js_values_the_new_forms_stay_unknown_and_undefined_is_null() {
    let x = extract("src/services/tags.ts#addTag", false);
    assert!(
        unknowns(&x)
            .iter()
            .any(|u| u.contains("method calls are not resolved")),
        "{}",
        rendered(&x)
    );
    assert!(calls(&x).is_empty());
    // The template literal is the receiver of `.toLowerCase()`: the call is
    // what is unknown.
    let x = extract("src/services/handles.ts#reserveHandle", false);
    assert!(
        unknowns(&x)
            .iter()
            .any(|u| u.contains("method calls are not resolved")),
        "{}",
        rendered(&x)
    );
    let x = extract("src/services/members.ts#roleSummary", false);
    assert!(
        unknowns(&x)
            .iter()
            .any(|u| u.contains("string concatenation is not modeled")),
        "{}",
        rendered(&x)
    );
    let x = extract("src/services/members.ts#recordNote", false);
    assert!(
        !rendered(&x).contains("undefined"),
        "`undefined` is `null` without the flag:\n{}",
        rendered(&x)
    );
}

#[test]
fn every_entry_extracts_identically_without_the_flag_as_before_phase_14() {
    // The Phase 13 forms of member reads and `undefined`: `Field` and `null`.
    for selector in [
        "src/services/tags.ts#addTag",
        "src/services/tags.ts#addTagLocked",
        "src/services/handles.ts#reserveHandle",
        "src/services/handles.ts#reserveHandleRaw",
        "src/services/members.ts#grantMembership",
        "src/services/members.ts#inviteAll",
        "src/services/members.ts#recordNote",
        "src/services/members.ts#roleSummary",
        "src/services/members.ts#unsupportedShapes",
        "src/services/members.ts#auditEach",
    ] {
        let x = extract(selector, false);
        assert!(calls(&x).is_empty(), "{selector}");
        for step in x.codes.iter().flatten() {
            if let Op::Let { expr, .. } = &step.op {
                assert!(
                    !matches!(
                        expr,
                        Expr::Lit(Value::Undefined(_)) | Expr::Array(_) | Expr::Each { .. }
                    ),
                    "{selector}: {}",
                    expr.render()
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// With the flag: lowering
// ---------------------------------------------------------------------------

#[test]
fn strings_and_arrays_guarding_a_write_are_lowered() {
    let x = extract("src/services/tags.ts#addTag", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let used = calls(&x);
    for b in [
        Builtin::Split,
        Builtin::Includes,
        Builtin::Join,
        Builtin::Concat,
        Builtin::At,
    ] {
        assert!(used.contains(&b), "{b:?} in\n{}", rendered(&x));
    }
    let text = rendered(&x);
    assert!(text.contains(".split(\",\")"), "{text}");
    assert!(text.contains(".includes("), "{text}");
    let x = extract("src/services/tags.ts#addTagLocked", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let x = extract("src/services/handles.ts#reserveHandle", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let used = calls(&x);
    for b in [Builtin::Trim, Builtin::Concat, Builtin::ToLowerCase] {
        assert!(used.contains(&b), "{b:?} in\n{}", rendered(&x));
    }
    // `first + "." + last` with a string literal operand concatenates.
    let x = extract("src/services/handles.ts#reserveHandleRaw", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    assert!(calls(&x).contains(&Builtin::Concat), "{}", rendered(&x));
}

#[test]
fn array_parameters_loops_and_relative_indexing_are_lowered() {
    let x = extract("src/services/members.ts#grantMembership", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let text = rendered(&x);
    assert!(text.contains("input.roles.includes(\"admin\")"), "{text}");
    // `.at(-1)` is `[length - 1]`; `??` falls back on null or undefined.
    assert!(
        text.contains("input.roles[(input.roles.length + -1)]"),
        "{text}"
    );
    assert!(text.contains("undefined"), "{text}");
    // A `for...of` over a parameter, with a Set that never escapes.
    let x = extract("src/services/members.ts#inviteAll", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let used = calls(&x);
    for b in [
        Builtin::NewSet,
        Builtin::Has,
        Builtin::SetAdd,
        Builtin::Length,
        Builtin::At,
    ] {
        assert!(used.contains(&b), "{b:?} in\n{}", rendered(&x));
    }
    // The loop jumps back to its head.
    let main = &x.codes[0];
    assert!(
        main.iter()
            .enumerate()
            .any(|(i, s)| matches!(s.op, Op::Jump { target } if target < i)),
        "{}",
        rendered(&x)
    );
    // The set is rebound in place: `seen = seen.add(email)`.
    assert!(
        rendered(&x)
            .lines()
            .any(|l| l.starts_with("seen = seen.add(")),
        "{}",
        rendered(&x)
    );
    // A Map counted in a loop.
    let x = extract("src/services/members.ts#roleSummary", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    assert!(
        rendered(&x)
            .lines()
            .any(|l| l.starts_with("counts = counts.set(")),
        "{}",
        rendered(&x)
    );
}

#[test]
fn undefined_is_distinct_from_null_with_the_flag() {
    let x = extract("src/services/members.ts#recordNote", true);
    assert_eq!(unknowns(&x), Vec::<String>::new(), "{}", rendered(&x));
    let text = rendered(&x);
    assert!(text.contains("(note = undefined)"), "{text}");
    assert!(text.contains("(note = null)"), "{text}");
}

#[test]
fn unsupported_value_shapes_are_refused_with_a_reason() {
    let x = extract("src/services/members.ts#unsupportedShapes", true);
    let u = unknowns(&x);
    assert!(u.iter().any(|d| d.contains("spread")), "{u:#?}");
    assert!(
        u.iter()
            .any(|d| d.contains("mutating one in place is not modeled")),
        "{u:#?}"
    );
    assert!(u.iter().any(|d| d.contains("never escapes it")), "{u:#?}");
    assert!(
        u.iter()
            .any(|d| d.contains("conditionally evaluated operand")),
        "{u:#?}"
    );
    let x = extract("src/services/members.ts#auditEach", true);
    assert!(
        unknowns(&x)
            .iter()
            .any(|d| d.contains("iterating a Map or a Set")),
        "{}",
        rendered(&x)
    );
    let x = extract("src/services/members.ts#unsupportedShapes", true);
    // The Map passed to a function is not mutated in place: why is stated.
    assert!(
        x.assumptions
            .iter()
            .any(|a| a.contains("`byEmail`") && a.contains("used as a value")),
        "{:#?}",
        x.assumptions
    );
}

// ---------------------------------------------------------------------------
// End to end: scenarios whose verdict depends on string/array operations
// ---------------------------------------------------------------------------

/// The scenario loader's provider with JavaScript values on (what the
/// `js-values` scenario feature requests).
struct JsValues;

impl ActorProvider for JsValues {
    fn extract_schema(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<commitrift_model::schema_facts::ExtractedSchema, String> {
        commitrift_analyzer::schema::extract_schema(root, file)
    }
    fn resolve_reference(&self, root: &Path, file: &str, local: &str) -> Option<(String, String)> {
        TsActorProvider::new(root.to_path_buf()).resolve_reference(root, file, local)
    }
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        let mut request = request.clone();
        request.js_values = true;
        TsActorProvider::new(request.root.clone()).extract(&request)
    }
}

/// Load a values-app scenario with JavaScript values (its `features` entry
/// is the one the loader will read once the feature is wired).
fn load(name: &str) -> commitrift_model::model::Model {
    let path = app().join("scenarios").join(name);
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(doc["features"], serde_json::json!(["js-values"]));
    doc.as_object_mut().unwrap().remove("features");
    let (model, _) =
        commitrift_model::scenario::load_str(&doc.to_string(), path.parent().unwrap(), &JsValues)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    model
}

#[test]
fn a_lost_tag_through_split_includes_and_join_is_found_and_the_lock_prevents_it() {
    let r = explore(&load("tags-lost-update.json"));
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    assert!(r.unsupported.is_empty());
    let r = explore(&load("tags-locked.json"));
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);
    assert!(r.unsupported.is_empty());
}

#[test]
fn a_duplicate_handle_exists_only_because_of_string_normalization() {
    // " ann" / "lee " and "Ann" / "Lee" normalize to "ann.lee": both callers
    // find it free and insert it.
    let r = explore(&load("handles-normalized.json"));
    assert_eq!(r.verdict, Verdict::CounterexampleFound);
    assert!(r.unsupported.is_empty());
    // Without normalization the same inputs are two handles.
    let r = explore(&load("handles-raw.json"));
    assert_eq!(r.verdict, Verdict::NoCounterexampleWithinBounds);
    assert!(r.unsupported.is_empty());
}

#[test]
fn without_the_feature_the_value_scenarios_are_unsupported() {
    for name in ["tags-lost-update.json", "handles-normalized.json"] {
        let path = app().join("scenarios").join(name);
        let mut doc: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        doc.as_object_mut().unwrap().remove("features");
        let (model, _) =
            commitrift_model::scenario::load_str(&doc.to_string(), path.parent().unwrap(), &Plain)
                .unwrap();
        let r = explore(&model);
        assert_eq!(r.verdict, Verdict::Unsupported, "{name}");
    }
}

struct Plain;

impl ActorProvider for Plain {
    fn extract_schema(
        &self,
        root: &Path,
        file: &str,
    ) -> Result<commitrift_model::schema_facts::ExtractedSchema, String> {
        commitrift_analyzer::schema::extract_schema(root, file)
    }
    fn resolve_reference(&self, root: &Path, file: &str, local: &str) -> Option<(String, String)> {
        TsActorProvider::new(root.to_path_buf()).resolve_reference(root, file, local)
    }
    fn extract(&self, request: &ExtractRequest) -> Result<Extracted, String> {
        TsActorProvider::new(request.root.clone()).extract(request)
    }
}

// ---------------------------------------------------------------------------
// State identity of Phase 6–13 scenarios
// ---------------------------------------------------------------------------

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

/// The number of reachable states and an FNV-1a digest of their sorted
/// canonical encodings.
fn digest(path: &Path) -> (usize, u64) {
    let (m, _) = commitrift_model::scenario::load_file(path, &Plain)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
    let init = initial_state(&m);
    seen.insert(init.canonical(&m));
    let mut stack = vec![init];
    while let Some(s) = stack.pop() {
        if seen.len() > 20_000 {
            break;
        }
        for t in enabled(&m, &s) {
            if let Ok(x) = apply(&m, &s, t, &mut Recorder::off())
                && seen.insert(x.state.canonical(&m))
            {
                stack.push(x.state);
            }
        }
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in &seen {
        fnv(&mut h, &(b.len() as u64).to_le_bytes());
        fnv(&mut h, b);
    }
    (seen.len(), h)
}

/// Reachable-state digests of fixture scenarios computed by the extractor
/// and model *before* the Phase 14 value domain (the same tree with the
/// Phase 14 Track B changes removed; `scripts`-free probe). Equal digests
/// mean byte-identical canonical states: neither the extraction (without
/// `js-values`) nor the encoding of states without the new values changed.
const BEFORE_PHASE_14: &[(&str, usize, u64)] = &[
    (
        "fixtures/concurrency/scenarios/doctors-write-skew-read-committed.json",
        27,
        0x02f580e7142ecd00,
    ),
    (
        "fixtures/concurrency/scenarios/doctors-write-skew-serializable.json",
        30,
        0xd3d233b5adca5401,
    ),
    (
        "fixtures/concurrency/scenarios/inventory-oversell-in-transaction.json",
        40,
        0xb1d50ceb1da20eb9,
    ),
    (
        "fixtures/concurrency/scenarios/job-double-processing.json",
        163,
        0xa8eee7c96d81cbdb,
    ),
    (
        "fixtures/concurrency/scenarios/redeem-check-then-act.json",
        11,
        0x260f06aa46731c24,
    ),
    (
        "fixtures/phase7/scenarios/team-app-accept-invitation-twice.json",
        55,
        0xb6b3f8b8b8c44e2c,
    ),
    (
        "fixtures/phase8/scenarios/accept-crash-lock-holder.json",
        121,
        0xd116f6e1e4b6be3b,
    ),
    (
        "fixtures/phase8/scenarios/inventory-retry-duplicate-notice.json",
        97,
        0xc2169f33bc6276e5,
    ),
    (
        "fixtures/phase8/scenarios/shipping-outbox-relay-duplicate.json",
        29,
        0x44dd6c27d4d31219,
    ),
    (
        "fixtures/phase9/model-scenarios/ambiguous-commit-lookup-then-insert-concurrent.json",
        353,
        0xe858eaa71f8273aa,
    ),
    (
        "fixtures/phase9/scenarios/ledger-shared-process-crash.json",
        61,
        0xa7d5d876522131fe,
    ),
    (
        "fixtures/phase10/scenarios/live-shared-process-lost-commit.json",
        188,
        0xfe785bc9efd8d288,
    ),
    (
        "fixtures/phase10/scenarios/pgjs-crash-reconciled-commit-in-flight.json",
        27,
        0x16701dc74b16c5e9,
    ),
    (
        "fixtures/phase13/model/replica-read-your-writes.json",
        6,
        0x8e3838e2bb74606c,
    ),
    (
        "fixtures/phase13/request-app/scenarios/claim-race.json",
        12,
        0xb9e4f001c8e791d9,
    ),
    (
        "fixtures/phase13/request-app/scenarios/signup-crash.json",
        7,
        0xa79804a189ea7671,
    ),
];

#[test]
fn phase_6_to_13_scenarios_keep_their_state_bytes() {
    for (rel, states, expected) in BEFORE_PHASE_14 {
        let (n, h) = digest(&workspace().join(rel));
        assert_eq!((n, h), (*states, *expected), "{rel}");
    }
}

// ---------------------------------------------------------------------------
// Parameter annotations (synthesis typing)
// ---------------------------------------------------------------------------

#[test]
fn parameter_annotations_are_read_by_path() {
    let provider = TsActorProvider::new(app());
    let a = provider.param_annotations(&app(), "src/services/members.ts#grantMembership");
    // Through the interface `GrantInput` declared in the module.
    assert_eq!(a.get("input").map(String::as_str), Some("GrantInput"));
    assert_eq!(a.get("input.roles").map(String::as_str), Some("string[]"));
    assert_eq!(a.get("input.email").map(String::as_str), Some("string"));
    assert_eq!(a.get("input.teamId").map(String::as_str), Some("number"));
    let a = provider.param_annotations(&app(), "src/services/members.ts#inviteAll");
    assert_eq!(a.get("emails").map(String::as_str), Some("string[]"));
    let a = provider.param_annotations(&app(), "src/services/members.ts#recordNote");
    assert_eq!(a.get("note").map(String::as_str), Some("string | null"));
    assert!(
        provider
            .param_annotations(&app(), "src/services/nowhere.ts#x")
            .is_empty()
    );
}
