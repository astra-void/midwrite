import { currentSession } from "./session.js";

export type SessionUser = { email: string };

// The request wrapper of every action: it requires a signed-in user and
// awaits the handler.
export function withUser<I, O>(handler: (input: I, user: SessionUser) => Promise<O>) {
  return async (input: I): Promise<O> => {
    const session = currentSession();
    if (!session) throw new Error("not signed in");
    return await handler(input, { email: session.email });
  };
}
