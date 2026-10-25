// @expect none
import { complete } from "./service";

export async function handler(id: string) {
  await complete(id);
}
