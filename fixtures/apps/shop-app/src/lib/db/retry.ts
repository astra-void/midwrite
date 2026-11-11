// Run a unit of work in a database transaction, re-running it in a NEW
// transaction when PostgreSQL aborts it with a serialization failure
// (40001) or as a deadlock victim (40P01).
import type { ExtractTablesWithRelations } from "drizzle-orm";
import type { NodePgQueryResultHKT } from "drizzle-orm/node-postgres";
import type { PgTransaction } from "drizzle-orm/pg-core";

import { db } from "./drizzle.js";
import type * as schema from "./schema.js";

export type Tx = PgTransaction<
  NodePgQueryResultHKT,
  typeof schema,
  ExtractTablesWithRelations<typeof schema>
>;

export interface RetryOptions {
  /** Total number of executions (1: no retry). */
  attempts: number;
  isolationLevel: "read committed" | "repeatable read" | "serializable";
}

function sqlstate(error: unknown): string | undefined {
  const outer = error as { code?: unknown; cause?: { code?: unknown } };
  const code = outer.cause?.code ?? outer.code;
  return typeof code === "string" ? code : undefined;
}

export async function withTransactionRetry<T>(
  work: (tx: Tx) => Promise<T>,
  options: RetryOptions,
): Promise<T> {
  for (let attempt = 1; ; attempt += 1) {
    try {
      return await db.transaction(work, { isolationLevel: options.isolationLevel });
    } catch (error) {
      const code = sqlstate(error);
      if ((code !== "40001" && code !== "40P01") || attempt >= options.attempts) {
        throw error;
      }
    }
  }
}
