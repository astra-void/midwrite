// Runs one probe function against a fresh schema of the disposable database
// at PGHOST and prints what Drizzle sent and what PostgreSQL stored.
import { readFileSync } from "node:fs";

import pg from "pg";

import { usePool } from "../src/db/pool.js";

const pool = new pg.Pool({ max: 4 });
usePool(pool);
const { sent } = await import("../src/db/client.js");
const { calls } = await import("../src/lib/calls.js");
const probes: Record<string, (...args: string[]) => Promise<void>> = await import("../src/probes.js");

const [name, ...args] = process.argv.slice(2);
const migration = readFileSync(new URL("../../drizzle/0000_init.sql", import.meta.url), "utf8");
await pool.query("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;");
await pool.query(migration);
const probe = probes[name];
if (!probe) throw new Error(`no probe ${name}`);
let error: string | null = null;
try {
  await probe(...args);
} catch (e) {
  error = String(e);
}
const tables: Record<string, unknown[]> = {};
for (const table of ["people", "profiles", "notes", "counters", "tickets"]) {
  const { rows } = await pool.query(`SELECT row_to_json(t) AS r FROM "${table}" t ORDER BY t.id::text`);
  tables[table] = rows.map((x) => x.r);
}
console.log(JSON.stringify({ probe: name, error, calls, sent, tables }));
await pool.end();
