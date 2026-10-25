//! Observations of the surviving state and the declared invariants over
//! them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use commitrift_core::verification::{ConditionResult, InvariantResult};

use crate::manifest::{ConditionSpec, EventMeasure, InvariantSpec, Measure, ObservationSpec};
use crate::postgres::Cluster;

pub type Observations = BTreeMap<String, Value>;

/// One line of the harness' event log (`events.jsonl`).
#[derive(Debug, serde::Deserialize)]
struct LoggedEvent {
    topic: String,
    #[serde(default)]
    id: Option<Value>,
}

/// Read the event log; a missing file means no event was published.
fn read_events(log: &Path) -> Result<Vec<LoggedEvent>, String> {
    let text = match std::fs::read_to_string(log) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read the event log: {error}")),
    };
    let mut events = Vec::new();
    for (number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        // A torn last line (the process died mid-write) is not an event:
        // the fake queue acknowledges a publication only after fsync.
        match serde_json::from_str::<LoggedEvent>(line) {
            Ok(event) => events.push(event),
            Err(_) if number + 1 == text.lines().count() && !text.ends_with('\n') => {}
            Err(error) => return Err(format!("event log line {}: {error}", number + 1)),
        }
    }
    Ok(events)
}

fn measure(events: &[LoggedEvent], spec: &EventMeasure) -> Value {
    let selected: Vec<&LoggedEvent> = events
        .iter()
        .filter(|e| spec.topic.as_ref().is_none_or(|t| &e.topic == t))
        .collect();
    let count = selected.len() as i64;
    let with_id: Vec<String> = selected
        .iter()
        .filter_map(|e| e.id.as_ref().map(Value::to_string))
        .collect();
    let without_id = count - with_id.len() as i64;
    let distinct = with_id.iter().collect::<BTreeSet<_>>().len() as i64 + without_id;
    Value::from(match spec.measure {
        Measure::Count => count,
        Measure::DistinctIds => distinct,
        Measure::Duplicates => count - distinct,
        Measure::WithoutId => without_id,
    })
}

/// Evaluate every observation against `database` and the event log.
pub fn observe(
    specs: &[ObservationSpec],
    cluster: &Cluster,
    database: &str,
    event_log: &Path,
) -> Result<Observations, String> {
    let events = read_events(event_log)?;
    let sql: Vec<&str> = specs.iter().filter_map(|s| s.sql.as_deref()).collect();
    let mut values = match cluster.query_values(database, &sql) {
        Ok(values) => values.into_iter(),
        Err(_) => {
            // Name the failing observation.
            for spec in specs {
                if let Some(sql) = &spec.sql {
                    cluster
                        .query_value(database, sql)
                        .map_err(|e| format!("observation `{}`: {e}", spec.name))?;
                }
            }
            return Err("an observation query failed".to_owned());
        }
    };
    let mut out = Observations::new();
    for spec in specs {
        let value = match (&spec.sql, &spec.events) {
            (Some(_), _) => values.next().unwrap_or(Value::Null),
            (None, Some(events_spec)) => measure(&events, events_spec),
            (None, None) => Value::Null,
        };
        out.insert(spec.name.clone(), value);
    }
    Ok(out)
}

fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Values compare equal when they are equal JSON, or when one is a string
/// and the other a number with the same text (psql returns text).
fn same(a: &Value, b: &Value) -> bool {
    a == b
        || match (a, b) {
            (Value::String(s), Value::Number(n)) | (Value::Number(n), Value::String(s)) => {
                s == &n.to_string()
            }
            _ => false,
        }
}

pub fn describe(condition: &ConditionSpec) -> String {
    let name = &condition.observation;
    if let Some(value) = &condition.equals {
        format!("{name} = {value}")
    } else if let Some(value) = &condition.not_equals {
        format!("{name} != {value}")
    } else if let Some(n) = condition.at_least {
        format!("{name} >= {n}")
    } else if let Some(n) = condition.at_most {
        format!("{name} <= {n}")
    } else {
        format!("{name} ?")
    }
}

pub fn holds(condition: &ConditionSpec, observations: &Observations) -> bool {
    let Some(value) = observations.get(&condition.observation) else {
        return false;
    };
    if let Some(expected) = &condition.equals {
        same(value, expected)
    } else if let Some(expected) = &condition.not_equals {
        !same(value, expected)
    } else if let Some(n) = condition.at_least {
        as_i64(value).is_some_and(|v| v >= n)
    } else if let Some(n) = condition.at_most {
        as_i64(value).is_some_and(|v| v <= n)
    } else {
        false
    }
}

pub fn check(conditions: &[ConditionSpec], observations: &Observations) -> Vec<ConditionResult> {
    conditions
        .iter()
        .map(|condition| ConditionResult {
            description: describe(condition),
            holds: holds(condition, observations),
            observed: observations
                .get(&condition.observation)
                .cloned()
                .unwrap_or(Value::Null),
        })
        .collect()
}

/// Evaluate the invariants at one stage.
pub fn invariants(
    specs: &[InvariantSpec],
    stage: &str,
    observations: &Observations,
) -> Vec<InvariantResult> {
    specs
        .iter()
        .map(|spec| {
            let applies = spec.when.iter().all(|c| holds(c, observations));
            let all = spec.require.iter().all(|c| holds(c, observations));
            let any = spec.require_any.is_empty()
                || spec.require_any.iter().any(|c| holds(c, observations));
            let observed = |conditions: &[ConditionSpec]| {
                conditions
                    .iter()
                    .map(|c| {
                        format!(
                            "{} (observed {})",
                            describe(c),
                            observations.get(&c.observation).unwrap_or(&Value::Null)
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let detail = if !applies {
                format!("not applicable: {}", observed(&spec.when).join(" and "))
            } else {
                let mut parts = observed(&spec.require);
                if !spec.require_any.is_empty() {
                    parts.push(format!(
                        "any of [{}]",
                        observed(&spec.require_any).join(", ")
                    ));
                }
                format!(
                    "{}: {}",
                    if all && any { "holds" } else { "VIOLATED" },
                    parts.join("; ")
                )
            };
            InvariantResult {
                id: spec.id.clone(),
                stage: stage.to_owned(),
                holds: !applies || (all && any),
                detail,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn condition(json: &str) -> ConditionSpec {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn invariants_apply_only_when_their_premise_holds() {
        let spec: InvariantSpec = serde_json::from_str(
            r#"{ "id": "paid-has-event", "description": "d",
                 "when": [{ "observation": "status", "equals": "paid" }],
                 "require": [{ "observation": "events", "atLeast": 1 }] }"#,
        )
        .unwrap();
        let obs = |status: &str, events: i64| {
            Observations::from([
                ("status".to_owned(), Value::from(status)),
                ("events".to_owned(), Value::from(events)),
            ])
        };
        let specs = std::slice::from_ref(&spec);
        assert!(invariants(specs, "after", &obs("paid", 1))[0].holds);
        assert!(!invariants(specs, "after", &obs("paid", 0))[0].holds);
        assert!(invariants(specs, "after", &obs("pending", 0))[0].holds);
        assert!(
            invariants(specs, "after", &obs("paid", 0))[0]
                .detail
                .starts_with("VIOLATED")
        );
    }

    #[test]
    fn require_any_needs_one_alternative() {
        let spec: InvariantSpec = serde_json::from_str(
            r#"{ "id": "owed", "description": "d",
                 "requireAny": [{ "observation": "a", "atLeast": 1 }, { "observation": "b", "atLeast": 1 }] }"#,
        )
        .unwrap();
        let specs = std::slice::from_ref(&spec);
        let obs = |a: i64, b: i64| {
            Observations::from([
                ("a".to_owned(), Value::from(a)),
                ("b".to_owned(), Value::from(b)),
            ])
        };
        assert!(invariants(specs, "s", &obs(0, 1))[0].holds);
        assert!(!invariants(specs, "s", &obs(0, 0))[0].holds);
    }

    #[test]
    fn conditions_compare_text_and_numbers() {
        let obs = Observations::from([("n".to_owned(), Value::from("3"))]);
        assert!(holds(
            &condition(r#"{ "observation": "n", "equals": 3 }"#),
            &obs
        ));
        assert!(holds(
            &condition(r#"{ "observation": "n", "atMost": 3 }"#),
            &obs
        ));
        assert!(!holds(
            &condition(r#"{ "observation": "n", "atLeast": 4 }"#),
            &obs
        ));
        assert!(!holds(
            &condition(r#"{ "observation": "missing", "equals": 1 }"#),
            &obs
        ));
    }

    #[test]
    fn event_measures_count_duplicates_and_ids() {
        let dir = std::env::temp_dir().join(format!("commitrift-events-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("events.jsonl");
        std::fs::write(
            &log,
            "{\"topic\":\"a\",\"id\":1}\n{\"topic\":\"a\",\"id\":1}\n{\"topic\":\"a\"}\n{\"topic\":\"b\",\"id\":2}\n{\"topic\":\"a\",\"id\"",
        )
        .unwrap();
        let events = read_events(&log).unwrap();
        assert_eq!(events.len(), 4, "the torn last line is not an event");
        let m = |measure: Measure| measure_value(&events, Some("a"), measure);
        assert_eq!(m(Measure::Count), 3);
        assert_eq!(m(Measure::DistinctIds), 2);
        assert_eq!(m(Measure::Duplicates), 1);
        assert_eq!(m(Measure::WithoutId), 1);
        assert!(read_events(&dir.join("missing.jsonl")).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn measure_value(events: &[LoggedEvent], topic: Option<&str>, measure: Measure) -> i64 {
        super::measure(
            events,
            &EventMeasure {
                topic: topic.map(str::to_owned),
                measure,
            },
        )
        .as_i64()
        .unwrap()
    }
}
