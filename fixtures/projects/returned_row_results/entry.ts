import * as tables from "./schema";
const { records } = tables;
import { capture } from "./capture";
import { Status } from "./status";
export async function updatedRow(status: string) {
  const outcome = await db.transaction(async tx => {
    const saved = await capture(tx.update(records).set({ status }).returning());
    return saved.map(([updated]) => {
      if (!updated) throw new Error("missing");
      return { record: updated };
    });
  });
  if (outcome.isErr()) return;
  const { record } = outcome.value;
  if (status === Status.Running && record.status === Status.Paused) {
    await queue.publish("unreachable", {});
  }
  await queue.publish("saved", {});
}
export async function previousRow(status: string) {
  const outcome = await db.transaction(async tx => {
    const [previous] = await tx.select().from(records);
    await tx.update(records).set({ status });
    return { record: previous };
  });
  if (status === Status.Running && outcome.record.status === Status.Paused) {
    await queue.publish("restart", {});
  }
}
export async function unresolvedMapper(status: string, transform: any) {
  const outcome = await db.transaction(async tx => {
    const saved = await capture(tx.update(records).set({ status }).returning());
    return transform(saved);
  });
  if (outcome.isErr()) return;
  if (status === Status.Running && outcome.value.status === Status.Paused) {
    await queue.publish("possible", {});
  }
}

// @expect MW9001 line=7 function=updatedRow
// @expect MW1001 line=18 function=updatedRow
// @expect MW1001 line=27 function=previousRow
// @expect MW9001 line=32 function=unresolvedMapper
// @expect MW1001 line=37 function=unresolvedMapper
