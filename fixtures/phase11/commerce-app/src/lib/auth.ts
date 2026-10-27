// Server Action wrappers: authenticate the member, validate the form, then
// run the action (the shape of saas-starter's `validatedActionWithUser`).
import { eq } from "drizzle-orm";

import { db } from "../db/client.js";
import { members, type Member } from "../db/schema.js";
import { currentSession } from "./session.js";
import type { Schema } from "./validation.js";

export type ActionState = {
  error?: string;
  success?: string;
  [key: string]: unknown;
};

export async function currentMember(): Promise<Member | null> {
  const session = currentSession();
  if (!session) {
    return null;
  }
  const [member] = await db.select().from(members).where(eq(members.id, session.userId)).limit(1);
  return member ?? null;
}

type MemberAction<T, R> = (data: T, member: Member) => Promise<R>;

export function withMember<T, R>(schema: Schema<T>, action: MemberAction<T, R>) {
  return async (prevState: ActionState, formData: FormData) => {
    const member = await currentMember();
    if (!member) {
      throw new Error("not signed in");
    }
    const parsed = schema.safeParse(Object.fromEntries(formData));
    if (!parsed.success) {
      return { error: parsed.error.errors[0].message };
    }
    return action(parsed.data, member);
  };
}
