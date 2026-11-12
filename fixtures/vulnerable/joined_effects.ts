import { Upload } from "@aws-sdk/lib-storage";
import { createClient } from "@clickhouse/client";

async function save(id: string) { await db.insert(items).values({ id }); }
function returnPromise(id: string) { return save(id); }
async function queryResult<T>(promise: PromiseLike<T>) {
  try { return { value: await promise }; }
  catch (error) { logger.warn(error); throw error; }
}

export async function returnedPromise(ids: string[]) {
  await queue.publish("batch", { ids });
  await Promise.all(ids.map(id => returnPromise(id)));
}
export async function joinedVariable(ids: string[]) {
  const tasks = ids.flatMap(async id => {
    await queue.publish("one", { id });
  });
  await Promise.all(tasks);
  await db.insert(batches).values({ ids });
}
export async function wrappedBuilder(id: string) {
  await db.transaction(async tx => {
    await queryResult(tx.update(items).set({ id }));
  });
  await queue.publish("changed", { id });
}
export async function mixedBatch(messages: any[]) {
  await Promise.all(messages.map(async message => {
    if (message.existing) {
      await db.update(items).set({ id: message.id });
      return;
    }
    await db.insert(items).values({ id: message.id });
  }));
}
export async function multipart(id: string) {
  const transfer = new Upload({ client, params: { Key: id } });
  await transfer.done();
  await db.insert(files).values({ id });
}
export async function analyticsInsert(id: string) {
  const analytics = createClient({});
  await db.insert(items).values({ id });
  await analytics.insert({ table: "events", values: [{ id }] });
}

import { trace } from "@opentelemetry/api";
async function traced(action: (span: any) => Promise<void>) {
  const tracer = trace.getTracer("example");
  return tracer.startActiveSpan("operation", async span => {
    try { return await action(span); }
    finally { span.end(); }
  });
}
export async function tracedBatch(ids: string[]) {
  const pending = ids.map(id => traced(async () => {
    await queue.publish("one", { id });
  }));
  await Promise.all(pending);
  await db.insert(batches).values({ ids });
}

// @expect MW1002 line=12 function=returnedPromise
// @expect MW1002 line=16 function=joinedVariable
// @expect MW9001 line=24 function=wrappedBuilder
// @expect MW1001 line=26 function=wrappedBuilder
// @expect MW1004 line=29 function=mixedBatch
// @expect MW1004 line=29 function=mixedBatch
// @expect MW1002 line=39 function=multipart
// @expect MW1001 line=45 function=analyticsInsert
// @expect MW1002 line=57 function=tracedBatch
