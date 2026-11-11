import type { ExtractTablesWithRelations } from "drizzle-orm";
import type { NodePgQueryResultHKT } from "drizzle-orm/node-postgres";
import { drizzle } from "drizzle-orm/node-postgres";
import type { PgTransaction } from "drizzle-orm/pg-core";
import { withReplicas } from "./replicas.js";
import * as schema from "./schema.js";

export const primaryDb = drizzle(process.env.DATABASE_PRIMARY_URL!, { schema });

const replicaDb = drizzle(process.env.DATABASE_REPLICA_URL!, { schema });

export const db = withReplicas(primaryDb, [replicaDb], (replicas) => replicas[0]!);

export const connectDb = async () => {
  return db;
};

export type Database = Awaited<ReturnType<typeof connectDb>>;

export type TransactionClient = PgTransaction<
  NodePgQueryResultHKT,
  typeof schema,
  ExtractTablesWithRelations<typeof schema>
>;

/** Works standalone and inside transactions: bound only when inlined. */
export type DatabaseOrTransaction = Database | TransactionClient;

export type DatabaseWithPrimary = Database & {
  $primary?: Database;
};
