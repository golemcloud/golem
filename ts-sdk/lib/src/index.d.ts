export function golemCreatePromise(): PromiseId;
export function golemAwaitPromise(promiseId: PromiseId): Uint8Array;
export function golemCompletePromise(promiseId: PromiseId, data: Uint8Array): boolean;
export function golemDeletePromise(promiseId: PromiseId): void;
export function getSelfUri(functionName: string): Uri;
export function getOplogIndex(): OplogIndex;
export function setOplogIndex(oplogIdx: OplogIndex): void;
export function oplogCommit(replicas: number): void;
export function markBeginOperation(): OplogIndex;
export function markEndOperation(begin: OplogIndex): void;
export function getRetryPolicy(): RetryPolicy;
export function setRetryPolicy(newRetryPolicy: RetryPolicy): void;
export function getOplogPersistenceLevel(): PersistenceLevel;
export function setOplogPersistenceLevel(newPersistenceLevel: PersistenceLevel): void;
export function getIdempotenceMode(): boolean;
export function setIdempotenceMode(idempotent: boolean): void;
export function generateIdempotencyKey(): Uuid;
export function updateWorker(workerId: WorkerId, targetVersion: ComponentVersion, mode: UpdateMode): void;

export class GetWorkers {
  constructor(componentId: ComponentId, filter: WorkerAnyFilter | undefined, precise: boolean)
  getNext(): WorkerMetadata[] | undefined;
}

export interface Uuid {
  highBits: bigint,
  lowBits: bigint,
}
export interface ComponentId {
  uuid: Uuid,
}
/**
 * # Variants
 * 
 * ## `"equal"`
 * 
 * ## `"not-equal"`
 * 
 * ## `"like"`
 * 
 * ## `"not-like"`
 */
export type StringFilterComparator = 'equal' | 'not-equal' | 'like' | 'not-like';
export interface WorkerNameFilter {
  comparator: StringFilterComparator,
  value: string,
}
/**
 * # Variants
 * 
 * ## `"equal"`
 * 
 * ## `"not-equal"`
 * 
 * ## `"greater-equal"`
 * 
 * ## `"greater"`
 * 
 * ## `"less-equal"`
 * 
 * ## `"less"`
 */
export type FilterComparator = 'equal' | 'not-equal' | 'greater-equal' | 'greater' | 'less-equal' | 'less';
/**
 * # Variants
 * 
 * ## `"running"`
 * 
 * ## `"idle"`
 * 
 * ## `"suspended"`
 * 
 * ## `"interrupted"`
 * 
 * ## `"retrying"`
 * 
 * ## `"failed"`
 * 
 * ## `"exited"`
 */
export type WorkerStatus = 'running' | 'idle' | 'suspended' | 'interrupted' | 'retrying' | 'failed' | 'exited';
export interface WorkerStatusFilter {
  comparator: FilterComparator,
  value: WorkerStatus,
}
export interface WorkerVersionFilter {
  comparator: FilterComparator,
  value: bigint,
}
export interface WorkerCreatedAtFilter {
  comparator: FilterComparator,
  value: bigint,
}
export interface WorkerEnvFilter {
  name: string,
  comparator: StringFilterComparator,
  value: string,
}
export type WorkerPropertyFilter = WorkerPropertyFilterName | WorkerPropertyFilterStatus | WorkerPropertyFilterVersion | WorkerPropertyFilterCreatedAt | WorkerPropertyFilterEnv;
export interface WorkerPropertyFilterName {
  tag: 'name',
  val: WorkerNameFilter,
}
export interface WorkerPropertyFilterStatus {
  tag: 'status',
  val: WorkerStatusFilter,
}
export interface WorkerPropertyFilterVersion {
  tag: 'version',
  val: WorkerVersionFilter,
}
export interface WorkerPropertyFilterCreatedAt {
  tag: 'created-at',
  val: WorkerCreatedAtFilter,
}
export interface WorkerPropertyFilterEnv {
  tag: 'env',
  val: WorkerEnvFilter,
}
export interface WorkerAllFilter {
  filters: WorkerPropertyFilter[],
}
export interface WorkerAnyFilter {
  filters: WorkerAllFilter[],
}
export interface WorkerId {
  componentId: ComponentId,
  workerName: string,
}
export interface WorkerMetadata {
  workerId: WorkerId,
  args: string[],
  env: [string, string][],
  status: WorkerStatus,
  componentVersion: bigint,
  retryCount: bigint,
}
export type OplogIndex = bigint;
export interface PromiseId {
  workerId: WorkerId,
  oplogIdx: OplogIndex,
}
export interface Uri {
  value: string,
}
export type Duration = bigint;
export interface RetryPolicy {
  maxAttempts: number,
  minDelay: Duration,
  maxDelay: Duration,
  multiplier: number,
}
export type PersistenceLevel = PersistenceLevelPersistNothing | PersistenceLevelPersistRemoteSideEffects | PersistenceLevelSmart;
export interface PersistenceLevelPersistNothing {
  tag: 'persist-nothing',
}
export interface PersistenceLevelPersistRemoteSideEffects {
  tag: 'persist-remote-side-effects',
}
export interface PersistenceLevelSmart {
  tag: 'smart',
}
export type ComponentVersion = bigint;
/**
 * # Variants
 * 
 * ## `"automatic"`
 * 
 * ## `"snapshot-based"`
 */
export type UpdateMode = 'automatic' | 'snapshot-based';

