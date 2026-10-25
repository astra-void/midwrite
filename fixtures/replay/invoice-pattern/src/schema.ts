import { table } from "../../shared/db.ts";

export const invoices = table("invoices", { id: "id", status: "status" });
