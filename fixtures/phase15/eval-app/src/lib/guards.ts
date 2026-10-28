import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { memberships } from "../schema/index.js";

// Throws unless the caller holds an "admin" membership of the organization.
export async function requireAdmin(orgId: number, callerId: string) {
  const [membership] = await db
    .select({ role: memberships.role })
    .from(memberships)
    .where(and(eq(memberships.orgId, orgId), eq(memberships.accountId, callerId)))
    .limit(1);
  if (!membership || membership.role !== "admin") throw new Error("forbidden");
}
