/** A `pg.Pool`, or anything with the same `query`/`connect` shape. */
export interface PoolLike {
  query(...args: any[]): any;
  connect(...args: any[]): any;
}

export type StatementClass =
  | { kind: "write"; table: string }
  | { kind: "begin" }
  | { kind: "commit" }
  | { kind: "rollback" }
  | { kind: "other" };

/** Classify one SQL statement by its leading keyword. */
export declare function classifyStatement(text: string | undefined): StatementClass;

/**
 * Instrument one pool in place: promise-style queries emit replay
 * checkpoints at write, commit and rollback boundaries. Returns the pool.
 */
export declare function instrumentPgPool<P extends PoolLike>(pool: P): P;

/**
 * `pg.PoolConfig` for the run's disposable database (Unix socket inside the
 * replay root). Connection fields cannot be overridden.
 */
export declare function replayPoolConfig(overrides?: { max?: number; idleTimeoutMillis?: number }): {
  max: number;
  idleTimeoutMillis: number;
  host: string;
  port: number;
  database: string;
  user: string;
};
