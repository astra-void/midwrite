import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { users } from "../db/schema/index.js";

// A repository object: its methods are called as `usersRepository.m(...)`.
export const usersRepository = {
  async findByEmail(email: string) {
    const [user] = await db.select().from(users).where(eq(users.email, email)).limit(1);
    return user;
  },

  async create(email: string, name: string) {
    const [created] = await db.insert(users).values({ email, name }).returning({ id: users.id });
    return created;
  },
};
