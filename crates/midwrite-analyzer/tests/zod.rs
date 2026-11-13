//! Zod validation, differentially checked against the
//! pinned zod runtimes.
//!
//! For every case of `fixtures/apps/zod/matrix.json` (a schema, the APIs
//! it exists in, inputs), a generated TypeScript module wraps the schema's
//! `safeParse` and `parse`; Midwrite extracts both functions, the test runs
//! the extracted steps on each input with the model's own expression
//! evaluator, and compares the decision and the parsed value with what
//! `fixtures/apps/zod/run.mjs` reports from the real zod package:
//!
//! * zod 4.6.5 imported as `zod` (zod 4 API) and as `zod/v3` (zod 3 API);
//! * zod 3.25.76 imported as `zod` (zod 3 API);
//! * a project whose zod version cannot be determined (both runtimes are
//!   the oracle: the model must refuse wherever they differ);
//! * zod 3.25.76 imported as `zod/v4` (an early zod 4 implementation): the
//!   model must refuse every schema outside the explicit validation subset.
//!
//! The runtime is the oracle: expected outcomes are never written by hand.
//! The test is skipped (with a message) when Node.js or the zod directories
//! are missing, unless `MIDWRITE_REQUIRE_ZOD=1`. The zod directories
//! default to the pnpm store paths of the development machine and can be
//! set with `MIDWRITE_ZOD3_DIR` and `MIDWRITE_ZOD4_DIR`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use midwrite_analyzer::TsActorProvider;
use midwrite_model::ir::Op;
use midwrite_model::provider::{ActorProvider, ExtractRequest, Extracted};
use midwrite_model::value::{EvalError, Locals, Mode, Value};
use midwrite_model::zodfn;

const ZOD4: &str = "/Users/returnf4lse/Library/pnpm/v11/links/@/zod/4.6.5/6448fd814698825170f93bd428080c609bd45ccbc007f4c2dfd5fac1bb52f28d/node_modules/zod";
const ZOD3: &str = "/Users/returnf4lse/Library/pnpm/v11/links/@/zod/3.25.76/7137186c3790243b593612573154eb5e6c6084f41028ee5828258e8c91251cd6/node_modules/zod";

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn matrix_path() -> PathBuf {
    workspace().join("fixtures/apps/zod/matrix.json")
}

fn zod_dir(var: &str, default: &str) -> PathBuf {
    std::env::var_os(var).map_or_else(|| PathBuf::from(default), PathBuf::from)
}

/// The zod directories, or why the differential cannot run.
fn environment() -> Result<(PathBuf, PathBuf), String> {
    let node = Command::new("node").arg("--version").output();
    if !node.is_ok_and(|o| o.status.success()) {
        return Err("Node.js (`node`) is not available".to_owned());
    }
    let z3 = zod_dir("MIDWRITE_ZOD3_DIR", ZOD3);
    let z4 = zod_dir("MIDWRITE_ZOD4_DIR", ZOD4);
    for (d, v) in [(&z3, "3.25.76"), (&z4, "4.6.5")] {
        let pkg = std::fs::read_to_string(d.join("package.json"))
            .map_err(|_| format!("zod {v} is not at {}", d.display()))?;
        let json: serde_json::Value = serde_json::from_str(&pkg).map_err(|e| e.to_string())?;
        if json["version"] != v {
            return Err(format!(
                "{} is zod {}, not {v}",
                d.display(),
                json["version"]
            ));
        }
    }
    Ok((z3, z4))
}

fn skip_or_fail(why: &str) {
    if std::env::var("MIDWRITE_REQUIRE_ZOD").as_deref() == Ok("1") {
        panic!("MIDWRITE_REQUIRE_ZOD=1 but the zod differential cannot run: {why}");
    }
    eprintln!("SKIPPED zod differential: {why} (set MIDWRITE_REQUIRE_ZOD=1 to require it)");
}

#[derive(Clone, Debug)]
struct Case {
    id: String,
    apis: Vec<u64>,
    schema: String,
    inputs: Vec<Value>,
}

fn cases() -> Vec<Case> {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(matrix_path()).unwrap()).unwrap();
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Case {
            id: c["id"].as_str().unwrap().to_owned(),
            apis: c["apis"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_u64().unwrap())
                .collect(),
            schema: c["schema"].as_str().unwrap().to_owned(),
            inputs: c["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| serde_json::from_value(v.clone()).unwrap())
                .collect(),
        })
        .collect()
}

/// What the real zod did with one input.
#[derive(Clone, Debug, PartialEq)]
struct Oracle {
    success: bool,
    data: Value,
    parse_threw: bool,
    /// `safeParse` itself threw (a refinement callback threw).
    safe_threw: bool,
}

fn node(dir: &Path, entry: &str, api: u64) -> BTreeMap<(String, usize), Oracle> {
    let out = Command::new("node")
        .arg(workspace().join("fixtures/apps/zod/run.mjs"))
        .arg(dir)
        .arg(entry)
        .arg(matrix_path())
        .arg(api.to_string())
        .output()
        .expect("node runs");
    assert!(
        out.status.success(),
        "run.mjs failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
    rows.into_iter()
        .map(|r| {
            (
                (
                    r["id"].as_str().unwrap().to_owned(),
                    r["input"].as_u64().unwrap() as usize,
                ),
                Oracle {
                    success: r["success"].as_bool().unwrap(),
                    data: serde_json::from_value(r["data"].clone()).unwrap(),
                    parse_threw: r["parseThrew"].as_bool().unwrap(),
                    safe_threw: r["safeThrew"].as_bool().unwrap(),
                },
            )
        })
        .collect()
}

/// The matrix's `const` arrays (`z.enum(KINDS)`), as TypeScript.
fn constants() -> String {
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(matrix_path()).unwrap()).unwrap();
    let mut out = String::new();
    if let Some(constants) = doc["constants"].as_object() {
        for (name, value) in constants {
            out.push_str(&format!("const {name} = {value} as const;\n"));
        }
    }
    out
}

/// One generated application: `package.json` (with or without a zod
/// dependency) and `src/check.ts` with `safe_<id>` / `parse_<id>`.
fn app(label: &str, zod_version: Option<&str>, specifier: &str, cases: &[&Case]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("midwrite-p16-zod-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    let deps = match zod_version {
        Some(v) => format!(r#", "dependencies": {{ "zod": "{v}" }}"#),
        None => String::new(),
    };
    std::fs::write(
        dir.join("package.json"),
        format!(r#"{{ "name": "p16-zod-{label}", "private": true{deps} }}"#),
    )
    .unwrap();
    let mut src = format!("import {{ z }} from \"{specifier}\";\n\n{}", constants());
    for c in cases {
        src.push_str(&format!(
            "const s_{id} = {schema};\n\
             export function safe_{id}(input: unknown) {{\n  const r = s_{id}.safeParse(input);\n  return {{ success: r.success, data: r.data }};\n}}\n\
             export function parse_{id}(input: unknown) {{\n  return s_{id}.parse(input);\n}}\n\n",
            id = c.id,
            schema = c.schema
        ));
    }
    std::fs::write(dir.join("src/check.ts"), src).unwrap();
    dir
}

fn extract(root: &Path, function: &str) -> Extracted {
    TsActorProvider::new(root)
        .extract(&ExtractRequest {
            root: root.to_path_buf(),
            file: String::new(),
            function: String::new(),
            arguments: Vec::new(),
            tables: Vec::new(),
            externals: Vec::new(),
            transaction_retries: Vec::new(),
            max_inline_depth: 3,
            selector: Some(format!("src/check.ts#{function}")),
            contract: Some(Default::default()),
            replicas: false,
            replica_factories: Vec::new(),
            return_values: true,
            js_values: true,
        })
        .unwrap_or_else(|e| panic!("{function}: {e}"))
}

#[derive(Clone, Debug, PartialEq)]
enum Run {
    Returned(Value),
    Threw,
    Refused(String),
}

/// Run the extracted main code on `input` with the model's evaluator
/// (JavaScript mode): the steps zod lowering emits are `Let`, `JumpIf`,
/// `Jump`, `Throw`, `ReturnValue`, `Return` and `Unknown`.
fn run(x: &Extracted, input: &Value) -> Run {
    let code = &x.codes[0];
    let mut locals = Locals::new();
    for p in &x.params {
        locals.insert(p.clone(), input.clone());
    }
    let eval =
        |e: &midwrite_model::value::Expr, locals: &Locals| match e.eval(locals, None, Mode::Js) {
            Ok(v) => Ok(v),
            Err(EvalError::Unknown(d)) => Err(Run::Refused(d)),
            // A JavaScript exception (a refinement callback throwing).
            Err(EvalError::Type(_)) => Err(Run::Threw),
            Err(other) => panic!("an error the zod lowering must not raise: {other}"),
        };
    let mut returned = Value::undefined();
    let mut pc = 0;
    let mut steps = 0;
    while pc < code.len() {
        steps += 1;
        assert!(steps < 10_000, "the extracted code does not terminate");
        match &code[pc].op {
            Op::Let { local, expr } => match eval(expr, &locals) {
                Ok(v) => {
                    locals.insert(local.clone(), v);
                    pc += 1;
                }
                Err(r) => return r,
            },
            Op::JumpIf { cond, target } => match eval(cond, &locals) {
                Ok(v) => pc = if v.truthy() { *target } else { pc + 1 },
                Err(r) => return r,
            },
            Op::Jump { target } => pc = *target,
            Op::Throw { .. } => return Run::Threw,
            Op::ReturnValue { expr } => match eval(expr, &locals) {
                Ok(v) => {
                    returned = v;
                    pc += 1;
                }
                Err(r) => return r,
            },
            Op::Return => break,
            Op::Unknown { detail, .. } => return Run::Refused(detail.clone()),
            other => return Run::Refused(format!("unexpected step {other:?}")),
        }
    }
    Run::Returned(returned)
}

/// The model's answer for one input: `Ok((success, data, parse threw,
/// safeParse threw))`, or the refusal.
fn model(
    safe: &Extracted,
    parse: &Extracted,
    input: &Value,
) -> Result<(bool, Value, bool, bool), String> {
    let s = match run(safe, input) {
        Run::Returned(Value::Row(r)) => r,
        Run::Refused(d) => return Err(d),
        Run::Threw => {
            return match run(parse, input) {
                Run::Threw => Ok((false, Value::undefined(), true, true)),
                Run::Refused(d) => Err(d),
                other => panic!("safeParse threw but parse did not: {other:?}"),
            };
        }
        other => panic!("safeParse wrapper: {other:?}"),
    };
    let success = match s.get("success") {
        Some(Value::Bool(b)) => *b,
        other => panic!("success: {other:?}"),
    };
    let data = s.get("data").cloned().unwrap_or_else(Value::undefined);
    let (threw, parsed) = match run(parse, input) {
        Run::Threw => (true, None),
        Run::Returned(v) => (false, Some(v)),
        Run::Refused(d) => return Err(d),
    };
    if let Some(p) = parsed {
        assert_eq!(p, data, "parse and safeParse disagree in the model");
    }
    Ok((success, data, threw, false))
}

#[derive(Default, Debug)]
struct Tally {
    compared: usize,
    agreed: usize,
    refused: Vec<String>,
    mismatches: Vec<String>,
}

fn compare(
    tally: &mut Tally,
    label: &str,
    case: &Case,
    i: usize,
    model: Result<(bool, Value, bool, bool), String>,
    oracle: &Oracle,
) {
    tally.compared += 1;
    let at = format!("{label} {}[{i}] {}", case.id, case.inputs[i].render());
    match model {
        Err(why) => tally.refused.push(format!("{at}: {why}")),
        Ok((success, data, threw, safe_threw)) => {
            let same = safe_threw == oracle.safe_threw
                && threw == oracle.parse_threw
                && (safe_threw || success == oracle.success)
                && (safe_threw || !success || data == oracle.data);
            if same {
                tally.agreed += 1;
            } else {
                tally.mismatches.push(format!(
                    "{at}: model success={success} data={} threw={threw} safeThrew={safe_threw}; zod success={} data={} threw={} safeThrew={}",
                    data.render(),
                    oracle.success,
                    oracle.data.render(),
                    oracle.parse_threw,
                    oracle.safe_threw
                ));
            }
        }
    }
}

/// Refusals the matrix expects (inputs or keys the model does not
/// support); any other refusal fails the test.
fn refusal_expected(label: &str, case: &str, i: usize) -> bool {
    match (label, case) {
        (_, "prototype_key") => true,
        // A field whose refinement throws when the key is absent: the
        // explicit lowering needs what an absent key yields, so the schema
        // is refused.
        (_, "refine_field_absent_vs_version") => true,
        // zod 4 accepts a Map or Set as an object; not modeled.
        ("zod4" | "unknown", "object_map_input") => i < 2,
        // zod 3 and 4 differ on these inputs or statically.
        ("unknown", _) => true,
        _ => false,
    }
}

#[test]
fn zod_parse_matches_the_pinned_runtimes() {
    let (z3, z4) = match environment() {
        Ok(x) => x,
        Err(why) => return skip_or_fail(&why),
    };
    let all = cases();
    let o4 = node(&z4, "index", 4);
    let o3 = node(&z3, "index", 3);
    let o4v3 = node(&z4, "v3", 3);
    let o3v4 = node(&z3, "v4", 4);
    let mut tally = Tally::default();
    let mut per_runtime: Vec<(String, usize, usize)> = Vec::new();

    // Known versions: the model must agree.
    for (label, version, specifier, api, oracle) in [
        ("zod4", Some("4.6.5"), "zod", 4u64, &o4),
        ("zod3", Some("3.25.76"), "zod", 3, &o3),
        ("zod4-v3", Some("4.6.5"), "zod/v3", 3, &o4v3),
    ] {
        let mine: Vec<&Case> = all.iter().filter(|c| c.apis.contains(&api)).collect();
        let root = app(label, version, specifier, &mine);
        let before = (tally.compared, tally.agreed);
        for c in &mine {
            let safe = extract(&root, &format!("safe_{}", c.id));
            let parse = extract(&root, &format!("parse_{}", c.id));
            for (i, input) in c.inputs.iter().enumerate() {
                let m = model(&safe, &parse, input);
                let o = &oracle[&(c.id.clone(), i)];
                if m.is_err() && !refusal_expected(label, &c.id, i) {
                    tally
                        .mismatches
                        .push(format!("{label} {}[{i}]: unexpected refusal {m:?}", c.id));
                }
                compare(&mut tally, label, c, i, m, o);
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        per_runtime.push((
            label.to_owned(),
            tally.compared - before.0,
            tally.agreed - before.1,
        ));
    }

    // Unknown version: refuse wherever zod 3 and zod 4 differ, agree
    // elsewhere.
    {
        let mine: Vec<&Case> = all
            .iter()
            .filter(|c| c.apis.contains(&3) && c.apis.contains(&4))
            .collect();
        let root = app("unknown", None, "zod", &mine);
        let before = (tally.compared, tally.agreed);
        for c in &mine {
            let safe = extract(&root, &format!("safe_{}", c.id));
            let parse = extract(&root, &format!("parse_{}", c.id));
            for (i, input) in c.inputs.iter().enumerate() {
                let key = (c.id.clone(), i);
                let (a, b) = (&o3[&key], &o4[&key]);
                let m = model(&safe, &parse, input);
                if a != b {
                    tally.compared += 1;
                    match m {
                        Err(_) => tally
                            .refused
                            .push(format!("unknown {}[{i}] (versions differ)", c.id)),
                        Ok(x) => tally.mismatches.push(format!(
                            "unknown {}[{i}]: zod 3 and 4 differ but the model answered {x:?}",
                            c.id
                        )),
                    }
                } else {
                    compare(&mut tally, "unknown", c, i, m, a);
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        per_runtime.push((
            "unknown".to_owned(),
            tally.compared - before.0,
            tally.agreed - before.1,
        ));
    }

    // `zod/v4` of zod 3.25: only the explicit validation subset is modeled.
    {
        let mine: Vec<&Case> = all.iter().filter(|c| c.apis.contains(&4)).collect();
        let root = app("zod3-v4", Some("3.25.76"), "zod/v4", &mine);
        let before = (tally.compared, tally.agreed);
        for c in &mine {
            let safe = extract(&root, &format!("safe_{}", c.id));
            let parse = extract(&root, &format!("parse_{}", c.id));
            for (i, input) in c.inputs.iter().enumerate() {
                let m = model(&safe, &parse, input);
                match &m {
                    Err(_) => {
                        tally.compared += 1;
                        tally.refused.push(format!("zod3-v4 {}[{i}]", c.id));
                    }
                    Ok(_) => compare(&mut tally, "zod3-v4", c, i, m, &o3v4[&(c.id.clone(), i)]),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        per_runtime.push((
            "zod3-v4".to_owned(),
            tally.compared - before.0,
            tally.agreed - before.1,
        ));
    }

    eprintln!("zod differential:");
    for (label, compared, agreed) in &per_runtime {
        eprintln!("  {label}: {compared} comparisons, {agreed} agreements");
    }
    eprintln!(
        "  total: {} comparisons, {} agreements, {} refusals, {} mismatches",
        tally.compared,
        tally.agreed,
        tally.refused.len(),
        tally.mismatches.len()
    );
    for r in &tally.refused {
        eprintln!("  refused: {r}");
    }
    assert!(
        tally.mismatches.is_empty(),
        "mismatches with the zod runtime:\n{}",
        tally.mismatches.join("\n")
    );
    assert_eq!(
        tally.compared,
        tally.agreed + tally.refused.len(),
        "every comparison agrees or is an expected refusal"
    );
}

/// Synthesized inputs: for every schema and runtime, the value input
/// synthesis builds (`zodfn::conform`) is accepted by the real zod.
#[test]
fn synthesized_inputs_satisfy_the_pinned_runtimes() {
    let (z3, z4) = match environment() {
        Ok(x) => x,
        Err(why) => return skip_or_fail(&why),
    };
    // A matrix of one input per case: the synthesized one.
    let mut doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(matrix_path()).unwrap()).unwrap();
    let all = cases();
    let dir = std::env::temp_dir().join(format!("midwrite-p16-zod-conform-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut checked = 0;
    for (version, api, zdir) in [
        (zodfn::Version::V4, 4u64, &z4),
        (zodfn::Version::V3, 3, &z3),
    ] {
        let root = app(
            &format!("conform{api}"),
            Some(if api == 4 { "4.6.5" } else { "3.25.76" }),
            "zod",
            &[],
        );
        let mut synthesized = Vec::new();
        for (k, c) in all.iter().enumerate() {
            if !c.apis.contains(&api) || c.id == "prototype_key" {
                continue;
            }
            // The schema as the extractor reads it.
            std::fs::write(
                root.join("src/check.ts"),
                format!(
                    "import {{ z }} from \"zod\";\n{}const s = {};\nexport function f(input: unknown) {{ return s.parse(input); }}\n",
                    constants(),
                    c.schema
                ),
            )
            .unwrap();
            let x = extract(&root, "f");
            let desc = x.codes[0].iter().find_map(|s| match &s.op {
                Op::Let {
                    expr:
                        midwrite_model::value::Expr::Call {
                            func: midwrite_model::value::Builtin::ZodCheck,
                            args,
                        },
                    ..
                } => match args.first() {
                    Some(midwrite_model::value::Expr::Lit(d)) => Some(d.clone()),
                    _ => None,
                },
                _ => None,
            });
            // explicit validation subset schemas of zod 3 keep their explicit lowering.
            let Some(desc) = desc else {
                // Refused schemas have nothing to synthesize from.
                if refusal_expected("synthesis", &c.id, 0) {
                    continue;
                }
                assert_eq!(
                    version,
                    zodfn::Version::V3,
                    "{}: no `ZodCheck` emitted",
                    c.id
                );
                continue;
            };
            let (v, schema) = zodfn::from_value(&desc).unwrap();
            assert_eq!(v, version);
            let Some(value) = zodfn::conform(v, &schema, None, "input") else {
                // A refinement may reject every placeholder (a cross-field
                // equality); then nothing is synthesized, never a wrong value.
                if zodfn::render(&schema).contains(".refine(") {
                    continue;
                }
                panic!("{}: no valid input synthesized", c.id);
            };
            synthesized.push((k, value));
        }
        let _ = std::fs::remove_dir_all(&root);
        // Run exactly those through the runtime.
        let constants = doc["constants"].clone();
        let cases_json = doc["cases"].as_array_mut().unwrap();
        let mut one = serde_json::json!({ "constants": constants, "cases": [] });
        for (k, value) in &synthesized {
            let mut c = cases_json[*k].clone();
            c["inputs"] = serde_json::json!([value]);
            one["cases"].as_array_mut().unwrap().push(c);
        }
        let path = dir.join(format!("conform{api}.json"));
        std::fs::write(&path, one.to_string()).unwrap();
        let out = Command::new("node")
            .arg(workspace().join("fixtures/apps/zod/run.mjs"))
            .arg(zdir)
            .arg("index")
            .arg(&path)
            .arg(api.to_string())
            .output()
            .unwrap();
        let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(rows.len(), synthesized.len());
        for r in rows {
            assert_eq!(
                r["success"], true,
                "zod {api} rejects the synthesized input of {}",
                r["id"]
            );
            checked += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("zod: {checked} synthesized inputs accepted by the pinned runtimes");
    assert!(checked > 40);
}
