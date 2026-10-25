export declare function publishEvent(topic: string, key: string): Promise<void>;
export declare function chargeProvider(idempotencyKey: string, amount: number): Promise<void>;
