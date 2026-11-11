// A minimal schema-validation library (the role zod plays in a Next.js
// app): `object({...}).safeParse(input)`.
export type Issue = { path: string; message: string };
export type SafeParse<T> = { success: true; data: T } | { success: false; error: { errors: Issue[] } };

export interface Schema<T> {
  safeParse(input: unknown): SafeParse<T>;
}

type Field<T> = (value: unknown, path: string) => { ok: true; value: T } | { ok: false; issue: Issue };

export function email(message = "Invalid email address"): Field<string> {
  return (value, path) =>
    typeof value === "string" && /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(value)
      ? { ok: true, value }
      : { ok: false, issue: { path, message } };
}

export function positiveInteger(message = "Invalid id"): Field<number> {
  return (value, path) => {
    const n = typeof value === "string" ? Number(value) : value;
    return typeof n === "number" && Number.isInteger(n) && n > 0
      ? { ok: true, value: n }
      : { ok: false, issue: { path, message } };
  };
}

export function oneOf<const V extends string>(values: readonly V[]): Field<V> {
  return (value, path) =>
    typeof value === "string" && (values as readonly string[]).includes(value)
      ? { ok: true, value: value as V }
      : { ok: false, issue: { path, message: `Expected one of ${values.join(", ")}` } };
}

export function object<S extends Record<string, Field<unknown>>>(
  shape: S,
): Schema<{ [K in keyof S]: S[K] extends Field<infer T> ? T : never }> {
  return {
    safeParse(input) {
      const record = (input ?? {}) as Record<string, unknown>;
      const data: Record<string, unknown> = {};
      const errors: Issue[] = [];
      for (const [key, field] of Object.entries(shape)) {
        const result = field(record[key], key);
        if (result.ok) data[key] = result.value;
        else errors.push(result.issue);
      }
      return errors.length === 0
        ? { success: true, data: data as never }
        : { success: false, error: { errors } };
    },
  };
}
