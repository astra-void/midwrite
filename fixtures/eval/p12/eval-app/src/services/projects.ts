import { db } from "../db.js";
import { ProjectStore, accountsRepo } from "../repositories/index.js";

const store = new ProjectStore(db);

// E1: UNIQUE(owner_id, slug) makes the second insert fail.
export async function createProject(ownerHandle: string, slug: string) {
  const owner = await accountsRepo.findByHandle(ownerHandle);
  if (!owner) throw new Error("unknown owner");
  const existing = await store.findBySlug(owner.id, slug);
  if (existing) return existing.id;
  const created = await store.create(owner.id, slug);
  if (!created) throw new Error("insert failed");
  return created.id;
}

// E15: membership checked, not enforced.
export async function joinProject(projectId: string, handle: string) {
  const account = await accountsRepo.findByHandle(handle);
  if (!account) throw new Error("unknown account");
  const member = await store.findMember(projectId, account.id);
  if (member) return;
  await store.addMember(projectId, account.id);
}
