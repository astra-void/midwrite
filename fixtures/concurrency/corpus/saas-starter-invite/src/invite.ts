// ADAPTED ISOLATED REPRODUCTION — not the original application code.
//
// Adapted from saas-starter (https://github.com/nextjs/saas-starter) at
// revision 6e33e58b1e553a41fe22e6b941a7229a002de361,
// app/(login)/actions.ts, `inviteTeamMember` (lines 399–459): the
// duplicate-invitation check and the insert are copied as written. Changes,
// each needed because the original cannot be extracted as is:
//   * the body was moved out of the `validatedActionWithUser(schema, async
//     (data, _, user) => …)` wrapper into a named function whose parameters
//     are the values the wrapper provides (`teamId`, `email`, `role`,
//     `userId`);
//   * the "already a member" check (a join of users and teamMembers) and the
//     activity log are omitted;
//   * the insert passes an explicit `id`: the original relies on a `serial`
//     default, which the model does not generate.
import { and, eq } from "drizzle-orm";
import { db } from "./db";
import { invitations } from "./schema";

export async function inviteTeamMember(
  id: number,
  teamId: number,
  email: string,
  role: string,
  userId: number,
) {
  // Check if there's an existing invitation
  const existingInvitation = await db
    .select()
    .from(invitations)
    .where(
      and(
        eq(invitations.email, email),
        eq(invitations.teamId, teamId),
        eq(invitations.status, "pending"),
      ),
    )
    .limit(1);

  if (existingInvitation.length > 0) {
    return { error: "An invitation has already been sent to this email" };
  }

  // Create a new invitation
  await db.insert(invitations).values({
    id,
    teamId,
    email,
    role,
    invitedBy: userId,
    status: "pending",
  });

  return { success: "Invitation sent successfully" };
}
