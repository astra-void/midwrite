// Request-scoped session, as the HTTP framework provides it: the server runs
// each request inside `runWithSession`.
import { AsyncLocalStorage } from "node:async_hooks";

export type SessionUser = { id: string; email: string };

const storage = new AsyncLocalStorage<SessionUser>();

export function runWithSession<T>(user: SessionUser, handler: () => Promise<T>): Promise<T> {
  return storage.run(user, handler);
}

export function currentUser(): SessionUser | null {
  return storage.getStore() ?? null;
}
