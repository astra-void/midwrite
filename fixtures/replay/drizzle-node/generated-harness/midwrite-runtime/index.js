// Midwrite replay client (protocol version 1). Plain ECMAScript module,
// no dependencies: runs unchanged on Node.js >= 22 and Bun >= 1.2.
//
// A harness calls `checkpoint(name, label)` right AFTER the operation the
// name describes has completed (for `after-db-commit`: after the database
// acknowledged the commit), and right BEFORE an operation for `before-*`
// names. Under `midwrite replay` the call blocks until the supervisor
// answers: continue, throw an InjectedFault, or (process-kill mode) never,
// because the supervisor kills the process while it waits. Outside replay
// every checkpoint is a no-op.
//
// See docs/phase3/REPLAY-PROTOCOL.md and docs/phase4/NODE-DRIZZLE.md.

import { connect } from "node:net";

export const PROTOCOL_VERSION = 1;

/** The error a checkpoint throws in exception mode. */
export class InjectedFault extends Error {
  constructor(checkpoint, message) {
    super(message);
    this.name = "InjectedFault";
    this.checkpoint = checkpoint;
  }
}

class Channel {
  constructor(socket) {
    this.socket = socket;
    this.buffer = "";
    this.lines = [];
    this.waiters = [];
    this.closed = false;
    this.seq = 0;
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
      for (const waiter of this.waiters.splice(0)) {
        waiter.reject(new Error("the replay supervisor closed the connection"));
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
    if (this.closed) return Promise.reject(new Error("the replay supervisor closed the connection"));
    return new Promise((resolve, reject) => this.waiters.push({ resolve, reject }));
  }

  close() {
    return new Promise((resolve) => this.socket.end(() => resolve()));
  }
}

let channel = null;

/** Whether the process runs under `midwrite replay`. */
export function replayActive() {
  return process.env.MIDWRITE_REPLAY === "1" && !!process.env.MIDWRITE_REPLAY_SOCKET;
}

/**
 * Report that the harness reached checkpoint `name` and wait for the
 * supervisor's decision. A no-op outside replay (and before
 * `runScenarios` connected).
 */
export async function checkpoint(name, label) {
  if (!channel) return;
  channel.seq += 1;
  await channel.send({ type: "checkpoint", seq: channel.seq, name, label });
  const reply = JSON.parse(await channel.next());
  if (reply.type === "continue") return;
  if (reply.type === "throw") throw new InjectedFault(name, String(reply.message));
  throw new Error(`unexpected replay protocol message: ${reply.type}`);
}

function insideRoot(path, root) {
  return typeof path === "string" && typeof root === "string" && root.length > 1 && path.startsWith(root + "/");
}

/**
 * The disposable database the supervisor created for this run. Refuses to
 * return anything that does not live inside the replay's private directory,
 * so a harness cannot be pointed at another database by accident.
 */
export function replayDatabase() {
  const root = process.env.MIDWRITE_REPLAY_ROOT;
  const path = process.env.MIDWRITE_REPLAY_PG_SOCKET;
  const database = process.env.MIDWRITE_REPLAY_PG_DATABASE;
  const username = process.env.MIDWRITE_REPLAY_PG_USER;
  if (!root || !path || !database || !username) {
    throw new Error("refusing to connect: not running under midwrite replay (no replay database)");
  }
  if (!insideRoot(path, root)) {
    throw new Error("refusing to connect: the database socket is outside the replay directory");
  }
  return { path, database, username };
}

/** The event log of the run (a fake queue's durable storage). */
export function eventLogPath() {
  const path = process.env.MIDWRITE_REPLAY_EVENT_LOG;
  const root = process.env.MIDWRITE_REPLAY_ROOT;
  if (!insideRoot(path, root)) {
    throw new Error("refusing to publish: no replay event log");
  }
  return path;
}

function describe(error) {
  return error instanceof Error ? `${error.name}: ${error.message}` : String(error);
}

/**
 * The InjectedFault behind `error`, following `cause` chains: libraries
 * wrap errors raised inside their driver calls (Drizzle ORM reports a
 * fault injected at a query checkpoint as `DrizzleQueryError` with the
 * fault as `cause`).
 */
export function injectedFaultIn(error) {
  let current = error;
  for (let depth = 0; depth < 16 && current; depth += 1) {
    if (current instanceof InjectedFault) return current;
    current = current instanceof Error ? current.cause : undefined;
  }
  return undefined;
}

/**
 * Entry point of a harness: run the scenario the supervisor selected and
 * report how it ended. `cleanup` runs after the scenario (closing pools);
 * it never runs after a process kill.
 */
export async function runScenarios(scenarios, cleanup) {
  if (!replayActive()) {
    console.error("this harness only runs under `midwrite replay`");
    process.exit(2);
  }
  channel = await Channel.open(process.env.MIDWRITE_REPLAY_SOCKET);
  const name = process.env.MIDWRITE_REPLAY_SCENARIO ?? "";
  await channel.send({ type: "hello", protocol: PROTOCOL_VERSION, scenario: name, pid: process.pid });
  const input = JSON.parse(process.env.MIDWRITE_REPLAY_INPUT ?? "null");
  let outcome = "returned";
  let error;
  let injected = false;
  const scenario = Object.prototype.hasOwnProperty.call(scenarios, name) ? scenarios[name] : undefined;
  try {
    if (typeof scenario !== "function") throw new Error(`unknown scenario \`${name}\``);
    await scenario(input);
  } catch (caught) {
    outcome = "threw";
    const fault = injectedFaultIn(caught);
    injected = fault !== undefined;
    error = describe(caught);
    if (fault && fault !== caught) error += ` (caused by ${describe(fault)})`;
  }
  try {
    await cleanup?.();
  } catch {
    // Cleanup failures do not change the scenario's outcome.
  }
  await channel.send({ type: "done", outcome, error, injected });
  await channel.close();
  process.exit(outcome === "returned" ? 0 : 1);
}
