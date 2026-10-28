//! Candidate discovery over effect summaries.
//!
//! Each detector is a small, deterministic rule over one entry's
//! [`EffectSummary`] (`docs/phase11/CANDIDATE-DISCOVERY.md`). Pairs of
//! different entries are formed through a resource index (table, column,
//! key columns), never by enumerating all pairs of functions. A detector
//! records a recognized protection on the candidate instead of dropping it,
//! so that the model checker, not the detector, decides whether the
//! protection works; it drops a combination only for a reason it records in
//! [`Rejected`].

use std::collections::{BTreeMap, BTreeSet};

use commitrift_analyzer::entries::{EntryFact, EntryKind};
use commitrift_model::ir::{Conn, Isolation, LockMode, OnConflict};
use commitrift_model::provider::EntryInfo;
use commitrift_model::value::Value;

use crate::candidate::{
    Assumption, Candidate, Class, ConflictOp, EntryRef, Environment, Origin, Property, Rejected,
    Relation, Resource, Spec, Strength,
};
use crate::contracts::{Contracts, EntryContract, Template};
use crate::effects::{Access, DbEffect, EffectSummary, Val};
use crate::project::ProjectIndex;

/// One analyzed entry.
pub struct Analyzed {
    pub fact: EntryFact,
    pub use_server: bool,
    pub summary: EffectSummary,
    pub info: Option<EntryInfo>,
    pub contract: Option<EntryContract>,
}

impl Analyzed {
    /// How the entry is invoked, as far as source and contracts say.
    pub fn invocation(&self) -> String {
        if self
            .contract
            .as_ref()
            .is_some_and(|c| c.invocation.is_some())
        {
            return "declared".to_owned();
        }
        match self.info.as_ref().and_then(|i| i.wrapper.as_ref()) {
            None => "direct".to_owned(),
            Some(w) => match w.invocation.as_str() {
                "returned-function-awaits"
                | "returned-function-returns-promise"
                | "invoked-during-wrap" => {
                    if w.conditional {
                        "wrapper-awaited-when-guards-pass".to_owned()
                    } else {
                        "wrapper-awaited".to_owned()
                    }
                }
                "returned-function-fire-and-forget" | "invoked-during-wrap-unawaited" => {
                    "wrapper-fire-and-forget".to_owned()
                }
                _ => "unknown".to_owned(),
            },
        }
    }

    pub fn entry_ref(&self) -> EntryRef {
        let wrapper = self.info.as_ref().and_then(|i| i.wrapper.as_ref());
        EntryRef {
            selector: self.fact.selector.clone(),
            kind: self.fact.kind.as_str().to_owned(),
            name: self.fact.name.clone(),
            anchor: self.info.as_ref().map_or_else(
                || format!("{}@{}:{}", self.file(), self.fact.line, self.fact.column),
                |i| i.anchor.clone(),
            ),
            invocation: self.invocation(),
            wrapper: wrapper.map(|w| w.wrapper.clone()),
            guards: wrapper.map(|w| w.guards.clone()).unwrap_or_default(),
        }
    }

    pub fn file(&self) -> &str {
        self.fact.selector.split('#').next().unwrap_or_default()
    }

    /// Whether two invocations of this entry may overlap, and on what basis.
    pub fn concurrency(&self) -> (bool, Assumption) {
        if let Some(c) = self.contract.as_ref().and_then(|c| c.concurrent) {
            return (
                c,
                Assumption::new(
                    "concurrency",
                    "declared",
                    if c {
                        "two invocations can overlap (declared)"
                    } else {
                        "invocations never overlap (declared)"
                    },
                ),
            );
        }
        if self
            .contract
            .as_ref()
            .and_then(|c| c.delivery.as_deref())
            .is_some()
        {
            return (
                true,
                Assumption::new(
                    "concurrency",
                    "declared",
                    "an at-least-once queue can hand the same message to two workers at once (declared delivery)",
                ),
            );
        }
        match self.fact.kind {
            EntryKind::ServerAction | EntryKind::RouteHandler => (
                true,
                Assumption::new(
                    "concurrency",
                    "framework",
                    format!(
                        "two requests can run the {} at the same time (double submit, two tabs, client retry)",
                        if self.fact.kind == EntryKind::ServerAction {
                            "Server Action"
                        } else {
                            "route handler"
                        }
                    ),
                ),
            ),
            _ => (
                true,
                Assumption::new(
                    "concurrency",
                    "discovery-default",
                    "the exported function is assumed to be called concurrently by its callers; this is NOT established from source (declare `concurrent` in commitrift-contracts.json)",
                ),
            ),
        }
    }
}

/// The value a column of the row returned by a read holds when the read
/// matched it by equality (`read#3.id` with `id = $input.id` → `$input.id`).
pub fn canon(v: &Val, s: &EffectSummary) -> Val {
    match v {
        Val::Read { effect, column } => {
            if let Some(e) = s.db.get(*effect)
                && let Some(x) = e.eq_val(column)
                && !matches!(x, Val::Read { .. })
            {
                return canon(x, s);
            }
            v.clone()
        }
        other => other.clone(),
    }
}

fn same_value(a: &Val, b: &Val, s: &EffectSummary) -> bool {
    let (a, b) = (canon(a, s), canon(b, s));
    a == b && !matches!(a, Val::Unknown { .. } | Val::Computed { .. })
}

fn is_literal(v: &Val) -> bool {
    matches!(v, Val::Lit { .. })
}

/// Unique column sets of a table as property names, with partial indexes
/// marked.
pub fn unique_sets(index: &ProjectIndex, table: &str) -> Vec<(Vec<String>, Option<String>)> {
    let Some(t) = index.source_table(table) else {
        return Vec::new();
    };
    let prop = |sql: &str| {
        t.column_by_sql(sql)
            .map_or_else(|| sql.to_owned(), |c| c.property.clone())
    };
    let mut out = Vec::new();
    if !t.primary_key.is_empty() {
        out.push((t.primary_key.iter().map(|c| prop(c)).collect(), None));
    }
    for u in &t.unique {
        out.push((
            u.columns.iter().map(|c| prop(c)).collect(),
            u.predicate.clone(),
        ));
    }
    out
}

fn op(entry: usize, role: &str, e: &DbEffect) -> ConflictOp {
    ConflictOp {
        entry,
        role: role.to_owned(),
        operation: e.label(),
        location: e.src.label(),
        function: e.src.function.clone(),
        effect: e.index,
        is_external: false,
    }
}

fn base_assumptions(a: &Analyzed) -> Vec<Assumption> {
    let mut out = Vec::new();
    match a.invocation().as_str() {
        "wrapper-awaited-when-guards-pass" => {
            let w = a.info.as_ref().and_then(|i| i.wrapper.as_ref());
            out.push(Assumption::new(
                "precondition",
                "source-derived",
                format!(
                    "`{}` runs the callback only when its guards pass; the scenario assumes they do: {}",
                    w.map(|w| w.wrapper.as_str()).unwrap_or("the wrapper"),
                    w.map(|w| w.guards.join("; ")).unwrap_or_default()
                ),
            ));
        }
        "declared" => {
            if let Some(c) = &a.contract {
                out.push(Assumption::new(
                    "invocation",
                    "declared",
                    format!(
                        "invocation `{}`{}",
                        c.invocation.clone().unwrap_or_default(),
                        if c.preconditions.is_empty() {
                            String::new()
                        } else {
                            format!(" when {}", c.preconditions.join("; "))
                        }
                    ),
                ));
            }
        }
        _ => {}
    }
    out
}

/// Unknown operations of an entry, and whether one can execute on a path
/// through `ops` (before the first or between them).
fn unsupported(a: &Analyzed, ops: &[&DbEffect]) -> (Vec<String>, bool) {
    let s = &a.summary;
    let list: Vec<String> = s
        .unknown
        .iter()
        .map(|u| format!("{} ({})", u.detail, u.src.label()))
        .collect();
    let Some(last) = ops.iter().map(|e| e.at.1).max() else {
        return (list, !s.unknown.is_empty());
    };
    let on_path = s.unknown.iter().any(|u| {
        // An unknown op before the last conflict op on the main task, or in a
        // child task, can influence the property.
        u.at.0 != 0 || u.at.1 < last
    });
    (list, on_path)
}

struct Detector<'a> {
    index: &'a ProjectIndex,
    contracts: &'a Contracts,
    entries: &'a [Analyzed],
    out: Vec<Candidate>,
    rejected: Vec<Rejected>,
}

/// Run every detector over every entry, then pair entries through shared
/// resources.
pub fn detect(
    index: &ProjectIndex,
    entries: &[Analyzed],
    max_partners: usize,
) -> (Vec<Candidate>, Vec<Rejected>, usize) {
    let mut d = Detector {
        index,
        contracts: &index.contracts,
        entries,
        out: Vec::new(),
        rejected: Vec::new(),
    };
    for (i, a) in entries.iter().enumerate() {
        d.check_then_act(i, a);
        d.status_transition(i, a);
        d.read_modify_write(i, a);
        d.guarded_update(i, a);
        d.write_skew(i, a);
        d.duplicate_processing(i, a);
        d.external_gap(i, a);
        d.retried_effect(i, a);
        d.ambiguous_commit(i, a);
        d.lock_order(i, a);
        d.resubmitted_write(i, a);
        d.parent_child(i, a);
        d.declared_outcome(i, a);
    }
    let truncated = d.pair(max_partners);
    (d.out, d.rejected, truncated)
}

impl Detector<'_> {
    fn reject(&mut self, class: Class, entries: &[&Analyzed], reason: impl Into<String>, at: &str) {
        self.rejected.push(Rejected {
            class,
            entries: entries.iter().map(|a| a.fact.selector.clone()).collect(),
            reason: reason.into(),
            location: at.to_owned(),
        });
    }

    fn declared_for(
        &self,
        selector: &str,
        table: Option<&str>,
        channel: Option<&str>,
    ) -> Vec<&crate::contracts::DeclaredInvariant> {
        self.contracts
            .invariants
            .iter()
            .filter(|inv| {
                let applies = inv.entries.is_empty() || inv.entries.iter().any(|e| e == selector);
                let names = match &inv.property {
                    Template::AtMostOne { table: t, .. }
                    | Template::AtLeast { table: t, .. }
                    | Template::NonNegative { table: t, .. } => table == Some(t.as_str()),
                    Template::MaxExternal { channel: c, .. } => channel == Some(c.as_str()),
                    Template::Formula { .. }
                    | Template::AtMostOneSuccess { .. }
                    | Template::ExactlyOneSuccess { .. }
                    | Template::EverySuccessReturns { .. } => !inv.entries.is_empty(),
                };
                applies && names
            })
            .collect()
    }

    fn declared_properties(
        &self,
        a: &Analyzed,
        table: Option<&str>,
        channel: Option<&str>,
        key: &[(String, Val)],
    ) -> Vec<Property> {
        self.declared_for(&a.fact.selector, table, channel)
            .into_iter()
            .map(|inv| Property {
                name: inv.name.clone(),
                description: if inv.description.is_empty() {
                    format!("declared invariant `{}`", inv.name)
                } else {
                    inv.description.clone()
                },
                origin: Origin::UserDeclared,
                strength: Strength::ApplicationSafety,
                spec: match &inv.property {
                    Template::AtMostOne {
                        table,
                        key: k,
                        filter,
                    } => Spec::AtMostOne {
                        table: table.clone(),
                        key: k
                            .iter()
                            .map(|c| {
                                // Phase 15: a key column the check does not
                                // compare by equality (a conditional `IS
                                // NULL`/`=` test) takes the argument the
                                // entry's own INSERT writes to it.
                                let written = || {
                                    a.summary
                                        .db
                                        .iter()
                                        .filter(|e| e.access == Access::Insert && e.table == *table)
                                        .find_map(|e| match e.write_val(c) {
                                            Some(v @ Val::Param { .. }) => Some(v.clone()),
                                            _ => None,
                                        })
                                };
                                (
                                    c.clone(),
                                    key.iter()
                                        .find(|(kc, _)| kc == c)
                                        .map(|(_, v)| v.clone())
                                        .or_else(written)
                                        .unwrap_or(Val::Unknown { text: c.clone() }),
                                )
                            })
                            .collect(),
                        filter: filter.clone(),
                    },
                    Template::AtLeast {
                        table,
                        group_by,
                        filter,
                        min,
                    } => Spec::AtLeast {
                        table: table.clone(),
                        group_by: group_by.clone(),
                        filter: filter.clone(),
                        min: *min,
                    },
                    Template::NonNegative { table, column } => Spec::NonNegative {
                        table: table.clone(),
                        column: column.clone(),
                    },
                    Template::MaxExternal { channel, max } => Spec::MaxExternal {
                        channel: channel.clone(),
                        max: *max,
                        per_request: false,
                    },
                    Template::Formula { assert } => Spec::Formula {
                        formula: assert.clone(),
                    },
                    Template::EverySuccessReturns {
                        function,
                        argument: Some(path),
                        ..
                    } => Spec::ReturnsArgument {
                        function: function.clone(),
                        argument: Val::Param { path: path.clone() },
                    },
                    t @ (Template::AtMostOneSuccess { .. }
                    | Template::ExactlyOneSuccess { .. }
                    | Template::EverySuccessReturns { .. }) => Spec::Formula {
                        // `Contracts::parse` refuses the other forms.
                        formula: t
                            .returned_formula()
                            .expect("a validated return-value template"),
                    },
                },
                rationale: "declared in commitrift-contracts.json".to_owned(),
                evidence: Vec::new(),
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        class: Class,
        title: String,
        actors: Vec<&Analyzed>,
        resource: Resource,
        conflict: Vec<ConflictOp>,
        mut properties: Vec<Property>,
        protections: Vec<String>,
        mut assumptions: Vec<Assumption>,
        environment: Environment,
        mut inconclusive_if: Vec<String>,
        ops_of: &[Vec<&DbEffect>],
    ) {
        let mut entries: Vec<EntryRef> = Vec::new();
        let mut actor_entries = Vec::new();
        let mut unsupported_list = BTreeSet::new();
        let mut on_path = false;
        for (k, a) in actors.iter().enumerate() {
            let pos = match entries.iter().position(|e| e.selector == a.fact.selector) {
                Some(p) => p,
                None => {
                    entries.push(a.entry_ref());
                    for x in base_assumptions(a) {
                        if !assumptions.contains(&x) {
                            assumptions.push(x);
                        }
                    }
                    let (list, path) =
                        unsupported(a, ops_of.get(k).map(Vec::as_slice).unwrap_or(&[]));
                    unsupported_list.extend(list);
                    on_path |= path;
                    entries.len() - 1
                }
            };
            actor_entries.push(pos);
        }
        if environment.actors > 1 {
            let mut seen = BTreeSet::new();
            for a in &actors {
                if seen.insert(a.fact.selector.clone()) {
                    let (_, why) = a.concurrency();
                    if !assumptions.contains(&why) {
                        assumptions.push(why);
                    }
                }
            }
        }
        // Declared invariants replace hypotheses of the same table.
        if properties.iter().any(|p| p.origin == Origin::UserDeclared) {
            properties.retain(|p| p.origin != Origin::HeuristicHypothesis);
        }
        properties.sort_by(|a, b| a.origin.cmp(&b.origin).then(a.name.cmp(&b.name)));
        properties.dedup_by(|a, b| a.spec == b.spec);
        if on_path {
            inconclusive_if.insert(
                0,
                "an operation the extractor did not understand lies on the path (listed under `unsupported`)".to_owned(),
            );
        }
        inconclusive_if.push(
            "the exploration bounds are exceeded (the result is then `not established`)".to_owned(),
        );
        assumptions.sort();
        assumptions.dedup();
        let blocked = if properties.is_empty() {
            Some(match class {
                Class::WriteSkew => "no declared invariant: CommitRift does not infer a multi-row invariant (declare one in commitrift-contracts.json)".to_owned(),
                _ => "no property to check".to_owned(),
            })
        } else {
            None
        };
        self.out.push(Candidate {
            id: String::new(),
            fingerprint: String::new(),
            class,
            title,
            entries,
            actor_entries,
            resource,
            conflict,
            properties,
            protections,
            assumptions,
            unsupported: unsupported_list.into_iter().collect(),
            unsupported_on_path: on_path,
            blocked,
            environment,
            inconclusive_if,
            request_keys: Vec::new(),
        });
    }

    fn two(relation: Relation) -> Environment {
        Environment {
            actors: 2,
            relation,
            retry: None,
            max_crashes: 0,
            max_retries: 0,
            max_in_flight: 0,
        }
    }

    // -- A: check-then-act (insert) -----------------------------------------

    fn cta_key(
        &self,
        s: &EffectSummary,
        read: &DbEffect,
        insert: &DbEffect,
    ) -> (Vec<(String, Val)>, BTreeMap<String, Value>) {
        let mut key = Vec::new();
        let mut filter = BTreeMap::new();
        for (c, v) in &read.eq {
            let Some(w) = insert.write_val(c) else {
                // A literal filter the insert leaves to the column default.
                if let Val::Lit { value } = v
                    && let Some(t) = self.index.source_table(&insert.table)
                    && let Some(col) = t.column(c)
                    && let Some(commitrift_model::schema_facts::SchemaDefault::Constant {
                        value: d,
                    }) = &col.default
                    && literal_eq(d, value)
                {
                    filter.insert(c.clone(), value.clone());
                }
                continue;
            };
            if is_literal(v) {
                if v == w
                    && let Val::Lit { value } = v
                {
                    filter.insert(c.clone(), value.clone());
                }
            } else if same_value(v, w, s) {
                key.push((c.clone(), canon(v, s)));
            }
        }
        (key, filter)
    }

    fn check_then_act(&mut self, i: usize, a: &Analyzed) {
        let s = &a.summary;
        for read in
            s.db.iter()
                .filter(|e| matches!(e.access, Access::Read | Access::Count))
        {
            if read.opaque_predicate || read.eq.is_empty() || !read.joins.is_empty() {
                continue;
            }
            for insert in s.db.iter().filter(|e| {
                e.access == Access::Insert
                    && e.table == read.table
                    && s.reaches(read.index, e.index)
            }) {
                if s.deciding_guard(read.index, insert.at).is_none() {
                    continue;
                }
                let (key, filter) = self.cta_key(s, read, insert);
                if key.is_empty() {
                    self.reject(
                        Class::CheckThenAct,
                        &[a],
                        format!(
                            "the read of `{}` and the insert share no argument-dependent key column",
                            read.table
                        ),
                        &insert.src.label(),
                    );
                    continue;
                }
                self.cta_candidate(i, a, read, insert, key, filter);
            }
        }
    }

    fn cta_candidate(
        &mut self,
        _i: usize,
        a: &Analyzed,
        read: &DbEffect,
        insert: &DbEffect,
        key: Vec<(String, Val)>,
        filter: BTreeMap<String, Value>,
    ) {
        let s = &a.summary;
        let columns: Vec<String> = key
            .iter()
            .map(|(c, _)| c.clone())
            .chain(filter.keys().cloned())
            .collect();
        let mut protections = Vec::new();
        let mut schema_unique = None;
        for (set, predicate) in unique_sets(self.index, &insert.table) {
            if set.iter().all(|c| columns.contains(c)) {
                match predicate {
                    None => {
                        protections.push(format!(
                            "UNIQUE({}) on `{}` (schema): a second insert of the same key fails{}",
                            set.join(", "),
                            insert.table,
                            if insert.on_conflict == OnConflict::DoNothing {
                                " or, with ON CONFLICT DO NOTHING, inserts nothing"
                            } else {
                                " with SQLSTATE 23505"
                            }
                        ));
                        schema_unique = Some(set);
                    }
                    Some(p) => protections.push(format!(
                        "partial unique index on ({}) WHERE {p}: not modeled as a global constraint",
                        set.join(", ")
                    )),
                }
            }
        }
        if let Some(t) = read.tx.and_then(|t| s.transactions.get(t))
            && insert.tx == read.tx
            && t.isolation == Some(Isolation::Serializable)
        {
            protections.push("check and insert run in one SERIALIZABLE transaction".to_owned());
        }
        let key_text = key
            .iter()
            .map(|(c, v)| format!("{c} = {}", v.render()))
            .chain(filter.iter().map(|(c, v)| format!("{c} = {}", v.render())))
            .collect::<Vec<_>>()
            .join(", ");
        let mut properties = Vec::new();
        let guard = s.deciding_guard(read.index, insert.at);
        let spec = Spec::AtMostOne {
            table: insert.table.clone(),
            key: key.clone(),
            filter: filter.clone(),
        };
        match &schema_unique {
            Some(set) => properties.push(Property {
                name: format!("unique-{}-{}", insert.table, set.join("-")),
                description: format!(
                    "at most one `{}` row per ({}) — enforced by the database",
                    insert.table,
                    set.join(", ")
                ),
                origin: Origin::SchemaEnforced,
                strength: Strength::DatabaseIntegrity,
                spec: spec.clone(),
                rationale: format!(
                    "UNIQUE({}) in the analyzed schema covers the key the code checks",
                    set.join(", ")
                ),
                evidence: vec![format!("schema: {}", insert.table)],
            }),
            None => properties.push(Property {
                name: format!("at-most-one-{}-per-key", insert.table),
                description: format!(
                    "at most one `{}` row per ({key_text})",
                    insert.table
                ),
                origin: Origin::HeuristicHypothesis,
                strength: Strength::ApplicationSafety,
                spec: spec.clone(),
                rationale: format!(
                    "the code inserts only after a read finds no row with this key ({}); that suggests, but does not establish, that the application intends one row per key. No supported UNIQUE constraint on these columns was found in the analyzed schema",
                    guard.map(|g| g.src.label()).unwrap_or_default()
                ),
                evidence: guard
                    .map(|g| vec![format!("guard `{}` at {}", g.condition, g.src.label())])
                    .unwrap_or_default(),
            }),
        }
        properties.extend(self.declared_properties(a, Some(&insert.table), None, &key));
        let (concurrent, _) = a.concurrency();
        if !concurrent {
            self.reject(
                Class::CheckThenAct,
                &[a],
                "declared non-concurrent",
                &insert.src.label(),
            );
            return;
        }
        let title = format!(
            "concurrent check-then-insert on `{}` ({key_text})",
            insert.table
        );
        self.push(
            Class::CheckThenAct,
            title,
            vec![a, a],
            Resource {
                kind: "unique-key".to_owned(),
                table: insert.table.clone(),
                columns,
                text: format!("{}({key_text})", insert.table),
            },
            vec![op(0, "check", read), op(0, "act", insert)],
            properties,
            protections,
            Vec::new(),
            Self::two(Relation::Same),
            vec!["the intended uniqueness of the key is not established by the source".to_owned()],
            &[vec![read, insert], vec![read, insert]],
        );
    }

    // -- B: status transition ---------------------------------------------

    fn status_transition(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        for read in
            s.db.iter()
                .filter(|e| e.access == Access::Read && e.joins.is_empty())
        {
            if read.eq.is_empty() {
                continue;
            }
            for guard in s.guards.iter().filter(|g| g.reads.contains(&read.index)) {
                // Tested states: compared with a literal in the guard, or
                // required by the read's own WHERE clause (`status =
                // 'pending'`) when the guard tests whether a row came back.
                let mut tested: Vec<String> = guard.state_tests.clone();
                for (c, v) in &read.eq {
                    if matches!(v, Val::Lit { .. }) && !tested.contains(c) {
                        tested.push(c.clone());
                    }
                }
                if tested.is_empty() {
                    continue;
                }
                // The unconditional transition write: a tested state column
                // set to another literal state, on the row the read found.
                let Some(write) = s.db.iter().find(|w| {
                    w.access == Access::Update
                        && w.table == read.table
                        && s.reaches(read.index, w.index)
                        && w.writes.iter().any(|(c, v)| {
                            tested.contains(c)
                                && matches!(v, Val::Lit { .. })
                                && read.eq_val(c) != Some(v)
                        })
                        && read
                            .eq
                            .iter()
                            .filter(|(_, v)| !matches!(v, Val::Lit { .. }))
                            .all(|(c, v)| {
                                w.eq_val(c).is_some_and(|x| same_value(x, v, s))
                                    || w.eq.iter().any(|(_, x)| same_value(x, v, s))
                            })
                        && s.deciding_guard(read.index, w.at).is_some()
                }) else {
                    continue;
                };
                let guarded: Vec<&String> = write
                    .writes
                    .iter()
                    .map(|(c, _)| c)
                    .filter(|c| tested.contains(c))
                    .collect();
                let conditional = write.eq.iter().any(|(c, _)| guarded.contains(&c))
                    || write
                        .other
                        .iter()
                        .any(|(_, cols)| cols.iter().any(|c| guarded.contains(&c)));
                if conditional && write.result_checked {
                    self.reject(
                        Class::StatusTransition,
                        &[a],
                        "the transition is a conditional update whose row count is checked",
                        &write.src.label(),
                    );
                    continue;
                }
                let locked = read.lock.is_exclusive() && read.tx.is_some() && read.tx == write.tx;
                // Consequences: other effects the same guard decides.
                let code = &s.codes[0];
                let side = |pc: usize| {
                    let a = crate::effects::cfg_reach_from(code, guard.targets.0).contains(&pc);
                    let b = crate::effects::cfg_reach_from(code, guard.targets.1).contains(&pc);
                    a != b
                };
                let inserts: Vec<&DbEffect> =
                    s.db.iter()
                        .filter(|e| e.access == Access::Insert && e.at.0 == 0 && side(e.at.1))
                        .collect();
                let externals: Vec<&crate::effects::ExternalEffect> = s
                    .externals
                    .iter()
                    .filter(|x| x.at.0 == 0 && side(x.at.1))
                    .collect();
                if inserts.is_empty() && externals.is_empty() {
                    self.reject(
                        Class::StatusTransition,
                        &[a],
                        format!(
                            "a repeated transition of `{}`.{} has no observable consequence (no insert or external effect depends on it)",
                            write.table,
                            guarded.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",")
                        ),
                        &write.src.label(),
                    );
                    continue;
                }
                let mut properties = Vec::new();
                let mut conflict = vec![op(0, "check", read), op(0, "transition", write)];
                let mut dedup_channels: Vec<String> = Vec::new();
                for ins in &inserts {
                    // The inserted row's key: what it takes from the
                    // transition's context (arguments, the row read).
                    let key: Vec<(String, Val)> = ins
                        .writes
                        .iter()
                        .map(|(c, v)| (c.clone(), canon(v, s)))
                        .filter(|(_, v)| match v {
                            Val::Param { .. } => true,
                            Val::Read { effect, .. } => *effect == read.index,
                            _ => false,
                        })
                        .collect();
                    if key.is_empty() {
                        continue;
                    }
                    conflict.push(op(0, "consequence", ins));
                    properties.push(Property {
                        name: format!("one-{}-per-transition", ins.table),
                        description: format!(
                            "at most one `{}` row per ({}) — the transition of `{}` happens once",
                            ins.table,
                            key.iter().map(|(c, v)| format!("{c} = {}", v.render())).collect::<Vec<_>>().join(", "),
                            read.table
                        ),
                        origin: Origin::HeuristicHypothesis,
                        strength: Strength::ApplicationSafety,
                        spec: Spec::AtMostOne {
                            table: ins.table.clone(),
                            key,
                            filter: BTreeMap::new(),
                        },
                        rationale: format!(
                            "the insert runs only after the guard `{}` ({}) saw the row in its old state; that suggests one insert per transition, which the source does not establish",
                            guard.condition,
                            guard.src.label()
                        ),
                        evidence: vec![format!("guard at {}", guard.src.label())],
                    });
                }
                for x in &externals {
                    conflict.push(ConflictOp {
                        entry: 0,
                        role: "consequence".to_owned(),
                        operation: format!("external effect on `{}`", x.channel),
                        location: x.src.label(),
                        function: x.src.function.clone(),
                        effect: x.index,
                        is_external: true,
                    });
                    if self.contracts.channel_dedup(&x.channel) == Some("byKey") && x.key.is_some()
                    {
                        dedup_channels.push(x.channel.clone());
                    }
                    properties.push(Property {
                        name: format!("one-{}-per-transition", x.channel),
                        description: format!(
                            "at most one effect on `{}` — the transition of `{}` happens once",
                            x.channel, read.table
                        ),
                        origin: Origin::HeuristicHypothesis,
                        strength: Strength::ApplicationSafety,
                        spec: Spec::MaxExternal {
                            channel: x.channel.clone(),
                            max: 1,
                    per_request: false,
                        },
                        rationale: format!(
                            "the effect runs only after the guard `{}` ({}) saw the row in its old state",
                            guard.condition,
                            guard.src.label()
                        ),
                        evidence: vec![format!("guard at {}", guard.src.label())],
                    });
                }
                if properties.is_empty() {
                    continue;
                }
                let mut protections: Vec<String> = dedup_channels
                    .iter()
                    .map(|ch| {
                        format!("the effect on `{ch}` carries an idempotency key and the channel deduplicates by key (declared)")
                    })
                    .collect();
                if locked {
                    protections.push(
                        "the row is read FOR UPDATE in the transaction that changes it".to_owned(),
                    );
                }
                if conditional {
                    protections.push(
                        "the transition's WHERE clause re-checks the state (row count not checked)"
                            .to_owned(),
                    );
                }
                let key_text = read
                    .eq
                    .iter()
                    .map(|(c, v)| format!("{c} = {}", v.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                let (concurrent, _) = a.concurrency();
                if !concurrent {
                    continue;
                }
                let ops: Vec<&DbEffect> = vec![read, write];
                self.push(
                    Class::StatusTransition,
                    format!(
                        "concurrent transition of `{}`.{} ({key_text})",
                        read.table,
                        guarded.iter().map(|c| c.as_str()).collect::<Vec<_>>().join(",")
                    ),
                    vec![a, a],
                    Resource {
                        kind: "row".to_owned(),
                        table: read.table.clone(),
                        columns: guarded.iter().map(|c| (*c).clone()).collect(),
                        text: format!("{}({key_text})", read.table),
                    },
                    conflict,
                    properties,
                    protections,
                    Vec::new(),
                    Self::two(Relation::Same),
                    vec!["whether a repeated transition is wrong for the application is not established".to_owned()],
                    &[ops.clone(), ops],
                );
                return;
            }
        }
    }

    // -- C: read-modify-write -----------------------------------------------

    /// (read, update, column) triples of an entry.
    fn rmw_sites(s: &EffectSummary) -> Vec<(&DbEffect, &DbEffect, String)> {
        let mut out = Vec::new();
        for read in
            s.db.iter()
                .filter(|e| e.access == Access::Read && e.joins.is_empty())
        {
            if read.eq.is_empty() {
                continue;
            }
            for upd in s.db.iter().filter(|u| {
                u.access == Access::Update
                    && u.table == read.table
                    && s.reaches(read.index, u.index)
            }) {
                let same_row =
                    read.eq
                        .iter()
                        .all(|(c, v)| upd.eq_val(c).is_some_and(|x| same_value(x, v, s)))
                        || upd.eq.iter().any(|(c, x)| {
                            read.eq.iter().any(|(rc, v)| rc == c && same_value(x, v, s))
                        }) && !read.eq.is_empty();
                if !same_row {
                    continue;
                }
                for (c, v) in &upd.writes {
                    if matches!(v, Val::ColumnRelative { .. }) {
                        continue;
                    }
                    let mut leaves = Vec::new();
                    v.leaves(&mut leaves);
                    if leaves
                        .iter()
                        .any(|l| matches!(l, Val::Read { effect, .. } if *effect == read.index))
                    {
                        out.push((read, upd, c.clone()));
                    }
                }
            }
        }
        out
    }

    fn read_modify_write(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        for (read, upd, column) in Self::rmw_sites(s) {
            let mut protections = Vec::new();
            if read.lock.is_exclusive() && read.tx.is_some() && read.tx == upd.tx {
                protections.push(
                    "the row is read FOR UPDATE in the transaction that writes it".to_owned(),
                );
            }
            let checks_old = upd.eq.iter().any(|(c, _)| *c == column)
                || upd.other.iter().any(|(_, cols)| cols.contains(&column));
            if checks_old {
                protections.push(format!(
                    "the UPDATE re-checks `{column}` in its WHERE clause{}",
                    if upd.result_checked {
                        " and the row count is checked"
                    } else {
                        ""
                    }
                ));
            }
            if let Some(t) = read.tx.and_then(|t| s.transactions.get(t))
                && upd.tx == read.tx
                && matches!(
                    t.isolation,
                    Some(Isolation::RepeatableRead | Isolation::Serializable)
                )
            {
                protections.push(format!(
                    "read and write run in one {} transaction",
                    t.isolation.map(Isolation::sql).unwrap_or_default()
                ));
            }
            let key: Vec<(String, Val)> = read
                .eq
                .iter()
                .map(|(c, v)| (c.clone(), canon(v, s)))
                .collect();
            let key_text = key
                .iter()
                .map(|(c, v)| format!("{c} = {}", v.render()))
                .collect::<Vec<_>>()
                .join(", ");
            let (concurrent, _) = a.concurrency();
            if !concurrent {
                continue;
            }
            let written = upd.write_val(&column).map(Val::render).unwrap_or_default();
            let mut properties = vec![Property {
                name: format!("serial-{}-{}", upd.table, column),
                description: format!(
                    "`{}.{column}` of the row ({key_text}) ends with a value some serial execution of the same requests produces",
                    upd.table
                ),
                origin: Origin::HeuristicHypothesis,
                strength: Strength::ConcurrencyRelational,
                spec: Spec::SerialCell {
                    table: upd.table.clone(),
                    column: column.clone(),
                    key: key.clone(),
                },
                rationale: format!(
                    "`{column}` is written as `{written}`, computed from the value read at {}; a concurrent update between the read and the write would be overwritten. Whether the application requires serial outcomes is not established",
                    read.src.label()
                ),
                evidence: vec![format!(
                    "read at {}, write at {}",
                    read.src.label(),
                    upd.src.label()
                )],
            }];
            properties.extend(self.declared_properties(a, Some(&upd.table), None, &key));
            self.push(
                Class::ReadModifyWrite,
                format!("read-modify-write of `{}.{column}` ({key_text})", upd.table),
                vec![a, a],
                Resource {
                    kind: "column".to_owned(),
                    table: upd.table.clone(),
                    columns: vec![column.clone()],
                    text: format!("{}.{column}({key_text})", upd.table),
                },
                vec![op(0, "read", read), op(0, "write", upd)],
                properties,
                protections,
                Vec::new(),
                Self::two(Relation::Same),
                vec![
                    "whether a non-serial outcome is wrong for the application is not established"
                        .to_owned(),
                ],
                &[vec![read, upd], vec![read, upd]],
            );
        }
    }

    /// Phase 15: a guard tests a column of a row the entry read, and an
    /// UPDATE of the same row then changes that column relatively
    /// (`remaining = remaining - 1`): the database computes the new value
    /// atomically, but the test was made on the value read. Only with a
    /// declared property over the table (as the return-value family): the
    /// source alone does not say which values are wrong.
    fn guarded_update(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        let mut seen = BTreeSet::new();
        for g in &s.guards {
            for &ri in &g.reads {
                let Some(read) = s.db.get(ri) else { continue };
                if read.access != Access::Read || !read.joins.is_empty() || read.eq.is_empty() {
                    continue;
                }
                for upd in s.db.iter().filter(|u| {
                    u.access == Access::Update
                        && u.table == read.table
                        && s.reaches(read.index, u.index)
                }) {
                    let same_row = read
                        .eq
                        .iter()
                        .all(|(c, v)| upd.eq_val(c).is_some_and(|x| same_value(x, v, s)));
                    if !same_row {
                        continue;
                    }
                    for (column, v) in &upd.writes {
                        if !matches!(v, Val::ColumnRelative { .. }) || !g.columns.contains(column) {
                            continue;
                        }
                        if !seen.insert((read.index, upd.index, column.clone())) {
                            continue;
                        }
                        let key: Vec<(String, Val)> = read
                            .eq
                            .iter()
                            .map(|(c, v)| (c.clone(), canon(v, s)))
                            .collect();
                        let properties = self.declared_properties(a, Some(&upd.table), None, &key);
                        if properties.is_empty() {
                            self.reject(
                                Class::CheckThenAct,
                                &[a, a],
                                format!(
                                    "the guard at {} tests `{column}` before a relative update of it, but no property over `{}` is declared (this family is never inferred)",
                                    g.src.label(),
                                    upd.table
                                ),
                                &upd.src.label(),
                            );
                            continue;
                        }
                        let (concurrent, _) = a.concurrency();
                        if !concurrent {
                            continue;
                        }
                        let key_text = key
                            .iter()
                            .map(|(c, v)| format!("{c} = {}", v.render()))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let mut protections = Vec::new();
                        if read.lock.is_exclusive() && read.tx.is_some() && read.tx == upd.tx {
                            protections.push(
                                "the row is read FOR UPDATE in the transaction that writes it"
                                    .to_owned(),
                            );
                        }
                        if upd.other.iter().any(|(_, cols)| cols.contains(column))
                            || upd.eq.iter().any(|(c, _)| c == column)
                        {
                            protections.push(format!(
                                "the UPDATE re-checks `{column}` in its WHERE clause"
                            ));
                        }
                        self.push(
                            Class::CheckThenAct,
                            format!(
                                "guarded relative update of `{}.{column}` ({key_text})",
                                upd.table
                            ),
                            vec![a, a],
                            Resource {
                                kind: "column".to_owned(),
                                table: upd.table.clone(),
                                columns: vec![column.clone()],
                                text: format!("{}.{column}({key_text})", upd.table),
                            },
                            vec![op(0, "read", read), op(0, "write", upd)],
                            properties,
                            protections,
                            Vec::new(),
                            Self::two(Relation::Same),
                            Vec::new(),
                            &[vec![read, upd], vec![read, upd]],
                        );
                    }
                }
            }
        }
    }

    // -- D: write skew --------------------------------------------------------

    fn write_skew(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        for read in
            s.db.iter()
                .filter(|e| matches!(e.access, Access::Count | Access::Read))
        {
            let pred_cols: BTreeSet<&String> = read
                .eq
                .iter()
                .map(|(c, _)| c)
                .chain(read.other.iter().flat_map(|(_, cols)| cols.iter()))
                .collect();
            if pred_cols.is_empty() {
                continue;
            }
            let pk: Vec<String> = unique_sets(self.index, &read.table)
                .into_iter()
                .next()
                .map(|(k, _)| k)
                .unwrap_or_default();
            // A read of one row by its primary key is not a multi-row predicate.
            if !pk.is_empty() && pk.iter().all(|c| read.eq.iter().any(|(rc, _)| rc == c)) {
                continue;
            }
            for w in s.db.iter().filter(|w| {
                w.access.writes()
                    && w.table == read.table
                    && s.reaches(read.index, w.index)
                    && s.deciding_guard(read.index, w.at).is_some()
            }) {
                // Inserting the very key that was checked is a
                // check-then-insert, not a write to another row.
                if w.access == Access::Insert
                    && !read.eq.is_empty()
                    && read.eq.iter().all(|(c, v)| {
                        w.write_val(c)
                            .is_some_and(|x| same_value(x, v, s) || x == v)
                    })
                {
                    continue;
                }
                let changes_pred = w.writes.iter().any(|(c, _)| pred_cols.contains(c))
                    || w.access == Access::Delete
                    || w.access == Access::Insert;
                if !changes_pred {
                    continue;
                }
                // The written row is chosen by an argument, not by the read.
                let other_row =
                    w.eq.iter()
                        .any(|(c, v)| pk.contains(c) && matches!(canon(v, s), Val::Param { .. }))
                        || w.access == Access::Insert;
                if !other_row {
                    continue;
                }
                let key: Vec<(String, Val)> = Vec::new();
                let properties = self.declared_properties(a, Some(&read.table), None, &key);
                let mut protections = Vec::new();
                if let Some(t) = read.tx.and_then(|t| s.transactions.get(t))
                    && w.tx == read.tx
                    && t.isolation == Some(Isolation::Serializable)
                {
                    protections
                        .push("read and write run in one SERIALIZABLE transaction".to_owned());
                }
                let pred_text = read
                    .eq
                    .iter()
                    .map(|(c, v)| format!("{c} = {}", v.render()))
                    .chain(read.other.iter().map(|(t, _)| t.clone()))
                    .collect::<Vec<_>>()
                    .join(" AND ");
                self.push(
                    Class::WriteSkew,
                    format!(
                        "write skew on `{}` (read {pred_text}, write another row)",
                        read.table
                    ),
                    vec![a, a],
                    Resource {
                        kind: "row-predicate".to_owned(),
                        table: read.table.clone(),
                        columns: pred_cols.iter().map(|c| (*c).clone()).collect(),
                        text: format!("{} WHERE {pred_text}", read.table),
                    },
                    vec![op(0, "read", read), op(0, "write", w)],
                    properties,
                    protections,
                    Vec::new(),
                    Self::two(Relation::DistinctRow),
                    vec!["without a declared invariant there is no property to check".to_owned()],
                    &[vec![read, w], vec![read, w]],
                );
                return;
            }
        }
    }

    // -- E: duplicate processing (declared at-least-once delivery) ---------

    fn duplicate_processing(&mut self, _i: usize, a: &Analyzed) {
        let Some(contract) = &a.contract else { return };
        if contract.delivery.as_deref() != Some("at-least-once") {
            return;
        }
        let s = &a.summary;
        let mut properties = Vec::new();
        let mut conflict = Vec::new();
        let mut protections = Vec::new();
        for x in &s.externals {
            conflict.push(ConflictOp {
                entry: 0,
                role: "external".to_owned(),
                operation: format!("external effect on `{}`", x.channel),
                location: x.src.label(),
                function: x.src.function.clone(),
                effect: x.index,
                is_external: true,
            });
            if self.contracts.channel_dedup(&x.channel) == Some("byKey") && x.key.is_some() {
                protections.push(format!(
                    "`{}` carries an idempotency key and the channel deduplicates by key (declared)",
                    x.channel
                ));
            }
            properties.push(Property {
                name: format!("one-{}-per-message", x.channel),
                description: format!(
                    "a message causes at most one effect on `{}`, however often it is delivered",
                    x.channel
                ),
                origin: Origin::FrameworkContract,
                strength: Strength::RecoverySafety,
                spec: Spec::MaxExternal {
                    channel: x.channel.clone(),
                    max: 1,
                    per_request: false,
                },
                rationale:
                    "the declared delivery is at-least-once: a consumer must tolerate redelivery"
                        .to_owned(),
                evidence: vec![format!("delivery contract of {}", a.fact.selector)],
            });
        }
        for ins in s.db.iter().filter(|e| e.access == Access::Insert) {
            // Phase 14: a redelivered message repeats its database writes
            // too; a property over them must be declared (none is inferred).
            let written: Vec<(String, Val)> = ins
                .writes
                .iter()
                .map(|(c, v)| (c.clone(), canon(v, s)))
                .collect();
            let declared = self.declared_properties(a, Some(&ins.table), None, &written);
            if !declared.is_empty() {
                conflict.push(op(0, "redelivered-write", ins));
                properties.extend(declared);
            }
            let cols: Vec<String> = ins.writes.iter().map(|(c, _)| c.clone()).collect();
            let unique = unique_sets(self.index, &ins.table)
                .into_iter()
                .any(|(set, p)| p.is_none() && set.iter().all(|c| cols.contains(c)));
            if unique {
                protections.push(format!(
                    "the insert into `{}` is keyed by a unique constraint{}",
                    ins.table,
                    if ins.on_conflict == OnConflict::DoNothing {
                        " (ON CONFLICT DO NOTHING)"
                    } else {
                        ""
                    }
                ));
            }
        }
        properties.extend(self.declared_properties(a, None, None, &[]));
        if conflict.is_empty() {
            return;
        }
        let ops: Vec<&DbEffect> = s.db.iter().collect();
        let mut env = Self::two(Relation::Same);
        env.retry = Some("at-least-once".to_owned());
        env.max_crashes = 1;
        env.max_retries = 1;
        let channels: Vec<String> = s.externals.iter().map(|x| x.channel.clone()).collect();
        self.push(
            Class::DuplicateProcessing,
            format!(
                "redelivered message repeats `{}`",
                if channels.is_empty() {
                    conflict
                        .iter()
                        .map(|o| o.operation.clone())
                        .collect::<Vec<_>>()
                        .join("`, `")
                } else {
                    channels.join("`, `")
                }
            ),
            vec![a, a],
            match (channels.is_empty(), conflict.iter().find(|o| o.role == "redelivered-write")) {
                (true, Some(w)) => {
                    let table = s.db[w.effect].table.clone();
                    Resource {
                        kind: "table".to_owned(),
                        text: table.clone(),
                        table,
                        columns: Vec::new(),
                    }
                }
                _ => Resource {
                    kind: "external-channel".to_owned(),
                    table: String::new(),
                    columns: channels.clone(),
                    text: channels.join(","),
                },
            },
            conflict,
            properties,
            protections,
            vec![Assumption::new(
                "delivery",
                "declared",
                "at-least-once: a message is redelivered after a crash or a lost acknowledgement, and two deliveries can overlap",
            )],
            env,
            Vec::new(),
            &[ops.clone(), ops],
        );
    }

    // -- F': an external effect inside a transaction the code retries ------

    fn retried_effect(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        for x in &s.externals {
            let Some(t) = x.tx.and_then(|t| s.transactions.get(t)) else {
                continue;
            };
            if t.retries == 0 {
                continue;
            }
            let (concurrent, _) = a.concurrency();
            if !concurrent {
                continue;
            }
            let ops: Vec<&DbEffect> = s.db.iter().filter(|e| e.tx == x.tx).collect();
            let mut conflict = vec![ConflictOp {
                entry: 0,
                role: "external".to_owned(),
                operation: format!("external effect on `{}` inside the transaction", x.channel),
                location: x.src.label(),
                function: x.src.function.clone(),
                effect: x.index,
                is_external: true,
            }];
            conflict.extend(
                ops.iter()
                    .filter(|e| e.access.writes())
                    .map(|e| op(0, "write", e)),
            );
            let mut protections = Vec::new();
            if self.contracts.channel_dedup(&x.channel) == Some("byKey") && x.key.is_some() {
                protections.push(format!(
                    "the effect carries an idempotency key and `{}` deduplicates by key (declared)",
                    x.channel
                ));
            }
            let isolation = t
                .isolation
                .map_or("the scenario's isolation", Isolation::sql);
            let mut properties = vec![Property {
                name: format!("one-{}-per-request", x.channel),
                description: format!(
                    "each request causes at most one `{}` effect, retried transactions included",
                    x.channel
                ),
                origin: Origin::HeuristicHypothesis,
                strength: Strength::RecoverySafety,
                spec: Spec::MaxExternal {
                    channel: x.channel.clone(),
                    max: 1,
                    per_request: true,
                },
                rationale: format!(
                    "the effect at {} runs inside a transaction that is re-run up to {} time(s) after a serialization failure or deadlock ({isolation}); a re-run repeats the effect. That a request should cause one effect is a hypothesis",
                    x.src.label(),
                    t.retries
                ),
                evidence: vec![format!(
                    "transaction at {:?} with {} retries",
                    t.begin, t.retries
                )],
            }];
            properties.extend(self.declared_properties(a, None, Some(&x.channel), &[]));
            self.push(
                Class::ExternalEffectGap,
                format!(
                    "`{}` inside a transaction that is retried",
                    x.channel
                ),
                vec![a, a],
                Resource {
                    kind: "external-channel".to_owned(),
                    table: String::new(),
                    columns: vec![x.channel.clone()],
                    text: x.channel.clone(),
                },
                conflict,
                properties,
                protections,
                vec![Assumption::new(
                    "retry",
                    "source-derived",
                    format!(
                        "the transaction helper re-runs the callback (up to {} retries, from the source or a declared helper)",
                        t.retries
                    ),
                )],
                Self::two(Relation::Same),
                Vec::new(),
                &[ops.clone(), ops],
            );
            return;
        }
    }

    // -- F: external effect and database writes (declared retry) -----------

    fn external_gap(&mut self, _i: usize, a: &Analyzed) {
        let retry = a.contract.as_ref().and_then(|c| c.retry.clone());
        let s = &a.summary;
        for x in s.externals.iter().filter(|x| x.at.0 == 0) {
            let writes_after: Vec<&DbEffect> =
                s.db.iter()
                    .filter(|e| {
                        e.access.writes()
                            && e.at.0 == 0
                            && crate::effects::cfg_reach(&s.codes[0], x.at.1).contains(&e.at.1)
                    })
                    .collect();
            if writes_after.is_empty() {
                continue;
            }
            let Some(retry) = retry.clone() else {
                self.reject(
                    Class::ExternalEffectGap,
                    &[a],
                    format!(
                        "`{}` is followed by database writes, but no retry or redelivery of the entry is declared (a crash alone repeats nothing)",
                        x.channel
                    ),
                    &x.src.label(),
                );
                return;
            };
            let mut protections = Vec::new();
            if self.contracts.channel_dedup(&x.channel) == Some("byKey") && x.key.is_some() {
                protections.push(format!(
                    "the effect carries an idempotency key ({}) and `{}` deduplicates by key (declared)",
                    x.key.as_ref().map(Val::render).unwrap_or_default(),
                    x.channel
                ));
            }
            let mut conflict = vec![ConflictOp {
                entry: 0,
                role: "external".to_owned(),
                operation: format!("external effect on `{}`", x.channel),
                location: x.src.label(),
                function: x.src.function.clone(),
                effect: x.index,
                is_external: true,
            }];
            conflict.extend(writes_after.iter().map(|w| op(0, "write-after", w)));
            let mut properties = vec![Property {
                name: format!("one-effective-{}", x.channel),
                description: format!(
                    "one request causes at most one effective `{}` effect, retries included",
                    x.channel
                ),
                origin: Origin::HeuristicHypothesis,
                strength: Strength::RecoverySafety,
                spec: Spec::MaxExternal {
                    channel: x.channel.clone(),
                    max: 1,
                    per_request: false,
                },
                rationale: format!(
                    "the effect at {} is not atomic with the writes after it; a retried request repeats it. That one request should cause one effect is a hypothesis",
                    x.src.label()
                ),
                evidence: Vec::new(),
            }];
            properties.extend(self.declared_properties(a, None, Some(&x.channel), &[]));
            let env = Environment {
                actors: 1,
                relation: Relation::Same,
                retry: Some(retry.clone()),
                max_crashes: 1,
                max_retries: 1,
                max_in_flight: 0,
            };
            let ops = writes_after.clone();
            self.push(
                Class::ExternalEffectGap,
                format!(
                    "crash between `{}` and the database writes, then a retry",
                    x.channel
                ),
                vec![a],
                Resource {
                    kind: "external-channel".to_owned(),
                    table: String::new(),
                    columns: vec![x.channel.clone()],
                    text: x.channel.clone(),
                },
                conflict,
                properties,
                protections,
                vec![Assumption::new(
                    "retry",
                    "declared",
                    format!("the caller retries the request ({retry})"),
                )],
                env,
                Vec::new(),
                &[ops],
            );
            return;
        }
    }

    // -- Phase 12: a committed insert repeated by a declared retry --------

    /// An entry the caller re-submits after a crash (declared `retry`)
    /// performs an INSERT: when the crash comes after the insert committed,
    /// the retry inserts again. Whether that is a duplicate depends on the
    /// key: a generated key is new on every execution (no collision); a key
    /// from the request collides with the first insert. A candidate is made
    /// only with a property to check: a declared invariant on the table, or
    /// a unique constraint on the written columns (schema-enforced).
    fn resubmitted_write(&mut self, _i: usize, a: &Analyzed) {
        let Some(retry) = a.contract.as_ref().and_then(|c| c.retry.clone()) else {
            return;
        };
        if !retry.contains("crash") {
            return;
        }
        let s = &a.summary;
        if !s.externals.is_empty() {
            // `external_gap` covers entries with external effects.
            return;
        }
        for e in
            s.db.iter()
                .filter(|e| e.access == Access::Insert && e.at.0 == 0)
        {
            let cols: Vec<String> = e.writes.iter().map(|(c, _)| c.clone()).collect();
            let mut properties = self.declared_properties(a, Some(&e.table), None, &e.writes);
            let mut protections = Vec::new();
            if let Some((set, _)) = unique_sets(self.index, &e.table)
                .into_iter()
                .find(|(set, p)| p.is_none() && set.iter().all(|c| cols.contains(c)))
            {
                protections.push(format!(
                    "UNIQUE({}) on `{}`: a repeated insert with the same values fails",
                    set.join(", "),
                    e.table
                ));
                properties.push(Property {
                    name: format!("unique-{}-{}", e.table, set.join("-")),
                    description: format!("at most one `{}` row per ({})", e.table, set.join(", ")),
                    origin: Origin::SchemaEnforced,
                    strength: Strength::DatabaseIntegrity,
                    spec: Spec::AtMostOne {
                        table: e.table.clone(),
                        key: set
                            .iter()
                            .map(|c| {
                                (
                                    c.clone(),
                                    e.write_val(c)
                                        .cloned()
                                        .unwrap_or(Val::Unknown { text: c.clone() }),
                                )
                            })
                            .collect(),
                        filter: BTreeMap::new(),
                    },
                    rationale: format!("UNIQUE({}) in the analyzed schema", set.join(", ")),
                    evidence: Vec::new(),
                });
            }
            if properties.is_empty() {
                self.reject(
                    Class::ResubmittedWrite,
                    &[a],
                    format!(
                        "the insert into `{}` is repeated by the declared retry, but no invariant is declared for the table and no unique constraint covers its columns",
                        e.table
                    ),
                    &e.src.label(),
                );
                continue;
            }
            let env = Environment {
                actors: 1,
                relation: Relation::Same,
                retry: Some(retry.clone()),
                max_crashes: 1,
                max_retries: 1,
                max_in_flight: 0,
            };
            self.push(
                Class::ResubmittedWrite,
                format!("crash after the insert into `{}`, then a retry", e.table),
                vec![a],
                Resource {
                    kind: "table".to_owned(),
                    table: e.table.clone(),
                    columns: cols.clone(),
                    text: e.table.clone(),
                },
                vec![op(0, "insert", e)],
                properties,
                protections,
                vec![Assumption::new(
                    "retry",
                    "declared",
                    format!("the caller retries the request ({retry})"),
                )],
                env,
                Vec::new(),
                &[vec![e]],
            );
            return;
        }
    }

    // -- G: ambiguous COMMIT ---------------------------------------------

    fn ambiguous_commit(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        // Phase 14: also a write inside a transaction whose COMMIT is in the
        // `try` block (the acknowledgment of the COMMIT can be lost), as a
        // retry wrapper's callback produces.
        for first in s.db.iter().filter(|e| {
            e.access.writes()
                && e.in_try
                && (e.conn == Conn::Pool || (e.conn == Conn::Tx && e.tx.is_some()))
        }) {
            let Some(second) = s.db.iter().find(|e| {
                e.in_handler
                    && e.conn == first.conn
                    && e.access == first.access
                    && e.table == first.table
                    && e.writes.len() == first.writes.len()
                    && e.writes
                        .iter()
                        .all(|(c, v)| first.write_val(c).is_some_and(|f| same_value(f, v, s)))
            }) else {
                continue;
            };
            let cols: Vec<String> = first.writes.iter().map(|(c, _)| c.clone()).collect();
            let unique = unique_sets(self.index, &first.table)
                .into_iter()
                .find(|(set, p)| p.is_none() && set.iter().all(|c| cols.contains(c)));
            let mut protections = Vec::new();
            let (key_cols, origin, strength, rationale) = match &unique {
                Some((set, _)) => {
                    protections.push(format!(
                        "UNIQUE({}) on `{}`{}",
                        set.join(", "),
                        first.table,
                        if first.on_conflict == OnConflict::DoNothing {
                            " with ON CONFLICT DO NOTHING"
                        } else {
                            ""
                        }
                    ));
                    (
                        set.clone(),
                        Origin::SchemaEnforced,
                        Strength::DatabaseIntegrity,
                        format!("UNIQUE({}) in the analyzed schema", set.join(", ")),
                    )
                }
                None => (
                    first
                        .writes
                        .iter()
                        .filter(|(_, v)| matches!(canon(v, s), Val::Param { .. }))
                        .map(|(c, _)| c.clone())
                        .collect(),
                    Origin::HeuristicHypothesis,
                    Strength::RecoverySafety,
                    "the handler repeats the same insert; that one call should record one row is a hypothesis".to_owned(),
                ),
            };
            if key_cols.is_empty() || first.access != Access::Insert {
                continue;
            }
            let key: Vec<(String, Val)> = key_cols
                .iter()
                .filter_map(|c| first.write_val(c).map(|v| (c.clone(), canon(v, s))))
                .collect();
            let mut properties = vec![Property {
                name: format!("one-{}-per-call", first.table),
                description: format!(
                    "one call records at most one `{}` row per ({})",
                    first.table,
                    key_cols.join(", ")
                ),
                origin,
                strength,
                spec: Spec::AtMostOne {
                    table: first.table.clone(),
                    key: key.clone(),
                    filter: BTreeMap::new(),
                },
                rationale,
                evidence: vec![format!(
                    "write at {} repeated by the catch block at {}",
                    first.src.label(),
                    second.src.label()
                )],
            }];
            // A declared invariant picks its own key columns: every written
            // value (literals included) is available to it.
            let written: Vec<(String, Val)> = first
                .writes
                .iter()
                .map(|(c, v)| (c.clone(), canon(v, s)))
                .collect();
            properties.extend(self.declared_properties(a, Some(&first.table), None, &written));
            self.push(
                Class::AmbiguousCommit,
                format!(
                    "write to `{}` repeated after an unknown outcome",
                    first.table
                ),
                vec![a],
                Resource {
                    kind: "unique-key".to_owned(),
                    table: first.table.clone(),
                    columns: key_cols.clone(),
                    text: format!("{}({})", first.table, key_cols.join(", ")),
                },
                vec![op(0, "first-write", first), op(0, "retried-write", second)],
                properties,
                protections,
                vec![Assumption::new(
                    "fault",
                    "discovery-default",
                    if first.conn == Conn::Pool {
                        "the acknowledgment of an autocommit write can be lost after the server committed it (a broken connection); the driver then raises an error"
                    } else {
                        "the acknowledgment of the transaction's COMMIT can be lost after the server committed it (a broken connection); the driver then raises an error"
                    },
                )],
                Environment {
                    actors: 1,
                    relation: Relation::Same,
                    retry: None,
                    max_crashes: 0,
                    max_retries: 0,
                    max_in_flight: 1,
                },
                vec!["the driver's error does not say whether the write committed".to_owned()],
                &[vec![first, second]],
            );
            return;
        }
    }

    // -- Phase 14: declared return-value properties -----------------------------

    /// A property over what callers are told, DECLARED for this entry
    /// (`atMostOneSuccess`, `exactlyOneSuccess`, `everySuccessReturns`):
    /// the entry's own database effects, two concurrent calls with the same
    /// arguments (one call for `everySuccessReturns`, whose violation needs
    /// no partner), and the declared retry. Nothing here is inferred: an
    /// entry without such a declaration gets no candidate of this class.
    fn declared_outcome(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        let declared: Vec<&crate::contracts::DeclaredInvariant> = self
            .contracts
            .invariants
            .iter()
            .filter(|inv| {
                inv.entries.iter().any(|e| e == &a.fact.selector)
                    && match &inv.property {
                        Template::AtMostOneSuccess { .. }
                        | Template::ExactlyOneSuccess { .. }
                        | Template::EverySuccessReturns { .. } => true,
                        // A declared formula over return values.
                        Template::Formula { assert } => assert.reads_returned(),
                        _ => false,
                    }
            })
            .collect();
        if declared.is_empty() || s.db.is_empty() {
            return;
        }
        let single = declared
            .iter()
            .all(|inv| matches!(inv.property, Template::EverySuccessReturns { .. }));
        let properties: Vec<Property> = self
            .declared_properties(a, None, None, &[])
            .into_iter()
            .filter(|p| declared.iter().any(|d| d.name == p.name))
            .collect();
        let conflict: Vec<ConflictOp> =
            s.db.iter()
                .filter(|e| e.at.0 == 0)
                .map(|e| op(0, if e.access.writes() { "act" } else { "check" }, e))
                .collect();
        let tables: BTreeSet<String> = s.db.iter().map(|e| e.table.clone()).collect();
        let table = tables.iter().next().cloned().unwrap_or_default();
        let mut env = if single {
            Environment {
                actors: 1,
                relation: Relation::Same,
                retry: None,
                max_crashes: 0,
                max_retries: 0,
                max_in_flight: 0,
            }
        } else {
            Self::two(Relation::Same)
        };
        if let Some(retry) = a.contract.as_ref().and_then(|c| c.retry.clone()) {
            env.retry = Some(retry);
            env.max_crashes = 1;
            env.max_retries = 1;
        }
        let ops: Vec<&DbEffect> = s.db.iter().collect();
        let actors = if single { vec![a] } else { vec![a, a] };
        let ops_of = if single {
            vec![ops]
        } else {
            vec![ops.clone(), ops]
        };
        self.push(
            Class::ReturnValue,
            format!("declared property over what `{}` returns", a.fact.name),
            actors,
            Resource {
                kind: "return-value".to_owned(),
                table,
                columns: Vec::new(),
                text: format!(
                    "{}(): {}",
                    a.fact.name,
                    tables.into_iter().collect::<Vec<_>>().join(", ")
                ),
            },
            conflict,
            properties,
            Vec::new(),
            Vec::new(),
            env,
            Vec::new(),
            &ops_of,
        );
    }

    // -- Phase 14: parent DELETE x child INSERT ---------------------------------

    /// A read that finds the parent row (by a unique key of the parent
    /// table) guards an insert of a child row whose column holds that key;
    /// another entry deletes parent rows by that key. With a foreign key
    /// (`NO ACTION`/`RESTRICT`) the database enforces the reference and the
    /// property is schema-enforced (a 23503 for one of them is not an
    /// application defect); without one, that the reference must hold is a
    /// hypothesis the existence check suggests.
    fn parent_child(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        for read in s.db.iter().filter(|e| {
            matches!(e.access, Access::Read | Access::Count)
                && !e.opaque_predicate
                && e.joins.is_empty()
                && e.eq.len() == 1
        }) {
            let (pcol, pval) = &read.eq[0];
            if !matches!(canon(pval, s), Val::Param { .. }) {
                continue;
            }
            let unique = unique_sets(self.index, &read.table)
                .into_iter()
                .any(|(set, p)| p.is_none() && set == [pcol.clone()]);
            if !unique {
                continue;
            }
            for insert in s.db.iter().filter(|e| {
                e.access == Access::Insert
                    && e.table != read.table
                    && s.reaches(read.index, e.index)
                    && s.deciding_guard(read.index, e.at).is_some()
            }) {
                let Some((ccol, _)) = insert.writes.iter().find(|(_, v)| same_value(v, pval, s))
                else {
                    continue;
                };
                let deleters: Vec<(usize, usize)> = self
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.fact.selector != a.fact.selector)
                    .flat_map(|(j, b)| {
                        b.summary
                            .db
                            .iter()
                            .filter(|e| {
                                e.access == Access::Delete
                                    && e.table == read.table
                                    && matches!(
                                        e.eq_val(pcol).map(|v| canon(v, &b.summary)),
                                        Some(Val::Param { .. })
                                    )
                            })
                            .map(move |e| (j, e.index))
                    })
                    .collect();
                let fk = self.foreign_key(&insert.table, ccol);
                for (j, d) in deleters {
                    let b = &self.entries[j];
                    let delete = &b.summary.db[d];
                    let enforced = fk
                        .as_ref()
                        .is_some_and(|(t, c, _)| t == &read.table && c == pcol);
                    let mut protections = Vec::new();
                    let (origin, strength, rationale) = if enforced {
                        let action = fk
                            .as_ref()
                            .map(|(_, _, a)| a.as_str())
                            .unwrap_or("no action");
                        protections.push(format!(
                            "FOREIGN KEY `{}`({ccol}) REFERENCES `{}`({pcol}) ON DELETE {action}",
                            insert.table, read.table
                        ));
                        (
                            Origin::SchemaEnforced,
                            Strength::DatabaseIntegrity,
                            format!(
                                "the foreign key `{}`({ccol}) → `{}`({pcol}) in the analyzed schema: the database rejects one of the two (SQLSTATE 23503), which is not by itself an application defect",
                                insert.table, read.table
                            ),
                        )
                    } else {
                        (
                            Origin::HeuristicHypothesis,
                            Strength::DatabaseIntegrity,
                            format!(
                                "`{}` checks that the `{}` row exists before inserting a `{}` row that holds its key; no foreign key enforces the reference, so that it must hold is a hypothesis",
                                a.fact.name, read.table, insert.table
                            ),
                        )
                    };
                    let mut properties = vec![Property {
                        name: format!("no-orphan-{}", insert.table),
                        description: format!(
                            "every `{}` row's `{ccol}` names an existing `{}` row",
                            insert.table, read.table
                        ),
                        origin,
                        strength,
                        spec: Spec::NoOrphan {
                            child: insert.table.clone(),
                            column: ccol.clone(),
                            parent: read.table.clone(),
                            parent_column: pcol.clone(),
                            key: canon(pval, s),
                        },
                        rationale,
                        evidence: vec![format!(
                            "existence check at {}, insert at {}, delete in `{}` at {}",
                            read.src.label(),
                            insert.src.label(),
                            b.fact.name,
                            delete.src.label()
                        )],
                    }];
                    let written: Vec<(String, Val)> = insert
                        .writes
                        .iter()
                        .map(|(c, v)| (c.clone(), canon(v, s)))
                        .collect();
                    properties.extend(self.declared_properties(
                        a,
                        Some(&insert.table),
                        None,
                        &written,
                    ));
                    self.push(
                        Class::ParentChild,
                        format!(
                            "insert into `{}` after finding its `{}` row, against `{}` deleting it",
                            insert.table, read.table, b.fact.name
                        ),
                        vec![a, b],
                        Resource {
                            kind: "row".to_owned(),
                            table: read.table.clone(),
                            columns: vec![pcol.clone()],
                            text: format!("{}({pcol})", read.table),
                        },
                        vec![
                            op(0, "check", read),
                            op(0, "act", insert),
                            op(1, "act", delete),
                        ],
                        properties,
                        protections,
                        vec![Assumption::new(
                            "concurrency",
                            "discovery-default",
                            format!(
                                "`{}` and `{}` can run at the same time on the same `{}` row",
                                a.fact.name, b.fact.name, read.table
                            ),
                        )],
                        Self::two(Relation::Same),
                        Vec::new(),
                        &[vec![read, insert], vec![delete]],
                    );
                }
            }
        }
    }

    /// Phase 14: the foreign key of `table.column` in the analyzed schema:
    /// (referenced table, referenced column property, ON DELETE action).
    fn foreign_key(&self, table: &str, column: &str) -> Option<(String, String, String)> {
        let t = self.index.source_table(table)?;
        let c = t.columns.iter().find(|c| c.property == column)?;
        let (binding, col) = c.references.as_ref()?;
        let parent = self.index.referenced_table(&t.file, binding)?;
        let action = t
            .foreign_keys
            .iter()
            .find(|f| f.columns.iter().any(|x| x == &c.sql_name))
            .and_then(|f| f.on_delete.clone())
            .unwrap_or_else(|| "no action".to_owned());
        Some((parent.name.clone(), col.clone(), action))
    }

    // -- H: lock order ------------------------------------------------------

    fn lock_order(&mut self, _i: usize, a: &Analyzed) {
        let s = &a.summary;
        let locks: Vec<&DbEffect> =
            s.db.iter()
                .filter(|e| {
                    e.tx.is_some()
                        && (matches!(e.access, Access::Update | Access::Delete)
                            // Any row lock can be part of a wait cycle.
                            || (e.access == Access::Read && e.lock != LockMode::None))
                })
                .collect();
        for first in &locks {
            for second in &locks {
                if first.index == second.index
                    || first.table != second.table
                    || first.tx != second.tx
                    || !s.reaches(first.index, second.index)
                {
                    continue;
                }
                let pk: Vec<String> = unique_sets(self.index, &first.table)
                    .into_iter()
                    .next()
                    .map(|(k, _)| k)
                    .unwrap_or_default();
                let key_of = |e: &DbEffect| -> Option<Val> {
                    pk.first()
                        .and_then(|c| e.eq_val(c))
                        .map(|v| canon(v, s))
                        .filter(|v| matches!(v, Val::Param { .. }))
                };
                let (Some(k1), Some(k2)) = (key_of(first), key_of(second)) else {
                    continue;
                };
                if k1 == k2 {
                    continue;
                }
                let (concurrent, _) = a.concurrency();
                if !concurrent {
                    return;
                }
                let mut env = Self::two(Relation::Swapped);
                env.max_crashes = 0;
                self.push(
                    Class::LockOrder,
                    format!(
                        "rows of `{}` locked in argument order ({} then {})",
                        first.table,
                        k1.render(),
                        k2.render()
                    ),
                    vec![a, a],
                    Resource {
                        kind: "row".to_owned(),
                        table: first.table.clone(),
                        columns: pk.clone(),
                        text: format!("{}({} / {})", first.table, k1.render(), k2.render()),
                    },
                    vec![op(0, "first-lock", first), op(0, "second-lock", second)],
                    vec![Property {
                        name: "no-request-fails".to_owned(),
                        description: "no invocation fails (a deadlock victim fails with 40P01)".to_owned(),
                        origin: Origin::HeuristicHypothesis,
                        strength: Strength::BoundedLiveness,
                        spec: Spec::NoFailedActors,
                        rationale: format!(
                            "two invocations with swapped `{}` / `{}` lock the same rows in opposite orders; PostgreSQL aborts one of them. That the operation should not fail this way is a hypothesis",
                            k1.render(),
                            k2.render()
                        ),
                        evidence: vec![first.src.label(), second.src.label()],
                    }],
                    Vec::new(),
                    Vec::new(),
                    env,
                    vec!["a failure for another reason also violates `no-request-fails`".to_owned()],
                    &[vec![first, second], vec![first, second]],
                );
                return;
            }
        }
    }

    // -- cross-entry pairs through the resource index -----------------------

    fn pair(&mut self, max_partners: usize) -> usize {
        let mut truncated = 0;
        // (table, column) → entries that write it, with the effect.
        let mut writers: BTreeMap<(String, String), Vec<(usize, usize)>> = BTreeMap::new();
        for (i, a) in self.entries.iter().enumerate() {
            for e in a.summary.db.iter().filter(|e| e.access.writes()) {
                for (c, _) in &e.writes {
                    writers
                        .entry((e.table.clone(), c.clone()))
                        .or_default()
                        .push((i, e.index));
                }
            }
        }
        // An exported function another entry inlines is a helper of that
        // entry (a repository method): pairing with it would repeat the
        // entry's own candidate.
        let helpers: BTreeSet<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(j, h)| {
                let file = h.file();
                self.entries.iter().enumerate().any(|(k, other)| {
                    k != *j
                        && other
                            .summary
                            .db
                            .iter()
                            .any(|e| e.src.file == file && e.src.function == h.fact.name)
                })
            })
            .map(|(j, _)| j)
            .collect();
        let mut done: BTreeSet<(Class, usize, usize)> = BTreeSet::new();
        let singles: Vec<Candidate> = self.out.clone();
        for c in singles
            .iter()
            .filter(|c| matches!(c.class, Class::CheckThenAct | Class::ReadModifyWrite))
        {
            let Some(i) = self
                .entries
                .iter()
                .position(|a| a.fact.selector == c.entries[0].selector)
            else {
                continue;
            };
            let a = &self.entries[i];
            let column = match c.class {
                Class::ReadModifyWrite => c.resource.columns[0].clone(),
                _ => c.resource.columns[0].clone(),
            };
            let mut partners: Vec<usize> = writers
                .get(&(c.resource.table.clone(), column.clone()))
                .map(|v| {
                    v.iter()
                        .map(|(p, _)| *p)
                        .filter(|p| *p != i && !helpers.contains(p))
                        .collect()
                })
                .unwrap_or_default();
            partners.sort_unstable();
            partners.dedup();
            partners.retain(|p| done.insert((c.class, i.min(*p), i.max(*p))));
            if partners.len() > max_partners {
                truncated += partners.len() - max_partners;
                partners.truncate(max_partners);
            }
            for p in partners {
                let b = &self.entries[p];
                match c.class {
                    Class::CheckThenAct => self.cta_pair(c, a, b),
                    Class::ReadModifyWrite => self.rmw_pair(c, a, b),
                    _ => {}
                }
            }
        }
        truncated
    }

    fn cta_pair(&mut self, single: &Candidate, a: &Analyzed, b: &Analyzed) {
        let table = &single.resource.table;
        let Some(Spec::AtMostOne { key, filter, .. }) = single
            .properties
            .iter()
            .map(|p| &p.spec)
            .find(|s| matches!(s, Spec::AtMostOne { .. }))
        else {
            return;
        };
        let sb = &b.summary;
        let Some(insert) = sb.db.iter().find(|e| {
            e.access == Access::Insert
                && &e.table == table
                && key.iter().all(|(c, _)| e.write_val(c).is_some())
        }) else {
            return;
        };
        // A literal of the partner that differs from the candidate's filter
        // makes the rows provably distinct.
        for (c, v) in filter {
            if let Some(Val::Lit { value }) = insert.write_val(c)
                && value != v
            {
                self.reject(
                    Class::CheckThenAct,
                    &[a, b],
                    format!(
                        "the partner inserts `{c}` = {} and the check filters `{c}` = {}: provably distinct rows",
                        value.render(),
                        v.render()
                    ),
                    &insert.src.label(),
                );
                return;
            }
        }
        let read_i = single.conflict[0].effect;
        let act_i = single.conflict[1].effect;
        let read = &a.summary.db[read_i];
        let act = &a.summary.db[act_i];
        let mut properties = single.properties.clone();
        for p in &mut properties {
            p.rationale = format!(
                "{} The partner `{}` inserts the same key columns at {}",
                p.rationale,
                b.fact.name,
                insert.src.label()
            );
        }
        let mut protections = single.protections.clone();
        if insert.on_conflict == OnConflict::DoNothing {
            protections.push("the partner's insert is ON CONFLICT DO NOTHING".to_owned());
        }
        self.push(
            Class::CheckThenAct,
            format!("check-then-insert on `{table}` against `{}`", b.fact.name),
            vec![a, b],
            single.resource.clone(),
            vec![
                op(0, "check", read),
                op(0, "act", act),
                op(1, "act", insert),
            ],
            properties,
            protections,
            vec![Assumption::new(
                "concurrency",
                "discovery-default",
                format!(
                    "`{}` and `{}` can run at the same time with the same key",
                    a.fact.name, b.fact.name
                ),
            )],
            Self::two(Relation::Same),
            single.inconclusive_if.clone(),
            &[vec![read, act], vec![insert]],
        );
    }

    fn rmw_pair(&mut self, single: &Candidate, a: &Analyzed, b: &Analyzed) {
        let table = &single.resource.table;
        let column = &single.resource.columns[0];
        let sb = &b.summary;
        let Some(w) = sb.db.iter().find(|e| {
            e.access == Access::Update && &e.table == table && e.write_val(column).is_some()
        }) else {
            return;
        };
        let read = &a.summary.db[single.conflict[0].effect];
        let upd = &a.summary.db[single.conflict[1].effect];
        let mut properties = single.properties.clone();
        for p in &mut properties {
            p.rationale = format!(
                "{} The partner `{}` writes `{column}` at {}",
                p.rationale,
                b.fact.name,
                w.src.label()
            );
        }
        self.push(
            Class::ReadModifyWrite,
            format!(
                "read-modify-write of `{table}.{column}` against `{}`",
                b.fact.name
            ),
            vec![a, b],
            single.resource.clone(),
            vec![op(0, "read", read), op(0, "write", upd), op(1, "write", w)],
            properties,
            single.protections.clone(),
            vec![Assumption::new(
                "concurrency",
                "discovery-default",
                format!(
                    "`{}` and `{}` can run at the same time on the same row",
                    a.fact.name, b.fact.name
                ),
            )],
            Self::two(Relation::Same),
            single.inconclusive_if.clone(),
            &[vec![read, upd], vec![w]],
        );
    }
}

fn literal_eq(d: &commitrift_model::schema_facts::SchemaLiteral, v: &Value) -> bool {
    use commitrift_model::schema_facts::SchemaLiteral;
    match (d, v) {
        (SchemaLiteral::Integer(a), Value::Int(b)) => a == b,
        (SchemaLiteral::Text(a), Value::Str(b)) => a == b,
        (SchemaLiteral::Boolean(a), Value::Bool(b)) => a == b,
        (SchemaLiteral::Null, Value::Null) => true,
        _ => false,
    }
}
