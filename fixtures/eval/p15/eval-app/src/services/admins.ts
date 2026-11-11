import { and, count, eq } from "drizzle-orm";

import { db } from "../db.js";
import { memberships, orgs } from "../schema/index.js";

// P15-E20: an organization admin demotes an admin (possibly themself) to
// member, unless that would leave the organization without an admin.
export async function demoteAdmin(orgId: number, callerId: string, targetId: string) {
  const [caller] = await db
    .select({ role: memberships.role })
    .from(memberships)
    .where(and(eq(memberships.orgId, orgId), eq(memberships.accountId, callerId)))
    .limit(1);
  if (!caller || caller.role !== "admin") throw new Error("forbidden");
  const [admins] = await db
    .select({ n: count() })
    .from(memberships)
    .where(and(eq(memberships.orgId, orgId), eq(memberships.role, "admin")));
  if (!admins || admins.n <= 1) throw new Error("an organization needs an admin");
  await db
    .update(memberships)
    .set({ role: "member" })
    .where(and(eq(memberships.orgId, orgId), eq(memberships.accountId, targetId), eq(memberships.role, "admin")));
}

// P15-E21: the same, holding the organization row FOR UPDATE for the checks
// and the update.
export async function demoteAdminLocked(orgId: number, callerId: string, targetId: string) {
  await db.transaction(async (tx) => {
    const [org] = await tx.select({ id: orgs.id }).from(orgs).where(eq(orgs.id, orgId)).limit(1).for("update");
    if (!org) throw new Error("no organization");
    const [caller] = await tx
      .select({ role: memberships.role })
      .from(memberships)
      .where(and(eq(memberships.orgId, orgId), eq(memberships.accountId, callerId)))
      .limit(1);
    if (!caller || caller.role !== "admin") throw new Error("forbidden");
    const [admins] = await tx
      .select({ n: count() })
      .from(memberships)
      .where(and(eq(memberships.orgId, orgId), eq(memberships.role, "admin")));
    if (!admins || admins.n <= 1) throw new Error("an organization needs an admin");
    await tx
      .update(memberships)
      .set({ role: "member" })
      .where(and(eq(memberships.orgId, orgId), eq(memberships.accountId, targetId), eq(memberships.role, "admin")));
  });
}
