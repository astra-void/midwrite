//! Print the Drizzle schema facts of a file (development aid):
//! `cargo run -p commitrift-analyzer --example schema_dump ROOT FILE`.
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let schema =
        commitrift_analyzer::schema::extract_schema(std::path::Path::new(&args[1]), &args[2]);
    match schema {
        Ok(s) => println!("{}", serde_json::to_string_pretty(&s).unwrap()),
        Err(e) => eprintln!("{e}"),
    }
}
