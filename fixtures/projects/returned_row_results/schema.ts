// @expect none
import { pgTable, text } from "drizzle-orm/pg-core";
export const records = pgTable("records", { status: text(), ready: text() });
