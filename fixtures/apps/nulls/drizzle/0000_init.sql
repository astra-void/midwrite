-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "tags" (
	"id" serial PRIMARY KEY NOT NULL,
	"owner_id" integer NOT NULL,
	"group_id" integer,
	"name" text NOT NULL,
	CONSTRAINT "tags_owner_group_name" UNIQUE("owner_id","group_id","name")
);
CREATE TABLE "badges" (
	"id" serial PRIMARY KEY NOT NULL,
	"owner_id" integer NOT NULL,
	"group_id" integer,
	"name" text NOT NULL,
	CONSTRAINT "badges_owner_group_name" UNIQUE NULLS NOT DISTINCT("owner_id","group_id","name")
);
CREATE TABLE "devices" (
	"id" serial PRIMARY KEY NOT NULL,
	"serial_no" text,
	CONSTRAINT "devices_serial_no" UNIQUE("serial_no")
);
CREATE TABLE "scanners" (
	"id" serial PRIMARY KEY NOT NULL,
	"serial_no" text,
	CONSTRAINT "scanners_serial_no" UNIQUE NULLS NOT DISTINCT("serial_no")
);
