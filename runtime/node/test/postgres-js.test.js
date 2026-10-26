// The postgres-js adapter's guards (Phase 8): `node --test
// runtime/node/test/postgres-js.test.js`. Uses the postgres-js installed in
// fixtures/phase8/pgjs-app; nothing connects to a database.

import { test } from "node:test";
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { POSTGRES_JS_VERSIONS, coordinatePostgresJs } from "../source.js";

const here = dirname(fileURLToPath(import.meta.url));
const installed = join(here, "../../../fixtures/phase8/pgjs-app/node_modules/postgres/src/index.js");
const { default: postgres } = await import(pathToFileURL(installed).href);

test("an unverified postgres-js version is refused, not assumed compatible", () => {
  assert.deepEqual(POSTGRES_JS_VERSIONS, ["3.4.5"]);
  assert.throws(() => coordinatePostgresJs(postgres, "3.4.9"), /not supported/);
  assert.throws(() => coordinatePostgresJs(postgres, "4.0.0"), /verified: 3.4.5/);
});

test("the verified version is coordinated once, on the shared Query prototype", () => {
  const before = Object.getPrototypeOf(postgres({ host: "/nonexistent", max: 1 }).unsafe("select 1")).handle;
  coordinatePostgresJs(postgres, "3.4.5");
  const proto = Object.getPrototypeOf(postgres({ host: "/nonexistent", max: 1 }).unsafe("select 1"));
  assert.notEqual(proto.handle, before);
  const patched = proto.handle;
  coordinatePostgresJs(postgres, "3.4.5");
  assert.equal(proto.handle, patched, "not wrapped twice");
});

test("a tagged template is not executed (or announced) by being called", () => {
  const sql = postgres({ host: "/nonexistent", max: 1 });
  const query = sql`select 1`;
  assert.equal(query.executed, false);
});
