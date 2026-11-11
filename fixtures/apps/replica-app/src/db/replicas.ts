// A project-defined replica factory modeled after drizzle-orm's own
// `withReplicas` (routing by getters, an explicit `usePrimaryOnly()` and
// `$primary`). Midwrite does not interpret this body: the routing is taken
// from a declaration (`ExtractRequest.replica_factories`), never inferred.
import type { PgDatabase } from "drizzle-orm/pg-core";

export type ReplicatedDatabase<Q extends PgDatabase<any, any, any>> = Q & {
  usePrimaryOnly: () => ReplicatedDatabase<Q>;
  $primary: Q;
};

export const withReplicas = <Q extends PgDatabase<any, any, any>>(
  primary: Q,
  replicas: [Q, ...Q[]],
  getReplica: (replicas: Q[]) => Q = () =>
    replicas[Math.floor(Math.random() * replicas.length)]!,
): ReplicatedDatabase<Q> => {
  const createDatabase = (usePrimary = false): ReplicatedDatabase<Q> => {
    const getDbForRead = () => (usePrimary ? primary : getReplica(replicas));
    const bind = (db: Q, method: any): any =>
      typeof method === "function" ? method.bind(db) : method;
    return {
      ...primary,
      get select() {
        const db = getDbForRead();
        return bind(db, db.select);
      },
      get query() {
        return getDbForRead().query;
      },
      get insert() {
        return bind(primary, primary.insert);
      },
      get update() {
        return bind(primary, primary.update);
      },
      get delete() {
        return bind(primary, primary.delete);
      },
      get transaction() {
        return bind(primary, primary.transaction);
      },
      $primary: primary,
      usePrimaryOnly: () => createDatabase(true),
    } as ReplicatedDatabase<Q>;
  };
  return createDatabase(false);
};
