// Schema-extraction fixture (Phase 8): every supported fact and every
// construct the extractor must refuse to generalize. Never executed.
import { sql } from "drizzle-orm";
import {
  boolean,
  check,
  foreignKey,
  index,
  integer,
  pgEnum,
  pgSchema,
  pgTable,
  primaryKey,
  serial,
  text,
  timestamp,
  unique,
  uniqueIndex,
  varchar,
} from "drizzle-orm/pg-core";

export const roleEnum = pgEnum("member_role", ["owner", "member", "viewer"]);

export const accounts = pgTable("accounts", {
  id: integer("id").primaryKey().generatedAlwaysAsIdentity(),
  email: varchar("email", { length: 255 }).notNull().unique(),
  // Nullable and unique: several NULLs never conflict.
  externalId: text("external_id").unique("accounts_external_id_key"),
  role: roleEnum("role").notNull().default("member"),
  active: boolean("active").notNull().default(true),
  createdAt: timestamp("created_at", { withTimezone: true }).notNull().defaultNow(),
  // Computed by Drizzle in the application, not by the database.
  token: text("token").notNull().$defaultFn(() => crypto.randomUUID()),
});

export const memberships = pgTable(
  "memberships",
  {
    accountId: integer("account_id").notNull(),
    teamId: integer("team_id").notNull(),
    seat: integer("seat"),
    invitedEmail: text("invited_email"),
    archived: boolean("archived").notNull().default(false),
  },
  (table) => [
    primaryKey({ columns: [table.accountId, table.teamId] }),
    foreignKey({ columns: [table.accountId], foreignColumns: [accounts.id] }),
    // Composite uniqueness with a nullable column (NULLs are distinct).
    unique("memberships_team_seat").on(table.teamId, table.seat),
    // Partial: unique only among non-archived rows. Not a global rule.
    uniqueIndex("memberships_active_invite")
      .on(table.teamId, table.invitedEmail)
      .where(sql`${table.archived} = false`),
    // NULLS NOT DISTINCT on a nullable column (PostgreSQL 15+).
    unique("memberships_invite_any").on(table.invitedEmail).nullsNotDistinct(),
    index("memberships_team_idx").on(table.teamId),
    check("memberships_seat_positive", sql`${table.seat} > 0`),
  ],
);

export const audit = pgSchema("audit");

export const events = audit.table("events", {
  id: serial("id").primaryKey(),
  accountId: integer("account_id").references(() => accounts.id),
  kind: text("kind").notNull(),
  // No explicit SQL name: depends on drizzle()'s casing option.
  occurredAt: timestamp().notNull().defaultNow(),
});

const prefix = "tenant_";
// Dynamic table name: not extracted.
export const dynamic = pgTable(prefix + "settings", {
  id: integer("id").primaryKey(),
});

// A column helper: spread columns are not extracted.
const timestamps = { updatedAt: timestamp("updated_at") };
export const notes = pgTable("notes", {
  id: serial("id").primaryKey(),
  body: text("body").notNull(),
  ...timestamps,
});
