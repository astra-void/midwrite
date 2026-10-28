"use server";

import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../../db.js";
import { categories } from "../../schema/index.js";

// Category names are stored trimmed and lowercased.
const categoryInput = z.object({
  accountId: z.string().uuid(),
  name: z.string().trim().toLowerCase().min(1).max(40),
});

// P14-E11: server action of the category form. UNIQUE(account_id, name).
export async function createCategory(input: unknown) {
  const parsed = categoryInput.safeParse(input);
  if (!parsed.success) return { error: "invalid category" };
  const { accountId, name } = parsed.data;
  const [existing] = await db
    .select({ id: categories.id })
    .from(categories)
    .where(and(eq(categories.accountId, accountId), eq(categories.name, name)))
    .limit(1);
  if (existing) return { id: existing.id, created: false };
  const [created] = await db
    .insert(categories)
    .values({ accountId, name })
    .onConflictDoNothing({ target: [categories.accountId, categories.name] })
    .returning({ id: categories.id });
  if (created) return { id: created.id, created: true };
  // Another request created it in the meantime.
  const [winner] = await db
    .select({ id: categories.id })
    .from(categories)
    .where(and(eq(categories.accountId, accountId), eq(categories.name, name)))
    .limit(1);
  return { id: winner?.id, created: false };
}
