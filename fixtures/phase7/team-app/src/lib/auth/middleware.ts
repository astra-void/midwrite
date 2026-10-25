import { getUser } from "../db/queries.js";
import type { User } from "../db/schema.js";
import type { Schema } from "../validation.js";

export type ActionState = {
  error?: string;
  success?: string;
  [key: string]: unknown;
};

type ValidatedActionFunction<T, R> = (data: T, formData: FormData) => Promise<R>;

export function validatedAction<T, R>(schema: Schema<T>, action: ValidatedActionFunction<T, R>) {
  return async (prevState: ActionState, formData: FormData) => {
    const result = schema.safeParse(Object.fromEntries(formData));
    if (!result.success) {
      return { error: result.error.errors[0].message };
    }

    return action(result.data, formData);
  };
}

type ValidatedActionWithUserFunction<T, R> = (data: T, formData: FormData, user: User) => Promise<R>;

export function validatedActionWithUser<T, R>(
  schema: Schema<T>,
  action: ValidatedActionWithUserFunction<T, R>,
) {
  return async (prevState: ActionState, formData: FormData) => {
    const user = await getUser();
    if (!user) {
      throw new Error("User is not authenticated");
    }

    const result = schema.safeParse(Object.fromEntries(formData));
    if (!result.success) {
      return { error: result.error.errors[0].message };
    }

    return action(result.data, formData, user);
  };
}
