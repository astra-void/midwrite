import { db } from "../db/client.js";
import { signupRequests, signups } from "../db/schema.js";

// The request/response shape of the application's HTTP layer: the server
// parses the JSON body into a SignupRequest and serializes the returned
// SignupResponse.
export type SignupRequest = { accountId: number; requestId: string };
export type SignupResponse = { status: number; body: { ok: boolean } };

// One INSERT, then the response. A client that gets no response resubmits.
export async function recordSignup(request: SignupRequest): Promise<SignupResponse> {
  await db.insert(signups).values({ accountId: request.accountId, requestId: request.requestId });
  return { status: 201, body: { ok: true } };
}

// The same, keyed by the client's request id: a resubmission is absorbed.
export async function recordSignupIdempotent(request: SignupRequest): Promise<SignupResponse> {
  await db
    .insert(signupRequests)
    .values({ requestId: request.requestId, accountId: request.accountId })
    .onConflictDoNothing();
  return { status: 201, body: { ok: true } };
}
