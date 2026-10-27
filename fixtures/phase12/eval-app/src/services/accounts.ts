import { db } from "../db.js";
import { accountsRepo } from "../repositories/index.js";
import { events } from "../schema/index.js";

// E7: the database-generated id is returned and used for the wallet;
// UNIQUE(handle) stops a second registration.
export async function registerAccount(handle: string) {
  const account = await accountsRepo.create(handle);
  if (!account) throw new Error("insert failed");
  await accountsRepo.openWallet(account.id);
  await db.insert(events).values({ accountId: account.id, kind: "registered" });
  return account.id;
}

// E10: the client resubmits after a lost response (declared): the event
// gets a new generated id.
export async function recordSignup(accountId: string) {
  await db.insert(events).values({ accountId, kind: "signup" });
}

// E11: retried after an unknown outcome, with a generated id.
export async function recordLogin(accountId: string) {
  try {
    await db.insert(events).values({ accountId, kind: "login" });
  } catch {
    await db.insert(events).values({ accountId, kind: "login" });
  }
}

// E12: retried with the caller's event id: the retry collides.
export async function recordLoginOnce(eventId: string, accountId: string) {
  try {
    await db.insert(events).values({ id: eventId, accountId, kind: "login" }).onConflictDoNothing();
  } catch {
    await db.insert(events).values({ id: eventId, accountId, kind: "login" }).onConflictDoNothing();
  }
}

// E14 (unsupported): the account comes from a remote service first.
export async function importAccount(url: string) {
  const response = await fetch(url);
  const body = (await response.json()) as { handle: string };
  await accountsRepo.create(body.handle);
}
