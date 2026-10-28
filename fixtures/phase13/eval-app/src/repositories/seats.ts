import { and, eq, isNull } from "drizzle-orm";

import type { Database } from "../db.js";
import { seats } from "../schema/index.js";

// A class repository; the handle (the pool or a transaction) is injected.
export class SeatStore {
  constructor(private readonly database: Database) {}

  async find(seatId: number) {
    const [found] = await this.database.select().from(seats).where(eq(seats.id, seatId)).limit(1);
    return found;
  }

  async assign(seatId: number, holder: string) {
    await this.database.update(seats).set({ holder }).where(eq(seats.id, seatId));
  }

  async assignIfFree(seatId: number, holder: string) {
    return this.database
      .update(seats)
      .set({ holder })
      .where(and(eq(seats.id, seatId), isNull(seats.holder)))
      .returning({ id: seats.id });
  }
}
