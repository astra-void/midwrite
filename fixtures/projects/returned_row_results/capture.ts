// @expect none
import { ok, err } from "neverthrow";
export async function capture<T>(query: PromiseLike<T>) {
  try { return ok(await query); }
  catch (error) { return err(error); }
}
