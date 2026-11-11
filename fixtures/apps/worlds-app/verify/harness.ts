// AUTHORIZED SOURCE-VERIFICATION HARNESS for the Phase 15 worlds
// application. It calls the application's ORIGINAL exported functions as
// its server would, one process per actor.
import * as drizzle from "drizzle-orm";
import * as pgCore from "drizzle-orm/pg-core";
import pg from "pg";
import {
  coordinatePool,
  instrumentDrizzle,
  runSourceActor,
  sourcePoolConfig,
} from "@midwrite/replay/source";

import { usePool } from "../src/db/pool.js";

instrumentDrizzle(drizzle, pgCore);
const pool = coordinatePool(new pg.Pool(sourcePoolConfig({ max: 10 })));
pool.on("error", () => {});
usePool(pool);

const folders = await import("../src/services/folders.js");
const exports = await import("../src/services/exports.js");

type Place = { ownerId: number; parentId: number | null; name: string };

await runSourceActor(
  {
    createFolder: (input: Place) => folders.createFolder(input.ownerId, input.parentId, input.name),
    createLabel: (input: Place) => folders.createLabel(input.ownerId, input.parentId, input.name),
    startExport: (input: { accountId: number }) => exports.startExport(input.accountId),
    startExportAtomic: (input: { accountId: number }) => exports.startExportAtomic(input.accountId),
  },
  () => pool.end(),
);
