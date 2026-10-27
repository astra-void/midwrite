import { pgTable, text, uniqueIndex } from "drizzle-orm/pg-core";

import { createId } from "../../lib/ids.js";
import { workspaces } from "./workspaces.js";

// Application-side key from a project helper; one pending invite per
// address and workspace, enforced by the database.
export const invites = pgTable(
  "invites",
  {
    id: text("id").$defaultFn(createId).primaryKey(),
    workspaceId: text("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    email: text("email").notNull(),
    status: text("status").notNull().default("pending"),
  },
  (t) => [uniqueIndex("invites_workspace_email").on(t.workspaceId, t.email)],
);
