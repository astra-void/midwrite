import { pgTable, text, uuid } from "drizzle-orm/pg-core";

import { users } from "./users.js";

// Application-side generated key: Drizzle calls the callback when it builds
// an INSERT that omits `id`.
export const workspaces = pgTable("workspaces", {
  id: text("id")
    .$defaultFn(() => crypto.randomUUID())
    .primaryKey(),
  name: text("name").notNull(),
  ownerId: uuid("owner_id")
    .notNull()
    .references(() => users.id),
});
