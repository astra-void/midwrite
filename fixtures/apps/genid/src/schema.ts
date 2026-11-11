import {
  integer,
  pgEnum,
  pgTable,
  primaryKey,
  smallint,
  text,
  unique,
  uuid,
} from "drizzle-orm/pg-core";

// A link an owner shares; its token is created on first use.
export const shareLinks = pgTable("share_links", {
  id: integer("id").primaryKey(),
  ownerId: integer("owner_id").notNull(),
  token: text("token"),
});

// Projects have an application-generated id (nanoid) and the id of the
// client request that created them (unique: a resubmission is idempotent).
export const projects = pgTable("projects", {
  id: text("id").primaryKey(),
  requestId: text("request_id").notNull().unique(),
  ownerId: integer("owner_id").notNull(),
});

// Invitations have a database-generated id (gen_random_uuid()).
export const invites = pgTable("invites", {
  id: uuid("id").defaultRandom().primaryKey(),
  teamId: integer("team_id").notNull(),
  email: text("email").notNull(),
});

// A page's view counter.
export const pageViews = pgTable("page_views", {
  pageId: integer("page_id").primaryKey(),
  views: integer("views").notNull(),
});

// Team membership in the shape of midday's `users_on_team` (table and
// column names as there), so that the Phase 16 contract templates written
// for midday load here unchanged.
export const teamRolesEnum = pgEnum("teamRoles", ["owner", "member"]);

export const teams = pgTable("teams", {
  id: uuid("id").defaultRandom().primaryKey(),
  name: text("name"),
});

export const users = pgTable("users", {
  id: uuid("id").primaryKey(),
  teamId: uuid("team_id"),
});

export const usersOnTeam = pgTable(
  "users_on_team",
  {
    userId: uuid("user_id").notNull(),
    teamId: uuid("team_id").notNull(),
    id: uuid("id").defaultRandom().notNull(),
    role: teamRolesEnum("role"),
  },
  (t) => [primaryKey({ columns: [t.userId, t.teamId, t.id], name: "members_pkey" })],
);

// Insights per team and period, in the shape of midday's `insights`.
export const insightStatusEnum = pgEnum("insightStatus", ["pending", "generating", "completed"]);

export const insights = pgTable(
  "insights",
  {
    id: uuid("id").defaultRandom().primaryKey(),
    teamId: uuid("team_id").notNull(),
    periodType: text("period_type").notNull(),
    periodYear: smallint("period_year").notNull(),
    periodNumber: smallint("period_number").notNull(),
    status: insightStatusEnum("status").default("pending").notNull(),
  },
  (t) => [unique("insights_team_period_unique").on(t.teamId, t.periodType, t.periodYear, t.periodNumber)],
);
