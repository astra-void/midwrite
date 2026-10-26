// Types of the CommitRift source-execution client (protocol version 3).

export declare const SOURCE_PROTOCOL_VERSION: 3;

/** Thrown into a held statement when the coordinator stops the run. */
export declare class CoordinatorStop extends Error {
  constructor(reason: string);
}

/** Whether this process runs under `commitrift model-check --verify-source`. */
export declare function sourceActive(): boolean;

/** Which execution of this actor this process is (1 for the first; a retry is a new process). */
export declare function sourceAttempt(): number;

/** The receipt of the coordinator's channel test double. */
export interface EffectReceipt {
  id: number | null;
  deduplicated: boolean;
}

/**
 * Perform an external effect through the coordinator's test double (for an
 * approved test adapter implementing the application's provider seam).
 */
export declare function sourceEffect(
  channel: string,
  request?: { key?: unknown; payload?: Record<string, unknown> },
): Promise<EffectReceipt>;

/** `pg.Pool` settings for this actor's sessions on the run's disposable database. */
export declare function sourcePoolConfig(overrides?: Record<string, unknown>): Record<string, unknown>;

/** Attribute Drizzle statements to application source (builder call sites). */
export declare function instrumentDrizzle(drizzle: unknown, pgCore: unknown): void;

/** Hold every statement of the pool's connections until the coordinator releases it. */
export declare function coordinatePool<P extends { on(event: "connect", listener: (client: any) => void): unknown }>(pool: P): P;

export type Entry = (input: any) => Promise<unknown>;

/** Run the entry the coordinator selected and report how it ended. */
export declare function runSourceActor(entries: Record<string, Entry>, cleanup?: () => Promise<void>): Promise<never>;

/** postgres-js versions the adapter was verified against. */
export declare const POSTGRES_JS_VERSIONS: string[];

/** `POSTGRES_URL` for this actor's sessions (sets PGHOST, PGPORT, PGUSER for postgres-js). */
export declare function sourcePostgresUrl(): string;

/** Hold every query of the postgres-js module until the coordinator releases it. */
export declare function coordinatePostgresJs(postgres: unknown, version: string): void;
