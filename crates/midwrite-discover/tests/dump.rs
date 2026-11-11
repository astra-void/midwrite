//! Prints the extracted IR of every entry of a project (a debugging tool).
use std::path::PathBuf;

#[test]
#[ignore = "debugging tool: prints the extracted IR of a project"]
fn dump_ir() {
    let root = PathBuf::from(std::env::var("DUMP_ROOT").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/apps/commerce-app"
        )
        .to_owned()
    }));
    let provider = midwrite_discover::extract::MemoProvider::new(&root);
    let contracts = midwrite_discover::contracts::Contracts::default();
    let index = midwrite_discover::project::index(&root, contracts, &provider).unwrap();
    println!("schema files: {:?}", index.schema_files);
    for m in &index.modules {
        if m.parse_error.is_some() || m.declares_tables {
            println!("{} {:?} {}", m.file, m.parse_error, m.declares_tables);
        }
    }
    println!("tables: {:#?}", index.tables);
    println!(
        "refused: {:#?}\nhelpers: {:#?}\nnotes: {:#?}",
        index.refused_tables, index.helpers, index.notes
    );
    let filter = std::env::var("DUMP_FILTER").unwrap_or_default();
    for (_, e) in index.entries() {
        if !e.selector.contains(&filter) {
            continue;
        }
        println!("=== {} ({:?})", e.selector, e.kind);
        match midwrite_discover::extract::extract_entry(
            &provider,
            &index.bindings,
            &e.selector,
            None,
        ) {
            Err(err) => println!("ERR {err}"),
            Ok(x) => {
                println!("params {:?}", x.params);
                println!("assumptions {:#?}", x.assumptions);
                if let Some(entry) = &x.entry {
                    println!(
                        "entry {:?} wrapper {:?}",
                        entry.contract_status, entry.wrapper
                    );
                }
                for (ci, code) in x.codes.iter().enumerate() {
                    for (pc, s) in code.iter().enumerate() {
                        println!(
                            "  [{ci}:{pc}] {} @{}:{}",
                            serde_json::to_string(&s.op).unwrap(),
                            s.src.file,
                            s.src.line
                        );
                    }
                }
            }
        }
    }
}
