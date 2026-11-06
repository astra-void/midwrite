//! Built-in SDK effects (DEV-148): calls on clients of known SDK packages
//! are external effects without a configuration entry, when the file shows
//! where the client comes from. Cross-module provenance (a client imported
//! from a project module) is covered by `fixtures/projects/sdk_clients`.

use midwrite_analyzer::Analyzer;
use midwrite_core::config::Config;
use midwrite_core::diagnostic::Diagnostic;
use midwrite_core::ir::RecognitionBasis;

fn analyze_with(config: Config, source: &str) -> Vec<Diagnostic> {
    Analyzer::new(config)
        .analyze_source("test.ts", source)
        .expect("source parses")
        .diagnostics
}

fn analyze(source: &str) -> Vec<Diagnostic> {
    analyze_with(Config::default(), source)
}

/// `(function, code)` of every diagnostic.
fn found(diagnostics: &[Diagnostic]) -> Vec<(String, &'static str)> {
    let mut out: Vec<_> = diagnostics
        .iter()
        .map(|d| (d.function.clone(), d.code.as_str()))
        .collect();
    out.sort();
    out
}

const SOURCE: &str = r#"
import { drizzle } from "drizzle-orm/node-postgres";
import { Resend } from "resend";
import Stripe from "stripe";
import { type SupabaseClient } from "@supabase/supabase-js";
import Redis from "ioredis";
import { put } from "@vercel/blob";
import { S3Client, PutObjectCommand, GetObjectCommand } from "@aws-sdk/client-s3";

const db = drizzle(process.env.DATABASE_URL!);
const resend = new Resend("key");
const stripe = new Stripe("key");
const redis = new Redis();
const s3 = new S3Client({});

export async function constructed(id: string) {
  await db.update(users).set({ id });
  await resend.emails.send({ to: id });
}
export async function payment(id: string) {
  await stripe.refunds.create({ payment_intent: id });
  await db.update(orders).set({ refunded: true });
}
export async function typedParameter(client: SupabaseClient, id: string) {
  await db.delete(documents);
  await client.storage.from("vault").remove([id]);
}
export async function cache(id: string) {
  await db.delete(apiKeys);
  await redis.del(id);
}
export async function packageFunction(id: string) {
  await db.insert(files).values({ id });
  await put(id, "body", { access: "public" });
}
export async function s3Write(id: string) {
  await db.insert(files).values({ id });
  await s3.send(new PutObjectCommand({ Key: id }));
}
"#;

#[test]
fn calls_on_provable_sdk_clients_are_external_effects() {
    let diagnostics = analyze(SOURCE);
    assert_eq!(
        found(&diagnostics),
        [
            ("cache".to_owned(), "MW1001"),
            ("constructed".to_owned(), "MW1001"),
            ("packageFunction".to_owned(), "MW1001"),
            ("payment".to_owned(), "MW1002"),
            ("s3Write".to_owned(), "MW1001"),
            ("typedParameter".to_owned(), "MW1001"),
        ],
        "{diagnostics:#?}"
    );
    let constructed = diagnostics
        .iter()
        .find(|d| d.function == "constructed")
        .unwrap();
    assert!(
        constructed
            .evidence
            .iter()
            .any(|e| e.contains("built-in rule") && e.contains("Resend")),
        "{:#?}",
        constructed.evidence
    );
}

#[test]
fn reads_and_lookalikes_are_not_effects() {
    let diagnostics = analyze(
        r#"
        import { drizzle } from "drizzle-orm/node-postgres";
        import Stripe from "stripe";
        import Redis from "ioredis";
        import { S3Client, GetObjectCommand } from "@aws-sdk/client-s3";
        const db = drizzle(process.env.DATABASE_URL!);
        const stripe = new Stripe("key");
        const redis = new Redis();
        const s3 = new S3Client({});
        export async function reads(id: string) {
          await db.update(users).set({ id });
          await stripe.customers.retrieve(id);
          await redis.get(id);
          await s3.send(new GetObjectCommand({ Key: id }));
        }
        export async function lookalike(
          resend: { emails: { send(m: unknown): Promise<void> } },
          id: string,
        ) {
          await db.update(users).set({ id });
          await resend.emails.send({ to: id });
        }
        "#,
    );
    assert!(
        diagnostics.iter().all(|d| d.code.as_str() == "MW9001"),
        "{diagnostics:#?}"
    );
}

#[test]
fn configured_rules_come_first_and_built_ins_can_be_turned_off() {
    let configured = Config::from_json(
        r#"{ "externalEffects": [{ "function": "put", "category": "file-write", "importedFrom": "@vercel/blob" }] }"#,
        "test",
    )
    .unwrap();
    let analysis = Analyzer::new(configured)
        .analyze_source("test.ts", SOURCE)
        .unwrap();
    let function = analysis
        .ir
        .functions
        .iter()
        .find(|f| f.name == "packageFunction")
        .unwrap();
    let put = function.effects()[1];
    assert_eq!(
        put.recognition.as_ref().unwrap().basis,
        RecognitionBasis::ConfiguredImport,
        "the configured rule matched first"
    );

    let off = Config::from_json(r#"{ "builtinEffects": false }"#, "test").unwrap();
    let diagnostics = analyze_with(off, SOURCE);
    assert!(
        diagnostics.iter().all(|d| d.code.as_str() == "MW9001"),
        "{diagnostics:#?}"
    );
}
