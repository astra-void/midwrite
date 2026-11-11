import { integer, pgTable, serial, text, unique, uuid } from "drizzle-orm/pg-core";

import { users } from "./users.js";

export const teams = pgTable("teams", {
  id: serial("id").primaryKey(),
  ownerId: uuid("owner_id")
    .notNull()
    .references(() => users.id),
  name: text("name").notNull(),
  memberCount: integer("member_count").notNull().default(0),
});

export const teamMembers = pgTable("team_members", {
  id: serial("id").primaryKey(),
  teamId: integer("team_id")
    .notNull()
    .references(() => teams.id),
  userId: uuid("user_id")
    .notNull()
    .references(() => users.id),
});

export const invites = pgTable(
  "invites",
  {
    id: serial("id").primaryKey(),
    teamId: integer("team_id")
      .notNull()
      .references(() => teams.id),
    email: text("email").notNull(),
    invitedBy: uuid("invited_by")
      .notNull()
      .references(() => users.id),
  },
  (t) => [unique("invites_team_email").on(t.teamId, t.email)],
);

export const labels = pgTable("labels", {
  id: serial("id").primaryKey(),
  teamId: integer("team_id")
    .notNull()
    .references(() => teams.id),
  name: text("name").notNull(),
});
