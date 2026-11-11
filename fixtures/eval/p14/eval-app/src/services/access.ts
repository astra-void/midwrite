import { and, eq } from "drizzle-orm";

import { db } from "../db.js";
import { apiScopes, roleGrants } from "../schema/index.js";

// Roles every new member of an account starts with.
export const DEFAULT_ROLES = ["viewer", "commenter"] as const;

// Scopes every new API integration of an account starts with.
export const DEFAULT_SCOPES = ["read:projects", "read:tasks"] as const;

// P14-E01: grants each default role the member does not have yet.
export async function grantDefaultRoles(accountId: string, memberEmail: string) {
  for (const role of DEFAULT_ROLES) {
    const [existing] = await db
      .select({ id: roleGrants.id })
      .from(roleGrants)
      .where(
        and(eq(roleGrants.accountId, accountId), eq(roleGrants.memberEmail, memberEmail), eq(roleGrants.role, role)),
      )
      .limit(1);
    if (!existing) {
      await db.insert(roleGrants).values({ accountId, memberEmail, role });
    }
  }
}

// P14-E02: grants the default scopes in one statement; (account_id, scope)
// is the primary key of api_scopes.
export async function grantDefaultScopes(accountId: string) {
  const rows = DEFAULT_SCOPES.map((scope) => ({ accountId, scope }));
  const granted = await db.insert(apiScopes).values(rows).onConflictDoNothing().returning({ scope: apiScopes.scope });
  return granted.length;
}
