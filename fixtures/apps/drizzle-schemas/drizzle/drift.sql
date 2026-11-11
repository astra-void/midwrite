-- A database that drifted from src/schema.ts: a missing unique constraint,
-- a global unique index where the source has a partial one, a nullable
-- column the source declares NOT NULL, a different default.
CREATE TYPE "public"."member_role" AS ENUM('owner', 'member', 'viewer');
CREATE SCHEMA "audit";
CREATE TABLE "accounts" (
	"id" integer PRIMARY KEY GENERATED ALWAYS AS IDENTITY NOT NULL,
	"email" varchar(255),
	"external_id" text,
	"role" "member_role" DEFAULT 'viewer' NOT NULL,
	"active" boolean DEFAULT true NOT NULL,
	"created_at" timestamp with time zone DEFAULT now() NOT NULL,
	"token" text NOT NULL,
	CONSTRAINT "accounts_external_id_key" UNIQUE("external_id")
);
CREATE TABLE "memberships" (
	"account_id" integer NOT NULL,
	"team_id" integer NOT NULL,
	"seat" integer,
	"invited_email" text,
	"archived" boolean DEFAULT false NOT NULL,
	CONSTRAINT "memberships_account_id_team_id_pk" PRIMARY KEY("account_id","team_id"),
	CONSTRAINT "memberships_team_seat" UNIQUE("team_id","seat")
);
CREATE TABLE "audit"."events" (
	"id" serial PRIMARY KEY NOT NULL,
	"account_id" integer,
	"kind" text NOT NULL,
	"occurredAt" timestamp DEFAULT now() NOT NULL
);
CREATE TABLE "notes" (
	"id" serial PRIMARY KEY NOT NULL,
	"body" text NOT NULL,
	"updated_at" timestamp
);
CREATE UNIQUE INDEX "memberships_active_invite" ON "memberships" USING btree ("team_id","invited_email");
