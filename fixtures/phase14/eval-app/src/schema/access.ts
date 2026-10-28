import { pgTable, primaryKey, serial, text, timestamp, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

// Roles granted to an account member (by email). No unique constraint.
export const roleGrants = pgTable("role_grants", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  memberEmail: text("member_email").notNull(),
  role: text("role").notNull(),
  grantedAt: timestamp("granted_at", { withTimezone: true }).notNull().defaultNow(),
});

// Scopes of an account's API integration: (account_id, scope) is the key.
export const apiScopes = pgTable(
  "api_scopes",
  {
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    scope: text("scope").notNull(),
    grantedAt: timestamp("granted_at", { withTimezone: true }).notNull().defaultNow(),
  },
  (t) => [primaryKey({ columns: [t.accountId, t.scope] })],
);

// Beta enrollments (beta builds only). No unique constraint.
export const betaSignups = pgTable("beta_signups", {
  id: serial("id").primaryKey(),
  accountId: uuid("account_id")
    .notNull()
    .references(() => accounts.id),
  enrolledAt: timestamp("enrolled_at", { withTimezone: true }).notNull().defaultNow(),
});

// The beta waitlist (stable builds): one row per account.
export const waitlist = pgTable("waitlist", {
  accountId: uuid("account_id")
    .primaryKey()
    .references(() => accounts.id),
  joinedAt: timestamp("joined_at", { withTimezone: true }).notNull().defaultNow(),
});
