import wrapped, { disabled, enabled } from "./module_barrel";
import { forwardedWrite, identity } from "./factories";

export async function fromNamed() { await enabled("one"); }
export async function fromDefault() { await wrapped("two"); }
export async function inactive() { await disabled("three"); }

const write = forwardedWrite();
export async function afterModuleWrite() {
  await write();
  await queue.publish({});
}

const publish = identity(async () => { await queue.publish({}); });
export async function beforeModulePublish() {
  await db.insert(orders).values({});
  await publish();
}

// @expect MW1001 line=4 function=fromNamed
// @expect MW1001 line=5 function=fromDefault
// @expect MW1001 line=11 function=afterModuleWrite
// @expect MW1001 line=17 function=beforeModulePublish
