// @expect MW1001 line=13 confidence=high function=unreconciled
import { notify } from "./publish";

export async function reconciled() {
  await db.insert(orders).values({});
  // midwrite-ignore MW1001: nightly reconciliation retries notify
  await notify();
}

export async function unreconciled() {
  await db.insert(orders).values({});

  await notify();
}
