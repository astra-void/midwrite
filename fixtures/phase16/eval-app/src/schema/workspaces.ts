import {
  boolean,
  foreignKey,
  index,
  integer,
  pgTable,
  primaryKey,
  serial,
  text,
  unique,
  uuid,
} from "drizzle-orm/pg-core";

import { accounts } from "./accounts.js";

export const workspaces = pgTable("workspaces", {
  id: serial("id").primaryKey(),
  name: text("name").notNull(),
  ownerId: uuid("owner_id")
    .notNull()
    .references(() => accounts.id),
});

// Projects: (workspace_id, slug) is indexed for lookups, NOT unique.
export const projects = pgTable(
  "projects",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    slug: text("slug").notNull(),
    name: text("name").notNull(),
    archived: boolean("archived").notNull().default(false),
  },
  (t) => [index("projects_workspace_slug").on(t.workspaceId, t.slug)],
);

// Boards: the same shape with UNIQUE(workspace_id, slug).
export const boards = pgTable(
  "boards",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    slug: text("slug").notNull(),
    name: text("name").notNull(),
    archived: boolean("archived").notNull().default(false),
  },
  (t) => [unique("boards_workspace_slug").on(t.workspaceId, t.slug)],
);

export const labelGroups = pgTable("label_groups", {
  id: serial("id").primaryKey(),
  workspaceId: integer("workspace_id")
    .notNull()
    .references(() => workspaces.id),
  name: text("name").notNull(),
});

// Labels belong to a group, or to no group (group_id NULL).
// UNIQUE(workspace_id, group_id, name) with PostgreSQL's default NULLS
// DISTINCT: two ungrouped labels never conflict.
export const labels = pgTable(
  "labels",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    groupId: integer("group_id").references(() => labelGroups.id),
    name: text("name").notNull(),
  },
  (t) => [unique("labels_workspace_group_name").on(t.workspaceId, t.groupId, t.name)],
);

// Tags: ungrouped tags use group_id 0 (no foreign key); the column is never
// NULL, so UNIQUE(workspace_id, group_id, name) covers ungrouped tags too.
export const tags = pgTable(
  "tags",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    groupId: integer("group_id").notNull().default(0),
    name: text("name").notNull(),
  },
  (t) => [unique("tags_workspace_group_name").on(t.workspaceId, t.groupId, t.name)],
);

// Workspace-level invitations: (workspace_id, email) indexed, NOT unique.
export const workspaceInvites = pgTable(
  "workspace_invites",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    email: text("email").notNull(),
  },
  (t) => [index("workspace_invites_workspace_email").on(t.workspaceId, t.email)],
);

// Project-level invitations: primary key (project_id, email).
export const projectInvites = pgTable(
  "project_invites",
  {
    projectId: integer("project_id")
      .notNull()
      .references(() => projects.id),
    email: text("email").notNull(),
  },
  (t) => [primaryKey({ columns: [t.projectId, t.email] })],
);

// A tree of collections; parent_id NULL is the workspace root.
// UNIQUE(workspace_id, parent_id, name) with the default NULLS DISTINCT.
export const collections = pgTable(
  "collections",
  {
    id: serial("id").primaryKey(),
    workspaceId: integer("workspace_id")
      .notNull()
      .references(() => workspaces.id),
    parentId: integer("parent_id"),
    name: text("name").notNull(),
    archived: boolean("archived").notNull().default(false),
  },
  (t) => [
    foreignKey({ name: "collections_parent", columns: [t.parentId], foreignColumns: [t.id] }),
    unique("collections_workspace_parent_name").on(t.workspaceId, t.parentId, t.name),
  ],
);

// User-written automation scripts (JavaScript source stored as text).
export const automations = pgTable("automations", {
  id: serial("id").primaryKey(),
  workspaceId: integer("workspace_id")
    .notNull()
    .references(() => workspaces.id),
  source: text("source").notNull(),
  enabled: boolean("enabled").notNull().default(true),
  runs: integer("runs").notNull().default(0),
});
