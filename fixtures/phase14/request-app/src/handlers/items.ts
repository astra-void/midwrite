import { db } from "../db/client.js";
import { orderItems, requestItems } from "../db/schema.js";

// The request/response shape of the application's HTTP layer.
export type AddItemsRequest = { orderId: number; requestId: string };
export type AddItemsResponse = { status: number; body: { ok: boolean } };

// POST /orders/:id/items: two lines in one INSERT, then the response. A
// client that gets no response resubmits.
export async function addOrderItems(request: AddItemsRequest): Promise<AddItemsResponse> {
  await db.insert(orderItems).values([
    { orderId: request.orderId, requestId: request.requestId, sku: "A", qty: 1 },
    { orderId: request.orderId, requestId: request.requestId, sku: "B", qty: 2 },
  ]);
  return { status: 201, body: { ok: true } };
}

// The same, keyed by (request, sku): a resubmission is absorbed.
export async function addOrderItemsIdempotent(
  request: AddItemsRequest,
): Promise<AddItemsResponse> {
  await db
    .insert(requestItems)
    .values([
      { orderId: request.orderId, requestId: request.requestId, sku: "A", qty: 1 },
      { orderId: request.orderId, requestId: request.requestId, sku: "B", qty: 2 },
    ])
    .onConflictDoNothing();
  return { status: 201, body: { ok: true } };
}
