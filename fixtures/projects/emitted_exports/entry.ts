// @expect MW1001 line=5 function=submit
import { save } from "@example/storage/src/save";
export async function submit(id: string) {
  await save(id);
  await queue.publish("stored", { id });
}
