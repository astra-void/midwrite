import { pgTable, serial, text, unique, uuid } from "drizzle-orm/pg-core";

// Invitations of a team. No unique key: two concurrent invitations of one
// address both insert.
export const invites = pgTable("invites", {
  id: serial("id").primaryKey(),
  teamId: uuid("team_id").notNull(),
  email: text("email").notNull(),
  role: text("role").notNull(),
  note: text("note"),
});

// The same with UNIQUE(team_id, email): the duplicate is rejected.
export const members = pgTable(
  "members",
  {
    id: serial("id").primaryKey(),
    teamId: uuid("team_id").notNull(),
    email: text("email").notNull(),
    role: text("role").notNull(),
  },
  (t) => [unique("members_team_email").on(t.teamId, t.email)],
);
