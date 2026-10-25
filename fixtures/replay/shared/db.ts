// A minimal, dependency-free subset of the Drizzle ORM query API over Bun's
// built-in PostgreSQL client, for replay fixtures. Only what the fixtures
// use: update/insert/select with equality and IS NULL filters, and callback
// transactions.
//
// Replay checkpoints are emitted HERE, at the effect boundaries, so the
// application code under test contains no instrumentation:
//
//   before-db-write   before an UPDATE/INSERT is sent (label: table)
//   after-db-commit   after an auto-commit write was acknowledged by the
//                     server (label: table), or after a transaction's
//                     COMMIT was acknowledged (label: "transaction")
//   after-tx-write    after a write inside a transaction (not committed)
//   before-tx-commit  after the transaction callback returned, before COMMIT
//
// Commit semantics come from PostgreSQL, not from this file: outside a
// transaction every statement auto-commits; `begin` commits when the
// callback resolves and rolls back when it throws or the connection drops.

import { SQL } from "bun";
import { checkpoint, replayDatabase } from "../../../runtime/typescript/commitrift-replay.ts";

export type Column = { readonly table: string; readonly column: string };
export type Table<C extends Record<string, string>> = { readonly __table: string } & {
  readonly [K in keyof C]: Column;
};

export function table<C extends Record<string, string>>(name: string, columns: C): Table<C> {
  const out: Record<string, unknown> = { __table: name };
  for (const [key, column] of Object.entries(columns)) out[key] = { table: name, column };
  return out as Table<C>;
}

type Condition = { readonly render: (params: unknown[]) => string };

export function eq(column: Column, value: unknown): Condition {
  return {
    render: (params) => {
      params.push(value);
      return `"${column.column}" = $${params.length}`;
    },
  };
}

export function isNull(column: Column): Condition {
  return { render: () => `"${column.column}" IS NULL` };
}

type Client = { unsafe: (query: string, params?: unknown[]) => Promise<any[]> };

function columnName(tableDef: Record<string, unknown>, key: string): string {
  const column = tableDef[key] as Column | undefined;
  if (!column || typeof column !== "object") throw new Error(`unknown column ${key}`);
  return column.column;
}

function encode(value: unknown): unknown {
  if (value instanceof Date) return value.toISOString();
  if (value !== null && typeof value === "object") return JSON.stringify(value);
  return value;
}

class Executor {
  constructor(
    private readonly client: Client,
    private readonly inTransaction: boolean,
  ) {}

  private async write(tableName: string, query: string, params: unknown[]): Promise<void> {
    await checkpoint("before-db-write", tableName);
    await this.client.unsafe(query, params);
    await checkpoint(this.inTransaction ? "after-tx-write" : "after-db-commit", tableName);
  }

  update(target: Table<any>) {
    const self = this;
    return {
      set(values: Record<string, unknown>) {
        return {
          where(condition: Condition) {
            const params: unknown[] = [];
            const assignments = Object.entries(values).map(([key, value]) => {
              params.push(encode(value));
              return `"${columnName(target, key)}" = $${params.length}`;
            });
            const query = `UPDATE "${target.__table}" SET ${assignments.join(", ")} WHERE ${condition.render(params)}`;
            return self.write(target.__table, query, params);
          },
        };
      },
    };
  }

  insert(target: Table<any>) {
    const self = this;
    return {
      values(values: Record<string, unknown>) {
        const keys = Object.keys(values);
        const params = keys.map((key) => encode(values[key]));
        const columns = keys.map((key) => `"${columnName(target, key)}"`).join(", ");
        const placeholders = keys.map((_, i) => `$${i + 1}`).join(", ");
        const query = `INSERT INTO "${target.__table}" (${columns}) VALUES (${placeholders})`;
        return self.write(target.__table, query, params);
      },
    };
  }

  select() {
    const self = this;
    return {
      from(target: Table<any>) {
        return {
          async where(condition: Condition): Promise<Record<string, unknown>[]> {
            const params: unknown[] = [];
            const query = `SELECT * FROM "${target.__table}" WHERE ${condition.render(params)} ORDER BY 1`;
            const rows = await self.client.unsafe(query, params);
            // Map column names back to property names.
            const byColumn = new Map<string, string>();
            for (const [key, value] of Object.entries(target)) {
              if (key !== "__table") byColumn.set((value as Column).column, key);
            }
            return rows.map((row: Record<string, unknown>) =>
              Object.fromEntries(Object.entries(row).map(([c, v]) => [byColumn.get(c) ?? c, v])),
            );
          },
        };
      },
    };
  }
}

class Database extends Executor {
  constructor(private readonly sql: SQL) {
    super(sql as unknown as Client, false);
  }

  async transaction<T>(callback: (tx: Executor) => Promise<T>): Promise<T> {
    const result = await this.sql.begin(async (tx) => {
      const value = await callback(new Executor(tx as unknown as Client, true));
      await checkpoint("before-tx-commit", "transaction");
      return value;
    });
    await checkpoint("after-db-commit", "transaction");
    return result as T;
  }

  close(): Promise<void> {
    return this.sql.close();
  }
}

function connectReplayDatabase(): Database {
  const { path, database, username } = replayDatabase();
  return new Database(new SQL({ path, database, username, max: 2, idleTimeout: 0 }));
}

export const db = connectReplayDatabase();
