// node-postgres (`pg`) adapter for CommitRift replay harnesses.
//
// `instrumentPgPool(pool)` wraps the query methods of ONE pool instance so
// that replay checkpoints are emitted at the database effect boundaries of
// whatever library sends the queries (Drizzle ORM, Kysely, raw `pg`). The
// application code under test stays uninstrumented; nothing is patched
// globally; `pg` itself is not imported (duck typing), so this module adds
// no dependency.
//
// Checkpoints (label in parentheses), emitted only for promise-style calls:
//
//   before-db-write   (table)        before an INSERT/UPDATE/DELETE is sent
//   after-db-commit   (table)        after an auto-commit write was acknowledged
//   after-tx-write    (table)        after a write inside an open transaction
//   before-tx-commit  (transaction)  before COMMIT is sent
//   after-db-commit   (transaction)  after the server acknowledged COMMIT as a commit
//   after-tx-rollback (transaction)  after ROLLBACK, or after a COMMIT the
//                                    server answered with ROLLBACK (failed transaction)
//
// What is a commit comes from PostgreSQL's reply, not from this file: a
// COMMIT of an aborted transaction is answered with the command tag
// ROLLBACK and therefore never produces `after-db-commit`. A COMMIT that
// fails with an error (e.g. a deferred constraint) produces no checkpoint;
// the error propagates to the caller unchanged.
//
// Statements are classified by their leading keyword (after comments), the
// way ORMs generate them. Statements this adapter does not understand
// (SELECT, DDL, CTEs that write, `SAVEPOINT`) pass through without a
// checkpoint; a write hidden in a CTE or a function call is therefore not
// a checkpoint boundary.

import { checkpoint, replayDatabase } from "./index.js";

const INSTRUMENTED = Symbol.for("commitrift.replay.pg.instrumented");

function stripLeadingComments(text) {
  let rest = text;
  for (;;) {
    rest = rest.replace(/^\s+/, "");
    if (rest.startsWith("--")) {
      const end = rest.indexOf("\n");
      rest = end < 0 ? "" : rest.slice(end + 1);
    } else if (rest.startsWith("/*")) {
      const end = rest.indexOf("*/");
      rest = end < 0 ? "" : rest.slice(end + 2);
    } else {
      return rest;
    }
  }
}

const IDENT = String.raw`(?:"(?:[^"]|"")+"|[A-Za-z_][\w$]*)`;
const WRITE = new RegExp(String.raw`^(insert\s+into|update(?:\s+only)?|delete\s+from(?:\s+only)?)\s+(${IDENT}(?:\s*\.\s*${IDENT})?)`, "i");

const LAST_IDENT = new RegExp(`(${IDENT})$`);

/** The table of a possibly schema-qualified identifier, as PostgreSQL names it. */
function unquote(identifier) {
  const last = LAST_IDENT.exec(identifier)?.[1] ?? identifier;
  return last.startsWith('"') ? last.slice(1, -1).replace(/""/g, '"') : last.toLowerCase();
}

/**
 * Classify one SQL statement: `{ kind: "write", table }`, `begin`,
 * `commit`, `rollback` (a whole-transaction rollback, not `ROLLBACK TO
 * SAVEPOINT`), or `other`.
 */
export function classifyStatement(text) {
  if (typeof text !== "string") return { kind: "other" };
  const sql = stripLeadingComments(text);
  const write = WRITE.exec(sql);
  if (write) return { kind: "write", table: unquote(write[2]) };
  if (/^(begin|start\s+transaction)\b/i.test(sql)) return { kind: "begin" };
  if (/^(commit|end)\b(?!\s+prepared)/i.test(sql)) return { kind: "commit" };
  if (/^(rollback|abort)\b(?!\s+(?:work\s+|transaction\s+)?to\b)(?!\s+prepared)/i.test(sql)) {
    return { kind: "rollback" };
  }
  return { kind: "other" };
}

function statementText(args) {
  const first = args[0];
  if (typeof first === "string") return first;
  if (first && typeof first === "object" && typeof first.text === "string") return first.text;
  return undefined;
}

function wrapQuery(target, state) {
  const original = target.query;
  return function query(...args) {
    // Callback-style calls and submittables (cursors, streams) pass through
    // untouched: pg-pool's own `pool.query` uses them internally, and the
    // promise-style wrapper of the pool already covers that call.
    if (args.some((arg) => typeof arg === "function") || (args[0] && typeof args[0].submit === "function")) {
      return original.apply(target, args);
    }
    const statement = classifyStatement(statementText(args));
    return (async () => {
      if (statement.kind === "write") await checkpoint("before-db-write", statement.table);
      if (statement.kind === "commit" && state.inTransaction) await checkpoint("before-tx-commit", "transaction");
      const result = await original.apply(target, args);
      switch (statement.kind) {
        case "begin":
          state.inTransaction = true;
          break;
        case "write":
          await checkpoint(state.inTransaction ? "after-tx-write" : "after-db-commit", statement.table);
          break;
        case "commit": {
          const wasOpen = state.inTransaction;
          state.inTransaction = false;
          if (wasOpen) {
            const committed = !result || typeof result.command !== "string" || result.command.toUpperCase() === "COMMIT";
            await checkpoint(committed ? "after-db-commit" : "after-tx-rollback", "transaction");
          }
          break;
        }
        case "rollback": {
          const wasOpen = state.inTransaction;
          state.inTransaction = false;
          if (wasOpen) await checkpoint("after-tx-rollback", "transaction");
          break;
        }
      }
      return result;
    })();
  };
}

function instrumentClient(client) {
  if (client[INSTRUMENTED]) return client;
  const state = { inTransaction: false };
  client.query = wrapQuery(client, state);
  Object.defineProperty(client, INSTRUMENTED, { value: state });
  // A client returned to the pool in the middle of a transaction is
  // released by the pool; forget the transaction state then.
  const release = client.release;
  if (typeof release === "function") {
    client.release = function (...args) {
      state.inTransaction = false;
      return release.apply(client, args);
    };
  }
  return client;
}

/**
 * Instrument one `pg.Pool` (or anything with the same `query`/`connect`
 * shape) in place and return it.
 */
export function instrumentPgPool(pool) {
  if (pool[INSTRUMENTED]) return pool;
  const connect = pool.connect;
  pool.query = wrapQuery(pool, { inTransaction: false });
  pool.connect = function (...args) {
    if (args.length > 0) return connect.apply(pool, args);
    return connect.call(pool).then(instrumentClient);
  };
  Object.defineProperty(pool, INSTRUMENTED, { value: true });
  return pool;
}

/**
 * Connection settings for `new pg.Pool(...)` that point at the run's
 * disposable database: the Unix socket directory inside the replay root.
 * Never reads DATABASE_URL or PG* variables.
 */
export function replayPoolConfig(overrides = {}) {
  const { path, database, username } = replayDatabase();
  const match = /^(.*)\/\.s\.PGSQL\.(\d+)$/.exec(path);
  if (!match) throw new Error("refusing to connect: unexpected replay socket path");
  return {
    max: 4,
    idleTimeoutMillis: 0,
    ...overrides,
    host: match[1],
    port: Number(match[2]),
    database,
    user: username,
  };
}
