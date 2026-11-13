import { forwardedWrite, identity, makeWrite } from "./factories";

export async function stored() {
  const write = makeWrite();
  await write();
  await queue.publish({});
}

export async function inline() {
  await makeWrite()();
  await queue.publish({});
}

export async function forwarded() {
  const write = forwardedWrite();
  await write();
  await queue.publish({});
}

export async function passedFunction() {
  const notify = identity(async () => { await mailer.send({}); });
  await notify();
  await queue.publish({});
}

export async function unused() {
  const write = makeWrite();
  await queue.publish({});
}

// @expect MW1001 line=6 confidence=high function=stored
// @expect MW1001 line=11 confidence=high function=inline
// @expect MW1001 line=17 confidence=high function=forwarded
// @expect MW1005 line=23 confidence=high function=passedFunction
