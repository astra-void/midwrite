import { z } from "zod";

import { db } from "../db.js";
import { withAuth } from "../lib/auth.js";
import { invites } from "../schema/index.js";

export const inviteInput = z.object({
  teamId: z.number().int().positive(),
  email: z.string().trim().toLowerCase().email(),
});

// E21: POST /api/invites for the signed-in user. UNIQUE(team_id, email).
export const inviteRoute = withAuth(async (input: unknown, user) => {
  const { teamId, email } = inviteInput.parse(input);
  const rows = await db
    .insert(invites)
    .values({ teamId, email, invitedBy: user.id })
    .onConflictDoNothing({ target: [invites.teamId, invites.email] })
    .returning({ id: invites.id });
  return { invited: rows.length > 0 };
});
