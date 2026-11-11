import { eq } from "drizzle-orm";

import { getDb } from "../db/client.js";
import { users, workspaces } from "../db/schema/index.js";
import {
  MembershipRepository,
  membershipRepository,
  usersRepository,
  workspacesRepository,
} from "../repositories/index.js";

// One workspace per owner — checked, not enforced: two concurrent requests
// can both see none and both create one.
export async function createWorkspace(ownerEmail: string, name: string) {
  const owner = await usersRepository.findByEmail(ownerEmail);
  if (!owner) throw new Error("unknown owner");
  const existing = await workspacesRepository.findByOwner(owner.id);
  if (existing) return existing.id;
  const created = await workspacesRepository.create(name, owner.id);
  if (!created) throw new Error("workspace insert failed");
  await membershipRepository.add(created.id, owner.id, "owner");
  return created.id;
}

// The same check under a lock on the owner's row.
export async function createWorkspaceLocked(ownerEmail: string, name: string) {
  return getDb().transaction(async (tx) => {
    const [owner] = await tx
      .select()
      .from(users)
      .where(eq(users.email, ownerEmail))
      .limit(1)
      .for("update");
    if (!owner) throw new Error("unknown owner");
    const [existing] = await tx
      .select()
      .from(workspaces)
      .where(eq(workspaces.ownerId, owner.id))
      .limit(1);
    if (existing) return existing.id;
    const [created] = await tx
      .insert(workspaces)
      .values({ name, ownerId: owner.id })
      .returning({ id: workspaces.id });
    if (!created) throw new Error("workspace insert failed");
    const members = new MembershipRepository(tx);
    await members.add(created.id, owner.id, "owner");
    return created.id;
  });
}

// Creates a workspace for an existing owner, without a check. Clients
// resubmit when they get no response.
export async function addWorkspace(ownerId: string, name: string) {
  const created = await workspacesRepository.create(name, ownerId);
  if (!created) throw new Error("workspace insert failed");
  await membershipRepository.add(created.id, ownerId, "owner");
  return created.id;
}

// The same with the request id as the workspace's key: a resubmission
// collides with the first insert instead of creating a second workspace.
export async function addWorkspaceIdempotent(requestId: string, ownerId: string, name: string) {
  const created = await workspacesRepository.createWithId(requestId, name, ownerId);
  if (!created) throw new Error("workspace insert failed");
  await membershipRepository.add(created.id, ownerId, "owner");
  return created.id;
}

// A new user with a first workspace: the database generates the user's id,
// RETURNING hands it to the application, which stores it in the workspace
// and the membership.
export async function registerUser(email: string, name: string, workspaceName: string) {
  const user = await usersRepository.create(email, name);
  if (!user) throw new Error("user insert failed");
  const workspace = await workspacesRepository.create(workspaceName, user.id);
  if (!workspace) throw new Error("workspace insert failed");
  await membershipRepository.add(workspace.id, user.id, "owner");
  return workspace.id;
}
