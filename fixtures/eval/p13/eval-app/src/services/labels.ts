import { and, eq } from "drizzle-orm";

import { replicatedDb } from "../db.js";
import { labels } from "../schema/index.js";

// E19: looks the label up through the replica set, creates it if missing.
export async function addLabel(teamId: number, name: string) {
  const [existing] = await replicatedDb
    .select({ id: labels.id })
    .from(labels)
    .where(and(eq(labels.teamId, teamId), eq(labels.name, name)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await replicatedDb.insert(labels).values({ teamId, name }).returning({ id: labels.id });
  return created?.id;
}
