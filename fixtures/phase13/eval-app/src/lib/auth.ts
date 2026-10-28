import { currentUser, type SessionUser } from "./session.js";

// The wrapper of every route handler: it requires a signed-in user and
// awaits the handler with the request body and that user.
export function withAuth<I, O>(handler: (input: I, user: SessionUser) => Promise<O>) {
  return async (input: I): Promise<O> => {
    const user = currentUser();
    if (!user) throw new Error("unauthorized");
    return await handler(input, user);
  };
}
