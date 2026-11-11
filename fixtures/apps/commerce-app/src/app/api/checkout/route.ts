// The HTTP route in front of the checkout service.
import { checkout } from "../../checkout.js";

export async function POST(request: Request) {
  const body = (await request.json()) as { requestId: string; orderId: number };
  const result = await checkout({ requestId: body.requestId, orderId: body.orderId });
  return Response.json(result);
}
