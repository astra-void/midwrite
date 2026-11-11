import { placeOrder } from "../../../services/orders.js";

// POST /api/orders: the JSON body names the client's request, the customer
// and the amount; `placeOrder` writes the order (with its retry loop).
export async function POST(request: Request) {
  const body = (await request.json()) as { requestId: string; customerId: number; amount: number };
  const status = await placeOrder(body.requestId, body.customerId, body.amount);
  return Response.json({ status }, { status: 201 });
}
