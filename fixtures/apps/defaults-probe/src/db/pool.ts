import type { Pool } from "pg";

let configured: Pool | undefined;

export function usePool(pool: Pool): void {
  configured = pool;
}

export function configuredPool(): Pool {
  if (!configured) throw new Error("no database pool: call usePool() at startup");
  return configured;
}
