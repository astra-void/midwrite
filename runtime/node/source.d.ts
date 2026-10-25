// Types of the CommitRift source-execution client (protocol version 2).

export declare const SOURCE_PROTOCOL_VERSION: 2;

/** Thrown into a held statement when the coordinator stops the run. */
export declare class CoordinatorStop extends Error {
  constructor(reason: string);
}

/** Whether this process runs under `commitrift model-check --verify-source`. */
export declare function sourceActive(): boolean;

/** `pg.Pool` settings for this actor's sessions on the run's disposable database. */
export declare function sourcePoolConfig(overrides?: Record<string, unknown>): Record<string, unknown>;

/** Attribute Drizzle statements to application source (builder call sites). */
export declare function instrumentDrizzle(drizzle: unknown, pgCore: unknown): void;

/** Hold every statement of the pool's connections until the coordinator releases it. */
export declare function coordinatePool<P extends { on(event: "connect", listener: (client: any) => void): unknown }>(pool: P): P;

export type Entry = (input: any) => Promise<unknown>;

/** Run the entry the coordinator selected and report how it ended. */
export declare function runSourceActor(entries: Record<string, Entry>, cleanup?: () => Promise<void>): Promise<never>;
