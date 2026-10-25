//! A deterministic random scenario generator for differential tests.
#![allow(dead_code)]

use serde_json::{Value, json};

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

const ISOS: [&str; 3] = ["read-committed", "repeatable-read", "serializable"];

fn program(rng: &mut Rng, ops: usize, in_tx: bool, iso: Option<&str>) -> String {
    let mut out = String::new();
    if in_tx {
        match iso {
            Some(i) => out.push_str(&format!("begin isolation {i}\n")),
            None => out.push_str("begin\n"),
        }
    }
    let mut n = 0;
    for _ in 0..ops {
        n += 1;
        let k = rng.below(3) + 1;
        match rng.below(9) {
            0 => out.push_str(&format!("let r{n} = select t where @id == {k}\n")),
            1 => {
                out.push_str(&format!(
                    "let r{n} = select t where @id == {k}\nif r{n} != null {{\n  update t where @id == {k} set @v = r{n}.v + {}\n}}\n",
                    rng.below(3) + 1
                ));
            }
            2 => out.push_str(&format!(
                "update t where @id == {k} set @v = @v + {} into a{n}\n",
                rng.below(3) + 1
            )),
            3 => out.push_str(&format!(
                "let c{n} = count t where @v >= {}\n",
                rng.below(4)
            )),
            4 => out.push_str(&format!(
                "insert t set @id = {}, @v = {}, @w = {} on conflict do nothing into i{n}\n",
                rng.below(5) + 1,
                rng.below(4),
                rng.below(6) + 10
            )),
            5 => out.push_str(&format!("delete t where @id == {k} into d{n}\n")),
            6 => out.push_str(&format!(
                "update t where @id == {k} && @v > {} set @v = @v - 1 into u{n}\n",
                rng.below(3)
            )),
            7 => out.push_str(&format!(
                "let l{n} = select t where @id == {k} for update skip locked\n"
            )),
            _ => out.push_str(&format!("external ch key {} set n = {n}\n", rng.below(2))),
        }
        if rng.chance(15) {
            out.push_str(&format!(
                "if {} == 1 {{\n  fail \"random failure\"\n}}\n",
                rng.below(2)
            ));
        }
    }
    if in_tx {
        if rng.chance(20) {
            out.push_str("rollback\n");
        } else {
            out.push_str("commit\n");
        }
    }
    out
}

pub fn random_scenario(seed: u64) -> Value {
    let mut rng = Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) | 1);
    for _ in 0..4 {
        rng.next();
    }
    let iso = ISOS[rng.below(3) as usize];
    let n_actors = if rng.chance(30) { 3 } else { 2 };
    let max_ops = if n_actors == 3 { 3 } else { 4 };
    let mut actors = Vec::new();
    for i in 0..n_actors {
        let ops = 1 + rng.below(max_ops) as usize;
        let in_tx = rng.chance(65);
        let tx_iso = if in_tx && rng.chance(25) {
            Some(ISOS[rng.below(3) as usize])
        } else {
            None
        };
        let name = ["A", "B", "C"][i];
        let mut a = json!({
            "name": name,
            "function": "work",
            "program": program(&mut rng, ops, in_tx, tx_iso),
            "arguments": {}
        });
        if rng.chance(25) {
            let triggers = ["onFailure", "onCrash", "onFailureOrCrash", "atLeastOnce"];
            let trigger = triggers[rng.below(4) as usize];
            a["retry"] = json!({ "trigger": trigger });
        }
        actors.push(a);
    }
    let crashes = if rng.chance(30) { 1 } else { 0 };
    let retries = if rng.chance(30) { 1 } else { 0 };
    json!({
        "schemaVersion": 1,
        "name": format!("random-{seed}"),
        "database": {
            "isolation": iso,
            "tables": [{
                "name": "t",
                "columns": [{"name":"id","notNull":true,"type":"int"}, {"name":"v","type":"int"}, {"name":"w","type":"int"}],
                "primaryKey": ["id"],
                "unique": [["w"]],
                "checks": ["@v >= 0 || @v < 0"]
            }],
            "initialState": { "t": [
                {"id": 1, "v": 1, "w": 10}, {"id": 2, "v": 2, "w": 11}, {"id": 3, "v": 0, "w": 12}
            ] }
        },
        "channels": { "ch": { "dedup": if rng.chance(50) { "byKey" } else { "none" } } },
        "actors": actors,
        "bounds": { "maxSteps": 40, "maxStates": 60000, "maxCrashes": crashes, "maxRetries": retries },
        "invariants": [
            { "name": "sum-bound", "when": "always", "assert": { "le": [ { "sum": { "table": "t", "where": {}, "column": "v" } }, 7 ] } },
            { "name": "few-effects", "when": "final", "assert": { "le": [ { "externals": { "channel": "ch" } }, 1 ] } },
            { "name": "rows-kept", "when": "final", "assert": { "ge": [ { "count": { "table": "t", "where": {} } }, 2 ] } }
        ]
    })
}
