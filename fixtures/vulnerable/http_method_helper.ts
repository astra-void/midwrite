// @expect MW9001 line=27 function=lookup
// @expect MW1001 line=33 function=createTopic
// @expect MW1001 line=39 function=upsertSubscription
// @expect MW1001 line=48 function=removeTopic
// @expect MW9001 line=54 function=send
// @expect MW9001 line=59 function=sendWithInit
// The method of a request made through a helper that spreads its `options`
// parameter into `fetch` is decided at each call, from the argument passed.
// The fixture rule restricts `fetch` to POST, PUT, PATCH and DELETE.

async function request(url: string, options: RequestInit = {}) {
  return fetch(url, {
    ...options,
    headers: { ...options.headers, Authorization: "Bearer token" },
  });
}

// Forwards its own parameter: decided where `put` is called.
async function put(url: string, init: RequestInit) {
  return request(url, init);
}

// No options: an implicit GET, which no rule matches. It stays an unknown
// call (MW9001 after the write), never an HTTP write.
export async function lookup(id: string) {
  await db.update(orders).set({ seen: true });
  await request(`https://example.test/topics/${id}`);
}

// A literal PUT: the configured HTTP effect.
export async function createTopic(id: string) {
  await db.update(orders).set({ topic: id });
  await request(`https://example.test/topics/${id}`, { method: "PUT" });
}

// Either of two literal methods, both listed.
export async function upsertSubscription(id: string, exists: boolean) {
  await db.update(orders).set({ subscription: id });
  await request(`https://example.test/subscriptions/${id}`, {
    method: exists ? "POST" : "PUT",
    headers: { "Content-Type": "application/json" },
  });
}

// A literal DELETE, passed to `put`, which forwards it to `request`.
export async function removeTopic(id: string) {
  await db.update(orders).set({ topic: null });
  await put(`https://example.test/topics/${id}`, { method: "DELETE" });
}

// A dynamic method stays unknown (MW9001), as it does in a direct call.
export async function send(id: string, verb: string) {
  await db.update(orders).set({ sent: id });
  await request(`https://example.test/messages/${id}`, { method: verb });
}

export async function sendWithInit(id: string, init: RequestInit) {
  await db.update(orders).set({ sent: id });
  await request(`https://example.test/messages/${id}`, init);
}
