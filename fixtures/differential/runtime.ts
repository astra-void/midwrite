// Differential-test runtime: Drizzle- and queue-shaped fakes that record the
// operations a program performs, in completion order. Not a database.
// @expect none
export const log: string[] = [];

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function table(name: string) {
  return { name };
}

export const orders = table("orders");
export const payments = table("payments");

export const db = {
  update(target: { name: string }) {
    return {
      set(_values: unknown) {
        return {
          then(resolve: (value: unknown) => void, reject: (error: unknown) => void) {
            delay(1).then(() => {
              log.push(`write:update:${target.name}`);
              resolve([{ id: "1" }]);
            }, reject);
          },
        };
      },
    };
  },
  insert(target: { name: string }) {
    return {
      values(_values: unknown) {
        return {
          then(resolve: (value: unknown) => void, reject: (error: unknown) => void) {
            delay(1).then(() => {
              log.push(`write:insert:${target.name}`);
              resolve([{ id: "1" }]);
            }, reject);
          },
        };
      },
    };
  },
};

export const queue = {
  async publish(topic: string, _payload: unknown) {
    // Different delays make concurrent completion order observable.
    await delay(topic.endsWith(".slow") ? 5 : 1);
    log.push(`external:${topic}`);
  },
};
