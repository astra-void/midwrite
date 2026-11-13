import { pgTable, serial, text, uuid } from "drizzle-orm/pg-core";

// Invitations of a team. No unique key: two concurrent invitations of one
// address both insert.
export const invites = pgTable("invites", {
  id: serial("id").primaryKey(),
  teamId: uuid("team_id").notNull(),
  email: text("email").notNull(),
  role: text("role").notNull(),
});
