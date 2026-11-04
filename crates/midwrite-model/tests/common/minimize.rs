//! Shrinking a scenario document while some property still holds (Phase 10).
//!
//! The property is a closure: "the model still loads, the unreduced search
//! still finishes, and reduction X still differs from it". Candidates are
//! tried greedily until none applies: fewer actors, fewer statements, no
//! faults, no pool or shared process, fewer rows, fewer tables, plainer
//! isolation. A candidate that no longer loads is simply not accepted.
#![allow(dead_code)]

use serde_json::Value;

fn actors_mut(doc: &mut Value) -> &mut Vec<Value> {
    doc["actors"].as_array_mut().expect("actors")
}

fn programs(doc: &Value) -> Vec<String> {
    doc["actors"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| x["program"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One round of candidates, simplest-effect first.
fn candidates(doc: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let n = doc["actors"].as_array().map_or(0, Vec::len);
    // Fewer actors.
    if n > 1 {
        for i in 0..n {
            let mut d = doc.clone();
            actors_mut(&mut d).remove(i);
            out.push(d);
        }
    }
    // Fewer statements: a line, then a block of up to four lines.
    for (ai, prog) in programs(doc).iter().enumerate() {
        let lines: Vec<&str> = prog.lines().collect();
        for len in [1usize, 2, 3, 4, 6] {
            if lines.len() < len + 1 {
                continue;
            }
            for start in 0..=lines.len() - len {
                let kept: Vec<&str> = lines[..start]
                    .iter()
                    .chain(lines[start + len..].iter())
                    .copied()
                    .collect();
                let mut d = doc.clone();
                d["actors"][ai]["program"] = Value::String(format!("{}\n", kept.join("\n")));
                out.push(d);
            }
        }
    }
    // No faults.
    for key in ["maxCrashes", "maxRetries", "maxInFlight"] {
        if doc["bounds"][key].as_u64().unwrap_or(0) > 0 {
            let mut d = doc.clone();
            d["bounds"][key] = 0.into();
            out.push(d);
        }
    }
    // No per-actor retry policy.
    for i in 0..n {
        if doc["actors"][i].get("retry").is_some() {
            let mut d = doc.clone();
            d["actors"][i].as_object_mut().unwrap().remove("retry");
            out.push(d);
        }
    }
    // No pool, no shared process; a smaller pool.
    if let Some(pools) = doc.get("pools").and_then(Value::as_object) {
        let mut d = doc.clone();
        d.as_object_mut().unwrap().remove("pools");
        out.push(d);
        for (name, cap) in pools {
            if cap.as_u64().unwrap_or(0) > 1 {
                let mut d = doc.clone();
                d["pools"][name] = (cap.as_u64().unwrap() - 1).into();
                out.push(d);
            }
        }
    }
    for i in 0..n {
        if doc["actors"][i].get("process").is_some() {
            let mut d = doc.clone();
            d["actors"][i].as_object_mut().unwrap().remove("process");
            out.push(d);
        }
    }
    // Fewer rows.
    if let Some(init) = doc["database"]["initialState"].as_object() {
        for (table, rows) in init {
            let len = rows.as_array().map_or(0, Vec::len);
            for r in 0..len {
                let mut d = doc.clone();
                d["database"]["initialState"][table]
                    .as_array_mut()
                    .unwrap()
                    .remove(r);
                out.push(d);
            }
        }
    }
    // Fewer tables, channels and variables.
    if let Some(tables) = doc["database"]["tables"].as_array() {
        for (i, table) in tables.iter().enumerate() {
            let name = table["name"].as_str().unwrap_or("").to_owned();
            let mut d = doc.clone();
            d["database"]["tables"].as_array_mut().unwrap().remove(i);
            if let Some(m) = d["database"]["initialState"].as_object_mut() {
                m.remove(&name);
            }
            out.push(d);
        }
    }
    if doc.get("app").is_some() {
        let mut d = doc.clone();
        d.as_object_mut().unwrap().remove("app");
        out.push(d);
    }
    // Plainer isolation and transactions.
    if doc["database"]["isolation"] != "read-committed" {
        let mut d = doc.clone();
        d["database"]["isolation"] = "read-committed".into();
        out.push(d);
    }
    for (ai, prog) in programs(doc).iter().enumerate() {
        if prog.contains("begin isolation") {
            let mut d = doc.clone();
            let plain: String = prog
                .lines()
                .map(|l| {
                    if l.starts_with("begin isolation") {
                        "begin"
                    } else {
                        l
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            d["actors"][ai]["program"] = Value::String(format!("{plain}\n"));
            out.push(d);
        }
    }
    // Smaller bounds.
    let steps = doc["bounds"]["maxSteps"].as_u64().unwrap_or(0);
    if steps > 12 {
        let mut d = doc.clone();
        d["bounds"]["maxSteps"] = (steps * 2 / 3).into();
        out.push(d);
    }
    out
}

/// A size measure for progress and reporting.
pub fn size(doc: &Value) -> usize {
    let prog: usize = programs(doc).iter().map(|p| p.lines().count()).sum();
    let rows: usize = doc["database"]["initialState"].as_object().map_or(0, |m| {
        m.values().map(|v| v.as_array().map_or(0, Vec::len)).sum()
    });
    doc["actors"].as_array().map_or(0, Vec::len) * 100
        + prog * 3
        + rows
        + doc["database"]["tables"].as_array().map_or(0, Vec::len) * 5
}

/// Shrink `doc` while `holds` stays true (it must hold for `doc`).
pub fn minimize(doc: &Value, holds: &mut dyn FnMut(&Value) -> bool) -> Value {
    assert!(holds(doc), "the property does not hold for the original");
    let mut best = doc.clone();
    let mut budget = 4000usize;
    'outer: loop {
        for cand in candidates(&best) {
            if budget == 0 {
                break 'outer;
            }
            budget -= 1;
            if size(&cand) < size(&best) && holds(&cand) {
                best = cand;
                continue 'outer;
            }
        }
        break;
    }
    best
}
