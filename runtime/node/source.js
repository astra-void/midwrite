// CommitRift source-execution client (protocol version 3, Phases 7–8).
//
// Runs ONE actor of a model-checking scenario: an authorized harness calls
// an original application function (for example an exported Server Action)
// and every SQL statement the application sends through node-postgres is
// held until the coordinator (`commitrift model-check --verify-source`)
// releases it. The coordinator releases statements in the order of the
// model's counterexample and observes what PostgreSQL does.
//
// Nothing in the application is modified. Instrumentation is confined to:
//
//   * the `pg.Pool` the harness creates (`coordinatePool`): every client's
//     `query` is wrapped; statements are sent unchanged, in the same order,
//     on the same connection, with the same await semantics;
//   * Drizzle's builder prototypes (`instrumentDrizzle`): when application
//     code calls a builder method (`db.select()`, `.where()`, `.limit()`,
//     `db.transaction()`), the innermost application stack frame is
//     remembered on the builder, and `then`/`transaction` run inside an
//     AsyncLocalStorage scope carrying it. This is how a statement is
//     attributed to a source location: the application awaits a thenable,
//     so the driver call itself has no application frame on its stack.
//
// Protocol (JSON lines over the Unix socket in COMMITRIFT_SOURCE_SOCKET):
//
//   worker -> coordinator  {"type":"hello","protocol":3,"actor":"A","attempt":1,"pid":123}
//   worker -> coordinator  {"type":"query","seq":1,"client":1,"backendPid":456,
//                           "sql":"...","params":[...],"site":{...}|null}
//   coordinator -> worker  {"type":"go"} | {"type":"stop","reason":"..."}
//   worker -> coordinator  {"type":"result","seq":1,"command":"SELECT","rowCount":1,
//                           "fields":[{"name","tableID","columnID"}],"rows":[[...]]}
//                        | {"type":"error","seq":1,"sqlstate":"23505","message":"..."}
//   worker -> coordinator  {"type":"done","outcome":"returned"|"threw","value":...,"error":"..."}
//
// Phase 8 (protocol 3) adds external effects and restarted workers:
//
//   worker -> coordinator  {"type":"effect","seq":2,"channel":"card-charge",
//                           "key":"req-1"|null,"payload":{...},"site":{...}|null}
//   coordinator -> worker  {"type":"go","effect":{"id":1,"deduplicated":false}}
//   worker -> coordinator  {"type":"result","seq":2,"command":"EFFECT",...}
//
// An effect is held like a statement. The receiving side of the channel is
// a TEST DOUBLE hosted by the coordinator (it outlives a killed worker): it
// records the effect when it releases it and applies the channel's
// DECLARED deduplication contract. `COMMITRIFT_SOURCE_ATTEMPT` numbers the
// executions of one actor: a retried actor is a NEW process (attempt 2, 3,
// ...) with fresh connections; nothing of a killed attempt is reused.
//
// A "stop" makes the held statement fail with CoordinatorStop (the run is
// then inconclusive); the coordinator never fakes a result.

import { AsyncLocalStorage } from "node:async_hooks";
import { connect } from "node:net";
import { fileURLToPath } from "node:url";

export const SOURCE_PROTOCOL_VERSION = 3;

/** Thrown into a held statement when the coordinator stops the run. */
export class CoordinatorStop extends Error {
  constructor(reason) {
    super(`stopped by the coordinator: ${reason}`);
    this.name = "CoordinatorStop";
  }
}

const SITE = Symbol.for("commitrift.source.site");
const COORDINATED = Symbol.for("commitrift.source.coordinated");
const scope = new AsyncLocalStorage();

let channel = null;
let seq = 0;
let clients = 0;

class Channel {
  constructor(socket) {
    this.socket = socket;
    this.buffer = "";
    this.lines = [];
    this.waiters = [];
    this.closed = false;
    socket.setEncoding("utf8");
    socket.on("data", (chunk) => {
      this.buffer += chunk;
      let newline;
      while ((newline = this.buffer.indexOf("\n")) >= 0) {
        const line = this.buffer.slice(0, newline);
        this.buffer = this.buffer.slice(newline + 1);
        const waiter = this.waiters.shift();
        if (waiter) waiter.resolve(line);
        else this.lines.push(line);
      }
    });
    socket.on("close", () => {
      this.closed = true;
      // The coordinator is gone (it ended the run, or it died): this worker
      // must not keep running unsupervised.
      if (!this.ending) process.exit(75);
      for (const waiter of this.waiters.splice(0)) {
        waiter.reject(new Error("the coordinator closed the connection"));
      }
    });
  }

  static open(path) {
    return new Promise((resolve, reject) => {
      const socket = connect(path);
      socket.once("connect", () => resolve(new Channel(socket)));
      socket.once("error", reject);
    });
  }

  send(message) {
    return new Promise((resolve, reject) => {
      this.socket.write(JSON.stringify(message) + "\n", (error) => (error ? reject(error) : resolve()));
    });
  }

  next() {
    const line = this.lines.shift();
    if (line !== undefined) return Promise.resolve(line);
    if (this.closed) return Promise.reject(new Error("the coordinator closed the connection"));
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  close() {
    this.ending = true;
    return new Promise((resolve) => this.socket.end(() => resolve()));
  }
}

/** Whether this process runs under `commitrift model-check --verify-source`. */
export function sourceActive() {
  return process.env.COMMITRIFT_SOURCE === "1" && !!process.env.COMMITRIFT_SOURCE_SOCKET;
}

/** Which execution of this actor this process is (1 for the first). */
export function sourceAttempt() {
  const n = Number(process.env.COMMITRIFT_SOURCE_ATTEMPT ?? "1");
  return Number.isInteger(n) && n > 0 ? n : 1;
}

function insideRoot(path, root) {
  return typeof path === "string" && typeof root === "string" && root.length > 1 && path.startsWith(root + "/");
}

/**
 * `pg.Pool` settings for this actor: the run's disposable database (a Unix
 * socket inside the private run directory), an application name that
 * identifies the actor's sessions to the coordinator. Never reads
 * DATABASE_URL or PG* variables.
 */
export function sourcePoolConfig(overrides = {}) {
  const root = process.env.COMMITRIFT_REPLAY_ROOT;
  const path = process.env.COMMITRIFT_REPLAY_PG_SOCKET;
  const database = process.env.COMMITRIFT_REPLAY_PG_DATABASE;
  const user = process.env.COMMITRIFT_REPLAY_PG_USER;
  if (!root || !path || !database || !user) {
    throw new Error("refusing to connect: not running under commitrift source verification");
  }
  if (!insideRoot(path, root)) {
    throw new Error("refusing to connect: the database socket is outside the run directory");
  }
  const match = /^(.*)\/\.s\.PGSQL\.(\d+)$/.exec(path);
  if (!match) throw new Error("refusing to connect: unexpected socket path");
  return {
    idleTimeoutMillis: 0,
    ...overrides,
    host: match[1],
    port: Number(match[2]),
    database,
    user,
    application_name: `commitrift-actor-${process.env.COMMITRIFT_SOURCE_ACTOR ?? "?"}#${sourceAttempt()}`,
  };
}

// ---------------------------------------------------------------------------
// Source attribution
// ---------------------------------------------------------------------------

const FRAME = /^\s*at (?:(.*?) \()?(.*?):(\d+):(\d+)\)?$/;

function projectRoot() {
  return process.env.COMMITRIFT_SOURCE_PROJECT ?? "";
}

/** Application frames of the current stack, innermost first. */
function applicationFrames() {
  const root = projectRoot();
  if (!root) return [];
  const saved = Error.stackTraceLimit;
  Error.stackTraceLimit = 64;
  const stack = new Error().stack ?? "";
  Error.stackTraceLimit = saved;
  const frames = [];
  for (const line of stack.split("\n").slice(1)) {
    const m = FRAME.exec(line);
    if (!m) continue;
    let file = m[2];
    if (file.startsWith("file://")) {
      try {
        file = fileURLToPath(file);
      } catch {
        continue;
      }
    }
    if (!insideRoot(file, root) || file.includes("/node_modules/")) continue;
    const rel = file.slice(root.length + 1);
    // The harness itself is not application code.
    if (rel.startsWith("verify/") || rel.startsWith("dist/verify/")) continue;
    frames.push({
      function: (m[1] ?? "").replace(/^async /, ""),
      file: rel,
      line: Number(m[3]),
      column: Number(m[4]),
    });
  }
  return frames;
}

// Async call-chain reconstruction. V8's async stack traces follow `await`
// chains only: after `return helper()` (no `await`) resumes the helper from
// a microtask, the caller's frame is missing. Within ONE execution of the
// actor, each application frame position is remembered with the outer
// application frames seen below it the first time; a chain whose outermost
// frame was seen with exactly one caller chain is extended by it. When a
// position was seen with different caller chains, nothing is extended.
const callers = new Map();

function frameKey(f) {
  return `${f.file}:${f.line}:${f.column}:${f.function}`;
}

function rememberCallers(frames) {
  for (let i = 0; i < frames.length; i += 1) {
    const key = frameKey(frames[i]);
    const rest = JSON.stringify(frames.slice(i + 1));
    const seen = callers.get(key);
    if (seen === undefined) callers.set(key, rest);
    else if (seen !== rest && rest !== "[]" && seen !== "[]") callers.set(key, null);
    else if (seen === "[]" && rest !== "[]") callers.set(key, rest);
  }
}

/** The application frames of the current stack, as a site. */
function currentSite() {
  const frames = applicationFrames();
  if (frames.length === 0) return null;
  rememberCallers(frames);
  const outer = callers.get(frameKey(frames[frames.length - 1]));
  if (typeof outer === "string" && outer !== "[]") {
    const restored = JSON.parse(outer);
    // A complete chain ends where the remembered one ends: only extend a
    // chain that is a strict prefix of what was seen before.
    return { frames: frames.concat(restored), reconstructed: true };
  }
  return { frames };
}

function wrapMethods(prototype) {
  if (!prototype || prototype[COORDINATED]) return;
  Object.defineProperty(prototype, COORDINATED, { value: true });
  for (const name of Object.getOwnPropertyNames(prototype)) {
    if (["constructor", "then", "catch", "finally", "execute", "transaction"].includes(name)) continue;
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
    if (!descriptor || typeof descriptor.value !== "function" || descriptor.get || descriptor.set) continue;
    const original = descriptor.value;
    const wrapped = function (...args) {
      const site = currentSite();
      const result = original.apply(this, args);
      if (site) {
        for (const target of [this, result]) {
          if (target && typeof target === "object") {
            try {
              Object.defineProperty(target, SITE, { value: site, configurable: true, writable: true });
            } catch {
              // Frozen objects keep their previous site.
            }
          }
        }
      }
      return result;
    };
    Object.defineProperty(prototype, name, { ...descriptor, value: wrapped });
  }
}

/** Run `then` inside a scope carrying the builder's call site. */
function patchThen(prototype) {
  if (!prototype || !Object.prototype.hasOwnProperty.call(prototype, "then")) return;
  const then = prototype.then;
  if (typeof then !== "function" || then[COORDINATED]) return;
  const wrapped = function (onFulfilled, onRejected) {
    const site = this[SITE];
    if (!site) return then.call(this, onFulfilled, onRejected);
    return scope.run({ site }, () => then.call(this, onFulfilled, onRejected));
  };
  Object.defineProperty(wrapped, COORDINATED, { value: true });
  prototype.then = wrapped;
}

/**
 * Attribute Drizzle statements to application source. `drizzle` is the
 * `drizzle-orm` module, `pgCore` the `drizzle-orm/pg-core` module, as the
 * application resolves them. Builder methods are often inherited (`where`
 * and `limit` live on a query-builder base class) and `then` is copied
 * into some builders by Drizzle's mixins, so every prototype of each
 * builder's chain is instrumented.
 */
export function instrumentDrizzle(drizzle, pgCore) {
  patchThen(drizzle.QueryPromise?.prototype);
  for (const name of [
    "PgDatabase",
    "PgTransaction",
    "PgSelectBuilder",
    "PgSelectBase",
    "PgInsertBuilder",
    "PgInsertBase",
    "PgUpdateBuilder",
    "PgUpdateBase",
    "PgDeleteBase",
  ]) {
    let prototype = pgCore[name]?.prototype;
    while (prototype && prototype !== Object.prototype) {
      patchThen(prototype);
      wrapMethods(prototype);
      prototype = Object.getPrototypeOf(prototype);
    }
  }
  // `db.transaction(cb)`: BEGIN/COMMIT are attributed to the call.
  const database = pgCore.PgDatabase?.prototype;
  if (database && typeof database.transaction === "function" && !database.transaction[COORDINATED]) {
    const transaction = database.transaction;
    const wrapped = function (...args) {
      const site = currentSite() ?? scope.getStore()?.site;
      return site ? scope.run({ site }, () => transaction.apply(this, args)) : transaction.apply(this, args);
    };
    Object.defineProperty(wrapped, COORDINATED, { value: true });
    database.transaction = wrapped;
  }
}

// ---------------------------------------------------------------------------
// Statement coordination
// ---------------------------------------------------------------------------

function statementText(args) {
  const first = args[0];
  if (typeof first === "string") return first;
  if (first && typeof first === "object" && typeof first.text === "string") return first.text;
  return undefined;
}

function statementParams(args) {
  const first = args[0];
  if (first && typeof first === "object" && Array.isArray(first.values)) return first.values;
  return Array.isArray(args[1]) ? args[1] : [];
}

function jsonable(value) {
  if (value === undefined) return null;
  if (typeof value === "bigint") return value.toString();
  if (value instanceof Date) return value.toISOString();
  if (Buffer.isBuffer?.(value)) return `\\x${value.toString("hex")}`;
  if (Array.isArray(value)) return value.map(jsonable);
  if (value && typeof value === "object") {
    const out = {};
    for (const [k, v] of Object.entries(value)) out[k] = jsonable(v);
    return out;
  }
  return value;
}

/**
 * Result columns with their origin (`tableID`, `columnID`: PostgreSQL's
 * table OID and attribute number, 0 for computed columns) and rows as
 * arrays: joins return several columns with the same name.
 */
function resultShape(result) {
  const fields = Array.isArray(result?.fields)
    ? result.fields.map((f) => ({ name: f.name, tableID: f.tableID ?? 0, columnID: f.columnID ?? 0 }))
    : [];
  const rows = Array.isArray(result?.rows)
    ? result.rows.map((row) => (Array.isArray(row) ? row.map(jsonable) : fields.map((f) => jsonable(row[f.name]))))
    : [];
  return { fields, rows };
}

async function hold(client, text, params) {
  if (!channel) throw new Error("source coordination is not connected");
  seq += 1;
  const id = seq;
  const site = scope.getStore()?.site ?? currentSite();
  await channel.send({
    type: "query",
    seq: id,
    client: client[COORDINATED]?.id ?? 0,
    backendPid: typeof client.processID === "number" ? client.processID : null,
    sql: text ?? null,
    params: jsonable(params),
    site,
  });
  const reply = JSON.parse(await channel.next());
  if (reply.type === "go") return id;
  if (reply.type === "stop") throw new CoordinatorStop(String(reply.reason ?? ""));
  throw new Error(`unexpected coordination message: ${reply.type}`);
}

async function report(id, result, error, shape = resultShape) {
  if (!channel) return;
  if (error) {
    await channel.send({
      type: "error",
      seq: id,
      sqlstate: typeof error.code === "string" ? error.code : null,
      message: String(error.message ?? error),
    });
  } else {
    const { fields, rows } = shape(result);
    await channel.send({
      type: "result",
      seq: id,
      command: typeof result?.command === "string" ? result.command : null,
      // node-postgres: `rowCount`; postgres-js: `count`.
      rowCount:
        typeof result?.rowCount === "number"
          ? result.rowCount
          : typeof result?.count === "number"
            ? result.count
            : null,
      fields,
      rows,
    });
  }
}

/**
 * Perform an external effect (a payment-provider call, a queue publish)
 * through the coordinator's test double. Held until released, like a
 * statement; resolves with the double's receipt `{ id, deduplicated }`.
 * Application code never calls this: an approved test adapter in the
 * harness implements the application's provider seam with it.
 */
export async function sourceEffect(name, request = {}) {
  if (!channel) throw new Error("source coordination is not connected");
  if (typeof name !== "string" || name.length === 0) throw new Error("an effect needs a channel name");
  seq += 1;
  const id = seq;
  await channel.send({
    type: "effect",
    seq: id,
    channel: name,
    key: jsonable(request.key ?? null),
    payload: jsonable(request.payload ?? {}),
    site: currentSite(),
  });
  const reply = JSON.parse(await channel.next());
  if (reply.type === "stop") throw new CoordinatorStop(String(reply.reason ?? ""));
  if (reply.type !== "go") throw new Error(`unexpected coordination message: ${reply.type}`);
  const receipt = {
    id: reply.effect?.id ?? null,
    deduplicated: reply.effect?.deduplicated === true,
  };
  await channel.send({ type: "result", seq: id, command: "EFFECT", rowCount: 1, fields: [], rows: [] });
  return receipt;
}

function coordinateClient(client) {
  if (client[COORDINATED]) return client;
  clients += 1;
  Object.defineProperty(client, COORDINATED, { value: { id: clients } });
  const original = client.query;
  client.query = function (...args) {
    // Submittables (cursors, streams) are not coordinated: the run is
    // reported as unsupported when one is used.
    if (args[0] && typeof args[0].submit === "function") {
      const id = (seq += 1);
      channel?.send({ type: "unsupported", seq: id, reason: "a submittable (cursor/stream) query" });
      return original.apply(client, args);
    }
    const callback = typeof args[args.length - 1] === "function" ? args.pop() : undefined;
    const text = statementText(args);
    const params = statementParams(args);
    const run = (async () => {
      const id = await hold(client, text, params);
      let result;
      try {
        result = await original.apply(client, args);
      } catch (error) {
        await report(id, undefined, error);
        throw error;
      }
      await report(id, result, undefined);
      return result;
    })();
    if (callback) {
      run.then(
        (result) => callback(null, result),
        (error) => callback(error),
      );
      return undefined;
    }
    return run;
  };
  return client;
}

// ---------------------------------------------------------------------------
// postgres-js (Phase 8)
// ---------------------------------------------------------------------------

/**
 * postgres-js versions whose internals the adapter was verified against. The
 * adapter holds statements in `Query.prototype.handle`, the one place every
 * query (the application's, Drizzle's, and postgres-js's own BEGIN, COMMIT,
 * ROLLBACK and SAVEPOINT) is handed to a connection; postgres-js has no
 * public hook that can delay a statement (`debug` is synchronous). Other
 * versions are refused rather than assumed compatible.
 */
export const POSTGRES_JS_VERSIONS = ["3.4.5"];

/**
 * `POSTGRES_URL` for this actor on the run's disposable database: no host
 * in the URL, so postgres-js takes the Unix socket directory from PGHOST
 * (set here, with PGPORT and PGUSER); `application_name` identifies the
 * actor's sessions to the coordinator. Never reads an existing URL.
 */
export function sourcePostgresUrl() {
  const config = sourcePoolConfig();
  process.env.PGHOST = config.host;
  process.env.PGPORT = String(config.port);
  process.env.PGUSER = config.user;
  delete process.env.PGPASSWORD;
  return `postgres:///${encodeURIComponent(config.database)}?application_name=${encodeURIComponent(config.application_name)}`;
}

function postgresJsShape(result) {
  const fields = Array.isArray(result?.columns)
    ? result.columns.map((c) => ({ name: c.name, tableID: c.table ?? 0, columnID: c.number ?? 0 }))
    : [];
  const rows = Array.isArray(result)
    ? Array.from(result, (row) => (Array.isArray(row) ? row.map(jsonable) : fields.map((f) => jsonable(row[f.name]))))
    : [];
  return { fields, rows };
}

/**
 * Coordinate every query of the postgres-js module `postgres` (its default
 * export): queries are announced and held until released, then executed
 * unchanged; their completion is the query's own settlement (never the
 * call of a tagged template). Cursors, `forEach` and streams are reported as
 * unsupported. `version` is the installed package version.
 */
export function coordinatePostgresJs(postgres, version) {
  if (!POSTGRES_JS_VERSIONS.includes(version)) {
    throw new Error(
      `postgres-js ${version} is not supported by source verification (verified: ${POSTGRES_JS_VERSIONS.join(", ")})`,
    );
  }
  // A client that never connects, to reach the Query prototype shared by
  // every client of this module instance.
  const probe = postgres({ host: "/nonexistent-commitrift", max: 1, onnotice: () => {} });
  const prototype = Object.getPrototypeOf(probe.unsafe("select 1"));
  if (prototype[COORDINATED]) return;
  if (typeof prototype.handle !== "function" || typeof prototype.then !== "function") {
    throw new Error("postgres-js internals differ from the verified version: refusing to coordinate");
  }
  Object.defineProperty(prototype, COORDINATED, { value: true });
  const connection = { [COORDINATED]: { id: "postgres-js" } };
  prototype.handle = async function () {
    if (this.executed) return;
    this.executed = true;
    await 1;
    // Driver-internal catalog reads issued by a connection for itself
    // (array types after connecting, `target_session_attrs` probes) use the
    // connection's own `execute` as handler; they precede the application's
    // statements on that connection and are not held (they would block it).
    // Reported, not hidden.
    if (this.handler?.name === "execute") {
      const text = Array.isArray(this.strings) ? this.strings.join("$?") : String(this.strings);
      channel?.send({ type: "driver-internal", sql: text.trim() });
      return this.handler(this);
    }
    if (this.cursorFn || this.forEachFn || this.streaming) {
      seq += 1;
      channel?.send({ type: "unsupported", seq, reason: "a postgres-js cursor, forEach or stream" });
      return this.handler(this);
    }
    const text = Array.isArray(this.strings) ? this.strings.join("$?") : String(this.strings);
    let id;
    try {
      id = await hold(connection, text, this.args ?? []);
    } catch (error) {
      return this.reject(error);
    }
    Promise.prototype.then.call(
      this,
      (result) => report(id, result, undefined, postgresJsShape),
      (error) => report(id, undefined, error),
    );
    return this.handler(this);
  };
}

/**
 * Coordinate every connection of `pool` (a `pg.Pool`): clients the pool
 * creates (for `pool.query` and `pool.connect`) have their statements held
 * until released. Returns the pool.
 */
export function coordinatePool(pool) {
  if (pool[COORDINATED]) return pool;
  Object.defineProperty(pool, COORDINATED, { value: true });
  pool.on("connect", (client) => coordinateClient(client));
  return pool;
}

function describe(error) {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

/**
 * Entry point of a source-verification harness: run the entry the
 * coordinator selected with its input, report how it ended. `cleanup` runs
 * afterwards (closing pools).
 */
export async function runSourceActor(entries, cleanup) {
  if (!sourceActive()) {
    console.error("this harness only runs under `commitrift model-check --verify-source`");
    process.exit(2);
  }
  channel = await Channel.open(process.env.COMMITRIFT_SOURCE_SOCKET);
  const actor = process.env.COMMITRIFT_SOURCE_ACTOR ?? "";
  const entry = process.env.COMMITRIFT_SOURCE_ENTRY ?? "";
  await channel.send({
    type: "hello",
    protocol: SOURCE_PROTOCOL_VERSION,
    actor,
    entry,
    attempt: sourceAttempt(),
    pid: process.pid,
  });
  const input = JSON.parse(process.env.COMMITRIFT_SOURCE_INPUT ?? "null");
  let outcome = "returned";
  let value = null;
  let error;
  const run = Object.prototype.hasOwnProperty.call(entries, entry) ? entries[entry] : undefined;
  try {
    if (typeof run !== "function") throw new Error(`unknown entry \`${entry}\``);
    value = jsonable(await run(input));
  } catch (caught) {
    outcome = "threw";
    error = describe(caught);
    if (caught instanceof Error && caught.cause) error += ` (caused by ${describe(caught.cause)})`;
  }
  try {
    await cleanup?.();
  } catch {
    // Cleanup failures do not change the outcome.
  }
  await channel.send({ type: "done", outcome, value, error: error ?? null });
  await channel.close();
  process.exit(outcome === "returned" ? 0 : 1);
}
