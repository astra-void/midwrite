// @expect none
import { db } from "@/db";
import { orders } from "@/db/schema";

export function makeWrite() {
  return async () => { await db.insert(orders).values({}); };
}

export function forwardedWrite() {
  return makeWrite();
}

export function identity<T>(action: T): T {
  return action;
}
