// First-party higher-order wrappers, one per invocation shape.
type Handler<I, R> = (input: I, user: { id: number }) => Promise<R>;

declare function currentUser(): Promise<{ id: number } | null>;
const registry: unknown[] = [];

/** Returns a function that always returns the callback's promise. */
export function action<I, R>(fn: (input: I) => Promise<R>) {
  return async (input: I) => {
    return fn(input);
  };
}

/** Returns a function that awaits the callback after an authentication guard. */
export function authed<I, R>(fn: Handler<I, R>) {
  return async (input: I) => {
    const user = await currentUser();
    if (!user) {
      throw new Error("unauthorized");
    }
    const result = await fn(input, user);
    return result;
  };
}

/** Calls the callback while wrapping. */
export function immediate<R>(fn: (reason: string) => R) {
  return fn("boot");
}

/** Stores the callback for later. */
export function store<I>(fn: (input: I) => Promise<void>) {
  registry.push(fn);
  return () => registry.length;
}

/** Never calls the callback. */
export function ignore<I>(fn: (input: I) => Promise<void>) {
  return async (_input: I) => 1;
}

/** Starts the callback without waiting for it. */
export function later<I>(fn: (input: I) => Promise<void>) {
  return async (input: I) => {
    void fn(input);
  };
}
