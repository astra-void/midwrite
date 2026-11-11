import { pgTable, serial, text, uuid } from "drizzle-orm/pg-core";

import { users as accounts } from "./users.js";
import { workspaces } from "./workspaces.js";

// No UNIQUE(workspace_id, user_id): one person can be added twice.
export const memberships = pgTable("memberships", {
  id: serial("id").primaryKey(),
  workspaceId: text("workspace_id")
    .notNull()
    .references(() => workspaces.id),
  userId: uuid("user_id")
    .notNull()
    .references(() => accounts.id),
  role: text("role").notNull(),
});
