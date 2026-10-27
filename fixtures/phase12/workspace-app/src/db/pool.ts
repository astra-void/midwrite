// The application's database pool seam: the process entry point (the
// server, a worker, or a test harness) configures the pool once, before the
// modules that use `db` are loaded.
import type { Pool } from "pg";

let configured: Pool | undefined;

export function usePool(pool: Pool): void {
  if (configured) throw new Error("the database pool is already configured");
  configured = pool;
}

export function configuredPool(): Pool {
  if (!configured) throw new Error("no database pool: call usePool() at startup");
  return configured;
}
