"use server";

import { withUser } from "../lib/auth.js";
import { createWorkspace } from "../services/workspaces.js";

export type CreateWorkspaceInput = { name: string };

// The Server Action behind "Create workspace".
export const createWorkspaceAction = withUser(async (input: CreateWorkspaceInput, user) => {
  const id = await createWorkspace(user.email, input.name);
  return { id };
});
