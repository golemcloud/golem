export namespace GolemItApi {
  export function failWithCustomMaxRetries(maxRetries: number): void;
  export function explicitCommit(replicas: number): void;
  export function fallibleTransactionTest(): boolean;
  export function infallibleTransactionTest(): number;
}
