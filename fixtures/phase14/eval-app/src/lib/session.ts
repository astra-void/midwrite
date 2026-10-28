// Request-scoped session, as the HTTP framework's middleware provides it: the
// server runs each authenticated request inside `runWithSession`.
import { AsyncLocalStorage } from "node:async_hooks";

export type Session = { userId: string; accountId: string };

const storage = new AsyncLocalStorage<Session>();

export function runWithSession<T>(session: Session, handler: () => Promise<T>): Promise<T> {
  return storage.run(session, handler);
}

export function currentSession(): Session | null {
  return storage.getStore() ?? null;
}
