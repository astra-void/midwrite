//! Discovery on the first-party commerce application and on small
//! projects written for one property each. Static and model checking only:
//! nothing here executes application code.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use midwrite_discover::candidate::Origin;
use midwrite_discover::pipeline::{Options, discover};
use midwrite_discover::report::DiscoveryReport;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn commerce() -> PathBuf {
    repo().join("fixtures/apps/commerce-app")
}

fn options(model_check: bool) -> Options {
    Options {
        model_check,
        include_timing: false,
        tool_version: "test".to_owned(),
        ..Options::default()
    }
}

fn commerce_report() -> &'static DiscoveryReport {
    static REPORT: OnceLock<DiscoveryReport> = OnceLock::new();
    REPORT.get_or_init(|| discover(&commerce(), &options(true)).expect("discovery runs"))
}

/// Candidates whose actors are all `selector`.
fn own<'r>(
    r: &'r DiscoveryReport,
    selector: &str,
) -> Vec<&'r midwrite_discover::report::CandidateReport> {
    r.candidates
        .iter()
        .filter(|c| {
            c.candidate
                .actor_entries
                .iter()
                .all(|&i| c.candidate.entries[i].selector == selector)
        })
        .collect()
}

/// A temporary copy of a directory tree (node_modules and dist excluded).
struct TempTree(PathBuf);

impl TempTree {
    fn copy(from: &Path, name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("midwrite-discover-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(from, &dir);
        Self(dir)
    }

    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("midwrite-discover-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        Self(dir)
    }

    fn edit(&self, path: &str, f: impl FnOnce(String) -> String) {
        let p = self.0.join(path);
        let text = std::fs::read_to_string(&p).unwrap();
        let new = f(text.clone());
        assert_ne!(text, new, "the edit of {path} changed nothing");
        std::fs::write(p, new).unwrap();
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "node_modules" || name == "dist" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// The labeled first-party application
// ---------------------------------------------------------------------------

#[test]
fn labeled_entries_are_classified_as_labeled() {
    let r = commerce_report();
    let labels: midwrite_discover::eval::Labels = serde_json::from_str(
        &std::fs::read_to_string(repo().join("fixtures/eval/p11/labels.json")).unwrap(),
    )
    .unwrap();
    let e = midwrite_discover::eval::evaluate(r, &labels);
    let by: BTreeMap<&str, &str> = e
        .cases
        .iter()
        .map(|c| (c.entry.as_str(), c.outcome.as_str()))
        .collect();
    // Every labeled positive has a validated model counterexample.
    for c in labels.cases.iter().filter(|c| c.label == "positive") {
        assert_eq!(by[c.entry.as_str()], "TP", "{}", c.entry);
    }
    // Every labeled negative but one has none; the exception is recorded
    // in docs/research/evaluation-methodology.md: two `charges` rows for one
    // deduplicated charge, against a heuristic property the label did not
    // consider.
    for c in labels.cases.iter().filter(|c| c.label == "negative") {
        let expected = if c.entry == "src/app/checkout.ts#checkoutIdempotent" {
            "FP"
        } else {
            "TN"
        };
        assert_eq!(by[c.entry.as_str()], expected, "{}", c.entry);
    }
    // Phase 16 models `request.json()`: the checkout route, labeled
    // unsupported only because Phase 11 did not model it, is now analyzed
    // (recorded in the labels' `corrections`, the label itself unchanged).
    assert_eq!(e.unsupported_reported, 2);
    assert_eq!(e.unsupported_missed, 1);
    assert_eq!(e.labels_corrected, 1);
    // The false positive is against a hypothesis, never an established one.
    for c in own(r, "src/app/checkout.ts#checkoutIdempotent") {
        if c.status == "MODEL_COUNTEREXAMPLE" {
            assert!(c.finding.contains("HEURISTIC_HYPOTHESIS"), "{}", c.finding);
        }
    }
}

#[test]
fn safe_counterparts_are_not_reported_as_the_same_hazard() {
    let r = commerce_report();
    for (unsafe_entry, safe_entry) in [
        (
            "src/app/invitations.ts#inviteMember/arg[1]",
            "src/app/invitations.ts#inviteSeat/arg[1]",
        ),
        (
            "src/app/inventory.ts#reserveItem",
            "src/app/inventory.ts#reserveItemLocked",
        ),
        (
            "src/services/orders.ts#endShift",
            "src/services/orders.ts#endShiftSerializable",
        ),
        (
            "src/services/jobs.ts#processRefund",
            "src/services/jobs.ts#processRefundOnce",
        ),
        (
            "src/services/orders.ts#recordPayment",
            "src/services/orders.ts#recordPaymentIdempotent",
        ),
        (
            "src/app/inventory.ts#transferStock",
            "src/app/inventory.ts#transferStockOrdered",
        ),
    ] {
        assert!(
            own(r, unsafe_entry)
                .iter()
                .any(|c| c.status == "MODEL_COUNTEREXAMPLE"),
            "{unsafe_entry}"
        );
        let safe = own(r, safe_entry);
        assert!(!safe.is_empty(), "{safe_entry} has a candidate of its own");
        for c in safe {
            assert_eq!(
                c.status, "NO_VIOLATION_WITHIN_BOUNDS",
                "{safe_entry}: {}",
                c.finding
            );
            assert!(c.finding.contains("not a proof"));
        }
    }
    // The safe counterparts without a candidate of their own.
    for safe in [
        "src/app/inventory.ts#reserveItemAtomic",
        "src/services/orders.ts#claimTaskConditional",
        "src/services/reports.ts#stockLevel",
    ] {
        assert!(own(r, safe).is_empty(), "{safe}");
    }
}

#[test]
fn property_origins_are_kept_and_never_upgraded() {
    let r = commerce_report();
    let origin_of = |selector: &str, class: &str| -> Vec<Origin> {
        own(r, selector)
            .iter()
            .filter(|c| c.candidate.class.as_str() == class)
            .flat_map(|c| c.candidate.properties.iter().map(|p| p.origin))
            .collect()
    };
    assert_eq!(
        origin_of("src/app/invitations.ts#inviteSeat/arg[1]", "check-then-act"),
        vec![Origin::SchemaEnforced]
    );
    assert_eq!(
        origin_of(
            "src/app/invitations.ts#inviteMember/arg[1]",
            "check-then-act"
        ),
        vec![Origin::HeuristicHypothesis]
    );
    assert_eq!(
        origin_of("src/services/orders.ts#endShift", "write-skew"),
        vec![Origin::UserDeclared]
    );
    assert_eq!(
        origin_of("src/services/jobs.ts#processRefund", "duplicate-processing"),
        vec![Origin::FrameworkContract]
    );
    for c in &r.candidates {
        if c.status != "MODEL_COUNTEREXAMPLE" {
            continue;
        }
        let deciding = c
            .scenarios
            .iter()
            .filter(|s| format!("{:?}", s.variant.kind) == "Actual")
            .find_map(|s| s.model.as_ref().and_then(|m| m.violated.clone()))
            .expect("a counterexample names its invariant");
        let p = c
            .candidate
            .properties
            .iter()
            .find(|p| p.name == deciding)
            .expect("the violated invariant is a property of the candidate");
        if p.origin == Origin::HeuristicHypothesis {
            assert!(
                c.finding.contains("NOT been established")
                    && c.finding.contains("not a confirmed defect"),
                "{}",
                c.finding
            );
        }
        // Every invariant in a scenario states its origin.
        for s in &c.scenarios {
            for i in &s.invariants {
                assert!(!i.origin.is_empty());
            }
        }
    }
    assert_eq!(
        r.summary.model_counterexamples,
        r.summary.counterexamples_against_hypotheses
            + r.candidates
                .iter()
                .filter(|c| c.status == "MODEL_COUNTEREXAMPLE")
                .filter(|c| c.finding.starts_with("MODEL_COUNTEREXAMPLE:"))
                .count()
    );
}

#[test]
fn counterexamples_replay_violate_and_exercise_the_candidate() {
    let r = commerce_report();
    let mut checked = 0;
    for c in &r.candidates {
        for s in &c.scenarios {
            let Some(m) = &s.model else { continue };
            if m.verdict != "COUNTEREXAMPLE_FOUND" {
                continue;
            }
            let v = m.validation.as_ref().expect("validated");
            assert!(v.replayed, "{} {}", c.candidate.id, s.variant.name);
            assert!(v.violates, "{} {}", c.candidate.id, s.variant.name);
            assert!(v.initial_state_valid);
            assert!(!v.operations_exercised.is_empty(), "{}", c.candidate.id);
            checked += 1;
        }
        if c.status == "MODEL_COUNTEREXAMPLE" && c.candidate.class.as_str() == "check-then-act" {
            let s = &c.scenarios[0];
            let v = s.model.as_ref().unwrap().validation.as_ref().unwrap();
            assert!(
                v.operations_not_exercised.is_empty(),
                "{:?}",
                v.operations_not_exercised
            );
        }
    }
    assert!(checked >= 9, "{checked}");
}

#[test]
fn synthesized_states_satisfy_keys_foreign_keys_and_defaults() {
    let r = commerce_report();
    let invite = own(r, "src/app/invitations.ts#inviteMember/arg[1]")[0];
    let s = &invite.scenarios[0];
    // The member the wrapper passes is the `members` row, in its organization.
    let members = &s.initial_state["members"];
    let orgs = &s.initial_state["organizations"];
    assert_eq!(members.len(), 1);
    let org = members[0]["orgId"].clone();
    assert!(orgs.iter().any(|o| o["id"] == org));
    assert_eq!(s.actors[0].arguments[1].clone(), {
        let mut m = std::collections::BTreeMap::new();
        m.insert("id".to_owned(), members[0]["id"].clone());
        m.insert("orgId".to_owned(), org.clone());
        midwrite_model::value::Value::Row(m)
    });
    // The check must come back empty: no invitation is synthesized.
    assert!(!s.initial_state.contains_key("invitations"));
    // Same and distinct keys are both explored.
    assert_eq!(invite.scenarios[1].variant.name, "distinct-key");
    assert_ne!(
        invite.scenarios[1].actors[0].arguments,
        invite.scenarios[1].actors[1].arguments
    );
    // Write skew: both managers' shifts are the two on duty in one store.
    let skew = own(r, "src/services/orders.ts#endShift")[0];
    let shifts = &skew.scenarios[0].initial_state["shifts"];
    assert_eq!(shifts.len(), 2, "{shifts:?}");
    assert_eq!(shifts[0]["storeId"], shifts[1]["storeId"]);
}

#[test]
fn discovery_is_deterministic_and_scenarios_are_stable() {
    let a = discover(&commerce(), &options(false)).unwrap();
    let b = discover(&commerce(), &options(false)).unwrap();
    assert_eq!(a.to_json(), b.to_json());
    let out1 = TempTree::new("emit-1", &[]);
    let out2 = TempTree::new("emit-2", &[]);
    for out in [&out1, &out2] {
        let mut o = options(false);
        o.emit_dir = Some(out.0.clone());
        discover(&commerce(), &o).unwrap();
    }
    let list = |d: &Path| -> BTreeMap<String, String> {
        std::fs::read_dir(d)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read_to_string(e.path()).unwrap(),
                )
            })
            .collect()
    };
    let (l1, l2) = (list(&out1.0), list(&out2.0));
    assert!(!l1.is_empty());
    assert_eq!(l1.keys().collect::<Vec<_>>(), l2.keys().collect::<Vec<_>>());
    for (k, v) in &l1 {
        // Only the relative project root differs between the directories.
        let strip = |t: &str| {
            t.replace(
                &out2.0.file_name().unwrap().to_string_lossy().to_string(),
                "X",
            )
            .replace(
                &out1.0.file_name().unwrap().to_string_lossy().to_string(),
                "X",
            )
        };
        assert_eq!(strip(v), strip(&l2[k]), "{k}");
    }
}

#[test]
fn without_contracts_nothing_is_assumed_that_only_contracts_say() {
    let tree = TempTree::copy(&commerce(), "no-contracts");
    std::fs::remove_file(tree.0.join("midwrite-contracts.json")).unwrap();
    let r = discover(&tree.0, &options(true)).unwrap();
    assert!(
        r.notes
            .iter()
            .any(|n| n.contains("no midwrite-contracts.json"))
    );
    for c in &r.candidates {
        for p in &c.candidate.properties {
            assert!(
                !matches!(p.origin, Origin::UserDeclared | Origin::FrameworkContract),
                "{}: {:?}",
                c.candidate.id,
                p.origin
            );
        }
        assert_ne!(c.candidate.class.as_str(), "duplicate-processing");
    }
    // The write skew needs a declared invariant.
    let skew = own(&r, "src/services/orders.ts#endShift");
    assert!(
        skew.iter().all(|c| c.status == "BLOCKED"),
        "{:?}",
        skew.iter().map(|c| &c.status).collect::<Vec<_>>()
    );
    // The external-effect gap needs a declared retry; without the external
    // declaration the payout is an unknown operation.
    assert!(
        r.rejected
            .iter()
            .all(|x| x.class.as_str() != "external-effect-gap" || x.reason.contains("no retry"))
    );
    let refund = r
        .entries
        .iter()
        .find(|e| e.selector == "src/services/jobs.ts#processRefund")
        .unwrap();
    assert!(
        refund
            .unknown
            .iter()
            .any(|u| u.contains("method calls are not resolved")),
        "{:?}",
        refund.unknown
    );
    for c in own(&r, "src/services/jobs.ts#processRefund") {
        assert!(c.candidate.unsupported_on_path || c.status != "NO_VIOLATION_WITHIN_BOUNDS");
    }
}

// ---------------------------------------------------------------------------
// Metamorphic transformations
// ---------------------------------------------------------------------------

/// The candidate set without names that a semantics-preserving change may
/// alter (locations, parameter names): class, entries, table and columns,
/// property templates and origins, status.
fn normalized(r: &DiscoveryReport) -> BTreeSet<String> {
    r.candidates
        .iter()
        .map(|c| {
            let k = &c.candidate;
            let mut entries: Vec<&str> = k.entries.iter().map(|e| e.selector.as_str()).collect();
            entries.sort_unstable();
            let props: Vec<String> = k
                .properties
                .iter()
                .map(|p| {
                    let v = serde_json::to_value(&p.spec).unwrap();
                    format!(
                        "{}:{}:{}",
                        v["template"].as_str().unwrap_or(""),
                        v["table"].as_str().or(v["channel"].as_str()).unwrap_or(""),
                        p.origin.as_str()
                    )
                })
                .collect();
            format!(
                "{}|{}|{}|{}|{}|{}",
                k.class.as_str(),
                entries.join(","),
                k.resource.table,
                k.resource.columns.join(","),
                props.join(";"),
                c.status
            )
        })
        .collect()
}

#[test]
fn semantics_preserving_changes_keep_the_candidate_set() {
    let base = normalized(commerce_report());
    let tree = TempTree::copy(&commerce(), "metamorphic");
    // 1. Rename local bindings and a callback parameter.
    tree.edit("src/app/invitations.ts", |t| {
        t.replace(
            "const existing = await findPendingInvitation",
            "const pendingAlready = await findPendingInvitation",
        )
        .replace(
            "  if (existing) {\n    return { error: \"An invitation",
            "  if (pendingAlready) {\n    return { error: \"An invitation",
        )
        .replace(
            "async (data, member) => {\n  const pendingAlready",
            "async (form, member) => {\n  const pendingAlready",
        )
        .replace(
            "findPendingInvitation(member.orgId, data.email)",
            "findPendingInvitation(member.orgId, form.email)",
        )
        .replace(
            "createInvitation(member.orgId, data.email, data.role, member.id)",
            "createInvitation(member.orgId, form.email, form.role, member.id)",
        )
    });
    tree.edit("midwrite-contracts.json", |t| {
        t.replace("\"data.role\"", "\"form.role\"")
    });
    // 2. Move a helper to another module.
    tree.edit("src/repositories/inventory-repository.ts", |t| {
        let start = t.find("export async function addReservation").unwrap();
        let end = t[start..].find("\n}\n").unwrap() + start + 3;
        let moved = t[start..end].to_owned();
        std::fs::write(
            tree.0.join("src/repositories/reservation-repository.ts"),
            format!("import {{ db }} from \"../db/client.js\";\nimport {{ reservations }} from \"../db/schema.js\";\n\n{moved}"),
        )
        .unwrap();
        format!(
            "{}export {{ addReservation }} from \"./reservation-repository.js\";\n{}",
            &t[..start],
            &t[end..]
        )
    });
    tree.edit("src/app/inventory.ts", |t| {
        t.replace(
            "import {\n  addReservation,\n  getProduct,",
            "import { addReservation } from \"../repositories/reservation-repository.js\";\nimport {\n  getProduct,",
        )
    });
    // 3. Reorder independent declarations.
    tree.edit("src/repositories/order-repository.ts", |t| {
        let a = t.find("export async function getOrder").unwrap();
        let b = t.find("export async function recordCharge").unwrap();
        format!("{}{}\n{}", &t[..a], &t[b..], t[a..b].trim_end())
    });
    // 4. Add an irrelevant pure function.
    tree.edit("src/services/orders.ts", |t| {
        format!("{t}\nexport function label(task: ClaimRequest) {{\n  return `${{task.taskId}}:${{task.assignee}}`;\n}}\n")
    });
    // 5. Call through a local wrapper.
    tree.edit("src/app/checkout.ts", |t| {
        t.replacen(
            "const order = await getOrder(input.orderId);",
            "const order = await loadOrder(input.orderId);",
            1,
        ) + "\nasync function loadOrder(id: number) {\n  return getOrder(id);\n}\n"
    });
    let changed = normalized(&discover(&tree.0, &options(true)).unwrap());
    // The added pure function and the re-export are new exported functions:
    // they must not add candidates.
    assert_eq!(
        base,
        changed,
        "\nonly before: {:#?}\nonly after: {:#?}",
        base.difference(&changed).collect::<Vec<_>>(),
        changed.difference(&base).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Small projects, one property each
// ---------------------------------------------------------------------------

const SCHEMA: &str = r#"import { integer, pgTable, serial, text, uniqueIndex } from "drizzle-orm/pg-core";
export const items = pgTable("items", {
  id: serial("id").primaryKey(),
  tenant: text("tenant").notNull(),
  name: text("name").notNull(),
  kind: text("kind").notNull().default("a"),
  qty: integer("qty").notNull(),
});
export const tags = pgTable(
  "tags",
  {
    id: serial("id").primaryKey(),
    tenant: text("tenant").notNull(),
    name: text("name").notNull(),
    deleted: integer("deleted"),
  },
  (t) => [uniqueIndex("tags_live").on(t.tenant, t.name).where(sql`deleted is null`)],
);
"#;

fn project(name: &str, code: &str) -> TempTree {
    TempTree::new(name, &[("src/schema.ts", SCHEMA), ("src/app.ts", code)])
}

fn run(tree: &TempTree) -> DiscoveryReport {
    discover(&tree.0, &options(true)).unwrap()
}

#[test]
fn distinct_literal_keys_are_rejected_as_disjoint() {
    let tree = project(
        "tenants",
        r#"import { and, eq } from "drizzle-orm";
import { items } from "./schema";
export async function addA(tenant: string, name: string) {
  const [found] = await db.select().from(items).where(and(eq(items.tenant, tenant), eq(items.name, name), eq(items.kind, "a"))).limit(1);
  if (found) { return { exists: true }; }
  await db.insert(items).values({ tenant, name, kind: "a", qty: 1 });
  return { exists: false };
}
export async function addB(tenant: string, name: string) {
  await db.insert(items).values({ tenant, name, kind: "b", qty: 1 });
}
"#,
    );
    let r = run(&tree);
    assert!(
        r.rejected
            .iter()
            .any(|x| x.reason.contains("provably distinct")),
        "{:?}",
        r.rejected
    );
    assert!(r.candidates.iter().all(|c| c.candidate.entries.len() == 1));
}

#[test]
fn exclusive_branches_read_only_and_atomic_updates_are_not_candidates() {
    let tree = project(
        "negatives",
        r#"import { and, eq, gte, sql } from "drizzle-orm";
import { items } from "./schema";
export async function exclusive(id: number, fresh: boolean, tenant: string, name: string) {
  if (fresh) {
    const [found] = await db.select().from(items).where(eq(items.id, id)).limit(1);
    return found;
  } else {
    await db.insert(items).values({ tenant, name, qty: 1 });
  }
}
export async function readOnly(id: number) {
  const [found] = await db.select().from(items).where(eq(items.id, id)).limit(1);
  return found ? found.qty : 0;
}
export async function atomicTake(id: number, n: number) {
  const changed = await db.update(items).set({ qty: sql`${items.qty} - ${n}` }).where(and(eq(items.id, id), gte(items.qty, n))).returning();
  if (changed.length === 0) { throw new Error("none left"); }
}
"#,
    );
    let r = run(&tree);
    assert!(
        r.candidates.is_empty(),
        "{:#?}",
        r.candidates
            .iter()
            .map(|c| &c.candidate.title)
            .collect::<Vec<_>>()
    );
    let ro = r
        .entries
        .iter()
        .find(|e| e.selector.ends_with("#readOnly"))
        .unwrap();
    assert_eq!(ro.status, "read-only");
}

#[test]
fn unknown_sql_and_undeclared_calls_stay_visible() {
    let tree = project(
        "unknowns",
        r#"import { eq } from "drizzle-orm";
import { items } from "./schema";
export async function raw(id: number) {
  await db.execute(`update items set qty = 0 where id = ${id}`);
}
export async function notify(id: number, mailer: { send(id: number): Promise<void> }) {
  const [found] = await db.select().from(items).where(eq(items.id, id)).limit(1);
  if (!found) { return; }
  await mailer.send(found.id);
  await db.update(items).set({ qty: found.qty - 1 }).where(eq(items.id, found.id));
}
"#,
    );
    let r = run(&tree);
    let raw = r
        .entries
        .iter()
        .find(|e| e.selector.ends_with("#raw"))
        .unwrap();
    assert!(
        raw.unknown.iter().any(|u| u.contains("raw SQL")),
        "{:?}",
        raw.unknown
    );
    // The read-modify-write candidate exists but the unknown call lies on its
    // path: it is never reported safe.
    let notify: Vec<_> = r
        .candidates
        .iter()
        .filter(|c| c.candidate.entries[0].selector.ends_with("#notify"))
        .collect();
    assert!(!notify.is_empty());
    for c in notify {
        assert!(c.candidate.unsupported_on_path, "{}", c.candidate.title);
        assert_ne!(c.status, "NO_VIOLATION_WITHIN_BOUNDS", "{}", c.finding);
    }
}

#[test]
fn partial_unique_indexes_do_not_count_as_protection() {
    let tree = project(
        "partial",
        r#"import { and, eq } from "drizzle-orm";
import { tags } from "./schema";
export async function tag(tenant: string, name: string) {
  const [found] = await db.select().from(tags).where(and(eq(tags.tenant, tenant), eq(tags.name, name))).limit(1);
  if (found) { return; }
  await db.insert(tags).values({ tenant, name });
}
"#,
    );
    let r = run(&tree);
    let c = &r.candidates[0];
    assert_eq!(c.candidate.class.as_str(), "check-then-act");
    assert!(
        c.candidate
            .protections
            .iter()
            .any(|p| p.contains("partial unique index")),
        "{:?}",
        c.candidate.protections
    );
    assert!(
        c.candidate
            .properties
            .iter()
            .all(|p| p.origin == Origin::HeuristicHypothesis)
    );
    // The index is still not a protection, but the model does not
    // enforce it, and here it would reject the duplicate (the inserted rows
    // have `deleted` null): the schedule the model finds is not a finding.
    // (Phase 11-13 reported it as MODEL_COUNTEREXAMPLE, a false alarm.)
    assert_eq!(c.status, "INCONCLUSIVE", "{}", c.finding);
    assert!(c.finding.contains("partial unique index"), "{}", c.finding);
}

#[test]
fn contradictory_conditions_are_unsatisfiable_not_invented() {
    let tree = project(
        "contradiction",
        r#"import { and, eq } from "drizzle-orm";
import { items } from "./schema";
export async function bump(id: number) {
  const [found] = await db.select().from(items).where(and(eq(items.kind, "a"), eq(items.kind, "b"), eq(items.id, id))).limit(1);
  if (!found) { throw new Error("missing"); }
  await db.update(items).set({ qty: found.qty + 1 }).where(eq(items.id, found.id));
}
"#,
    );
    let r = run(&tree);
    let c = &r.candidates[0];
    assert_eq!(c.status, "INCONCLUSIVE", "{}", c.finding);
    assert!(
        c.scenarios[0].synthesis.detail.contains("contradictory"),
        "{}",
        c.scenarios[0].synthesis.detail
    );
    assert!(c.scenarios.iter().all(|s| s.model.is_none()));
}

// ---------------------------------------------------------------------------
// Cache invalidation
// ---------------------------------------------------------------------------

#[test]
fn the_extraction_cache_is_invalidated_by_what_extraction_reads() {
    let tree = TempTree::copy(&commerce(), "cache");
    let cache = tree.0.join("cache.json");
    let mut o = options(false);
    o.cache = Some(cache.clone());
    let first = discover(&tree.0, &o).unwrap();
    assert_eq!(first.index.cache_hits, 0);
    let n = first.index.cache_misses;
    assert!(n > 10, "{n}");
    let second = discover(&tree.0, &o).unwrap();
    assert_eq!((second.index.cache_hits, second.index.cache_misses), (n, 0));
    assert_eq!(normalized(&first), normalized(&second));
    // A helper changes: its importers miss, the others hit.
    tree.edit("src/repositories/payment-repository.ts", |t| {
        t.replace(
            "{ orderId, reference, amount }",
            "{ amount, orderId, reference }",
        )
    });
    let third = discover(&tree.0, &o).unwrap();
    assert!(
        third.index.cache_misses >= 3 && third.index.cache_misses < n,
        "{}",
        third.index.cache_misses
    );
    assert_eq!(third.index.cache_hits + third.index.cache_misses, n);
    // The schema changes: every binding changes, nothing is reused.
    tree.edit("src/db/schema.ts", |t| {
        t.replace(
            "stock: integer(\"stock\").notNull(),",
            "stock: integer(\"stock\").notNull(),\n  reserved: integer(\"reserved\"),",
        )
    });
    let fourth = discover(&tree.0, &o).unwrap();
    assert_eq!(fourth.index.cache_hits, 0);
    // A channel's deduplication is not read by extraction: everything is
    // reused (candidates are recomputed; they are never cached).
    tree.edit("midwrite-contracts.json", |t| {
        t.replace("\"dedup\": \"none\"", "\"dedup\": \"byKey\"")
    });
    let fifth = discover(&tree.0, &o).unwrap();
    assert_eq!(fifth.index.cache_misses, 0);
    assert_eq!(fifth.index.cache_hits, fourth.index.cache_misses);
    // An external binding is: nothing is reused.
    tree.edit("midwrite-contracts.json", |t| {
        t.replace("\"channel\": \"payout\"", "\"channel\": \"payouts\"")
    });
    let sixth = discover(&tree.0, &o).unwrap();
    assert_eq!(sixth.index.cache_hits, 0);
}

#[test]
fn a_change_to_one_schema_module_invalidates_only_the_entries_that_reach_it() {
    let app = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/apps/workspace-app");
    let tree = TempTree::copy(&app, "cache-p12");
    let cache = tree.0.join("cache.json");
    let mut o = options(false);
    o.cache = Some(cache.clone());
    let first = discover(&tree.0, &o).unwrap();
    let n = first.index.cache_misses;
    let second = discover(&tree.0, &o).unwrap();
    assert_eq!((second.index.cache_hits, second.index.cache_misses), (n, 0));
    assert_eq!(normalized(&first), normalized(&second));
    // The invites schema changes: entries that reach it (the invite
    // service, everything importing the schema barrel) miss; entries that
    // do not (the id helper, the session helpers) still hit.
    tree.edit("src/db/schema/invites.ts", |t| {
        t.replace(
            "status: text(\"status\")",
            "status: text(\"invite_status\")",
        )
    });
    let third = discover(&tree.0, &o).unwrap();
    assert!(
        third.index.cache_misses > 0,
        "the change must invalidate its readers"
    );
    assert!(
        third.index.cache_hits > 0,
        "an unrelated entry must not be invalidated"
    );
    assert_eq!(third.index.cache_hits + third.index.cache_misses, n);
    // Nothing is reused under changed schema semantics: the result equals a
    // run without a cache.
    let mut fresh = options(false);
    fresh.cache = None;
    let uncached = discover(&tree.0, &fresh).unwrap();
    assert_eq!(normalized(&third), normalized(&uncached));
}
