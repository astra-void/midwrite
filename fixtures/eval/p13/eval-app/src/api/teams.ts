import { z } from "zod";

import { withAuth } from "../lib/auth.js";
import { teamRepo } from "../repositories/index.js";

export const createTeamInput = z.object({
  name: z.string().trim().min(1).max(80),
});

// E20: POST /api/teams for the signed-in user.
export const createTeamRoute = withAuth(async (input: unknown, user) => {
  const { name } = createTeamInput.parse(input);
  const existing = await teamRepo.findByOwner(user.id);
  if (existing) return { id: existing.id, created: false };
  const created = await teamRepo.create(user.id, name);
  return { id: created.id, created: true };
});
