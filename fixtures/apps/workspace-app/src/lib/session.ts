// Request-scoped session, as a framework provides it (in Next.js: the
// session cookie). The server entry point runs each request inside
// `runWithSession`.
import { AsyncLocalStorage } from "node:async_hooks";

export type Session = { email: string };

const storage = new AsyncLocalStorage<Session>();

export function runWithSession<T>(session: Session, handler: () => Promise<T>): Promise<T> {
  return storage.run(session, handler);
}

export function currentSession(): Session | null {
  return storage.getStore() ?? null;
}
