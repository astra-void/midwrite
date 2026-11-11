-- The application's schema migration (what `drizzle-kit generate` produces
-- for src/db/schema.ts). Applied to the disposable database only.
CREATE TABLE "tasks" (
	"id" integer PRIMARY KEY NOT NULL,
	"title" text NOT NULL,
	"assignee" text
);
CREATE TABLE "signups" (
	"id" uuid PRIMARY KEY DEFAULT gen_random_uuid() NOT NULL,
	"account_id" integer NOT NULL,
	"request_id" text NOT NULL
);
CREATE TABLE "signup_requests" (
	"request_id" text PRIMARY KEY NOT NULL,
	"account_id" integer NOT NULL
);
CREATE TABLE "views" (
	"id" integer PRIMARY KEY NOT NULL,
	"hits" integer NOT NULL
);
CREATE TABLE "handles" (
	"handle" text PRIMARY KEY NOT NULL,
	"owner" text
);
