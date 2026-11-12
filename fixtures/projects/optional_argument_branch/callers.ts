// Optional option objects decide the callee's branch when the call leaves
// them out or passes literal properties. Each caller writes `events`
// first; the delete pairs with it only where the branch can run.
import { db } from "@/db";
import { events } from "@/db/schema";
import { close, closeDirect, closeReassigned, closeVia } from "./inbox";

// Decided: no options, so `purge` is `false`.
export async function closeQuietly(id: string) {
  await db.insert(events).values({ id });
  await close(db, id);
}

export async function closeQuietlyVia(id: string) {
  await db.insert(events).values({ id });
  await closeVia(id);
}

export async function closeDirectQuietly(id: string) {
  await db.insert(events).values({ id });
  await closeDirect(db, id, undefined);
}

export async function closeDirectKeeping(id: string) {
  await db.insert(events).values({ id });
  await closeDirect(db, id, { purge: false });
}

// Not decided: an object handed to other code first, a reassigned
// parameter, or a value chosen by the caller.
export async function closeKeeping(id: string) {
  await db.insert(events).values({ id });
  await close(db, id, { purge: false });
}

export async function closeReassignedQuietly(id: string) {
  await db.insert(events).values({ id });
  await closeReassigned(db, id);
}

export async function closeChosen(id: string, purge: boolean) {
  await db.insert(events).values({ id });
  await closeDirect(db, id, { purge });
}

// Decided the other way: the delete always runs first.
export async function closeDirectPurging(id: string) {
  await db.insert(events).values({ id });
  await closeDirect(db, id, { purge: true });
}

// Decided: only the pair with the update is left.
// @expect MW1004 line=11 confidence=medium function=closeQuietly
// @expect MW1004 line=16 confidence=medium function=closeQuietlyVia
// @expect MW1004 line=21 confidence=medium function=closeDirectQuietly
// @expect MW1004 line=26 confidence=medium function=closeDirectKeeping
// Not decided: the pairs with the delete and with the update.
// @expect MW1004 line=33 confidence=medium function=closeKeeping
// @expect MW1004 line=33 confidence=medium function=closeKeeping
// @expect MW1004 line=38 confidence=medium function=closeReassignedQuietly
// @expect MW1004 line=38 confidence=medium function=closeReassignedQuietly
// @expect MW1004 line=43 confidence=medium function=closeChosen
// @expect MW1004 line=43 confidence=medium function=closeChosen
// The delete always runs first: only the pair with the delete.
// @expect MW1004 line=49 confidence=medium function=closeDirectPurging
