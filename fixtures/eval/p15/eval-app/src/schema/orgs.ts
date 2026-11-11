import { index, integer, pgTable, primaryKey, serial, text, unique, uuid } from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

export type Role = "member" | "admin";
export type RouteKind = "page" | "redirect";

// vacancies: seats that may still be taken in the organization.
export const orgs = pgTable("orgs", {
  id: serial("id").primaryKey(),
  name: text("name").notNull(),
  vacancies: integer("vacancies").notNull().default(0),
});

// One membership per (org, account); role is "member" or "admin".
export const memberships = pgTable(
  "memberships",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
    role: text("role").$type<Role>().notNull(),
  },
  (t) => [unique("memberships_org_account").on(t.orgId, t.accountId)],
);

export const seats = pgTable(
  "seats",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    accountId: uuid("account_id")
      .notNull()
      .references(() => accounts.id),
  },
  (t) => [unique("seats_org_account").on(t.orgId, t.accountId)],
);

// Deployment environments of an organization. Looked up by (org, name); the
// index is not unique.
export const environments = pgTable(
  "environments",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    name: text("name").notNull(),
    createdBy: uuid("created_by")
      .notNull()
      .references(() => accounts.id),
  },
  (t) => [index("environments_org_name").on(t.orgId, t.name)],
);

// Release channels: UNIQUE(org_id, name).
export const channels = pgTable(
  "channels",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    name: text("name").notNull(),
    createdBy: uuid("created_by")
      .notNull()
      .references(() => accounts.id),
  },
  (t) => [unique("channels_org_name").on(t.orgId, t.name)],
);

// Organization-level invitations. Looked up by (org, email); not unique.
export const invitations = pgTable(
  "invitations",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    email: text("email").notNull(),
  },
  (t) => [index("invitations_org_email").on(t.orgId, t.email)],
);

export const teams = pgTable("teams", {
  id: serial("id").primaryKey(),
  orgId: integer("org_id")
    .notNull()
    .references(() => orgs.id),
  name: text("name").notNull(),
});

// Team invitations: primary key (team_id, email).
export const teamInvites = pgTable(
  "team_invites",
  {
    teamId: integer("team_id")
      .notNull()
      .references(() => teams.id),
    email: text("email").notNull(),
  },
  (t) => [primaryKey({ columns: [t.teamId, t.email] })],
);

// URL routes of an organization's site: UNIQUE(org_id, kind, slug).
export const routes = pgTable(
  "routes",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    kind: text("kind").$type<RouteKind>().notNull(),
    slug: text("slug").notNull(),
  },
  (t) => [unique("routes_org_kind_slug").on(t.orgId, t.kind, t.slug)],
);

// Short aliases of an organization's site: UNIQUE(org_id, slug).
export const aliases = pgTable(
  "aliases",
  {
    id: serial("id").primaryKey(),
    orgId: integer("org_id")
      .notNull()
      .references(() => orgs.id),
    kind: text("kind").$type<RouteKind>().notNull(),
    slug: text("slug").notNull(),
  },
  (t) => [unique("aliases_org_slug").on(t.orgId, t.slug)],
);
