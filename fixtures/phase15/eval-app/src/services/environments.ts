import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { requireAdmin } from "../lib/guards.js";
import { channels, environments } from "../schema/index.js";

// P15-E18: an organization admin creates a deployment environment, unless one
// with that name exists.
export async function createEnvironment(orgId: number, callerId: string, name: string) {
  await requireAdmin(orgId, callerId);
  const [existing] = await db
    .select({ id: environments.id })
    .from(environments)
    .where(and(eq(environments.orgId, orgId), eq(environments.name, name)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(environments)
    .values({ orgId, name, createdBy: callerId })
    .returning({ id: environments.id });
  return created?.id ?? null;
}

// P15-E19: an organization admin creates a release channel, unless one with
// that name exists (UNIQUE(org_id, name)).
export async function createReleaseChannel(orgId: number, callerId: string, name: string) {
  await requireAdmin(orgId, callerId);
  const [existing] = await db
    .select({ id: channels.id })
    .from(channels)
    .where(and(eq(channels.orgId, orgId), eq(channels.name, name)))
    .limit(1);
  if (existing) return existing.id;
  const [created] = await db
    .insert(channels)
    .values({ orgId, name, createdBy: callerId })
    .onConflictDoNothing()
    .returning({ id: channels.id });
  return created?.id ?? null;
}
