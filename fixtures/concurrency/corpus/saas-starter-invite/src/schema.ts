// Columns as in saas-starter lib/db/schema.ts (`invitations`).
import { integer, pgTable, serial, varchar } from "drizzle-orm/pg-core";
export const invitations = pgTable("invitations", {
  id: serial("id").primaryKey(),
  teamId: integer("team_id").notNull(),
  email: varchar("email", { length: 255 }).notNull(),
  role: varchar("role", { length: 50 }).notNull(),
  invitedBy: integer("invited_by").notNull(),
  status: varchar("status", { length: 20 }).notNull().default("pending"),
});
