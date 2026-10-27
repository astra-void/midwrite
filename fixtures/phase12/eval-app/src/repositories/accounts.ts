import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { accounts, wallets } from "../schema/index.js";

export const accountsRepo = {
  async findByHandle(handle: string) {
    const [found] = await db.select().from(accounts).where(eq(accounts.handle, handle)).limit(1);
    return found;
  },
  async create(handle: string) {
    const [created] = await db.insert(accounts).values({ handle }).returning({ id: accounts.id });
    return created;
  },
  async openWallet(accountId: string) {
    await db.insert(wallets).values({ accountId, balance: 0 });
  },
};
