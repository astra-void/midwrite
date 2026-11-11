import { currentSession } from "./session.js";

export type AuthContext = { req: Request; userId: string; accountId: string };

// Wraps a route handler: requires a signed-in session, awaits the handler
// once with the request and the session's user and account, and sends its
// result as JSON.
export function withAuth<T>(handler: (ctx: AuthContext) => Promise<T>) {
  return async (req: Request): Promise<Response> => {
    const session = currentSession();
    if (!session) return Response.json({ error: "unauthorized" }, { status: 401 });
    const result = await handler({ req, userId: session.userId, accountId: session.accountId });
    return Response.json(result);
  };
}
