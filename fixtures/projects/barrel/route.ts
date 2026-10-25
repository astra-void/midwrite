// Both functions are reached through the barrel.
// @expect CP1001 line=7 confidence=high function=POST
import { close, announce } from "./services";

export async function POST(id: string) {
  await close(id);
  await announce(id);
}
