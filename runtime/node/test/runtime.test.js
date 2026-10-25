// Unit tests of the replay client and the pg adapter: `node --test runtime/node/test`.
// They use a fake supervisor (a Unix socket server in a temporary
// directory) and a fake pool; no database is involved.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:net";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

import { classifyStatement, instrumentPgPool } from "../pg.js";
import { checkpoint, replayActive } from "../index.js";

const here = dirname(fileURLToPath(import.meta.url));

test("statements are classified by their leading keyword", () => {
  assert.deepEqual(classifyStatement('update "orders" set "status" = $1 where "orders"."id" = $2'), {
    kind: "write",
    table: "orders",
  });
  assert.deepEqual(classifyStatement('insert into "public"."outbox" ("id") values ($1)'), { kind: "write", table: "outbox" });
  assert.deepEqual(classifyStatement("/* c */ -- x\n DELETE FROM Orders WHERE id = 1"), { kind: "write", table: "orders" });
  assert.deepEqual(classifyStatement('update only "a""b" set x = 1'), { kind: "write", table: 'a"b' });
  assert.equal(classifyStatement("begin").kind, "begin");
  assert.equal(classifyStatement("START TRANSACTION ISOLATION LEVEL SERIALIZABLE").kind, "begin");
  assert.equal(classifyStatement("commit").kind, "commit");
  assert.equal(classifyStatement("rollback").kind, "rollback");
  for (const other of [
    "rollback to savepoint sp1",
    "ROLLBACK TRANSACTION TO SAVEPOINT sp1",
    "savepoint sp1",
    "release savepoint sp1",
    "commit prepared 'x'",
    'select * from "orders"',
    "with x as (update orders set a = 1 returning *) select * from x",
    undefined,
  ]) {
    assert.equal(classifyStatement(other).kind, "other", String(other));
  }
});

test("outside replay a checkpoint is a no-op", async () => {
  assert.equal(replayActive(), false);
  await checkpoint("after-db-commit", "orders");
});

/** A fake pool that records the statements it executes. */
function fakePool(log, commandFor = () => "OK") {
  const makeClient = () => ({
    query(config) {
      const text = typeof config === "string" ? config : config.text;
      log.push(`query:${text}`);
      return Promise.resolve({ command: commandFor(text), rows: [] });
    },
    release() {
      log.push("release");
    },
  });
  return {
    query(config) {
      const text = typeof config === "string" ? config : config.text;
      log.push(`pool.query:${text}`);
      return Promise.resolve({ command: "UPDATE", rows: [] });
    },
    connect(callback) {
      if (callback) {
        log.push("connect:callback");
        callback(undefined, makeClient());
        return undefined;
      }
      return Promise.resolve(makeClient());
    },
  };
}

/**
 * Run `body` in a child process under a fake supervisor that answers every
 * checkpoint with `continue` (or `throw` for the named one) and returns the
 * protocol messages and the child's output.
 */
async function underFakeSupervisor(script, throwAt) {
  const dir = mkdtempSync(join(tmpdir(), "crt-"));
  const socket = join(dir, "s.sock");
  const messages = [];
  const server = createServer((connection) => {
    let buffer = "";
    connection.setEncoding("utf8");
    connection.on("data", (chunk) => {
      buffer += chunk;
      let newline;
      while ((newline = buffer.indexOf("\n")) >= 0) {
        const message = JSON.parse(buffer.slice(0, newline));
        buffer = buffer.slice(newline + 1);
        messages.push(message);
        if (message.type === "checkpoint") {
          const reply = message.name === throwAt ? { type: "throw", message: `injected at ${message.name}` } : { type: "continue" };
          connection.write(JSON.stringify(reply) + "\n");
        }
      }
    });
  });
  await new Promise((resolve) => server.listen(socket, resolve));
  const file = join(dir, "harness.mjs");
  writeFileSync(file, script.replaceAll("@RUNTIME@", join(here, "..")));
  const child = spawn(process.execPath, [file], {
    env: {
      PATH: process.env.PATH,
      COMMITRIFT_REPLAY: "1",
      COMMITRIFT_REPLAY_SOCKET: socket,
      COMMITRIFT_REPLAY_SCENARIO: "main",
      COMMITRIFT_REPLAY_INPUT: '{"n":1}',
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (d) => (stdout += d));
  child.stderr.on("data", (d) => (stderr += d));
  const code = await new Promise((resolve) => child.on("exit", resolve));
  server.close();
  rmSync(dir, { recursive: true, force: true });
  return { messages, code, stdout, stderr };
}

test("a scenario reports hello, checkpoints in order and done", async () => {
  const { messages, code } = await underFakeSupervisor(`
    import { checkpoint, runScenarios } from "@RUNTIME@/index.js";
    await runScenarios({ main: async (input) => { await checkpoint("a", "x"); await checkpoint("b"); } });
  `);
  assert.equal(code, 0);
  assert.deepEqual(
    messages.map((m) => [m.type, m.name ?? m.outcome ?? m.protocol]),
    [
      ["hello", 1],
      ["checkpoint", "a"],
      ["checkpoint", "b"],
      ["done", "returned"],
    ],
  );
  assert.equal(messages[1].label, "x");
  assert.deepEqual(
    messages.filter((m) => m.type === "checkpoint").map((m) => m.seq),
    [1, 2],
  );
});

test("an injected exception is reported as an injected throw, distinct from other errors", async () => {
  const injected = await underFakeSupervisor(
    `
    import { checkpoint, runScenarios } from "@RUNTIME@/index.js";
    await runScenarios({ main: async () => { await checkpoint("a"); await checkpoint("b"); } });
  `,
    "a",
  );
  assert.equal(injected.code, 1);
  const done = injected.messages.at(-1);
  assert.equal(done.type, "done");
  assert.equal(done.outcome, "threw");
  assert.equal(done.injected, true);
  assert.match(done.error, /^InjectedFault: injected at a/);
  assert.equal(injected.messages.filter((m) => m.type === "checkpoint").length, 1, "nothing runs after the throw");

  const ordinary = await underFakeSupervisor(`
    import { runScenarios } from "@RUNTIME@/index.js";
    await runScenarios({ main: async () => { throw new TypeError("boom"); } });
  `);
  const done2 = ordinary.messages.at(-1);
  assert.equal(done2.outcome, "threw");
  assert.equal(done2.injected, false);
  assert.match(done2.error, /^TypeError: boom/);
});

test("unknown scenarios and prototype keys are refused", async () => {
  const { messages } = await underFakeSupervisor(`
    import { runScenarios } from "@RUNTIME@/index.js";
    process.env.COMMITRIFT_REPLAY_SCENARIO = "toString";
    await runScenarios({});
  `);
  assert.match(messages.at(-1).error, /unknown scenario/);
});

test("the pg adapter emits checkpoints around writes and commits, in order", async () => {
  const script = `
    import { runScenarios, checkpoint } from "@RUNTIME@/index.js";
    import { instrumentPgPool } from "@RUNTIME@/pg.js";
    const log = [];
    const client = () => ({
      query(config) { const t = typeof config === "string" ? config : config.text; log.push(t);
        return Promise.resolve({ command: t === "commit" ? (globalThis.failCommit ? "ROLLBACK" : "COMMIT") : "X", rows: [] }); },
      release() { log.push("release"); },
    });
    const pool = instrumentPgPool({
      query(config) { log.push("pool:" + (config.text ?? config)); return Promise.resolve({ command: "UPDATE", rows: [] }); },
      connect(cb) { if (cb) { cb(undefined, client()); return; } return Promise.resolve(client()); },
    });
    await runScenarios({ main: async () => {
      await pool.query({ text: 'update "orders" set "status" = $1' }, ["paid"]);
      const c = await pool.connect();
      await c.query("begin");
      await c.query({ text: 'insert into "outbox" ("id") values ($1)' }, ["x"]);
      await c.query("commit");
      c.release();
      globalThis.failCommit = true;
      const d = await pool.connect();
      await d.query("begin");
      await d.query('update "orders" set x = 1');
      await d.query("commit");
      d.release();
      console.log(JSON.stringify(log));
    } });
  `;
  const { messages, code, stdout, stderr } = await underFakeSupervisor(script);
  assert.equal(code, 0, stderr);
  const checkpoints = messages.filter((m) => m.type === "checkpoint").map((m) => `${m.name}(${m.label})`);
  assert.deepEqual(checkpoints, [
    "before-db-write(orders)",
    "after-db-commit(orders)",
    "before-db-write(outbox)",
    "after-tx-write(outbox)",
    "before-tx-commit(transaction)",
    "after-db-commit(transaction)",
    "before-db-write(orders)",
    "after-tx-write(orders)",
    "before-tx-commit(transaction)",
    // The server answered COMMIT with ROLLBACK: no commit is claimed.
    "after-tx-rollback(transaction)",
  ]);
  assert.match(stdout, /pool:update/);
});

test("an exception injected before COMMIT is raised before the statement is sent", async () => {
  const script = `
    import { runScenarios } from "@RUNTIME@/index.js";
    import { instrumentPgPool } from "@RUNTIME@/pg.js";
    const sent = [];
    const pool = instrumentPgPool({
      query() { throw new Error("unused"); },
      connect() { return Promise.resolve({ query(t) { sent.push(t); return Promise.resolve({ command: "X" }); }, release() {} }); },
    });
    await runScenarios({ main: async () => {
      const c = await pool.connect();
      await c.query("begin");
      try { await c.query("commit"); } catch (e) { await c.query("rollback"); console.log("sent:" + JSON.stringify(sent)); throw e; }
    } });
  `;
  const { messages, stdout } = await underFakeSupervisor(script, "before-tx-commit");
  assert.match(stdout, /sent:\["begin","rollback"\]/, "COMMIT was never sent");
  assert.equal(messages.at(-1).injected, true);
});

test("callback-style calls pass through without checkpoints", () => {
  const log = [];
  const pool = instrumentPgPool(fakePool(log));
  let called = false;
  pool.connect((error, client) => {
    called = !error && !!client;
  });
  assert.ok(called);
  assert.deepEqual(log, ["connect:callback"]);
  assert.equal(instrumentPgPool(pool), pool, "instrumenting twice is a no-op");
});

test("a fault wrapped by a library (error.cause) is still reported as injected", async () => {
  const { messages } = await underFakeSupervisor(
    `
    import { checkpoint, runScenarios } from "@RUNTIME@/index.js";
    class DrizzleQueryError extends Error { constructor(q, cause) { super("Failed query: " + q); this.name = "DrizzleQueryError"; this.cause = cause; } }
    await runScenarios({ main: async () => {
      try { await checkpoint("after-db-commit", "orders"); } catch (e) { throw new DrizzleQueryError("update", e); }
    } });
  `,
    "after-db-commit",
  );
  const done = messages.at(-1);
  assert.equal(done.injected, true);
  assert.match(done.error, /^DrizzleQueryError: Failed query: update \(caused by InjectedFault: /);
});
