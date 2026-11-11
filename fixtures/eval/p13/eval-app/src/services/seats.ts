import { eq } from "drizzle-orm";

import { db } from "../db.js";
import { SeatStore } from "../repositories/index.js";
import { seats } from "../schema/index.js";

const store = new SeatStore(db);

// E09: reads the seat, assigns it if it has no holder, reports success.
export async function holdSeat(seatId: number, holder: string) {
  const seat = await store.find(seatId);
  if (!seat || seat.holder !== null) return false;
  await store.assign(seatId, holder);
  return true;
}

// E10: one conditional UPDATE ... RETURNING decides.
export async function holdSeatAtomic(seatId: number, holder: string) {
  const rows = await store.assignIfFree(seatId, holder);
  return rows.length > 0;
}

// E11: an UPDATE ... RETURNING without a condition on the holder.
export async function holdSeatDirect(seatId: number, holder: string) {
  const rows = await db.update(seats).set({ holder }).where(eq(seats.id, seatId)).returning({ id: seats.id });
  return rows.length > 0;
}
