import { and, eq } from "drizzle-orm";

import { tasks, wallets } from "../schema/index.js";

// Methods take the executor (the pool or a transaction) as an argument.
export const taskRepo = {
  async findByTitle(executor: any, projectId: string, title: string) {
    const [found] = await executor
      .select()
      .from(tasks)
      .where(and(eq(tasks.projectId, projectId), eq(tasks.title, title)))
      .limit(1);
    return found;
  },
  async wallet(executor: any, accountId: string) {
    const [found] = await executor.select().from(wallets).where(eq(wallets.accountId, accountId)).limit(1);
    return found;
  },
  async lockedWallet(executor: any, accountId: string) {
    const [found] = await executor
      .select()
      .from(wallets)
      .where(eq(wallets.accountId, accountId))
      .limit(1)
      .for("update");
    return found;
  },
};
