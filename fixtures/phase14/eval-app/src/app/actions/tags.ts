"use server";

import { and, eq } from "drizzle-orm";
import { z } from "zod";

import { db } from "../../db.js";
import { tags } from "../../schema/index.js";

// Tag names are stored trimmed and lowercased.
const tagInput = z.object({
  accountId: z.string().uuid(),
  name: z.string().trim().toLowerCase().min(1).max(40),
});

// P14-E10: server action of the tag form. tags has no unique constraint.
export async function createTag(input: unknown) {
  const { accountId, name } = tagInput.parse(input);
  const [existing] = await db
    .select({ id: tags.id })
    .from(tags)
    .where(and(eq(tags.accountId, accountId), eq(tags.name, name)))
    .limit(1);
  if (existing) return { id: existing.id, created: false };
  const [created] = await db.insert(tags).values({ accountId, name }).returning({ id: tags.id });
  if (!created) throw new Error("tag insert failed");
  return { id: created.id, created: true };
}
