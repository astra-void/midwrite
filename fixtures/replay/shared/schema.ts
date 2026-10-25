import { table } from "./db.ts";

export const orders = table("orders", { id: "id", status: "status" });

export const outbox = table("outbox", {
  id: "id",
  topic: "topic",
  payload: "payload",
  publishedAt: "published_at",
});
