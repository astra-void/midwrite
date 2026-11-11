"use server";

import { withMember } from "../lib/auth.js";
import { email, object, oneOf } from "../lib/validation.js";
import {
  createInvitation,
  createSeatInvite,
  findPendingInvitation,
  findSeatInvite,
} from "../repositories/invitation-repository.js";

const inviteSchema = object({
  email: email(),
  role: oneOf(["member", "admin"] as const),
});

// Invite someone to the member's organization unless an invitation for the
// address is already pending.
export const inviteMember = withMember(inviteSchema, async (data, member) => {
  const existing = await findPendingInvitation(member.orgId, data.email);
  if (existing) {
    return { error: "An invitation is already pending for this address" };
  }
  await createInvitation(member.orgId, data.email, data.role, member.id);
  return { success: "Invitation sent" };
});

const seatSchema = object({ email: email() });

// The same check for seat invitations, whose table has UNIQUE(org_id, email).
export const inviteSeat = withMember(seatSchema, async (data, member) => {
  const existing = await findSeatInvite(member.orgId, data.email);
  if (existing) {
    return { error: "This address already has a seat invitation" };
  }
  await createSeatInvite(member.orgId, data.email);
  return { success: "Seat invitation sent" };
});
