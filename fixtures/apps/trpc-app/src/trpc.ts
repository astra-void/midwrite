import { initTRPC, TRPCError } from "@trpc/server";

import { db, type Database } from "./db/client.js";

type Session = { userId: string; teamId: string };

export type Context = { db: Database; session: Session | null; teamId?: string };

// What the HTTP adapter calls for each request.
export async function createContext(session: Session | null): Promise<Context> {
  return { db, session };
}

const t = initTRPC.context<Context>().create();

export const createTRPCRouter = t.router;

// Lets a request through only with a session, and puts its team in the
// context.
export const protectedProcedure = t.procedure.use(async (opts) => {
  const { session } = opts.ctx;
  if (!session) throw new TRPCError({ code: "UNAUTHORIZED" });
  return opts.next({ ctx: { teamId: session.teamId } });
});

// Not declared in midwrite-contracts.json: its resolvers stay unextracted.
export const adminProcedure = protectedProcedure.use(async (opts) => opts.next());
