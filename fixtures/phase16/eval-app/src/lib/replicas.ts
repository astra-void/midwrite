import type { Database } from "../db.js";

// A project-defined replica set, older than Drizzle's withReplicas and kept
// for the settings pages.
//
//   * select and $count run on a replica, chosen round-robin;
//   * insert, update, delete, execute and transaction run on the primary
//     (the first argument);
//   * `$primary` is the primary, and `usePrimaryOnly()` returns it, for
//     callers that must read their own writes outside a transaction.
//
// Replicas are asynchronous streaming standbys and may lag.
export function createReplicaSet(primary: Database, replicas: readonly Database[]) {
  let cursor = 0;
  const pickReplica = (): Database => {
    if (replicas.length === 0) return primary;
    const replica = replicas[cursor % replicas.length] ?? primary;
    cursor += 1;
    return replica;
  };

  const select = ((...args: Parameters<Database["select"]>) =>
    pickReplica().select(...args)) as unknown as Database["select"];
  const $count = ((...args: Parameters<Database["$count"]>) =>
    pickReplica().$count(...args)) as unknown as Database["$count"];

  return {
    $primary: primary,
    usePrimaryOnly: (): Database => primary,
    select,
    $count,
    insert: primary.insert.bind(primary) as Database["insert"],
    update: primary.update.bind(primary) as Database["update"],
    delete: primary.delete.bind(primary) as Database["delete"],
    execute: primary.execute.bind(primary) as Database["execute"],
    transaction: primary.transaction.bind(primary) as Database["transaction"],
  };
}

export type ReplicaSet = ReturnType<typeof createReplicaSet>;
