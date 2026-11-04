import { inArray } from "drizzle-orm";

import { db } from "../db.js";
import { boards, projects } from "../schema/index.js";

type BulkHandler = (ids: number[]) => Promise<void>;
type Repository = Record<string, BulkHandler>;

// Bulk actions by entity and verb. Plugins add entries at start-up through
// registerBulkHandler, so the set of handlers is only known at run time.
const repositories: Record<string, Repository> = {
  project: {
    async archiveMany(ids) {
      await db.update(projects).set({ archived: true }).where(inArray(projects.id, ids));
    },
    async deleteMany(ids) {
      await db.delete(projects).where(inArray(projects.id, ids));
    },
  },
  board: {
    async archiveMany(ids) {
      await db.update(boards).set({ archived: true }).where(inArray(boards.id, ids));
    },
  },
};

export function registerBulkHandler(entity: string, verb: string, handler: BulkHandler) {
  const repository = (repositories[entity] ??= {});
  repository[`${verb}Many`] = handler;
}

// P16-E38: runs `<verb>Many` of the entity's repository on the given ids;
// both names come from the request.
export async function runBulkAction(entity: string, verb: string, ids: number[]) {
  const repository = repositories[entity];
  const handler = repository?.[`${verb}Many`];
  if (typeof handler !== "function") throw new Error(`unsupported bulk action ${entity}.${verb}`);
  await handler.call(repository, ids);
}
