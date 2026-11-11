import { sql } from "drizzle-orm";

import { db } from "./db/client.js";
import { counters, notes, people, profiles, tickets } from "./db/schema/index.js";

// Two inserts that omit the application-generated key.
export async function twoNotes() {
  await db.insert(notes).values({ body: "a" });
  await db.insert(notes).values({ body: "b" });
}

// A database-generated key returned and stored in a second table.
export async function personWithProfile(email: string) {
  const [person] = await db.insert(people).values({ email }).returning({ id: people.id });
  if (!person) throw new Error("no person");
  await db.insert(profiles).values({ personId: person.id, bio: "hello" });
}

// An explicit value instead of the default.
export async function explicitNote(id: string) {
  await db.insert(notes).values({ id, body: "explicit" });
}

// A failed insert (unique email), caught, then another insert.
export async function duplicatePerson(email: string) {
  await db.insert(people).values({ email });
  try {
    await db.insert(people).values({ email });
  } catch {
    // the second insert fails with 23505
  }
  await db.insert(people).values({ email: "other@example.test" });
}

// A rolled-back transaction, then the same inserts again.
export async function rollbackThenInsert() {
  try {
    await db.transaction(async (tx) => {
      await tx.insert(counters).values({ label: "first" });
      await tx.insert(notes).values({ body: "rolled back" });
      throw new Error("abort");
    });
  } catch {
    // rolled back
  }
  await db.insert(counters).values({ label: "second" });
  await db.insert(notes).values({ body: "kept" });
}

// Identity and constant defaults.
export async function identityAndConstant() {
  await db.insert(tickets).values({ title: "t1" });
  await db.insert(tickets).values({ title: "t2" });
  await db.insert(counters).values({ label: "c" });
}

// --- shapes the model does not represent: runtime observations only -------

// A multi-row insert: one callback evaluation per row?
export async function multiRow() {
  await db.insert(notes).values([{ body: "m1" }, { body: "m2" }]);
}

// The same builder executed twice: one value or two?
export async function sameBuilderTwice() {
  const insert = db.insert(notes).values({ body: "twice" });
  await insert;
  try {
    await insert;
  } catch {
    // a second execution with the same key would fail with 23505
  }
}

// SQL DEFAULT for a database-generated key.
export async function sqlDefault(email: string) {
  await db.insert(people).values({ id: sql`default`, email });
}

// A failing insert (NOT NULL): is the callback evaluated anyway?
export async function failingInsert() {
  try {
    await db.insert(notes).values({ body: sql`null` });
  } catch {
    // 23502
  }
}
