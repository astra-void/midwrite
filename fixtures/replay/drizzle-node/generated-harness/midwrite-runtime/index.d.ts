// Types of the Midwrite replay client (protocol version 1).

export declare const PROTOCOL_VERSION: 1;

/** The error a checkpoint throws in exception mode. */
export declare class InjectedFault extends Error {
  readonly checkpoint: string;
  constructor(checkpoint: string, message: string);
}

/** The InjectedFault behind `error`, following `cause` chains (libraries wrap driver errors). */
export declare function injectedFaultIn(error: unknown): InjectedFault | undefined;

/** Whether the process runs under `midwrite replay`. */
export declare function replayActive(): boolean;

/**
 * Report that the harness reached checkpoint `name` and wait for the
 * supervisor's decision: resolves (continue), rejects with an
 * `InjectedFault` (exception mode), or never settles because the supervisor
 * kills the process (process-kill mode). A no-op outside replay.
 */
export declare function checkpoint(name: string, label?: string): Promise<void>;

/** The disposable database of this run (socket path inside the replay root). */
export declare function replayDatabase(): { path: string; database: string; username: string };

/** The event log of this run (a fake queue's durable storage). */
export declare function eventLogPath(): string;

export type Scenario = (input: any) => Promise<unknown>;

/**
 * Run the scenario the supervisor selected and report how it ended.
 * `cleanup` runs after the scenario; it never runs after a process kill.
 */
export declare function runScenarios(
  scenarios: Record<string, Scenario>,
  cleanup?: () => Promise<void>,
): Promise<never>;
