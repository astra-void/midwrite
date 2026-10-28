import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { aliases, routes, type RouteKind } from "../schema/index.js";

// P15-E22: claims a URL slug of the organization's site for a page or a
// redirect, unless the slug is taken.
export async function claimRoute(orgId: number, slug: string, kind: RouteKind) {
  const [taken] = await db
    .select({ id: routes.id })
    .from(routes)
    .where(and(eq(routes.orgId, orgId), eq(routes.slug, slug)))
    .limit(1);
  if (taken) return false;
  const created = await db
    .insert(routes)
    .values({ orgId, slug, kind })
    .onConflictDoNothing()
    .returning({ id: routes.id });
  return created.length > 0;
}

// P15-E23: the same for short aliases (UNIQUE(org_id, slug)).
export async function claimAlias(orgId: number, slug: string, kind: RouteKind) {
  const [taken] = await db
    .select({ id: aliases.id })
    .from(aliases)
    .where(and(eq(aliases.orgId, orgId), eq(aliases.slug, slug)))
    .limit(1);
  if (taken) return false;
  const created = await db
    .insert(aliases)
    .values({ orgId, slug, kind })
    .onConflictDoNothing()
    .returning({ id: aliases.id });
  return created.length > 0;
}
