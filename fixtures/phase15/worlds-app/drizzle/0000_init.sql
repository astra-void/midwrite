-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "folders" (
	"id" serial PRIMARY KEY NOT NULL,
	"owner_id" integer NOT NULL,
	"parent_id" integer,
	"name" text NOT NULL,
	CONSTRAINT "folders_owner_parent_name" UNIQUE("owner_id","parent_id","name")
);
CREATE TABLE "labels" (
	"id" serial PRIMARY KEY NOT NULL,
	"owner_id" integer NOT NULL,
	"parent_id" integer,
	"name" text NOT NULL,
	CONSTRAINT "labels_owner_parent_name" UNIQUE NULLS NOT DISTINCT("owner_id","parent_id","name")
);
CREATE TABLE "quotas" (
	"account_id" integer PRIMARY KEY NOT NULL,
	"remaining" integer NOT NULL
);
CREATE TABLE "export_jobs" (
	"id" serial PRIMARY KEY NOT NULL,
	"account_id" integer NOT NULL
);
